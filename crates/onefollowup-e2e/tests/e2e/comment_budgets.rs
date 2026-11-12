//! The workloads the comment pass's two budgets measure, each counted at onefollowup's store
//! boundary and recorded as the budget's telemetry; the comparison with the threshold is
//! onebudgetspec's.
//!
//! - `comment-pass-store-operations`: 40 comments on 20 proposals owned by 6 batches, beside 60
//!   other items of this host, gathered once, answered by one feedback dispatch per owning
//!   batch, 10 tickets changed by the answers, settled once — every store operation from
//!   `comments gather` to `comments settle` onefollowup makes at the destination.
//! - `ticket-lifecycle-store-operations`: one ticket filed by one batch, re-copied by a
//!   re-dispatch of that batch, and answered once by a comment pass whose answer withdraws it,
//!   on a destination holding 60 other items — every destination operation of its whole life,
//!   the local drafts source excluded.
//!
//! Neither counts the reply writes the answering agents make, one per comment answered.

use crate::answering::{
    DESTINATION, account, comment, destination_ops, edit_ticket, file, item_text, launches,
    post_reply, quoted, replies_to, response, withdraw_ticket,
};
use crate::journey::front;
use crate::telemetry;
use crate::world::{Ticket, World, answer};

/// The comment pass's budget.
pub const PASS: &str = "comment-pass-store-operations";
/// One ticket's whole life's budget.
pub const LIFECYCLE: &str = "ticket-lifecycle-store-operations";

/// Each destination operation kept, as the detail names it.
fn detail(world: &World) -> String {
    world
        .destination_operations()
        .iter()
        .map(|(phase, op, source, items)| format!("{phase}: {op} on {source} ({items} items)"))
        .collect::<Vec<_>>()
        .join("; ")
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the workload's setup, then its one pass"
)]
fn a_realistic_comment_pass_records_its_store_operations() {
    let mut world = World::new(&[DESTINATION], "host_label: build-host-1\n");
    world.seed(DESTINATION, 60);
    let shares = [4, 4, 3, 3, 3, 3];
    let mut owned: Vec<(String, String)> = Vec::new();
    for (at, count) in shares.iter().enumerate() {
        let batch = format!("nightly-2026-11-{:02}", at + 1);
        let slugs: Vec<String> = (0..*count).map(|one| format!("cause-{at}-{one}")).collect();
        let named: Vec<&str> = slugs.iter().map(String::as_str).collect();
        for card in file(&mut world, &batch, &named) {
            owned.push((batch.clone(), card));
        }
    }
    assert_eq!(owned.len(), 20, "20 proposals");
    for (at, (_, card)) in owned.iter().enumerate() {
        for one in 0..2 {
            comment(
                &world,
                card,
                &format!("20261110T1{at:02}{one}00Z-1"),
                &format!("person-{at}-{one}"),
                &format!("2026-11-10T1{}:{at:02}:{one}0Z", at % 10),
                "Is this still seen after the last release?",
            );
        }
    }
    world.reset_phases();

    let gathered = world.ok("gather", &["comments", "gather"], "");
    let selected: u64 = gathered["selected"]
        .as_object()
        .expect("selected")
        .values()
        .map(|count| count.as_u64().unwrap_or_default())
        .sum();
    assert_eq!(selected, 40, "{gathered}");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let launched = launches(&world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    ));
    assert_eq!(launched.len(), 6, "one feedback dispatch per owning batch");
    let mut changed = 0;
    for (batch, feedback) in &launched {
        let mut responses = Vec::new();
        let mut edited: Vec<String> = Vec::new();
        for one in quoted(feedback) {
            if changed < 10 && !edited.contains(&one.issue) {
                edit_ticket(
                    &one.ticket,
                    "Read the window size once per save.",
                    "Read the window size once per save, after every release.",
                );
                edited.push(one.issue.clone());
                changed += 1;
            }
            let reply = post_reply(
                &world,
                &one,
                "does-not-confirm",
                "Still seen: the fix covers it.",
            );
            responses.push(response(
                &one,
                &reply,
                "does-not-confirm",
                "Read it against the ticket.",
            ));
        }
        account(feedback, batch, &responses);
        let checked = world.ok(
            "check",
            &[
                "comments",
                "check",
                "--feedback",
                &feedback.display().to_string(),
            ],
            "",
        );
        assert_eq!(checked["sound"], true, "{checked}");
    }
    assert_eq!(changed, 10, "10 tickets changed by the answers");
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    let writes: u64 = settled["settled"]
        .as_array()
        .expect("settled")
        .iter()
        .map(|launch| launch["writes"].as_u64().unwrap_or_default())
        .sum();
    assert_eq!(writes, 10, "{settled}");
    assert!(settled.get("needs_attention").is_none(), "{settled}");
    for (_, card) in &owned {
        assert!(
            item_text(&world, card).matches("kind=\"reply\"").count() == 2,
            "both comments on {card} answered once"
        );
    }

    let operations = destination_ops(&world);
    telemetry::record(
        PASS,
        operations.len(),
        &format!(
            "{} store operations onefollowup made at the destination from comments gather to comments settle — the gathering's listing, settle's read and its writes — beside the 40 replies the answering agents posted: {}",
            operations.len(),
            detail(&world)
        ),
    );
}

#[test]
#[allow(clippy::too_many_lines, reason = "one ticket's whole life")]
fn one_tickets_whole_life_records_its_destination_store_operations() {
    const BATCH: &str = "nightly-2026-11-12";
    let mut world = World::new(&[DESTINATION], "host_label: build-host-1\n");
    world.seed(DESTINATION, 60);
    let slug = "layout-save-reads-size-twice";

    // Filed.
    let draft = world.draft(
        BATCH,
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Lost.\n",
    );
    world.ok("filed", &["prepare", "--batch", BATCH], "");
    world.ticket(
        BATCH,
        &Ticket::new(slug, "a layout save reads one window size", &[&draft]),
    );
    world.ok("filed", &["ticket", "copy", "--batch", BATCH, slug], "");
    world.answer(
        BATCH,
        &[answer(&draft, "root-cause", "filed", &[slug], "Verified.")],
    );
    let checked = world.ok("filed", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("filed", &["publish", "--batch", BATCH], "");
    world.ok("filed", &["settle", "--batch", BATCH], "");
    let card = format!("{DESTINATION}:{}", world.filed(DESTINATION)[0].0);

    // Re-copied by a re-dispatch of the batch.
    world.ok(
        "re-copied",
        &["prepare", "--batch", BATCH, "--unanswered"],
        "",
    );
    let mut ticket = Ticket::new(slug, "a layout save reads one window size", &[&draft]);
    ticket.evidence.push_str(" Seen again on the next run.");
    world.ticket(BATCH, &ticket);
    world.ok("re-copied", &["ticket", "copy", "--batch", BATCH, slug], "");
    let checked = world.ok("re-copied", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("re-copied", &["publish", "--batch", BATCH], "");
    world.ok("re-copied", &["settle", "--batch", BATCH], "");
    assert!(item_text(&world, &card).contains("Seen again on the next run."));

    // Answered once by a comment pass whose answer withdraws it.
    comment(
        &world,
        &card,
        "20261112T120000Z-1",
        "pat",
        "2026-11-12T12:00:00Z",
        "The layout rewrite removed this code path; is it still relevant?",
    );
    let gathered = world.ok("answered", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let launched = launches(&world.ok(
        "answered",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    ));
    let (batch, feedback) = &launched[0];
    let one = &quoted(feedback)[0];
    withdraw_ticket(&one.ticket);
    let reply = post_reply(
        &world,
        one,
        "does-not-confirm",
        "The rewrite removed it: withdrawn.",
    );
    account(
        feedback,
        batch,
        &[response(
            one,
            &reply,
            "does-not-confirm",
            "Withdrew the ticket.",
        )],
    );
    world.ok(
        "answered",
        &[
            "comments",
            "check",
            "--feedback",
            &feedback.display().to_string(),
        ],
        "",
    );
    world.ok(
        "answered",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(front(&item_text(&world, &card))["status"], "cancelled");
    assert_eq!(replies_to(&world, &card, "20261112T120000Z-1"), 1);

    let operations = world.destination_operations();
    telemetry::record(
        LIFECYCLE,
        operations.len(),
        &format!(
            "{} destination store operations over one ticket's whole life, the drafts source and the one reply the answering agent posted excluded: {}",
            operations.len(),
            detail(&world)
        ),
    );
}
