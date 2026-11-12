//! Under a relation with a rollup, gathering reads the related tickets of each item a selected
//! comment sits on — one `task relations` and one read of their other ends — so a reply
//! confirming an occurrence on a symptom re-estimates it with its client-symptoms' occurrences
//! included, as `examples/symptoms-and-root-causes` rolls them up.

use crate::answering::{
    account, comment, destination_ops, item_text, launches, post_reply, quoted, replies_to,
    response,
};
use crate::examples::{filed, symptom_ticket, world_of};
use crate::journey::record;
use crate::world::answer;

const BATCH: &str = "nightly-2026-11-10";

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the symptom and its clients filed, then its comment answered"
)]
fn a_reply_confirming_a_symptom_re_estimates_it_with_its_client_symptoms_occurrences() {
    let mut world = world_of("symptoms-and-root-causes");
    world.seed("eng", 10);
    let d1 = world.draft(
        BATCH,
        "TIMEOUT-42 for acme",
        &["src/cache.rs"],
        "acme: TIMEOUT-42 on checkout.\n",
    );
    let d2 = world.draft(
        BATCH,
        "TIMEOUT-42 for globex",
        &["src/cache.rs"],
        "globex: TIMEOUT-42 on checkout.\n",
    );
    world.ok("file", &["prepare", "--batch", BATCH], "");
    let tickets = [
        symptom_ticket(
            "timeout-42-acme",
            &[&d1],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "acme")],
        )
        .severities("medium", "low", "retry the checkout"),
        symptom_ticket(
            "timeout-42-globex",
            &[&d2],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "globex")],
        )
        .severities("medium", "low", "retry the checkout"),
        symptom_ticket(
            "timeout-42",
            &[&d1, &d2],
            "symptom",
            &[("error_signature", "TIMEOUT-42")],
        )
        .severities("medium", "low", "retry the checkout"),
    ];
    for ticket in &tickets {
        world.ticket(BATCH, ticket);
    }
    world.ok("file", &["prepare", "--batch", BATCH], "");
    for slug in ["timeout-42-acme", "timeout-42-globex", "timeout-42"] {
        world.ok("file", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "client-symptom",
                "filed",
                &["timeout-42-acme"],
                "Verified.",
            ),
            answer(
                &d2,
                "client-symptom",
                "filed",
                &["timeout-42-globex"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("file", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("file", &["publish", "--batch", BATCH], "");
    world.ok("file", &["settle", "--batch", BATCH], "");
    let symptom = format!("eng:{}", filed(&world, "eng", "timeout-42").0);
    let before = item_text(&world, &symptom);
    assert!(
        before.contains("3 occurrences;"),
        "the symptom counts its two clients' at filing: {before}"
    );

    comment(
        &world,
        &symptom,
        "20261110T120000Z-1",
        "pat",
        "2026-11-10T12:00:00Z",
        "TIMEOUT-42 again for initech this morning.",
    );
    world.reset_phases();
    let gathered = world.ok("gather", &["comments", "gather"], "");
    assert_eq!(
        destination_ops(&world),
        ["comments-since", "relations", "read"],
        "the listing, then one relations read and one read of the related tickets: {gathered}"
    );
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
    let reply = post_reply(
        &world,
        one,
        "confirms",
        "One more client sees TIMEOUT-42: counted.",
    );
    account(
        feedback,
        batch,
        &[response(one, &reply, "confirms", "Counted the occurrence.")],
    );
    world.reset_phases();
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(
        destination_ops(&world),
        ["read", "update-many"],
        "{settled}"
    );
    assert_eq!(replies_to(&world, &symptom, "20261110T120000Z-1"), 1);
    let after = item_text(&world, &symptom);
    assert!(
        after.contains("4 occurrences; raised one level because it has 3 or more occurrences"),
        "its own evidence, the confirming reply and each client-symptom's occurrence: {after}"
    );
    assert_eq!(record(&after)["priority_estimate"], "medium", "{after}");
}
