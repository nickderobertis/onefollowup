//! `groom publish` through onefollowup's real store status-writing path: what the status read
//! still admits is closed or folded with its comment, marker and record; what it does not —
//! Todo short of definitely outdated, claimed, or moved by another process after check — is
//! left for a person, byte for byte unchanged and listed in the report; every verified ticket
//! holds its record; a re-run after another process changed what landed sends nothing and
//! reports each change as a mismatch.

use serde_json::{Value, json};

use onefollowup::formats::groom::record_key;

use super::world::{NAMESPACE, Runner, World, grooming_marker, keep, verdict};

const WIDGETS: &str = "github.com/example/widgets";

/// Each ticket: its id and status.
const TICKETS: [(&str, &str); 19] = [
    ("b-fixed", "backlog"),
    ("b-obsolete", "backlog"),
    ("b-dup", "backlog"),
    ("t-fixed", "todo"),
    ("t-dup", "todo"),
    ("t-medium", "todo"),
    ("t-surv", "todo"),
    ("m-todo", "todo"),
    ("m-b1", "backlog"),
    ("m-b2", "backlog"),
    ("k-keep", "backlog"),
    ("q-keep", "queued"),
    ("u-unres", "backlog"),
    ("m-b3", "backlog"),
    ("g-gone", "backlog"),
    ("c-closed", "backlog"),
    ("p-surv", "in-progress"),
    ("b-dup2", "backlog"),
    ("n-norepo", "backlog"),
];

/// What the world's agents answer each ticket with.
fn verdicts(fix: &str) -> impl Fn(&str) -> Value + '_ {
    move |id| match id {
        "b-fixed" | "t-fixed" | "m-todo" => {
            verdict(id, "fixed", "high", Some(fix), &["src/a.rs"], None)
        }
        "b-obsolete" | "m-b3" => verdict(id, "obsolete", "medium", Some(fix), &["src/a.rs"], None),
        "g-gone" | "c-closed" => verdict(id, "fixed", "high", Some(fix), &["src/a.rs"], None),
        "b-dup2" => verdict(id, "keep", "high", None, &["src/b.rs"], Some("p-surv")),
        "t-medium" => verdict(id, "fixed", "medium", Some(fix), &["src/a.rs"], None),
        "b-dup" | "m-b1" | "m-b2" => {
            verdict(id, "keep", "high", None, &["src/b.rs"], Some("t-surv"))
        }
        "t-dup" => verdict(id, "keep", "high", None, &["src/b.rs"], Some("q-keep")),
        "u-unres" => verdict(id, "unresolved", "low", None, &[], None),
        _ => keep(id, &["src/b.rs"]),
    }
}

fn ids(list: &Value) -> Vec<String> {
    let mut found: Vec<String> = list
        .as_array()
        .expect("a list")
        .iter()
        .map(|one| {
            one["ticket"]
                .as_str()
                .expect("an id")
                .trim_start_matches("followups:")
                .to_owned()
        })
        .collect();
    found.sort();
    found
}

/// The ticket's title, content and every metadata key but its `groom` record, as the store
/// reads them.
fn without_record(world: &World, id: &str) -> Value {
    let item = world.read(id).item.expect("the ticket is there");
    let mut metadata = item.metadata.clone();
    metadata.remove(&record_key(NAMESPACE));
    json!({"title": item.title, "content": item.content, "metadata": metadata})
}

pub fn publish_closes_what_its_status_read_admits_and_leaves_the_rest(runner: &dyn Runner) {
    let world = World::new();
    world.repository(WIDGETS, &[("src/a.rs", "a\n"), ("src/b.rs", "b\n")]);
    let fix = world.commit(WIDGETS, &[("src/a.rs", "a fixed\n")], "fix the save");
    for (id, status) in TICKETS {
        let repositories: &[&str] = if id == "n-norepo" { &[] } else { &[WIDGETS] };
        world.ticket(
            id,
            &format!("widgets: ticket {id}"),
            status,
            repositories,
            &format!("The body of {id}."),
        );
    }
    let batch = world.checked(runner, verdicts(&fix)).expect("a batch"); // After check and before publish's status read, another process moves four tickets, a
    // person closes one by hand, and another deletes one.
    world.move_to("m-todo", "in-progress");
    world.move_to("m-b1", "todo");
    world.move_to("m-b2", "in-progress");
    world.move_to("m-b3", "todo");
    world.move_to("c-closed", "done");
    std::fs::remove_file(world.ticket_path("g-gone")).expect("g-gone is deleted");
    let untouched = [
        "t-dup", "t-medium", "m-todo", "m-b1", "m-b2", "m-b3", "c-closed", "k-keep", "q-keep",
        "u-unres", "n-norepo",
    ];
    let before: Vec<(&str, String, Value)> = untouched
        .iter()
        .map(|id| (*id, world.ticket_text(id), without_record(&world, id)))
        .collect();

    let published = world.json(runner, 0, &["groom", "publish", "--batch", &batch]);
    assert_eq!(published["batch"], batch.as_str());
    assert_eq!(published["mismatched"], json!([]), "{published:#}"); // The five closed tickets and the five recorded ones, the survivors among them.
    assert_eq!(published["confirmed"], 10, "{published:#}");
    assert!(
        published["quota_reads"]
            .as_u64()
            .is_some_and(|reads| reads >= 1),
        "the governor read the destination's quota, apart from the store operations: {published:#}"
    );
    assert_eq!(published["governor"], json!({"waits": []}), "{published:#}");

    for (id, status, mark) in [
        ("b-fixed", "done", "fixed"),
        ("t-fixed", "done", "fixed"),
        ("b-obsolete", "cancelled", "obsolete"),
        ("b-dup", "cancelled", "duplicate"),
        ("b-dup2", "cancelled", "duplicate"),
    ] {
        assert_eq!(world.status(id), status, "{id} reads closed");
        let comments = world.comments(id);
        assert_eq!(comments.len(), 1, "{id}: one comment: {comments:?}");
        assert!(
            comments[0]
                .trim_end()
                .ends_with(&grooming_marker(&batch, mark)),
            "{id}: {}",
            comments[0]
        );
        let record = world.groom_record(id).expect("its groom record");
        assert_eq!(record["batch"], batch.as_str());
        assert_eq!(record["commit"], fix.as_str(), "verified at main");
    }
    let duplicate = world.read("b-dup").item.expect("b-dup");
    assert_eq!(
        duplicate.duplicate_of().as_deref(),
        Some("followups:t-surv"),
        "linked to the survivor the priority order picks"
    );
    assert!(world.comments("b-dup")[0].contains("followups:t-surv"));

    // The survivor's one consolidation comment names the one duplicate closed, says it is Todo.
    let survivor = world.comments("t-surv");
    assert_eq!(survivor.len(), 1, "{survivor:?}");
    assert!(
        survivor[0]
            .trim_end()
            .ends_with(&grooming_marker(&batch, "survivor"))
    );
    assert!(survivor[0].contains("followups:b-dup"), "{}", survivor[0]);
    assert!(
        !survivor[0].contains("followups:m-b1") && !survivor[0].contains("followups:m-b2"),
        "it names only duplicates shown closed: {}",
        survivor[0]
    );
    assert!(
        survivor[0].contains("This ticket is todo"),
        "{}",
        survivor[0]
    );
    let claimed = world.comments("p-surv");
    assert_eq!(claimed.len(), 1, "{claimed:?}");
    assert!(
        claimed[0].contains("followups:b-dup2")
            && claimed[0].contains("This ticket is in-progress"),
        "{}",
        claimed[0]
    );

    // Left for a person: open, unchanged byte for byte, with no record. Kept and verified:
    // body and every other key unchanged, a record at main.
    for (id, text, read) in &before {
        let now = world.ticket_text(id);
        if *id == "n-norepo" {
            assert_eq!(
                &now, text,
                "a ticket naming no repository has no record to write"
            );
        } else if ["k-keep", "q-keep", "u-unres"].contains(id) {
            assert_eq!(
                &without_record(&world, id),
                read,
                "{id}'s body and other keys"
            );
            assert!(world.comments(id).is_empty(), "{id} has no new comment");
            let record = world.groom_record(id).expect("a verified ticket's record");
            assert_eq!(record["commit"], fix.as_str());
            assert_eq!(record["batch"], batch.as_str());
        } else {
            assert_eq!(&now, text, "{id} is left byte for byte as it was");
        }
    }
    assert_eq!(world.status("m-todo"), "in-progress");
    assert_eq!(world.status("m-b1"), "todo");
    assert_eq!(world.status("t-medium"), "todo");

    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(published["report_json"].as_str().expect("a path"))
            .expect("report.json"),
    )
    .expect("JSON");
    assert_eq!(
        ids(&report["left_for_person"]),
        [
            "c-closed", "g-gone", "m-b1", "m-b2", "m-b3", "m-todo", "t-dup", "t-medium"
        ],
        "{report:#}"
    );
    for left in report["left_for_person"].as_array().expect("a list") {
        assert!(!left["evidence"].as_str().unwrap_or_default().is_empty());
        assert!(!left["reason"].as_str().unwrap_or_default().is_empty());
        assert!(left["confidence"].is_string() && left["verdict"].is_string());
    }
    let left = |id: &str| {
        report["left_for_person"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|one| one["ticket"] == World::qualified(id))
            .cloned()
            .expect("listed")
    };
    assert_eq!(left("t-medium")["confidence"], "medium");
    assert_eq!(left("t-medium")["deciding_commit"], fix.as_str());
    assert_eq!(left("t-dup")["survivor"], "followups:q-keep");
    assert_eq!(left("m-b1")["survivor"], "followups:t-surv");
    assert_eq!(left("m-b1")["status"]["category"], "todo");
    assert!(
        left("m-b3")["reason"]
            .as_str()
            .is_some_and(|why| why.contains("Todo")),
        "a Backlog close short of definitely outdated, promoted to Todo"
    );
    assert!(
        left("c-closed")["reason"]
            .as_str()
            .is_some_and(|why| why.contains("no longer open"))
    );
    assert!(
        left("g-gone")["reason"]
            .as_str()
            .is_some_and(|why| why.contains("no such ticket"))
    );
    assert_eq!(
        ids(&report["outcomes"]).len(),
        TICKETS.len(),
        "every outcome"
    );
    let markdown =
        std::fs::read_to_string(published["report"].as_str().expect("a path")).expect("report.md");
    for id in ["m-b1", "m-b2", "m-b3", "m-todo", "t-dup", "t-medium"] {
        assert!(markdown.contains(&World::qualified(id)), "{id}: {markdown}");
    }
    let kept: Vec<String> = std::fs::read_dir(world.batch(&batch))
        .expect("the batch")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let mut kept = kept;
    kept.sort();
    assert_eq!(
        kept,
        ["plan.json", "receipts.json", "report.json", "report.md"],
        "publish retires every other working file"
    );

    // Another process changes what landed; publish run again sends nothing and reports each.
    world.move_to("b-fixed", "backlog");
    let obsolete = world.read("b-obsolete");
    let comment = &obsolete.comments.expect("comments")[0];
    world.edit_comment(
        "b-obsolete",
        &comment.id,
        &format!("Edited.\n\n{}", grooming_marker(&batch, "obsolete")),
    );
    let mut record = world.groom_record("k-keep").expect("a record");
    record["verdict"] = json!("unresolved");
    world.record("k-keep", &record);
    let fixed = world.read("t-fixed").comments.expect("comments")[0]
        .id
        .clone();
    world.remove_comment("t-fixed", &fixed);
    std::fs::remove_file(world.ticket_path("u-unres")).expect("u-unres is deleted");
    let again = world.json(runner, 1, &["groom", "publish", "--batch", &batch]);
    let operations: Vec<&str> = again["store_operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|one| one["op"].as_str().expect("an op"))
        .collect();
    assert!(
        operations
            .iter()
            .all(|op| !["update-many", "comment-add-many"].contains(op)),
        "a fully receipted batch sends nothing: {operations:?}"
    );
    assert_eq!(
        ids(&again["mismatched"]),
        ["b-fixed", "b-obsolete", "k-keep", "t-fixed", "u-unres"]
    );
    for one in again["mismatched"].as_array().expect("mismatches") {
        assert_ne!(one["expected"], one["found"], "{one}");
    }
    let mismatch = |id: &str| {
        again["mismatched"]
            .as_array()
            .expect("mismatches")
            .iter()
            .find(|one| one["ticket"] == World::qualified(id))
            .cloned()
            .expect("a mismatch")
    };
    assert!(
        mismatch("b-fixed")["found"]
            .as_str()
            .is_some_and(|found| found.contains("backlog"))
    );
    assert!(
        mismatch("b-obsolete")["found"]
            .as_str()
            .is_some_and(|found| found.contains("Edited."))
    );
    assert!(
        mismatch("k-keep")["found"]
            .as_str()
            .is_some_and(|found| found.contains("unresolved"))
    );
    assert!(
        mismatch("t-fixed")["found"]
            .as_str()
            .is_some_and(|found| found.contains("no such comment"))
    );
    assert_eq!(
        mismatch("u-unres")["expected"],
        "the ticket",
        "a deleted ticket"
    );
    assert_eq!(again["confirmed"], 5, "every untouched ticket is confirmed");
    let markdown =
        std::fs::read_to_string(again["report"].as_str().expect("a path")).expect("report.md");
    assert!(markdown.contains("## Mismatches"), "{markdown}");
    assert!(markdown.contains("`followups:k-keep`"), "{markdown}");
}

pub fn publish_refuses_a_batch_no_check_passed(runner: &dyn Runner) {
    let world = World::new();
    world.repository(WIDGETS, &[("src/a.rs", "a\n")]);
    world.ticket("w1", "widgets: w1", "backlog", &[WIDGETS], "The body.");
    let prepared = world.json(runner, 0, &["groom", "prepare"]);
    let unchecked = prepared["batch"].as_str().expect("a batch").to_owned();
    let refused = |batch: &str, code: i32, says: &str| {
        let ran = runner.run(&world, &["groom", "publish", "--batch", batch]);
        assert_eq!(ran.code, code, "{}", ran.stderr);
        assert!(ran.stderr.contains(says), "{says}: {}", ran.stderr);
    };
    refused(&unchecked, 2, "groom check");
    refused("../elsewhere", 2, "not a grooming batch id");

    // A plan, or receipts, of another batch.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let checked = world.checked(runner, |id| keep(id, &[])).expect("a batch");
    let plan = |batch: &str| world.batch(batch).join("plan.json");
    std::fs::copy(plan(&checked), plan(&unchecked)).expect("copied");
    refused(&unchecked, 2, &format!("is batch {checked}'s plan"));
    let mut own: Value =
        serde_json::from_str(&std::fs::read_to_string(plan(&checked)).expect("plan"))
            .expect("JSON");
    own["batch"] = json!(unchecked);
    std::fs::write(plan(&unchecked), own.to_string()).expect("written");
    let receipts = |batch: &str| world.batch(batch).join("receipts.json");
    std::fs::write(
        receipts(&unchecked),
        json!({"schema_version": 1, "batch": checked, "destination": "followups",
               "namespace": NAMESPACE, "tickets": [], "survivors": []})
        .to_string(),
    )
    .expect("written");
    refused(&unchecked, 2, &format!("holds batch {checked}'s receipts"));

    // A batch whose prepared record is gone cannot start a publish.
    let batch_json = world.batch(&checked).join("batch.json");
    let held = std::fs::read_to_string(&batch_json).expect("batch.json");
    std::fs::remove_file(&batch_json).expect("removed");
    refused(&checked, 1, "batch.json: could not be read");
    std::fs::write(&batch_json, held).expect("restored");

    // A comment template writing a marker itself is refused before any ticket is closed.
    std::fs::write(
        world.root.join("marker.md.j2"),
        "Closed.\n\n<!-- orchestrator:follow-up-comment run=\"x\" kind=\"groom\" verdict=\"fixed\" -->\n",
    )
    .expect("a template");
    std::fs::write(
        world.root.join("onefollowup.yaml"),
        "groom: {comment_template: marker.md.j2}\n",
    )
    .expect("the configuration");
    world.ticket("w2", "widgets: w2", "backlog", &[WIDGETS], "Another body.");
    let fix = world.commit(WIDGETS, &[("src/a.rs", "a fixed\n")], "fix");
    let closing = world
        .checked(runner, |id| {
            verdict(id, "fixed", "high", Some(&fix), &["src/a.rs"], None)
        })
        .expect("a batch");
    refused(&closing, 2, "writes a comment marker itself");
    assert_eq!(world.status("w2"), "backlog", "nothing was closed");
    assert!(world.groom_record("w2").is_none(), "nothing was written");
}

pub fn a_refused_close_is_sent_again_by_the_rerun(runner: &dyn Runner) {
    let world = World::new();
    world.repository(WIDGETS, &[("src/a.rs", "a\n"), ("src/b.rs", "b\n")]);
    let fix = world.commit(WIDGETS, &[("src/a.rs", "a fixed\n")], "fix the save");
    for id in ["a", "d", "s"] {
        world.ticket(
            id,
            &format!("widgets: ticket {id}"),
            "backlog",
            &[WIDGETS],
            &format!("The body of {id}."),
        );
    }
    let now = super::world::ago(60);
    world.comment("d", "c-d", &now, &now, "Is this the same as s?");
    let batch = world
        .checked(runner, |id| match id {
            "a" => verdict(id, "fixed", "high", Some(&fix), &["src/a.rs"], None),
            "d" => verdict(id, "keep", "high", None, &["src/b.rs"], Some("s")),
            _ => keep(id, &["src/a.rs", "src/b.rs"]),
        })
        .expect("a batch");
    // Another tool left d's metadata on one line, which the destination will not set a key of.
    world.edit_ticket(
        "d",
        "priority: low\n",
        "priority: low\nmetadata: {other.key: 1}\n",
    );
    let refused = world.json(runner, 1, &["groom", "publish", "--batch", &batch]);
    assert_eq!(world.status("a"), "done");
    assert_eq!(world.comments("a").len(), 1);
    assert_eq!(world.status("d"), "backlog", "d's close was refused");
    assert_eq!(world.comments("d").len(), 1, "only the person's comment");
    assert!(
        world.comments("s").is_empty(),
        "the survivor's comment waits for its duplicate's close"
    );
    let markdown =
        std::fs::read_to_string(refused["report"].as_str().expect("a path")).expect("report.md");
    assert!(
        markdown.contains("## Failed") && markdown.contains("`followups:d`"),
        "{markdown}"
    );

    // A person tidies d; the re-run sends only d's close, then its comment and the survivor's.
    world.edit_ticket(
        "d",
        "metadata: {other.key: 1}\n",
        "metadata:\n  other.key: 1\n",
    );
    let published = world.json(runner, 0, &["groom", "publish", "--batch", &batch]);
    assert_eq!(published["mismatched"], json!([]), "{published:#}");
    assert_eq!(
        world.comments("a").len(),
        1,
        "a's comment is not posted again"
    );
    assert_eq!(world.status("d"), "cancelled");
    let comments = world.comments("d");
    assert_eq!(comments.len(), 2, "{comments:?}");
    assert!(comments[1].contains(&grooming_marker(&batch, "duplicate")));
    let survivor = world.comments("s");
    assert_eq!(survivor.len(), 1);
    assert!(survivor[0].contains("followups:d"), "{}", survivor[0]);
}

pub fn a_ticket_under_review_is_claimed_and_never_closed_or_folded(runner: &dyn Runner) {
    let world = World::new();
    world.repository(WIDGETS, &[("src/a.rs", "a\n"), ("src/b.rs", "b\n")]);
    let fix = world.commit(WIDGETS, &[("src/a.rs", "a fixed\n")], "fix the save");
    for (id, status) in [
        ("r-held", "in-review"),
        ("r-surv", "in-review"),
        ("r-dup", "backlog"),
        ("r-moved", "backlog"),
    ] {
        world.ticket(
            id,
            &format!("widgets: ticket {id}"),
            status,
            &[WIDGETS],
            &format!("The body of {id}."),
        );
    }
    let batch = world
        .checked(runner, |id| match id {
            "r-held" | "r-moved" => verdict(id, "fixed", "high", Some(&fix), &["src/a.rs"], None),
            "r-dup" => verdict(id, "keep", "high", None, &["src/b.rs"], Some("r-surv")),
            _ => keep(id, &["src/b.rs"]),
        })
        .expect("prepare selects tickets under review");
    // After check, the work on another ticket stops for review.
    world.move_to("r-moved", "in-review");
    let held = world.ticket_text("r-held");
    let moved = world.ticket_text("r-moved");

    let published = world.json(runner, 0, &["groom", "publish", "--batch", &batch]);
    assert_eq!(published["mismatched"], json!([]), "{published:#}");
    assert_eq!(
        world.ticket_text("r-held"),
        held,
        "a claimed ticket is never closed"
    );
    assert_eq!(
        world.ticket_text("r-moved"),
        moved,
        "the status read finds it claimed"
    );
    assert_eq!(world.status("r-held"), "in-review");
    assert_eq!(world.status("r-moved"), "in-review");
    assert_eq!(
        world.status("r-dup"),
        "cancelled",
        "folded into the claimed survivor"
    );
    assert!(world.comments("r-held").is_empty());
    let survivor = world.comments("r-surv");
    assert_eq!(survivor.len(), 1, "{survivor:?}");
    assert!(
        survivor[0].contains("followups:r-dup") && survivor[0].contains("This ticket is in-review"),
        "{}",
        survivor[0]
    );

    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(published["report_json"].as_str().expect("a path"))
            .expect("report.json"),
    )
    .expect("JSON");
    assert_eq!(
        ids(&report["left_for_person"]),
        ["r-held", "r-moved"],
        "{report:#}"
    );
    for left in report["left_for_person"].as_array().expect("a list") {
        assert!(
            left["reason"]
                .as_str()
                .is_some_and(|why| why.contains("claimed")),
            "{left}"
        );
    }
}
