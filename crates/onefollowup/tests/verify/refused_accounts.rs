//! Scripted answering agents writing unsound accounts — a reply attributed to a batch that does
//! not own the comment's issue, a ticket changed on an issue no quoted comment sits on, a
//! response for a comment the gathering did not quote, and a quoted comment left with no
//! response — each refused by `comments check` naming the comment or ticket; `comments settle`
//! then writes no ticket change, posts nothing and leaves the watermark where it was.

use crate::answering::{
    DESTINATION, account, comment, edit_ticket, item_text, launches, post_reply, quoted,
    replies_to, response, watermark,
};
use crate::comments::{A, B, C, three_batches};

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "four unsound accounts, then the settle they hold"
)]
fn an_unsound_account_is_refused_by_check_and_settle_writes_nothing_for_it() {
    let (mut world, a, b, c) = three_batches(4);
    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "Which release did this start in?",
    );
    comment(
        &world,
        &b[0],
        "20261101T101000Z-1",
        "bob",
        "2026-11-01T10:10:00Z",
        "Is this the same as the margins bug?",
    );
    comment(
        &world,
        &c[0],
        "20261101T102000Z-1",
        "carol",
        "2026-11-01T10:20:00Z",
        "Seen again today.",
    );
    comment(
        &world,
        &c[0],
        "20261101T102500Z-1",
        "dave",
        "2026-11-01T10:25:00Z",
        "And on my machine.",
    );
    comment(
        &world,
        &c[1],
        "20261101T103000Z-1",
        "renovate[bot]",
        "2026-11-01T10:30:00Z",
        "Update dependency.",
    );
    let before_b1 = item_text(&world, &b[1]);

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
    let feedback_of = |batch: &str| {
        launched
            .iter()
            .find(|(one, _)| one == batch)
            .map(|(_, feedback)| feedback.clone())
            .expect("a launch")
    };
    let (fa, fb, fc) = (feedback_of(A), feedback_of(B), feedback_of(C));
    let (qa, qb, qc) = (quoted(&fa), quoted(&fb), quoted(&fc));

    // Batch A answers its own comment, and also bob's, which sits on batch B's issue.
    let mut responses = Vec::new();
    for one in qa.iter().chain(&qb) {
        let id = post_reply(&world, one, "does-not-confirm", "Answered.");
        responses.push(response(one, &id, "does-not-confirm", "Answered."));
    }
    account(&fa, A, &responses);
    // Batch B answers its comment, and changes the ticket of an issue no quoted comment sits on.
    let reply = post_reply(&world, &qb[0], "does-not-confirm", "Not the same bug.");
    account(
        &fb,
        B,
        &[response(
            &qb[0],
            &reply,
            "does-not-confirm",
            "Compared them.",
        )],
    );
    let unquoted = qb[0].ticket.with_file_name("export-loses-margins.md");
    edit_ticket(
        &unquoted,
        "Read the window size once per save.",
        "Read the margins once per save.",
    );
    // Batch C answers its first comment, accounts for the bot's it was never quoted, and leaves
    // its second quoted comment with no response.
    let reply = post_reply(&world, &qc[0], "confirms", "Counted.");
    let mut bot = qc[0].clone();
    bot.issue.clone_from(&c[1]);
    bot.comment = "20261101T103000Z-1".to_owned();
    account(
        &fc,
        C,
        &[
            response(&qc[0], &reply, "confirms", "Counted."),
            response(
                &bot,
                "20261101T103000Z-1-reply",
                "does-not-confirm",
                "Answered the bot.",
            ),
        ],
    );

    let checked = check(&mut world, &fa);
    assert_eq!(checked["sound"], false, "{checked}");
    assert!(
        checked["problems"]
            .as_array()
            .expect("problems")
            .iter()
            .any(|problem| {
                let problem = problem.as_str().unwrap_or_default();
                problem.contains("20261101T101000Z-1")
                    && problem.contains(&format!("batch `{B}` owns"))
            }),
        "the reply attributed to batch A on B's issue is named: {checked}"
    );
    let checked = check(&mut world, &fb);
    assert_eq!(checked["sound"], false, "{checked}");
    assert!(
        checked["problems"]
            .as_array()
            .expect("problems")
            .iter()
            .any(|problem| problem
                .as_str()
                .unwrap_or_default()
                .contains("ticket `export-loses-margins` changed")),
        "{checked}"
    );
    let checked = check(&mut world, &fc);
    assert_eq!(checked["sound"], false, "{checked}");
    let problems: Vec<&str> = checked["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|problem| problem.as_str().unwrap_or_default())
        .collect();
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("20261101T103000Z-1")
                && problem.contains("did not quote")),
        "{problems:?}"
    );
    assert!(
        problems
            .iter()
            .any(|problem| problem.contains("20261101T102500Z-1")
                && problem.contains("has no response")),
        "{problems:?}"
    );

    let replies_before = [
        replies_to(&world, &a[0], "20261101T100000Z-1"),
        replies_to(&world, &b[0], "20261101T101000Z-1"),
        replies_to(&world, &c[0], "20261101T102000Z-1"),
        replies_to(&world, &c[0], "20261101T102500Z-1"),
    ];
    let texts_before: Vec<String> = [&a[0], &b[0], &c[0]]
        .iter()
        .map(|item| item_text(&world, item))
        .collect();
    world.reset_phases();
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(settled["needs_attention"], true, "{settled}");
    assert!(
        settled["settled"]
            .as_array()
            .expect("settled")
            .iter()
            .all(|launch| launch["writes"] == 0
                && !launch["problems"]
                    .as_array()
                    .expect("its problems")
                    .is_empty()),
        "{settled}"
    );
    assert!(
        !crate::answering::destination_ops(&world).contains(&"update-many".to_owned()),
        "no ticket write: {settled}"
    );
    assert_eq!(
        item_text(&world, &b[1]),
        before_b1,
        "the unquoted ticket's item is untouched"
    );
    let texts_after: Vec<String> = [&a[0], &b[0], &c[0]]
        .iter()
        .map(|item| item_text(&world, item))
        .collect();
    assert_eq!(texts_after, texts_before, "settle posted and wrote nothing");
    let replies_after = [
        replies_to(&world, &a[0], "20261101T100000Z-1"),
        replies_to(&world, &b[0], "20261101T101000Z-1"),
        replies_to(&world, &c[0], "20261101T102000Z-1"),
        replies_to(&world, &c[0], "20261101T102500Z-1"),
    ];
    assert_eq!(replies_after, replies_before);
    assert!(
        watermark(&world, DESTINATION).is_none(),
        "the watermark stays where it was"
    );
}

fn check(world: &mut crate::world::World, feedback: &std::path::Path) -> serde_json::Value {
    world.ok(
        "check",
        &[
            "comments",
            "check",
            "--feedback",
            &feedback.display().to_string(),
        ],
        "",
    )
}
