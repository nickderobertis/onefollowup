//! The comment-answering pass end to end: three batches' proposals carrying people's, a bot's
//! and already-answered comments, gathered in one listing, launched one feedback dispatch per
//! batch, each dispatch scripted, checked and settled in one read and one `update-many` — and a
//! second pass from the moved watermark that answers nothing again, though its listing returns
//! a comment the first pass answered.

use serde_json::Value;

use crate::answering::{
    DESTINATION, account, comment, destination_ops, edit_ticket, file, item_text, launches,
    pending, post_reply, quoted, replies_to, response, watermark, withdraw_ticket,
};
use crate::journey::{front, record};
use crate::world::World;

pub const A: &str = "nightly-2026-11-01";
pub const B: &str = "nightly-2026-11-02";
pub const C: &str = "nightly-2026-11-03";

/// A world with three batches' proposals filed, two each, and `others` unrelated items.
pub fn three_batches(others: usize) -> (World, Vec<String>, Vec<String>, Vec<String>) {
    let mut world = World::new(&[DESTINATION], "host_label: build-host-1\n");
    world.seed(DESTINATION, others);
    let a = file(
        &mut world,
        A,
        &["layout-save-reads-size-twice", "theme-resets-on-update"],
    );
    let b = file(
        &mut world,
        B,
        &["export-drops-unicode", "export-loses-margins"],
    );
    let c = file(
        &mut world,
        C,
        &["cache-lease-goes-stale", "cache-evicts-pinned-items"],
    );
    world.reset_phases();
    (world, a, b, c)
}

#[test]
#[allow(clippy::too_many_lines, reason = "one pass, end to end, then the next")]
fn every_persons_unanswered_comment_is_answered_once_by_its_batch_and_the_watermark_moves() {
    let (mut world, a, b, c) = three_batches(8);
    // People's comments, a bot's, one already answered, and another batch's evidence.
    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "Is this still happening after the 2.3 release? I saw it on Linux too.",
    );
    comment(
        &world,
        &a[0],
        "20261101T100500Z-1",
        "dependabot[bot]",
        "2026-11-01T10:05:00Z",
        "Bumps serde from 1.0.1 to 1.0.2.",
    );
    comment(
        &world,
        &a[1],
        "20261101T101000Z-1",
        "bob",
        "2026-11-01T10:10:00Z",
        "Does this need a migration?",
    );
    comment(
        &world,
        &a[1],
        "20261101T101500Z-1",
        "follow-up-agent",
        "2026-11-01T10:15:00Z",
        &format!(
            "Reply from follow-up run `{A}` to @bob's comment: {}\n\nNo migration: the fix reads the size once.\n\n<!-- orchestrator:follow-up-comment run=\"{A}\" root_cause=\"theme-resets-on-update\" kind=\"reply\" answers=\"20261101T101000Z-1\" verdict=\"does-not-confirm\" -->",
            a[1]
        ),
    );
    comment(
        &world,
        &b[0],
        "20261101T102000Z-1",
        "carol",
        "2026-11-01T10:20:00Z",
        "This was fixed by the unicode rewrite last week; can it be closed?",
    );
    // A comment the destination dates after the first pass's read — as a destination whose
    // reads lag its writes does — so the second pass's listing returns it again.
    comment(
        &world,
        &b[1],
        "20991231T000000Z-1",
        "erin",
        "2099-12-31T00:00:00Z",
        "Margins are lost on A4 too.",
    );
    comment(
        &world,
        &c[0],
        "20261101T103000Z-1",
        "dave",
        "2026-11-01T10:30:00Z",
        "Seen again on the build farm this morning.",
    );
    comment(
        &world,
        &c[1],
        "20261101T103500Z-1",
        "follow-up-agent",
        "2026-11-01T10:35:00Z",
        &format!(
            "Additional evidence from follow-up run `{A}`.\n\nSeen.\n\n<!-- orchestrator:follow-up-comment run=\"{A}\" root_cause=\"cache-evicts-pinned-items\" -->"
        ),
    );
    let estimate_before = item_text(&world, &c[0]);
    assert!(
        estimate_before.contains("1 occurrence;"),
        "{estimate_before}"
    );

    let gathered = world.ok("gather", &["comments", "gather"], "");
    assert_eq!(gathered["dry_run"], false, "{gathered}");
    assert_eq!(gathered["selected"][A], 1, "{gathered}");
    assert_eq!(gathered["selected"][B], 2, "{gathered}");
    assert_eq!(gathered["selected"][C], 1, "{gathered}");
    assert_eq!(
        gathered["left"], 4,
        "the bot's, the answered one, its reply and the evidence: {gathered}"
    );
    assert_eq!(
        destination_ops(&world),
        ["comments-since"],
        "gathering is one listing, and under no rollup neither relation read: {gathered}"
    );
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let record_of: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{snapshot}/snapshot.json")).expect("the record"),
    )
    .expect("JSON");
    let reasons: Vec<&str> = record_of["left"]
        .as_array()
        .expect("left")
        .iter()
        .map(|left| left["reason"].as_str().expect("a reason"))
        .collect();
    assert!(
        reasons.contains(&"bot author dependabot[bot]"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"answered by reply 20261101T101500Z-1"),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&format!("marked by batch {A}").as_str()),
        "{reasons:?}"
    );

    world.reset_phases();
    let launched = world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    );
    assert!(
        destination_ops(&world).is_empty(),
        "launching reads nothing"
    );
    let launched = launches(&launched);
    assert_eq!(launched.len(), 3, "one feedback dispatch per owning batch");

    for (batch, feedback) in &launched {
        let text = std::fs::read_to_string(feedback).expect("the feedback file");
        assert!(
            text.contains("not as the destination holds them"),
            "the task says where the item is read from"
        );
        let quoted = quoted(feedback);
        let mut responses = Vec::new();
        for one in &quoted {
            let last_written =
                std::fs::read_to_string(&one.last_written).expect("what onefollowup last wrote");
            assert!(
                last_written.contains("not the destination's current text"),
                "{last_written}"
            );
            let (verdict, reply) = match one.comment.as_str() {
                "20261101T100000Z-1" => {
                    edit_ticket(
                        &one.ticket,
                        "Read the window size once per save.",
                        "Read the window size once per save, on every platform.",
                    );
                    (
                        "confirms",
                        "Confirmed on Linux too: the suggested fix now says every platform.",
                    )
                }
                "20261101T102000Z-1" => {
                    withdraw_ticket(&one.ticket);
                    (
                        "does-not-confirm",
                        "The rewrite fixed it, so this ticket is withdrawn.",
                    )
                }
                "20261101T103000Z-1" => (
                    "confirms",
                    "One more occurrence, counted into the estimate.",
                ),
                _ => ("does-not-confirm", "The ticket already covers A4."),
            };
            let id = post_reply(&world, one, verdict, reply);
            responses.push(response(one, &id, verdict, reply));
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

    world.reset_phases();
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(
        destination_ops(&world),
        ["read", "update-many"],
        "one batched read and one update-many: {settled}"
    );
    assert_eq!(settled["needs_attention"], Value::Null, "{settled}");
    assert!(pending(&world).is_empty());
    for (item, id) in [
        (&a[0], "20261101T100000Z-1"),
        (&b[0], "20261101T102000Z-1"),
        (&b[1], "20991231T000000Z-1"),
        (&c[0], "20261101T103000Z-1"),
    ] {
        assert_eq!(
            replies_to(&world, item, id),
            1,
            "{id} has exactly one reply"
        );
    }
    assert_eq!(
        replies_to(&world, &a[0], "20261101T100500Z-1"),
        0,
        "no reply to a bot"
    );
    assert_eq!(
        replies_to(&world, &a[1], "20261101T101000Z-1"),
        1,
        "the answered comment keeps its one reply"
    );
    let edited = item_text(&world, &a[0]);
    assert!(
        edited.contains("on every platform."),
        "the changed ticket carries its write: {edited}"
    );
    assert!(
        edited.contains("2 occurrences;"),
        "the confirming reply is counted: {edited}"
    );
    let withdrawn = item_text(&world, &b[0]);
    assert_eq!(front(&withdrawn)["status"], "cancelled", "{withdrawn}");
    let estimated = item_text(&world, &c[0]);
    assert!(estimated.contains("2 occurrences;"), "{estimated}");
    assert_eq!(record(&estimated)["priority_estimate"], "medium");
    let moved = watermark(&world, DESTINATION).expect("the watermark moved");
    assert_eq!(settled["watermark"], moved["since"], "{settled}");

    // The second pass, from the moved watermark: its listing returns erin's comment again, and
    // the reply the first pass verified answers it, so nothing is answered twice.
    world.reset_phases();
    let again = world.ok("gather", &["comments", "gather"], "");
    assert_eq!(again["selected"], serde_json::json!({}), "{again}");
    let again_snapshot = again["snapshot"].as_str().expect("a snapshot").to_owned();
    let again_record: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{again_snapshot}/snapshot.json")).expect("the record"),
    )
    .expect("JSON");
    assert!(
        again_record["left"]
            .as_array()
            .expect("left")
            .iter()
            .any(|left| left["comment"] == "20991231T000000Z-1"
                && left["reason"] == "answered by reply 20991231T000000Z-1-reply"),
        "{again_record}"
    );
    let relaunched = world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &again_snapshot],
        "",
    );
    assert!(launches(&relaunched).is_empty());
    world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &again_snapshot],
        "",
    );
    for (item, id) in [
        (&a[0], "20261101T100000Z-1"),
        (&b[1], "20991231T000000Z-1"),
        (&c[0], "20261101T103000Z-1"),
    ] {
        assert_eq!(
            replies_to(&world, item, id),
            1,
            "{id} still has exactly one reply"
        );
    }
}
