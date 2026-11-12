//! The snapshot every step after gathering reads: a dry run's launched by the command it
//! printed, with no destination read before its launches; a launched snapshot refused a second
//! launch; two dry runs sharing a comment, whichever launches first answering it and the other
//! refused; a person's edit — after the snapshot, or before the gathering — standing over the
//! write it would conflict with, which waits in `pending.json` and is retried by the next pass;
//! and passes scoped to one item and to one batch, which leave the watermark for the next
//! unscoped pass.

use serde_json::Value;

use crate::answering::{
    DESTINATION, account, answer_all, comment, destination_ops, edit_ticket, item_text, launches,
    now, pending, post_reply, quoted, replies_to, response, watermark,
};
use crate::comments::{A, B, C, three_batches};
use crate::world::World;

fn snapshot_of(printed: &Value) -> String {
    printed["snapshot"].as_str().expect("a snapshot").to_owned()
}

/// Launch `snapshot`, answer every comment it quotes, check each account and settle.
fn answer_and_settle(world: &mut World, snapshot: &str) -> Value {
    let launched = world.ok(
        "launches",
        &["comments", "launches", "--snapshot", snapshot],
        "",
    );
    for (batch, feedback) in launches(&launched) {
        answer_all(world, &batch, &feedback, &|_| "does-not-confirm");
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
    world.ok(
        "settle",
        &["comments", "settle", "--snapshot", snapshot],
        "",
    )
}

#[test]
fn a_dry_runs_printed_command_launches_it_with_no_further_read_and_a_second_launch_is_refused() {
    let (mut world, a, _, _) = three_batches(4);
    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "Does this lose the whole layout?",
    );
    let dry = world.ok("gather", &["comments", "gather", "--dry-run"], "");
    assert_eq!(dry["dry_run"], true, "{dry}");
    let snapshot = snapshot_of(&dry);
    let command = dry["launch_command"]
        .as_str()
        .expect("the command that launches it");
    assert_eq!(
        command,
        format!("onefollowup comments launches --snapshot {snapshot}")
    );
    assert!(
        !world
            .root
            .join(format!(".onefollowup/feedback/{A}"))
            .exists(),
        "a dry run writes no feedback"
    );

    world.reset_phases();
    let words: Vec<&str> = command.split(' ').skip(1).collect();
    let launched = world.ok("launches", &words, "");
    assert!(
        destination_ops(&world).is_empty(),
        "no destination read before its launches"
    );
    let launched = launches(&launched);
    assert_eq!(launched.len(), 1);
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{snapshot}/launched.json")).expect("the claim"),
    )
    .expect("JSON");
    assert_eq!(record["launch"], "from", "{record}");

    let refused = world.fails(2, &words, "");
    assert!(refused.contains("already launched"), "{refused}");
    assert!(
        refused.contains("comments gather --dry-run"),
        "names the fresh dry run: {refused}"
    );

    let (batch, feedback) = &launched[0];
    answer_all(&world, batch, feedback, &|_| "does-not-confirm");
    world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(replies_to(&world, &a[0], "20261101T100000Z-1"), 1);
}

/// Two dry runs sharing alice's comment, the one `first` names launched first.
fn two_dry_runs(first_is_later: bool) {
    let (mut world, a, _, _) = three_batches(4);
    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "Is the theme reset related?",
    );
    let earlier = snapshot_of(&world.ok("gather", &["comments", "gather", "--dry-run"], ""));
    let later = snapshot_of(&world.ok("gather", &["comments", "gather", "--dry-run"], ""));
    assert_ne!(earlier, later);
    let (first, second) = if first_is_later {
        (&later, &earlier)
    } else {
        (&earlier, &later)
    };
    let launched = world.ok(
        "launches",
        &["comments", "launches", "--snapshot", first],
        "",
    );
    let refused = world.fails(2, &["comments", "launches", "--snapshot", second], "");
    assert!(
        refused.contains("20261101T100000Z-1"),
        "names the shared comment: {refused}"
    );
    assert!(
        refused.contains("comments gather --dry-run"),
        "names the fresh dry run: {refused}"
    );
    for (batch, feedback) in launches(&launched) {
        answer_all(&world, &batch, &feedback, &|_| "does-not-confirm");
    }
    world.ok("settle", &["comments", "settle", "--snapshot", first], "");
    assert_eq!(
        replies_to(&world, &a[0], "20261101T100000Z-1"),
        1,
        "answered exactly once"
    );
}

#[test]
fn of_two_dry_runs_sharing_a_comment_whichever_launches_first_answers_it_and_the_other_is_refused()
{
    two_dry_runs(false);
    two_dry_runs(true);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the edit, the withheld write, its retries, then the edit before"
)]
fn a_persons_edit_stands_over_the_write_it_conflicts_with_and_the_next_pass_retries_it() {
    let (mut world, a, b, _) = three_batches(4);
    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "The fix should cover Linux too.",
    );
    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = snapshot_of(&gathered);
    let launched = launches(&world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    ));
    let (batch, feedback) = &launched[0];
    let one = &quoted(feedback)[0];
    edit_ticket(
        &one.ticket,
        "Read the window size once per save.",
        "Read the window size once per save, on Linux too.",
    );
    let reply = post_reply(&world, one, "confirms", "The fix now covers Linux.");
    account(
        feedback,
        batch,
        &[response(one, &reply, "confirms", "Widened the fix.")],
    );
    // A person edits the item after the snapshot.
    let file = a[0].split_once(':').expect("qualified").1.to_owned();
    world.edit_item(
        DESTINATION,
        &file,
        "Read the window size once per save.",
        "Read the window size once, at the start of the save.",
    );
    let person = item_text(&world, &a[0]);

    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(settled["needs_attention"], true, "{settled}");
    let after = item_text(&world, &a[0]);
    assert!(
        after.contains("at the start of the save."),
        "the person's edit stands: {after}"
    );
    assert!(!after.contains("on Linux too."), "{after}");
    assert_eq!(
        after.split("## Comments").next(),
        person.split("## Comments").next(),
        "the item is exactly as the person left it, but for the reply"
    );
    let held = pending(&world);
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0]["write"], "ticket-copy");
    assert_eq!(held[0]["state"], "withheld");
    assert!(
        held[0]["error"]
            .as_str()
            .expect("why")
            .contains("its content"),
        "{held:?}"
    );
    assert!(
        watermark(&world, DESTINATION).is_some(),
        "a pending ticket write does not hold the watermark"
    );

    // The next pass, with the person's edit still there: the write stays withheld.
    let next = snapshot_of(&world.ok("gather", &["comments", "gather"], ""));
    let resettled = answer_and_settle(&mut world, &next);
    assert_eq!(resettled["needs_attention"], true, "{resettled}");
    assert_eq!(pending(&world).len(), 1);
    assert!(item_text(&world, &a[0]).contains("at the start of the save."));

    // The person takes their edit back; the next pass retries the write, and it lands.
    world.edit_item(
        DESTINATION,
        &file,
        "Read the window size once, at the start of the save.",
        "Read the window size once per save.",
    );
    let last = snapshot_of(&world.ok("gather", &["comments", "gather"], ""));
    world.reset_phases();
    let retried = answer_and_settle(&mut world, &last);
    assert_eq!(retried["needs_attention"], Value::Null, "{retried}");
    assert!(pending(&world).is_empty());
    assert!(
        item_text(&world, &a[0]).contains("on Linux too."),
        "the retried write landed"
    );
    assert_eq!(
        destination_ops(&world),
        ["read", "update-many"],
        "the retry rides in settle's one read and one update-many"
    );

    // A person's edit made before the gathering withholds the write too: the item differs from
    // what onefollowup last wrote.
    let other = b[0].split_once(':').expect("qualified").1.to_owned();
    world.edit_item(
        DESTINATION,
        &other,
        "Read the window size once per save.",
        "Read the size from the saved layout.",
    );
    comment(
        &world,
        &b[0],
        "20261101T110000Z-1",
        "bob",
        &now(),
        "Please also mention exports.",
    );
    let before = snapshot_of(&world.ok("gather", &["comments", "gather"], ""));
    let launched = launches(&world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &before],
        "",
    ));
    let (batch, feedback) = &launched[0];
    assert_eq!(batch, B);
    let one = &quoted(feedback)[0];
    edit_ticket(
        &one.ticket,
        "Read the window size once per save.",
        "Read the window size once per save and per export.",
    );
    let reply = post_reply(&world, one, "does-not-confirm", "Exports are named now.");
    account(
        feedback,
        batch,
        &[response(one, &reply, "does-not-confirm", "Named exports.")],
    );
    let settled = world.ok("settle", &["comments", "settle", "--snapshot", &before], "");
    assert_eq!(settled["needs_attention"], true, "{settled}");
    let kept = item_text(&world, &b[0]);
    assert!(
        kept.contains("Read the size from the saved layout."),
        "{kept}"
    );
    assert!(!kept.contains("per export"), "{kept}");
    assert!(
        pending(&world)
            .iter()
            .any(|entry| entry["item"] == b[0].as_str() && entry["state"] == "withheld"),
        "{:?}",
        pending(&world)
    );
}

#[test]
fn a_pass_scoped_to_an_item_or_a_batch_answers_its_own_and_leaves_the_watermark() {
    let (mut world, a, b, c) = three_batches(4);
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
    comment(
        &world,
        &c[0],
        "20261101T102000Z-1",
        "carol",
        "2026-11-01T10:20:00Z",
        "Still seen?",
    );

    let by_item = world.ok("gather", &["comments", "gather", "--item", &a[0]], "");
    assert_eq!(by_item["selected"], serde_json::json!({A: 1}), "{by_item}");
    answer_and_settle(&mut world, &snapshot_of(&by_item));
    assert!(
        watermark(&world, DESTINATION).is_none(),
        "a scoped pass leaves the watermark"
    );

    let by_batch = world.ok("gather", &["comments", "gather", "--batch", B], "");
    assert_eq!(
        by_batch["selected"],
        serde_json::json!({B: 1}),
        "{by_batch}"
    );
    answer_and_settle(&mut world, &snapshot_of(&by_batch));
    assert!(
        watermark(&world, DESTINATION).is_none(),
        "a scoped pass leaves the watermark"
    );
    assert_eq!(replies_to(&world, &a[0], "20261101T100000Z-1"), 1);
    assert_eq!(replies_to(&world, &b[0], "20261101T101000Z-1"), 1);
    assert_eq!(replies_to(&world, &c[0], "20261101T102000Z-1"), 0);

    let refused = world.fails(
        2,
        &[
            "comments",
            "gather",
            "--item",
            "followups:nobody-filed-this",
        ],
        "",
    );
    assert!(refused.contains("no batch of this host filed"), "{refused}");

    let unscoped = world.ok("gather", &["comments", "gather"], "");
    assert_eq!(
        unscoped["selected"],
        serde_json::json!({C: 1}),
        "only carol's is still unanswered: {unscoped}"
    );
    answer_and_settle(&mut world, &snapshot_of(&unscoped));
    assert_eq!(replies_to(&world, &c[0], "20261101T102000Z-1"), 1);
    assert_eq!(replies_to(&world, &a[0], "20261101T100000Z-1"), 1);
    assert!(
        watermark(&world, DESTINATION).is_some(),
        "the unscoped pass moves it"
    );
}
