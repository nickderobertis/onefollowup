//! A pass in which one batch's reply cannot be verified at settle — its dispatch accounted for
//! a reply it never posted, and another's reply was removed before settle — records each in
//! `pending.json`, says it needs attention and holds the watermark; the next unscoped pass
//! answers exactly those comments, once each, and only then moves the watermark.

use crate::answering::{
    DESTINATION, account, answer_all, comment, launches, pending, quoted, remove_comment,
    replies_to, response, watermark,
};
use crate::comments::{B, C, three_batches};

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one pass that leaves replies outstanding, then the next"
)]
fn a_reply_settle_cannot_verify_is_pending_and_the_next_pass_answers_its_comment_once() {
    let (mut world, a, b, c) = three_batches(4);
    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "Does this happen on Windows?",
    );
    comment(
        &world,
        &b[0],
        "20261101T101000Z-1",
        "bob",
        "2026-11-01T10:10:00Z",
        "Is there a workaround?",
    );
    comment(
        &world,
        &c[0],
        "20261101T102000Z-1",
        "carol",
        "2026-11-01T10:20:00Z",
        "Seen on the farm.",
    );

    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let launched = world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    );
    for (batch, feedback) in launches(&launched) {
        match batch.as_str() {
            // The dispatch accounts for a reply it never posted.
            B => {
                let quoted = quoted(&feedback);
                let responses: Vec<_> = quoted
                    .iter()
                    .map(|one| {
                        response(
                            one,
                            &format!("{}-reply", one.comment),
                            "does-not-confirm",
                            "Answered.",
                        )
                    })
                    .collect();
                account(&feedback, &batch, &responses);
            }
            // The reply is posted, and removed again before settle.
            C => {
                let quoted = answer_all(&world, &batch, &feedback, &|_| "confirms");
                remove_comment(
                    &world,
                    &quoted[0].issue,
                    &format!("{}-reply", quoted[0].comment),
                );
            }
            _ => {
                answer_all(&world, &batch, &feedback, &|_| "does-not-confirm");
            }
        }
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
        assert_eq!(
            checked["sound"], true,
            "the account is sound; the destination is settle's to verify: {checked}"
        );
    }
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(settled["needs_attention"], true, "{settled}");
    assert_eq!(settled["watermark"], serde_json::Value::Null, "{settled}");
    assert!(
        watermark(&world, DESTINATION).is_none(),
        "the watermark holds"
    );
    let outstanding = pending(&world);
    let answered: Vec<&str> = outstanding
        .iter()
        .filter(|entry| entry["write"] == "comment")
        .map(|entry| entry["intended"]["answers"].as_str().expect("the comment"))
        .collect();
    assert_eq!(
        answered,
        ["20261101T101000Z-1", "20261101T102000Z-1"],
        "{outstanding:?}"
    );
    assert_eq!(replies_to(&world, &a[0], "20261101T100000Z-1"), 1);
    assert_eq!(replies_to(&world, &b[0], "20261101T101000Z-1"), 0);
    assert_eq!(replies_to(&world, &c[0], "20261101T102000Z-1"), 0);

    // The next unscoped pass answers exactly the two outstanding comments.
    let next = world.ok("gather", &["comments", "gather"], "");
    assert_eq!(next["selected"], serde_json::json!({B: 1, C: 1}), "{next}");
    let next_snapshot = next["snapshot"].as_str().expect("a snapshot").to_owned();
    let relaunched = world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &next_snapshot],
        "",
    );
    for (batch, feedback) in launches(&relaunched) {
        answer_all(&world, &batch, &feedback, &|_| "does-not-confirm");
    }
    let resettled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &next_snapshot],
        "",
    );
    assert_eq!(
        resettled["needs_attention"],
        serde_json::Value::Null,
        "{resettled}"
    );
    assert!(
        pending(&world).is_empty(),
        "both outstanding replies are verified now"
    );
    for (item, id) in [
        (&a[0], "20261101T100000Z-1"),
        (&b[0], "20261101T101000Z-1"),
        (&c[0], "20261101T102000Z-1"),
    ] {
        assert_eq!(
            replies_to(&world, item, id),
            1,
            "{id} has exactly one reply"
        );
    }
    let moved = watermark(&world, DESTINATION).expect("the watermark moved");
    assert_eq!(resettled["watermark"], moved["since"]);
}

#[test]
fn an_item_deleted_before_settle_is_pending_and_its_reply_outstanding() {
    let (mut world, a, b, _) = three_batches(4);
    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "Still seen?",
    );
    comment(
        &world,
        &b[0],
        "20261101T101000Z-1",
        "bob",
        "2026-11-01T10:10:00Z",
        "Still seen?",
    );
    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let launched = world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    );
    for (batch, feedback) in launches(&launched) {
        answer_all(&world, &batch, &feedback, &|_| "confirms");
    }
    // A person deletes batch B's item before settle.
    std::fs::remove_file(crate::answering::item_path(&world, &b[0])).expect("deleted");
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(settled["needs_attention"], true, "{settled}");
    let held = pending(&world);
    assert!(
        held.iter().any(|entry| entry["item"] == b[0].as_str()
            && entry["state"] == "failed"
            && entry["error"]
                .as_str()
                .unwrap_or_default()
                .contains("could not be read")),
        "{held:?}"
    );
    assert!(
        held.iter()
            .any(|entry| entry["item"] == b[0].as_str() && entry["write"] == "comment"),
        "its reply is outstanding: {held:?}"
    );
    assert_eq!(replies_to(&world, &a[0], "20261101T100000Z-1"), 1);
    assert!(
        watermark(&world, DESTINATION).is_none(),
        "the watermark holds"
    );
    // The next pass keeps both entries: the item is still gone.
    let next = world.ok("gather", &["comments", "gather"], "");
    let next_snapshot = next["snapshot"].as_str().expect("a snapshot").to_owned();
    world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &next_snapshot],
        "",
    );
    let resettled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &next_snapshot],
        "",
    );
    assert_eq!(resettled["needs_attention"], true, "{resettled}");
    assert_eq!(pending(&world).len(), held.len());
}

#[test]
fn a_drafts_source_that_cannot_keep_step_fails_settle_naming_what_to_do() {
    let (mut world, a, _, _) = three_batches(4);
    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "Covers Linux?",
    );
    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let launched = world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    );
    for (batch, feedback) in launches(&launched) {
        let one = &quoted(&feedback)[0];
        crate::answering::edit_ticket(
            &one.ticket,
            "Read the window size once per save.",
            "Read the window size once per save, on Linux too.",
        );
        answer_all(&world, &batch, &feedback, &|_| "confirms");
    }
    // The batch's own copy of the ticket in the drafts source is gone.
    let written: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{snapshot}/gathering.json")).expect("the gathering"),
    )
    .expect("JSON");
    let stored = written["last_written"][0]["stored_as"]
        .as_str()
        .expect("stored as")
        .to_owned();
    std::fs::remove_file(crate::answering::item_path(&world, &stored)).expect("deleted");
    let failed = world.fails(1, &["comments", "settle", "--snapshot", &snapshot], "");
    assert!(
        failed.contains("refused keeping") && failed.contains("publish its batch again"),
        "{failed}"
    );
    assert!(
        crate::answering::item_text(&world, &a[0]).contains("on Linux too."),
        "the destination took the write"
    );
}
