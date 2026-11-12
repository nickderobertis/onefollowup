//! A write `pending.json` holds is retried only while a person has not decided otherwise since:
//! a priority a person set after the write was withheld stands over its re-estimate, and a
//! status a person protected stands over its withdrawal — each keeping the whole ticket write
//! withheld and the next pass needing attention.

use serde_json::Value;

use crate::answering::{
    DESTINATION, account, comment, edit_ticket, item_text, launches, pending, post_reply, quoted,
    response, withdraw_ticket,
};
use crate::comments::three_batches;
use crate::journey::front;
use crate::world::World;

const FIX: &str = "Read the window size once per save.";
const PERSONS_FIX: &str = "Read the window size once, at the start of the save.";

/// Gather, launch and answer the one comment, the agent changing its ticket with `change`;
/// then a person edits the item's content before settle, so the write is withheld.
fn withheld(world: &mut World, item: &str, change: &dyn Fn(&std::path::Path)) {
    comment(
        world,
        item,
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "Is this right?",
    );
    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let launched = launches(&world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    ));
    let (batch, feedback) = &launched[0];
    let one = &quoted(feedback)[0];
    change(&one.ticket);
    let reply = post_reply(world, one, "does-not-confirm", "Changed the ticket.");
    account(
        feedback,
        batch,
        &[response(one, &reply, "does-not-confirm", "Changed it.")],
    );
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
    let native = item.split_once(':').expect("qualified").1;
    world.edit_item(DESTINATION, native, FIX, PERSONS_FIX);
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(settled["needs_attention"], true, "{settled}");
    assert_eq!(pending(world).len(), 1, "{:?}", pending(world));
}

/// The next pass: nothing new to answer, settle retrying what pending holds.
fn next_pass(world: &mut World) -> Value {
    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    );
    world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    )
}

#[test]
fn a_priority_a_person_set_after_the_write_was_withheld_stands_over_its_retry() {
    let (mut world, a, _, _) = three_batches(4);
    withheld(&mut world, &a[0], &|ticket| {
        edit_ticket(ticket, "- Severity: high\n", "- Severity: critical\n");
        edit_ticket(
            ticket,
            "Severity with the workaround: medium",
            "Severity with the workaround: high",
        );
    });
    let held = pending(&world);
    assert_eq!(
        held[0]["intended"]["priority"], "high",
        "the withheld write re-estimates: {held:?}"
    );

    // The person takes their content edit back, and sets the priority themselves.
    let native = a[0].split_once(':').expect("qualified").1.to_owned();
    world.edit_item(DESTINATION, &native, PERSONS_FIX, FIX);
    world.edit_item(DESTINATION, &native, "priority: medium", "priority: urgent");

    let settled = next_pass(&mut world);
    assert_eq!(settled["needs_attention"], true, "{settled}");
    let item = item_text(&world, &a[0]);
    assert_eq!(
        front(&item)["priority"],
        "urgent",
        "the person's priority stands: {item}"
    );
    assert!(
        item.contains("Severity with the workaround: medium"),
        "nothing of the write landed: {item}"
    );
    let kept = pending(&world);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0]["state"], "withheld");
    assert!(
        kept[0]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("priority a person set"),
        "{kept:?}"
    );
}

#[test]
fn a_status_a_person_protected_after_the_withdrawal_was_withheld_stands_over_its_retry() {
    let (mut world, a, _, _) = three_batches(4);
    withheld(&mut world, &a[0], &|ticket| withdraw_ticket(ticket));
    let held = pending(&world);
    assert_eq!(held[0]["write"], "withdrawal", "{held:?}");
    assert_eq!(held[0]["intended"]["status"], "cancelled", "{held:?}");

    // The person takes their content edit back, and defers the item.
    let native = a[0].split_once(':').expect("qualified").1.to_owned();
    world.edit_item(DESTINATION, &native, PERSONS_FIX, FIX);
    world.edit_item(DESTINATION, &native, "status: backlog", "status: draft");

    let settled = next_pass(&mut world);
    assert_eq!(settled["needs_attention"], true, "{settled}");
    let item = item_text(&world, &a[0]);
    assert_eq!(
        front(&item)["status"],
        "draft",
        "the person's deferral stands: {item}"
    );
    let kept = pending(&world);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0]["state"], "withheld");
    assert!(
        kept[0]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("a status a person decided on"),
        "{kept:?}"
    );
}
