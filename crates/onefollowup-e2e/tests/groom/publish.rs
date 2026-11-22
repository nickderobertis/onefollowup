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
const TICKETS: [(&str, &str); 13] = [
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
];

/// What the world's agents answer each ticket with.
fn verdicts(fix: &str) -> impl Fn(&str) -> Value + '_ {
    move |id| match id {
        "b-fixed" | "t-fixed" | "m-todo" => {
            verdict(id, "fixed", "high", Some(fix), &["src/a.rs"], None)
        }
        "b-obsolete" => verdict(id, "obsolete", "medium", Some(fix), &["src/a.rs"], None),
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
        world.ticket(
            id,
            &format!("widgets: ticket {id}"),
            status,
            &[WIDGETS],
            &format!("The body of {id}."),
        );
    }
    let batch = world.checked(runner, verdicts(&fix)).expect("a batch");

    // Another process moves three tickets after check and before publish's status read.
    world.move_to("m-todo", "in-progress");
    world.move_to("m-b1", "todo");
    world.move_to("m-b2", "in-progress");
    let untouched = [
        "t-dup", "t-medium", "m-todo", "m-b1", "m-b2", "k-keep", "q-keep", "u-unres",
    ];
    let before: Vec<(&str, String, Value)> = untouched
        .iter()
        .map(|id| (*id, world.ticket_text(id), without_record(&world, id)))
        .collect();

    let published = world.json(runner, 0, &["groom", "publish", "--batch", &batch]);
    assert_eq!(published["batch"], batch.as_str());
    assert_eq!(published["mismatched"], json!([]), "{published:#}");
    // The four closed tickets and the four recorded ones, the survivor among them.
    assert_eq!(published["confirmed"], 8, "{published:#}");

    for (id, status, mark) in [
        ("b-fixed", "done", "fixed"),
        ("t-fixed", "done", "fixed"),
        ("b-obsolete", "cancelled", "obsolete"),
        ("b-dup", "cancelled", "duplicate"),
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

    // Left for a person: open, unchanged byte for byte, with no record. Kept and verified:
    // body and every other key unchanged, a record at main.
    for (id, text, read) in &before {
        let now = world.ticket_text(id);
        if ["k-keep", "q-keep", "u-unres"].contains(id) {
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
        ["m-b1", "m-b2", "m-todo", "t-dup", "t-medium"],
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
    assert_eq!(
        ids(&report["outcomes"]).len(),
        TICKETS.len(),
        "every outcome"
    );
    let markdown =
        std::fs::read_to_string(published["report"].as_str().expect("a path")).expect("report.md");
    for id in ["m-b1", "m-b2", "m-todo", "t-dup", "t-medium"] {
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
        ["b-fixed", "b-obsolete", "k-keep"]
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
    let batch = prepared["batch"].as_str().expect("a batch");
    let ran = runner.run(&world, &["groom", "publish", "--batch", batch]);
    assert_eq!(ran.code, 2, "{}", ran.stderr);
    assert!(ran.stderr.contains("groom check"), "{}", ran.stderr);
    let ran = runner.run(&world, &["groom", "publish", "--batch", "../elsewhere"]);
    assert_eq!(ran.code, 2, "{}", ran.stderr);
    assert!(world.groom_record("w1").is_none(), "nothing was written");
}
