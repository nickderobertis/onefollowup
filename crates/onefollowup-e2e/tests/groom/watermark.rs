//! The comment watermark across separate passes: a record moves only in the publish carrying its
//! ticket's outcome, so a partial, abandoned or concurrent pass never lets a person's comment
//! activity be passed over; and the overlap's verified-comment set, which keeps a comment
//! already verified from selecting its ticket again while a late-indexed or edited one still
//! does.

use std::time::Duration;

use serde_json::{Value, json};

use super::world::{Runner, World, ago, keep, verdict};

const WIDGETS: &str = "github.com/example/widgets";

/// A world over one repository holding `tickets`, each at Backlog, its comment overlap
/// `overlap` seconds.
fn world(tickets: &[&str], overlap: Option<u64>) -> World {
    let world = World::new();
    if let Some(overlap) = overlap {
        std::fs::write(
            world.root.join("onefollowup.yaml"),
            format!("groom: {{comment_overlap_seconds: {overlap}}}\n"),
        )
        .expect("the configuration");
    }
    world.repository(WIDGETS, &[("src/a.rs", "a\n")]);
    for id in tickets {
        world.ticket(
            id,
            &format!("widgets: ticket {id}"),
            "backlog",
            &[WIDGETS],
            &format!("The body of {id}."),
        );
    }
    world
}

fn kept(id: &str) -> Value {
    keep(id, &["src/a.rs"])
}

/// The tickets a `groom prepare` now selects.
fn selects(world: &World, runner: &dyn Runner) -> Vec<String> {
    let printed = world.json(runner, 0, &["groom", "prepare"]);
    if printed["shards"] == json!([]) {
        return Vec::new();
    }
    world
        .selected(printed["batch"].as_str().expect("a batch"))
        .into_iter()
        .map(|id| id.trim_start_matches("followups:").to_owned())
        .collect()
}

fn instant(text: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(text)
        .expect("an RFC 3339 time")
        .with_timezone(&chrono::Utc)
}

fn verified_at(world: &World, id: &str) -> chrono::DateTime<chrono::Utc> {
    instant(
        world.groom_record(id).expect("a record")["verified_at"]
            .as_str()
            .expect("a time"),
    )
}

/// When the comment `comment` of the ticket `id` last changed, as the store reads it.
fn changed_at(world: &World, id: &str, comment: &str) -> chrono::DateTime<chrono::Utc> {
    let read = world.read(id);
    let held = read
        .comments
        .expect("comments")
        .into_iter()
        .find(|one| one.id == comment)
        .expect("the comment");
    instant(&held.last_changed.expect("a time"))
}

/// Rewrite the ticket `id`'s `metadata:` block as one flow mapping on one line, as another
/// tool might leave it: the destination then refuses to set a key of it without rewriting the
/// rest, until a person tidies it. Answers the text it replaced.
fn flow_metadata(world: &World, id: &str) -> String {
    let text = world.ticket_text(id);
    let mut lines: Vec<String> = Vec::new();
    let mut entries: Vec<String> = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line == "metadata:" {
            inside = true;
            continue;
        }
        if inside && let Some(entry) = line.strip_prefix("  ") {
            entries.push(entry.to_owned());
            continue;
        }
        if inside {
            inside = false;
            lines.push(format!("metadata: {{{}}}", entries.join(", ")));
        }
        lines.push(line.to_owned());
    }
    assert!(!entries.is_empty(), "{id} holds a metadata block: {text}");
    std::fs::write(world.ticket_path(id), lines.join("\n") + "\n").expect("rewritten");
    text
}

pub fn a_partial_publish_leaves_the_refused_ticket_selected(runner: &dyn Runner) {
    let world = world(&["a", "b"], Some(1));
    world.comment("a", "c-a", &ago(7_200), &ago(7_200), "An older question.");
    world.groomed_pass(runner, kept);

    // Another process edits a's existing comment through the store.
    std::thread::sleep(Duration::from_millis(1_100));
    world.edit_comment("a", "c-a", "An older question, edited.");
    let edited = changed_at(&world, "a", "c-a");
    let a_before = world.groom_record("a").expect("a's first record");

    // The second pass verifies both; the destination refuses a's write and lands b's.
    world.edit_ticket("b", "The body of b.", "The edited body of b.");
    let batch = world.checked(runner, kept).expect("a batch");
    assert_eq!(world.selected(&batch).len(), 2);
    let tidy = flow_metadata(&world, "a");
    assert_eq!(
        world.groom_record("a").as_ref(),
        Some(&a_before),
        "a still reads"
    );
    let refused = world.json(runner, 1, &["groom", "publish", "--batch", &batch]);
    std::fs::write(world.ticket_path("a"), tidy).expect("a person tidies a");
    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(refused["report_json"].as_str().expect("a path"))
            .expect("report.json"),
    )
    .expect("JSON");
    let outcome = |id: &str| {
        report["outcomes"]
            .as_array()
            .expect("outcomes")
            .iter()
            .find(|one| one["ticket"] == World::qualified(id))
            .map(|one| one["outcome"].clone())
    };
    assert_eq!(outcome("a"), Some(json!("failed")), "{report:#}");
    assert_eq!(outcome("b"), Some(json!("recorded")));
    assert!(
        verified_at(&world, "b") > edited,
        "b's record moved past the edit"
    );
    assert_eq!(
        world.groom_record("a"),
        Some(a_before),
        "a's record did not move"
    );

    // A third pass, in a fresh process, still selects a; once published, its record is newer.
    assert_eq!(selects(&world, runner), ["a"]);
    let (_, published) = world.groomed_pass(runner, kept);
    assert_eq!(published["mismatched"], json!([]));
    assert!(verified_at(&world, "a") > edited);
}

pub fn an_abandoned_batch_moves_no_record(runner: &dyn Runner) {
    let world = world(&["a", "b", "c"], Some(1));
    world.groomed_pass(runner, kept);
    world.edit_ticket("a", "The body of a.", "The edited body of a.");
    world.edit_ticket("c", "The body of c.", "The edited body of c.");
    let records: Vec<Option<Value>> = ["a", "b", "c"]
        .iter()
        .map(|id| world.groom_record(id))
        .collect();
    let abandoned = world.checked(runner, kept).expect("a batch");
    assert_eq!(world.selected(&abandoned).len(), 2);
    assert_eq!(
        ["a", "b", "c"]
            .iter()
            .map(|id| world.groom_record(id))
            .collect::<Vec<_>>(),
        records,
        "a batch checked and never published moves no record"
    );
    assert_eq!(selects(&world, runner), ["a", "c"]);
}

pub fn a_comment_made_during_verification_selects_its_ticket_next_pass(runner: &dyn Runner) {
    let world = world(&["a", "b"], Some(1));
    let batch = world.checked(runner, kept).expect("a batch");
    // A person comments on a after prepare's comment read began, while the batch is verified.
    std::thread::sleep(Duration::from_millis(1_100));
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    world.comment("a", "c-during", &now, &now, "Is this still a problem?");
    world.json(runner, 0, &["groom", "publish", "--batch", &batch]);
    assert!(
        world.groom_record("a").is_some(),
        "its record was just written"
    );
    assert_eq!(selects(&world, runner), ["a"]);
}

pub fn records_newer_than_every_comment_select_nothing(runner: &dyn Runner) {
    let world = world(&["a", "b", "c"], Some(1));
    world.comment("a", "c-a", &ago(7_200), &ago(7_200), "An older question.");
    world.comment("b", "c-b", &ago(3_600), &ago(3_600), "Another.");
    let (_, published) = world.groomed_pass(runner, |id| match id {
        "b" => verdict(id, "keep", "high", None, &["src/a.rs"], Some("c")),
        // c rests on the more evidence, so it survives the cluster.
        "c" => keep(id, &["src/a.rs", "src/b.rs"]),
        _ => kept(id),
    });
    assert_eq!(published["mismatched"], json!([]));
    // c survives, its consolidation comment grooming's own and newer than its record.
    assert_eq!(world.status("b"), "cancelled");
    assert_eq!(world.comments("c").len(), 1);
    let printed = world.json(runner, 0, &["groom", "prepare"]);
    assert_eq!(printed["selected"], 0, "{printed:#}");
}

pub fn the_overlap_holds_what_was_verified_and_reads_what_was_not(runner: &dyn Runner) {
    let world = world(&["a", "b", "c"], None);
    let before = ago(300);
    for id in ["a", "b", "c"] {
        world.comment(
            id,
            &format!("c-{id}"),
            &before,
            &before,
            "A question from minutes ago.",
        );
    }
    world.groomed_pass(runner, kept);
    for id in ["a", "b", "c"] {
        let record = world.groom_record(id).expect("a record");
        let held = &record["overlap_comments"];
        assert_eq!(held.as_array().map(Vec::len), Some(1), "{id}: {record:#}");
        assert_eq!(held[0]["id"], format!("c-{id}"));
        assert_eq!(
            instant(held[0]["edited_at"].as_str().expect("a time")),
            instant(&before)
        );
    }
    assert_eq!(
        selects(&world, runner),
        Vec::<String>::new(),
        "already verified"
    );

    // A comment the comment read returned late: created inside the overlap before b's record.
    let late = (verified_at(&world, "b") - chrono::Duration::seconds(600))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    world.comment("b", "c-late", &late, &late, "Indexed late.");
    assert_eq!(selects(&world, runner), ["b"]);

    // c's verified comment, edited after verification through the store.
    world.edit_comment("c", "c-c", "A question from minutes ago, edited.");
    assert_eq!(selects(&world, runner), ["b", "c"]);
}
