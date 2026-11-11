//! The workload `follow-up-batch-store-operations` measures: one batch on a destination
//! holding 400 other items — 12 new tickets, 4 tickets bound to existing cards re-copied, and
//! 4 evidence comments on other batches' items each followed by its re-estimate — prepared,
//! checked, published and settled. Every destination operation from `prepare` to `settle` is
//! counted at onefollowup's store boundary and recorded as the budget's telemetry; the
//! comparison with the threshold is onebudgetspec's.

use serde_json::json;

use crate::journey::record;
use crate::telemetry;
use crate::world::{Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-08";

/// The budget this journey measures.
pub const BUDGET: &str = "follow-up-batch-store-operations";

#[test]
fn a_realistic_batch_records_its_destination_store_operations() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    world.seed("followups", 400);
    let others: Vec<String> = (0..4).map(|at| format!("older-{at}")).collect();
    for other in &others {
        world.item(
            "followups",
            other,
            &filed_item(
                &format!("widgets: the {other} item drops unicode"),
                "nightly-2026-10-01",
                other,
                &json!({}),
                "medium",
            ),
            &filed_body(&format!("The {other} item drops unicode.")),
        );
    }

    // An earlier pass of the batch filed four tickets, now bound to their cards.
    let bound: Vec<String> = (0..4).map(|at| format!("bound-cause-{at}")).collect();
    let mut bound_drafts = Vec::new();
    for slug in &bound {
        bound_drafts.push(world.draft(
            BATCH,
            &format!("the {slug} recurs"),
            &["src/a.rs"],
            "Seen.\n",
        ));
    }
    world.ok("setup", &["prepare", "--batch", BATCH], "");
    let mut answers = Vec::new();
    for (slug, draft) in bound.iter().zip(&bound_drafts) {
        world.ticket(
            BATCH,
            &Ticket::new(slug, &format!("the {slug} recurs"), &[draft]),
        );
        world.ok("setup", &["ticket", "copy", "--batch", BATCH, slug], "");
        answers.push(answer(draft, "root-cause", "filed", &[slug], "Verified."));
    }
    world.answer(BATCH, &answers);
    world.ok("setup", &["check", "--batch", BATCH], "");
    world.ok("setup", &["publish", "--batch", BATCH], "");
    world.ok("setup", &["settle", "--batch", BATCH], "");
    world.reset_phases();

    // The measured pass.
    let fresh: Vec<String> = (0..12).map(|at| format!("new-cause-{at:02}")).collect();
    let mut fresh_drafts = Vec::new();
    for slug in &fresh {
        fresh_drafts.push(world.draft(
            BATCH,
            &format!("the {slug} breaks saving"),
            &["src/b.rs"],
            "Seen.\n",
        ));
    }
    let mut other_drafts = Vec::new();
    for other in &others {
        other_drafts.push(world.draft(
            BATCH,
            &format!("the {other} item drops unicode"),
            &["src/c.rs"],
            "Seen.\n",
        ));
    }
    world.reset_phases();
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut answers = Vec::new();
    for (slug, draft) in fresh.iter().zip(&fresh_drafts) {
        world.ticket(
            BATCH,
            &Ticket::new(slug, &format!("the {slug} breaks saving"), &[draft]),
        );
        let validated = world.ok("agent", &["ticket", "validate", "--batch", BATCH, slug], "");
        assert_eq!(validated["sound"], true, "{validated}");
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
        answers.push(answer(draft, "root-cause", "filed", &[slug], "Verified."));
    }
    for (slug, draft) in bound.iter().zip(&bound_drafts) {
        let mut ticket = Ticket::new(slug, &format!("the {slug} recurs"), &[draft]);
        ticket.evidence.push_str(" Seen again.");
        world.ticket(BATCH, &ticket);
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    for (other, draft) in others.iter().zip(&other_drafts) {
        let item = format!("followups:{other}");
        world.ok(
            "agent",
            &[
                "ticket",
                "comment",
                "--batch",
                BATCH,
                "--item",
                &item,
                "--root-cause",
                other,
            ],
            "Seen again at the basis.\n",
        );
        world.ok(
            "agent",
            &["ticket", "re-estimate", "--batch", BATCH, &item],
            "",
        );
        answers.push(answer(
            draft,
            "root-cause",
            "filed",
            &[other],
            "Evidence on the item.",
        ));
    }
    world.answer(BATCH, &answers);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let landed = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .filter(|one| {
            ["created", "updated", "written"].contains(&one["outcome"].as_str().unwrap_or_default())
        })
        .count();
    assert_eq!(
        landed, 24,
        "every write of the workload landed: {published}"
    );
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let filed = world.filed("followups");
    assert_eq!(
        filed
            .iter()
            .filter(|(_, text)| record(text)["created_by_run"] == BATCH)
            .count(),
        16,
        "12 new tickets and 4 bound ones"
    );

    let operations = world.destination_operations();
    let detail = operations
        .iter()
        .map(|(phase, op, source, items)| format!("{phase}: {op} on {source} ({items} items)"))
        .collect::<Vec<_>>()
        .join("; ");
    telemetry::record(
        BUDGET,
        operations.len(),
        &format!(
            "{} destination store operations from prepare to settle, the drafts source excluded: {detail}",
            operations.len()
        ),
    );
}
