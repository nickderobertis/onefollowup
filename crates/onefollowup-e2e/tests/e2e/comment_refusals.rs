//! What each comment verb refuses, and what it tells the person to do instead: a batch that
//! filed nothing, a snapshot never launched, a directory holding none, a dry run past
//! `snapshot.launchable_for`, a feedback file `comments launches` did not write, an account that
//! is missing, unreadable, misnamed, out of order or names no reply, a ticket written after the
//! launch, a `--launched` naming another batch's feedback — and a withdrawal of an item a person
//! deferred, which waits in `pending.json` while the item keeps the person's status.

use serde_json::{Value, json};

use crate::answering::{
    account, comment, item_path, item_text, launches, pending, post_reply, quoted, response,
    withdraw_ticket,
};
use crate::comments::{A, B, three_batches};
use crate::journey::front;

fn problems(checked: &Value) -> String {
    checked["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|problem| problem.as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "every refusal of the four verbs, in turn"
)]
fn each_comment_verb_refuses_what_breaks_its_rules_and_names_what_to_do() {
    let (mut world, a, b, _) = three_batches(4);
    let refused = world.fails(1, &["--destination", "nowhere", "comments", "gather"], "");
    assert!(
        refused.contains("nowhere"),
        "the store refuses a destination it does not configure: {refused}"
    );
    let refused = world.fails(
        2,
        &["comments", "gather", "--batch", "nightly-2026-12-31"],
        "",
    );
    assert!(
        refused.contains("has filed no ticket on this host"),
        "{refused}"
    );

    comment(
        &world,
        &a[0],
        "20261101T100000Z-1",
        "alice",
        "2026-11-01T10:00:00Z",
        "First question.",
    );
    comment(
        &world,
        &a[0],
        "20261101T100100Z-1",
        "alice",
        "2026-11-01T10:01:00Z",
        "Second question.",
    );
    comment(
        &world,
        &b[0],
        "20261101T101000Z-1",
        "bob",
        "2026-11-01T10:10:00Z",
        "Please close this.",
    );
    // A person defers batch B's item before the pass.
    let deferred = std::fs::read_to_string(item_path(&world, &b[0])).expect("the item");
    std::fs::write(
        item_path(&world, &b[0]),
        deferred.replacen("status: backlog", "status: draft", 1),
    )
    .expect("deferred");

    // An item of this host that no batch here filed: its comment is left, naming why.
    world.item(
        "followups",
        "filed-elsewhere",
        &crate::world::filed_item(
            "widgets: filed elsewhere",
            "nightly-2026-10-01",
            "filed-elsewhere",
            &json!({}),
            "medium",
        ),
        &crate::world::filed_body("Filed by a batch whose records are not here."),
    );
    comment(
        &world,
        "followups:filed-elsewhere",
        "20261101T090000Z-1",
        "erin",
        "2026-11-01T09:00:00Z",
        "Who owns this?",
    );
    let elsewhere_gathered = world.ok("gather", &["comments", "gather", "--dry-run"], "");
    let elsewhere_record: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/snapshot.json",
            elsewhere_gathered["snapshot"].as_str().expect("a snapshot")
        ))
        .expect("the record"),
    )
    .expect("JSON");
    assert!(
        elsewhere_record["left"]
            .as_array()
            .expect("left")
            .iter()
            .any(|left| left["comment"] == "20261101T090000Z-1"
                && left["reason"] == "no batch of this host filed the item, so none answers it"),
        "{elsewhere_record}"
    );
    // A watermark that is no instant is refused, naming its file.
    let mark = world.work().join("feedback/.watermark-followups.json");
    std::fs::create_dir_all(mark.parent().expect("a parent")).expect("a directory");
    std::fs::write(
        &mark,
        r#"{"schema": 1, "board": "followups", "since": "yesterday", "queried_at": "yesterday"}"#,
    )
    .expect("written");
    let refused = world.fails(1, &["comments", "gather"], "");
    assert!(
        refused.contains(".watermark-followups.json") && refused.contains("no instant"),
        "{refused}"
    );
    std::fs::remove_file(&mark).expect("removed");
    let unadmitted = world.ok(
        "gather",
        &[
            "--set",
            "batch_id_pattern=weekly-.*",
            "comments",
            "gather",
            "--dry-run",
        ],
        "",
    );
    assert_eq!(unadmitted["selected"], json!({}), "{unadmitted}");
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/snapshot.json",
            unadmitted["snapshot"].as_str().expect("a snapshot")
        ))
        .expect("the record"),
    )
    .expect("JSON");
    assert!(
        record["left"]
            .as_array()
            .expect("left")
            .iter()
            .filter(|left| left["issue"] != "followups:filed-elsewhere")
            .all(|left| left["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("is not a batch id `batch_id_pattern` admits")),
        "{record}"
    );
    let dry = world.ok("gather", &["comments", "gather", "--dry-run"], "");
    let expired = dry["snapshot"].as_str().expect("a snapshot").to_owned();
    let refused = world.fails(
        2,
        &[
            "--set",
            "snapshot.launchable_for=0s",
            "comments",
            "launches",
            "--snapshot",
            &expired,
        ],
        "",
    );
    assert!(
        refused.contains("longer ago than `snapshot.launchable_for` (0s)"),
        "{refused}"
    );
    assert!(refused.contains("comments gather --dry-run"), "{refused}");

    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let refused = world.fails(2, &["comments", "settle", "--snapshot", &snapshot], "");
    assert!(refused.contains("was never launched"), "{refused}");
    assert!(
        refused.contains("comments launches --snapshot"),
        "{refused}"
    );
    let refused = world.fails(
        2,
        &[
            "comments",
            "launches",
            "--snapshot",
            &world.root.display().to_string(),
        ],
        "",
    );
    assert!(refused.contains("holds no comment snapshot"), "{refused}");
    // A snapshot of another format, holding no gathering, and one whose instant is broken.
    let legacy = world.work().join("feedback/.snapshots/legacy");
    std::fs::create_dir_all(&legacy).expect("a directory");
    std::fs::copy(
        format!("{snapshot}/snapshot.json"),
        legacy.join("snapshot.json"),
    )
    .expect("copied");
    let refused = world.fails(
        2,
        &[
            "comments",
            "launches",
            "--snapshot",
            &legacy.display().to_string(),
        ],
        "",
    );
    assert!(refused.contains("holds no gathering.json"), "{refused}");
    std::fs::copy(
        format!("{snapshot}/gathering.json"),
        legacy.join("gathering.json"),
    )
    .expect("copied");
    let text = std::fs::read_to_string(legacy.join("snapshot.json")).expect("the record");
    let broken = text.replacen("\"queried_at\": \"", "\"queried_at\": \"not ", 1);
    std::fs::write(legacy.join("snapshot.json"), broken).expect("written");
    let refused = world.fails(
        1,
        &[
            "comments",
            "launches",
            "--snapshot",
            &legacy.display().to_string(),
        ],
        "",
    );
    assert!(refused.contains("`queried_at` is no instant"), "{refused}");
    std::fs::remove_dir_all(&legacy).expect("removed");

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
    let (fa, fb) = (feedback_of(A), feedback_of(B));
    let check = |world: &mut crate::world::World, feedback: &std::path::Path| {
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
    };

    let refused = world.fails(
        2,
        &[
            "comments",
            "check",
            "--feedback",
            &fa.with_file_name("nothing.md").display().to_string(),
        ],
        "",
    );
    assert!(refused.contains("there is no feedback file"), "{refused}");
    let stray = fa.with_file_name("stray.md");
    std::fs::write(&stray, "Not a feedback file.\n").expect("written");
    let refused = world.fails(
        2,
        &[
            "comments",
            "check",
            "--feedback",
            &stray.display().to_string(),
        ],
        "",
    );
    assert!(refused.contains("names no snapshot"), "{refused}");
    std::fs::remove_file(&stray).expect("removed");

    let elsewhere = fa
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the feedback directory")
        .join("nightly-2026-11-03")
        .join(fa.file_name().expect("a name"));
    std::fs::create_dir_all(elsewhere.parent().expect("a parent")).expect("a directory");
    std::fs::copy(&fa, &elsewhere).expect("copied");
    let refused = world.fails(
        2,
        &[
            "comments",
            "check",
            "--feedback",
            &elsewhere.display().to_string(),
        ],
        "",
    );
    assert!(
        refused.contains("quotes no comment for batch `nightly-2026-11-03`"),
        "{refused}"
    );
    std::fs::remove_file(&elsewhere).expect("removed");

    let checked = check(&mut world, &fa);
    assert!(
        problems(&checked).contains("there is no response account"),
        "{checked}"
    );
    let accounted = fa.with_file_name(format!(
        "{}.responses.json",
        fa.file_stem().expect("a stem").to_string_lossy()
    ));
    std::fs::write(&accounted, "{not JSON").expect("written");
    let checked = check(&mut world, &fa);
    assert!(
        problems(&checked).contains("the response account does not read"),
        "{checked}"
    );

    // Out of order, answered twice, misnamed, and naming no reply.
    let qa = quoted(&fa);
    assert_eq!(qa.len(), 2);
    let second = response(&qa[1], "r2", "does-not-confirm", "Answered.");
    let first = response(&qa[0], "", "does-not-confirm", "Answered.");
    std::fs::write(
        &accounted,
        serde_json::to_string_pretty(&json!({
            "schema": 3,
            "run": B,
            "feedback": "another.md",
            "responses": [second, first, response(&qa[0], "r1", "does-not-confirm", "Again.")],
        }))
        .expect("JSON"),
    )
    .expect("written");
    let checked = check(&mut world, &fa);
    let said = problems(&checked);
    assert_eq!(checked["sound"], false);
    for expected in [
        format!("the account names batch `{B}`"),
        "the account names the feedback `another.md`".to_owned(),
        "quoted comment 20261101T100000Z-1".to_owned(),
        "answered twice or out of the order".to_owned(),
        "names no reply id".to_owned(),
    ] {
        assert!(said.contains(&expected), "{expected}: {said}");
    }

    // A ticket written after the launch.
    let added = qa[0].ticket.with_file_name("written-after-launch.md");
    std::fs::write(
        &added,
        "---\ntitle: \"widgets: new\"\n---\n## Root cause\n\nNew.\n",
    )
    .expect("written");
    let checked = check(&mut world, &fa);
    assert!(
        problems(&checked).contains("ticket `written-after-launch` was written after the launch"),
        "{checked}"
    );
    std::fs::remove_file(&added).expect("removed");

    // Batch B withdraws the item a person deferred: the account is sound, and the withdrawal
    // waits while the person's status stands.
    let qb = quoted(&fb);
    withdraw_ticket(&qb[0].ticket);
    let reply = post_reply(&world, &qb[0], "does-not-confirm", "Withdrawn, as asked.");
    account(
        &fb,
        B,
        &[response(&qb[0], &reply, "does-not-confirm", "Withdrew it.")],
    );
    let checked = check(&mut world, &fb);
    assert_eq!(checked["sound"], true, "{checked}");

    let refused = world.fails(
        2,
        &[
            "comments",
            "settle",
            "--snapshot",
            &snapshot,
            "--launched",
            &format!("{A}={}", fb.display()),
        ],
        "",
    );
    assert!(refused.contains(&format!("not batch `{A}`'s")), "{refused}");

    let settled = world.ok(
        "settle",
        &[
            "comments",
            "settle",
            "--snapshot",
            &snapshot,
            "--launched",
            &format!("{B}={}", fb.display()),
        ],
        "",
    );
    assert_eq!(settled["needs_attention"], true, "{settled}");
    let held = pending(&world);
    assert!(
        held.iter().any(|entry| entry["write"] == "withdrawal"
            && entry["state"] == "withheld"
            && entry["error"]
                .as_str()
                .unwrap_or_default()
                .contains("a status a person decided on")),
        "{held:?}"
    );
    assert_eq!(
        front(&item_text(&world, &b[0]))["status"],
        "draft",
        "the person's deferral stands"
    );
}
