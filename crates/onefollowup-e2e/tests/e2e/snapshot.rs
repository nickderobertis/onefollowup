//! `snapshot`: whether a batch holds anything to verify, answered from prepare's own input-set
//! step with a real `batch_preparing` hook that logs each time it runs. An empty batch leaves no
//! working file; a non-empty one opens the pass the next `prepare` continues, running the hook
//! once across both verbs; and `snapshot` never reaches the destination. Journey (a) records the
//! `empty-batch-store-operations` budget's telemetry.

use std::path::Path;

use serde_json::{Value, json};

use crate::telemetry;
use crate::world::{Ticket, World, answer};

const BATCH: &str = "nightly-2026-11-20";

/// The budget the empty-batch journey measures.
pub const BUDGET: &str = "empty-batch-store-operations";

/// A host whose `batch_preparing` hook is the logging one `hook` writes.
const HOOKED: &str = "host_label: build-host-1\nhooks:\n  batch_preparing: [inputs-hook]\n";

/// The hook: append one line to `preparing.log` per run, then print `inputs`.
fn hook(world: &World, inputs: &Value) {
    let root = world.shell_root();
    std::fs::write(
        world.root.join("inputs.json"),
        json!({"schema_version": 1, "inputs": inputs}).to_string(),
    )
    .expect("the hook's inputs");
    world.script(
        "inputs-hook",
        &format!(
            "cat >> {root}/preparing.log\necho >> {root}/preparing.log\ncat {root}/inputs.json"
        ),
    );
}

/// How many times the hook ran.
fn hook_runs(world: &World) -> usize {
    std::fs::read_to_string(world.root.join("preparing.log"))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

/// Every working file under the world's work root that names `batch`.
fn working_files(world: &World, batch: &str) -> Vec<String> {
    fn walk(directory: &Path, batch: &str, into: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.to_string_lossy().contains(batch) {
                into.push(path.display().to_string());
            }
            if path.is_dir() {
                walk(&path, batch, into);
            }
        }
    }
    let mut found = Vec::new();
    walk(&world.work(), batch, &mut found);
    found
}

fn snapshot(world: &mut World, args: &[&str]) -> Value {
    let mut all = vec!["snapshot", "--batch", BATCH];
    all.extend_from_slice(args);
    world.reset_phases();
    world.ok("snapshot", &all, "")
}

/// The store operations `snapshot` printed, as `(op, source, destination, items)`.
fn operations(printed: &Value) -> Vec<(String, String, bool, u64)> {
    printed["store_operations"]
        .as_array()
        .expect("snapshot prints its store operations")
        .iter()
        .map(|operation| {
            (
                operation["op"].as_str().unwrap_or_default().to_owned(),
                operation["source"].as_str().unwrap_or_default().to_owned(),
                operation["destination"] == true,
                operation["items"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn an_empty_batch_is_reported_empty_leaves_no_working_file_and_records_its_store_operations() {
    let mut world = World::new(&["followups"], HOOKED);
    world.seed("followups", 400);
    for other in 0..30 {
        let batch = format!("nightly-2026-10-{other:02}");
        for at in 0..4 {
            world.draft(
                &batch,
                &format!("widgets: export {at} of batch {other} drops rows"),
                &["src/export.rs"],
                "Seen.\n",
            );
        }
    }
    hook(&world, &json!([]));

    let aliases = ["nightly-2026-11-20-retry", "nightly-2026-11-20-late"];
    let printed = snapshot(&mut world, &["--alias", aliases[0], "--alias", aliases[1]]);
    assert_eq!(printed["schema_version"], 1, "{printed}");
    assert_eq!(printed["batch"], BATCH, "{printed}");
    assert_eq!(printed["drafts"], 0, "{printed}");
    assert_eq!(printed["inputs"], 0, "{printed}");
    assert_eq!(printed["empty"], true, "{printed}");
    assert_eq!(hook_runs(&world), 1, "the hook ran once");
    assert_eq!(
        working_files(&world, BATCH),
        Vec::<String>::new(),
        "an empty batch leaves no working file"
    );
    assert!(!world.work().join(format!("state/{BATCH}.json")).exists());
    assert_eq!(
        world.destination_operations(),
        Vec::new(),
        "snapshot made no call against the destination"
    );

    let operations = operations(&printed);
    let part = |destination: bool| {
        operations
            .iter()
            .filter(|(_, _, on, _)| *on == destination)
            .map(|(op, source, _, items)| format!("{op} on {source} ({items} items)"))
            .collect::<Vec<_>>()
            .join("; ")
    };
    telemetry::record(
        BUDGET,
        operations.len(),
        &format!(
            "{} store operations for one snapshot of an empty batch with 2 aliases. Drafts source: [{}], its find carrying one search per id ({BATCH}, {}, {}). Destination: [{}]",
            operations.len(),
            part(false),
            aliases[0],
            aliases[1],
            part(true)
        ),
    );

    // Nothing of the first snapshot is seen later: a draft recorded since makes the batch
    // non-empty, and the hook runs again.
    world.draft(
        BATCH,
        "widgets: a resize loses the layout",
        &["src/a.rs"],
        "Seen.\n",
    );
    let printed = snapshot(&mut world, &[]);
    assert_eq!(printed["empty"], false, "{printed}");
    assert_eq!(printed["drafts"], 1, "{printed}");
    assert_eq!(hook_runs(&world), 2, "a later snapshot starts afresh");
}

#[test]
fn a_batch_with_drafts_is_reported_with_them_counted_and_its_pass_opened() {
    let mut world = World::new(&["followups"], HOOKED);
    world.seed("followups", 20);
    hook(&world, &json!([]));
    world.draft(
        BATCH,
        "widgets: a resize loses the layout",
        &["src/a.rs"],
        "Seen.\n",
    );
    world.draft(
        "nightly-2026-11-20-retry",
        "widgets: a save drops unicode",
        &["src/b.rs"],
        "Seen.\n",
    );
    world.draft(
        "nightly-2026-11-19",
        "widgets: another batch's",
        &["src/c.rs"],
        "Seen.\n",
    );

    let printed = snapshot(&mut world, &["--alias", "nightly-2026-11-20-retry"]);
    assert_eq!(printed["empty"], false, "{printed}");
    assert_eq!(
        printed["drafts"], 2,
        "the batch's draft and its alias's: {printed}"
    );
    assert_eq!(printed["inputs"], 0, "{printed}");
    assert_eq!(
        world.destination_operations(),
        Vec::new(),
        "snapshot made no call against the destination"
    );
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join(format!("state/{BATCH}.json")))
            .expect("a non-empty batch's state is written"),
    )
    .expect("JSON");
    assert_eq!(state["inputs"].as_array().map(Vec::len), Some(2), "{state}");
    assert_eq!(state["reads"], json!([]), "no destination reads: {state}");
    assert!(!world.work().join(format!("tasks/{BATCH}/task.md")).exists());
    assert!(
        !world
            .work()
            .join(format!("dispositions/{BATCH}.json"))
            .exists()
    );

    // A second snapshot within the pass reports the same set and runs no hook.
    let again = snapshot(&mut world, &[]);
    assert_eq!(again["drafts"], 2, "{again}");
    assert_eq!(hook_runs(&world), 1, "an open pass runs no hook");
}

#[test]
fn a_batch_whose_only_input_is_a_hook_input_is_not_empty() {
    let mut world = World::new(&["followups"], HOOKED);
    hook(
        &world,
        &json!([{"id": "budget-overrun-1", "source": "budget-overrun",
                 "summary": "the e2e budget overran", "detail": "Measured 74 against 60."}]),
    );
    let printed = snapshot(&mut world, &[]);
    assert_eq!(printed["empty"], false, "{printed}");
    assert_eq!(printed["drafts"], 0, "{printed}");
    assert_eq!(printed["inputs"], 1, "{printed}");
    assert!(world.work().join(format!("state/{BATCH}.json")).exists());
}

/// `printed` with the world's root and its drafts-source operations taken out: what a
/// `prepare` that continues a snapshot's pass and one that opened it itself share.
fn comparable(world: &World, printed: &Value) -> Value {
    // The root as it appears inside a JSON string, escaped: a Windows path's backslashes are
    // doubled there.
    let quoted = Value::from(world.root.display().to_string()).to_string();
    let root = &quoted[1..quoted.len() - 1];
    let mut printed: Value =
        serde_json::from_str(&printed.to_string().replace(root, "<root>")).expect("JSON");
    if let Some(operations) = printed["store_operations"].as_array_mut() {
        operations.retain(|operation| operation["destination"] == true);
    }
    printed
}

fn read_relative(world: &World, path: &str) -> String {
    std::fs::read_to_string(world.work().join(path))
        .expect("prepare wrote it")
        .replace(&world.root.display().to_string(), "<root>")
}

#[test]
fn a_prepare_after_a_snapshot_continues_its_pass_and_matches_a_plain_prepare() {
    let inputs = json!([{"id": "budget-overrun-1", "source": "budget-overrun",
                         "summary": "the e2e budget overran", "detail": "Measured 74."}]);
    let mut snapshotted = World::new(&["followups"], HOOKED);
    snapshotted.seed("followups", 30);
    hook(&snapshotted, &inputs);
    snapshotted.draft(BATCH, "gizmos: unrelated item 3", &["src/a.rs"], "Seen.\n");
    snapshotted.draft(
        BATCH,
        "widgets: a save drops unicode",
        &["src/b.rs"],
        "Seen.\n",
    );
    // The second store is the first one's copy, drafts and all.
    let mut plain = World::new(&["followups"], HOOKED);
    plain.seed("followups", 30);
    hook(&plain, &inputs);
    for (id, text) in snapshotted.items("drafts") {
        std::fs::write(plain.root.join(format!("drafts-root/tasks/{id}.md")), text)
            .expect("the copied draft");
    }

    let snapped = snapshot(&mut snapshotted, &[]);
    assert_eq!(snapped["empty"], false, "{snapped}");
    snapshotted.reset_phases();
    let continued = snapshotted.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(
        hook_runs(&snapshotted),
        1,
        "the hook ran once across both verbs"
    );
    assert!(
        continued["store_operations"]
            .as_array()
            .expect("operations")
            .iter()
            .all(|operation| operation["destination"] == true),
        "the continuing prepare searched no drafts again: {continued}"
    );
    let alone = plain.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(hook_runs(&plain), 1);
    assert_eq!(continued["drafts"], 2, "{continued}");
    assert_eq!(continued["inputs"], 1, "{continued}");
    assert!(
        snapshotted
            .destination_operations()
            .iter()
            .any(|(_, op, _, _)| op == "find"),
        "prepare made its destination read: {continued}"
    );
    assert_eq!(
        comparable(&snapshotted, &continued),
        comparable(&plain, &alone)
    );
    for file in [
        format!("tasks/{BATCH}/task.md"),
        format!("dispositions/{BATCH}.json"),
    ] {
        assert_eq!(
            read_relative(&snapshotted, &file),
            read_relative(&plain, &file),
            "{file}"
        );
    }
}

#[test]
fn a_failing_hook_fails_snapshot_naming_it_with_nothing_written() {
    let mut world = World::new(&["followups"], HOOKED);
    world.draft(
        BATCH,
        "widgets: a resize loses the layout",
        &["src/a.rs"],
        "Seen.\n",
    );
    world.script("inputs-hook", "echo 'no budgets file' >&2\nexit 3");
    let failed = world.fails(1, &["snapshot", "--batch", BATCH], "");
    assert!(
        failed.contains(
            "the `hooks.batch_preparing` command `inputs-hook` exited 3: no budgets file"
        ),
        "{failed}"
    );
    assert!(failed.contains("nothing was snapshotted"), "{failed}");
    assert_eq!(working_files(&world, BATCH), Vec::<String>::new());

    // Recovery: once the hook prints its inputs, the batch snapshots.
    hook(&world, &json!([]));
    let printed = snapshot(&mut world, &[]);
    assert_eq!(printed["drafts"], 1, "{printed}");
}

#[test]
fn a_snapshot_after_a_settled_pass_writes_nothing_and_prepare_opens_the_pass_as_before() {
    let settled = |world: &mut World| {
        hook(world, &json!([]));
        let draft = world.draft(
            BATCH,
            "widgets: a resize loses the layout",
            &["src/a.rs"],
            "Seen.\n",
        );
        world.ok("setup", &["prepare", "--batch", BATCH], "");
        world.ticket(
            BATCH,
            &Ticket::new(
                "resize-loses-layout",
                "widgets: a resize loses the layout",
                &[&draft],
            ),
        );
        world.ok(
            "setup",
            &["ticket", "copy", "--batch", BATCH, "resize-loses-layout"],
            "",
        );
        world.answer(
            BATCH,
            &[answer(
                &draft,
                "root-cause",
                "filed",
                &["resize-loses-layout"],
                "Verified.",
            )],
        );
        world.ok("setup", &["check", "--batch", BATCH], "");
        world.ok("setup", &["publish", "--batch", BATCH], "");
        world.ok("setup", &["settle", "--batch", BATCH], "");
    };
    let mut snapshotted = World::new(&["followups"], HOOKED);
    settled(&mut snapshotted);
    let mut plain = World::new(&["followups"], HOOKED);
    settled(&mut plain);
    for world in [&mut snapshotted, &mut plain] {
        world.draft(
            BATCH,
            "widgets: a save drops unicode",
            &["src/b.rs"],
            "Seen.\n",
        );
    }

    let state = snapshotted.work().join(format!("state/{BATCH}.json"));
    let before = std::fs::read(&state).expect("the settled batch's state");
    let printed = snapshot(&mut snapshotted, &[]);
    assert_eq!(printed["empty"], false, "{printed}");
    assert_eq!(
        printed["drafts"], 2,
        "the carried draft and the new one: {printed}"
    );
    assert_eq!(
        std::fs::read(&state).expect("still there"),
        before,
        "state unchanged"
    );
    assert_eq!(snapshotted.destination_operations(), Vec::new());

    let after = snapshotted.ok("prepare", &["prepare", "--batch", BATCH], "");
    let alone = plain.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(
        hook_runs(&snapshotted),
        3,
        "the first pass, the snapshot, the prepare"
    );
    assert_eq!(hook_runs(&plain), 2);
    assert_eq!(after["drafts"], 2, "{after}");
    let normalized = |world: &World, printed: &Value| {
        serde_json::from_str::<Value>(
            &printed
                .to_string()
                .replace(&world.root.display().to_string(), "<root>"),
        )
        .expect("JSON")
    };
    assert_eq!(normalized(&snapshotted, &after), normalized(&plain, &alone));
}
