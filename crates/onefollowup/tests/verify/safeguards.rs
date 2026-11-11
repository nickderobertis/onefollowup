//! A person's decisions are never overwritten by a batch: a protected status refuses a
//! withdrawal, a moved status and a person's priority are kept by a re-copy and by a
//! re-estimate, a person's edit made between `prepare` and `publish` refuses only the write it
//! would overwrite, and an unchanged re-copy is skipped by its digest — each outcome its own
//! receipt, what did not land recorded in `pending.json`. And `prepare --unanswered` scopes a
//! re-dispatch to what the first pass left unanswered.

use serde_json::{Value, json};

use crate::journey::{front, record};
use crate::world::{Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-04";

/// The card the batch filed for `slug`.
fn card(world: &World, slug: &str) -> String {
    world
        .filed("followups")
        .into_iter()
        .find(|(_, text)| record(text)["root_cause"] == slug)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("a card for {slug}"))
}

/// The receipt of this pass for `card`'s `command`.
fn receipt<'a>(receipts: &'a [Value], command: &str, card: &str) -> &'a Value {
    receipts
        .iter()
        .rev()
        .find(|receipt| {
            receipt["command"] == command && receipt["card"] == format!("followups:{card}")
        })
        .unwrap_or_else(|| panic!("a {command} receipt for {card}: {receipts:#?}"))
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey, end to end")]
fn a_persons_status_priority_and_edits_stand_and_every_outcome_is_receipted() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    world.seed("followups", 40);
    for (id, priority) in [("export-older", "medium"), ("print-older", "urgent")] {
        world.item(
            "followups",
            id,
            &filed_item(
                &format!("widgets: the {id} item drops unicode"),
                "nightly-2026-10-01",
                id,
                &json!({}),
                priority,
            ),
            &filed_body(&format!("The {id} item drops unicode.")),
        );
    }

    // The first pass files four tickets.
    let slugs = ["theme-resets", "zoom-resets", "font-resets", "grid-resets"];
    let mut drafts = Vec::new();
    for slug in slugs {
        drafts.push(world.draft(
            BATCH,
            &format!("the {slug} after an update"),
            &["src/a.rs"],
            "Seen.\n",
        ));
    }
    world.ok("setup", &["prepare", "--batch", BATCH], "");
    let mut answers = Vec::new();
    for (slug, draft) in slugs.iter().zip(&drafts) {
        world.ticket(
            BATCH,
            &Ticket::new(slug, &format!("the {slug} after an update"), &[draft]),
        );
        world.ok("setup", &["ticket", "copy", "--batch", BATCH, slug], "");
        answers.push(answer(draft, "root-cause", "filed", &[slug], "Verified."));
    }
    world.answer(BATCH, &answers);
    world.ok("setup", &["check", "--batch", BATCH], "");
    world.ok("setup", &["publish", "--batch", BATCH], "");
    world.ok("setup", &["settle", "--batch", BATCH], "");
    let cards: Vec<String> = slugs.iter().map(|slug| card(&world, slug)).collect();

    // People decide: one card accepted, one deferred with its priority raised.
    world.edit_item("followups", &cards[0], "status: backlog", "status: todo");
    world.edit_item("followups", &cards[1], "status: backlog", "status: draft");
    world.edit_item(
        "followups",
        &cards[1],
        "priority: medium",
        "priority: urgent",
    );

    // The next pass, with two drafts about other batches' items: withdraw the accepted ticket,
    // re-copy the others, comment on those items and re-estimate them.
    let mut about = Vec::new();
    for item in ["export-older", "print-older"] {
        about.push(world.draft(
            BATCH,
            &format!("the {item} item drops unicode"),
            &["src/b.rs"],
            "Seen.\n",
        ));
    }
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut withdrawn = Ticket::new(slugs[0], "the theme-resets after an update", &[&drafts[0]]);
    withdrawn.status = Some("cancelled".to_owned());
    world.ticket(BATCH, &withdrawn);
    let mut moved = Ticket::new(slugs[1], "the zoom-resets after an update", &[&drafts[1]]);
    moved.evidence.push_str(" Seen again.");
    world.ticket(BATCH, &moved);
    let mut edited = Ticket::new(slugs[3], "the grid-resets after an update", &[&drafts[3]]);
    edited.evidence.push_str(" Seen again.");
    world.ticket(BATCH, &edited);
    let status = world.ok(
        "agent",
        &["ticket", "board-status", "--batch", BATCH, slugs[1]],
        "",
    );
    assert_eq!(status["status_role"], "deferred", "{status}");
    assert_eq!(
        status["priority"], "urgent",
        "a person's priority: {status}"
    );
    for slug in slugs {
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    for item in ["export-older", "print-older"] {
        world.ok(
            "agent",
            &[
                "ticket",
                "comment",
                "--batch",
                BATCH,
                "--item",
                &format!("followups:{item}"),
                "--root-cause",
                item,
            ],
            "Seen again at the basis.\n",
        );
        world.ok(
            "agent",
            &[
                "ticket",
                "re-estimate",
                "--batch",
                BATCH,
                &format!("followups:{item}"),
            ],
            "",
        );
    }
    world.answer(
        BATCH,
        &[
            answer(
                &about[0],
                "root-cause",
                "filed",
                &["export-older"],
                "Evidence on the item.",
            ),
            answer(
                &about[1],
                "root-cause",
                "filed",
                &["print-older"],
                "Evidence on the item.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");

    // People edit two items between prepare and publish.
    world.edit_item(
        "followups",
        &cards[3],
        "## Root cause",
        "## Root cause\n\nA person's note.\n",
    );
    world.edit_item(
        "followups",
        "export-older",
        "Seen once.",
        "Seen once, and a person's note.",
    );
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let outcomes: Vec<String> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| {
            format!(
                "{} {}",
                one["op"].as_str().unwrap_or_default(),
                one["outcome"].as_str().unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            "copy protected",
            "copy updated",
            "copy skipped-unchanged",
            "copy version-moved",
            "comment written",
            "re-estimate version-moved",
            "comment written",
            "re-estimate written",
        ],
        "{published}"
    );

    let receipts = world.receipts(BATCH);
    let protected = receipt(&receipts, "copy", &cards[0]);
    assert_eq!(protected["outcome"]["error"]["kind"], "protected");
    assert_eq!(protected["outcome"]["held"]["status"]["category"], "todo");
    assert_eq!(
        front(&world.read_item("followups", &cards[0]))["status"],
        "todo",
        "the accepted card stands"
    );
    let kept = receipt(&receipts, "copy", &cards[1]);
    assert_eq!(kept["outcome"]["action"], "updated");
    assert_eq!(
        kept["outcome"]["priority_condition"],
        json!({"held": "urgent", "met": false})
    );
    let moved_card = world.read_item("followups", &cards[1]);
    assert_eq!(
        front(&moved_card)["status"],
        "draft",
        "the deferred status stands"
    );
    assert_eq!(
        front(&moved_card)["priority"],
        "urgent",
        "the person's priority stands"
    );
    assert!(moved_card.contains("Seen again."), "the re-copy landed");
    let skipped = receipt(&receipts, "copy", &cards[2]);
    assert_eq!(skipped["outcome"]["action"], "skipped-unchanged");
    assert!(skipped["outcome"]["unchanged_since"].as_u64().is_some());
    let moved_away = receipt(&receipts, "copy", &cards[3]);
    assert_eq!(moved_away["outcome"]["error"]["kind"], "version-moved");
    let grid = world.read_item("followups", &cards[3]);
    assert!(
        grid.contains("A person's note.") && !grid.contains("Seen again."),
        "the person's edit stands: {grid}"
    );
    let refused = receipt(&receipts, "re-estimate", "export-older");
    assert_eq!(refused["outcome"]["error"]["kind"], "version-moved");
    assert!(
        world
            .read_item("followups", "export-older")
            .contains("a person's note")
    );
    let estimated = receipt(&receipts, "re-estimate", "print-older");
    assert_eq!(estimated["outcome"]["error"], Value::Null);
    let print = world.read_item("followups", "print-older");
    assert_eq!(
        front(&print)["priority"],
        "urgent",
        "a re-estimate keeps a person's priority"
    );
    assert!(print.contains("2 occurrences"), "{print}");

    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(settled["needs_attention"], true, "{settled}");
    assert_eq!(settled["pending"], 3, "{settled}");
    let pending: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join("feedback/pending.json")).expect("pending.json"),
    )
    .expect("JSON");
    let entries: Vec<(String, String, String)> = pending["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["item"].as_str().unwrap_or_default().to_owned(),
                entry["write"].as_str().unwrap_or_default().to_owned(),
                entry["state"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        entries,
        [
            (
                format!("followups:{}", cards[0]),
                "withdrawal".to_owned(),
                "withheld".to_owned()
            ),
            (
                format!("followups:{}", cards[3]),
                "ticket-copy".to_owned(),
                "failed".to_owned()
            ),
            (
                "followups:export-older".to_owned(),
                "priority".to_owned(),
                "failed".to_owned()
            ),
        ]
    );
}

#[test]
fn prepare_unanswered_scopes_a_redispatch_to_what_the_first_pass_left() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    let first = world.draft(BATCH, "the theme resets", &["src/a.rs"], "Seen.\n");
    let second = world.draft(BATCH, "the zoom resets", &["src/b.rs"], "Seen.\n");
    let third = world.draft(BATCH, "the font resets", &["src/c.rs"], "Seen.\n");
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(prepared["drafts"], 3);
    world.answer(
        BATCH,
        &[answer(
            &first,
            "root-cause",
            "not-reproducible",
            &[],
            "Not at the basis.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], false, "two inputs are unanswered");
    let prepared = world.ok(
        "prepare",
        &["prepare", "--batch", BATCH, "--unanswered"],
        "",
    );
    assert_eq!(prepared["drafts"], 2, "{prepared}");
    let task = std::fs::read_to_string(prepared["task_template"].as_str().expect("a path"))
        .expect("the task");
    assert!(
        !task.contains(&format!("### `{first}`")),
        "the answered draft is out of scope"
    );
    assert!(task.contains(&format!("### `{second}`")) && task.contains(&format!("### `{third}`")));
    let carried: Value = serde_json::from_str(
        &std::fs::read_to_string(
            world
                .work()
                .join(format!("dispositions/{BATCH}.carried.json")),
        )
        .expect("carried"),
    )
    .expect("JSON");
    assert_eq!(
        carried["dispositions"][0]["draft"],
        first.as_str(),
        "the first pass's answer is carried"
    );
    world.answer(
        BATCH,
        &[
            answer(
                &first,
                "root-cause",
                "not-reproducible",
                &[],
                "Not at the basis.",
            ),
            answer(
                &second,
                "root-cause",
                "not-reproducible",
                &[],
                "Not at the basis.",
            ),
            answer(&third, "root-cause", "too-low-impact", &[], "Cosmetic."),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
}
