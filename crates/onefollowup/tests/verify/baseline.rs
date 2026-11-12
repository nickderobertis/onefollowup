//! The shipped default keeps today's records: one default-configuration batch — two new
//! tickets, a bound ticket re-copied, and evidence with its re-estimate on another batch's item
//! — writes ticket records, receipt journal lines and comment markers byte-identical to the
//! baselines committed under `tests/fixtures/baseline/`, which were taken from this journey at
//! the base of the relations change (the formats as `ofu-verify-loop` left them). Nothing of a
//! relation — no `task relations`, `task parent set-many` or `task link add-many` — is asked of
//! the store.
//!
//! What differs between two runs of one batch is masked the same way in the fixture and in the
//! run: a draft's recording time in its id, an item's version token, a comment's id, and the
//! times the store stamps on the comment it wraps.
//! `ONEFOLLOWUP_BLESS_BASELINE=1` rewrites the fixtures; it is for re-taking them at a base,
//! never to make a branch pass.

use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::journey::{bound_pass, comments, front, inputs, record};
use crate::world::{Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-05";

/// A draft's recording time, as its id carries it.
static STAMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/drafts/\d{8}T\d{6}(?:\.\d+)?Z-").expect("valid"));
/// A version token or a comment id, as a receipt or a comment names it.
static TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"("(?:version|id)": ?)"[^"]*""#).expect("valid"));

/// When the store wrapped a comment: its id and times, which the store stamps.
static WRAPPED_AT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\d{8}T\d{6}Z-\d+|\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z").expect("valid")
});

fn masked(text: &str) -> String {
    let text = STAMP.replace_all(text, "/drafts/<recorded>-");
    TOKEN.replace_all(&text, "$1\"<token>\"").into_owned()
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/baseline/{name}"))
}

fn held_to(name: &str, actual: &str) {
    let path = fixture(name);
    if std::env::var_os("ONEFOLLOWUP_BLESS_BASELINE").is_some() {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the fixtures directory");
        std::fs::write(&path, actual).expect("the fixture");
        return;
    }
    let expected = std::fs::read_to_string(&path).expect("the baseline fixture is committed");
    assert!(
        expected == actual,
        "{} differs from the default configuration's baseline:\n--- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

/// The record an item's front matter carries, as JSON in the order its keys are stored.
fn record_text(text: &str) -> String {
    serde_json::to_string(&record(text)).expect("JSON")
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey, end to end")]
fn the_default_configuration_writes_records_receipts_and_markers_identical_to_the_baseline() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    world.seed("followups", 5);
    world.item(
        "followups",
        "export-older",
        &filed_item(
            "widgets: the export dialog drops unicode titles",
            "nightly-2026-10-01",
            "export-dialog-drops-unicode",
            &json!({}),
            "medium",
        ),
        &filed_body("The export dialog drops unicode titles."),
    );
    let d0 = bound_pass(
        &mut world,
        BATCH,
        "theme-resets-after-update",
        "the theme resets after an update",
    );
    inputs(&world, &json!([]));
    let d1 = world.draft(
        BATCH,
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Saving while resizing loses the layout.\n",
    );
    let d2 = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "café became caf.\n",
    );
    let d3 = world.draft(
        BATCH,
        "the undo stack forgets renames",
        &["src/undo.rs"],
        "Undo skips a rename.\n",
    );
    world.reset_phases();
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    for ticket in [
        Ticket::new(
            "layout-save-reads-size-twice",
            "a layout save reads one window size",
            &[&d1],
        ),
        Ticket::new(
            "undo-forgets-renames",
            "the undo stack forgets renames",
            &[&d3],
        ),
    ] {
        world.ticket(BATCH, &ticket);
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }
    let mut rebound = Ticket::new(
        "theme-resets-after-update",
        "the theme resets after an update",
        &[&d0],
    );
    rebound.evidence.push_str(" Seen again on the next run.");
    world.ticket(BATCH, &rebound);
    world.ok(
        "agent",
        &[
            "ticket",
            "copy",
            "--batch",
            BATCH,
            "theme-resets-after-update",
        ],
        "",
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            BATCH,
            "--item",
            "followups:export-older",
            "--root-cause",
            "export-dialog-drops-unicode",
        ],
        "Host build-host-1: café exported as caf at the basis.\n",
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "re-estimate",
            "--batch",
            BATCH,
            "followups:export-older",
        ],
        "",
    );
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "root-cause",
                "filed",
                &["layout-save-reads-size-twice"],
                "Verified.",
            ),
            answer(
                &d2,
                "root-cause",
                "filed",
                &["export-dialog-drops-unicode"],
                "Evidence on the existing ticket.",
            ),
            answer(
                &d3,
                "root-cause",
                "filed",
                &["undo-forgets-renames"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let outcomes: Vec<(&str, &str)> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| {
            (
                one["op"].as_str().unwrap_or_default(),
                one["outcome"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("copy", "created"),
            ("copy", "created"),
            ("copy", "updated"),
            ("comment", "written"),
            ("re-estimate", "written"),
        ],
        "{published}"
    );
    world.ok("settle", &["settle", "--batch", BATCH], "");

    // No relation was asked of the store.
    let ops: Vec<String> = world
        .phases
        .iter()
        .flat_map(|(_, operations)| operations.iter())
        .map(|operation| operation["op"].as_str().unwrap_or_default().to_owned())
        .collect();
    for relation_op in ["relations", "parent-set-many", "link-add-many"] {
        assert!(
            !ops.iter().any(|op| op == relation_op),
            "the default configuration makes no `{relation_op}`: {ops:?}"
        );
    }

    // The records, in title order: what each card's metadata line holds.
    let mut cards: Vec<(String, String)> = world
        .filed("followups")
        .into_iter()
        .map(|(_, text)| {
            (
                front(&text)["title"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                text,
            )
        })
        .collect();
    cards.sort();
    let records: String = cards
        .iter()
        .map(|(title, text)| format!("{title}\n{}\n", masked(&record_text(text))))
        .collect();
    held_to("records.txt", &records);
    for (_, text) in &cards {
        let held = record(text);
        for absent in ["kind", "fields", "edges", "board_assignee"] {
            assert!(held.get(absent).is_none(), "{absent} is omitted: {held}");
        }
    }

    // The receipt journal, line for line.
    let journal = std::fs::read_to_string(world.work().join(format!("receipts/{BATCH}.jsonl")))
        .expect("the journal");
    held_to("receipts.jsonl", &masked(&journal));
    for line in journal.lines() {
        let receipt: Value = serde_json::from_str(line).expect("a receipt");
        assert!(
            ["copy", "comment", "re-estimate"].contains(&receipt["command"].as_str().unwrap_or("")),
            "a default batch writes only today's receipt kinds: {receipt}"
        );
    }

    // The comment markers: every comment on the item that took the evidence.
    let older = world.read_item("followups", "export-older");
    held_to(
        "comments.md",
        &WRAPPED_AT.replace_all(&masked(&comments(&older)), "<stamped>"),
    );
}
