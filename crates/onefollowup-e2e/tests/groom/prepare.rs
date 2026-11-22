//! `groom prepare` over the generated workload: one shard per `--shard-size` tickets, each task
//! carrying what its agent needs; `--dry-run` answering the same counts and writing nothing;
//! and a destination with nothing to select answering `"shards": []`.

use serde_json::Value;

use super::workload;
use super::world::{Runner, World};

fn counts(printed: &Value) -> (Value, Value, Value, Vec<Value>) {
    (
        printed["selected"].clone(),
        printed["unchanged"].clone(),
        printed["protected"].clone(),
        printed["shards"]
            .as_array()
            .expect("shards")
            .iter()
            .map(|shard| shard["tickets"].clone())
            .collect(),
    )
}

/// Every file under the world's grooming directory but the clones.
fn written(world: &World) -> Vec<String> {
    fn walk(directory: &std::path::Path, into: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.ends_with("clones") {
                continue;
            }
            if path.is_dir() {
                walk(&path, into);
            } else {
                into.push(path.display().to_string());
            }
        }
    }
    let mut found = Vec::new();
    walk(&world.root.join(".onefollowup/groom"), &mut found);
    found
}

pub fn shards_carry_each_agents_task(runner: &dyn Runner) {
    let world = World::new();
    let generated = workload::generate(&world, 6, 4, 48);
    let open = generated.open();
    assert_eq!(open.len(), 42);
    let claimed = open
        .iter()
        .filter(|ticket| matches!(ticket.status, "queued" | "in-progress"))
        .count();

    let dry = world.json(
        runner,
        0,
        &["groom", "prepare", "--shard-size", "10", "--dry-run"],
    );
    assert!(
        written(&world).is_empty(),
        "a dry run writes no batch: {:?}",
        written(&world)
    );
    let dry_batch = dry["batch"].as_str().expect("a batch").to_owned();
    assert!(!world.batch(&dry_batch).exists());

    let printed = world.json(runner, 0, &["groom", "prepare", "--shard-size", "10"]);
    assert_eq!(printed["schema_version"], 1);
    assert_eq!(printed["destination"], "followups");
    assert_eq!(
        printed["selected"], 42,
        "every open ticket is new: {printed}"
    );
    assert_eq!(printed["unchanged"], 0);
    assert_eq!(printed["protected"], claimed);
    assert_eq!(
        counts(&dry),
        counts(&printed),
        "a dry run answers the same counts"
    );
    let operations: Vec<(&str, u64)> = printed["store_operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|one| {
            (
                one["op"].as_str().expect("op"),
                one["items"].as_u64().expect("items"),
            )
        })
        .collect();
    assert_eq!(
        operations,
        [("list", 42), ("comments-since", 0), ("read", 42)],
        "three store operations: the open listing, the comment read and the selected tickets' read"
    );

    let batch = printed["batch"].as_str().expect("a batch");
    assert!(batch.starts_with("groom-"), "{batch}");
    let shards = printed["shards"].as_array().expect("shards");
    let sizes: Vec<u64> = shards
        .iter()
        .map(|shard| shard["tickets"].as_u64().expect("a count"))
        .collect();
    assert_eq!(
        sizes,
        [10, 10, 10, 10, 2],
        "one shard per --shard-size tickets"
    );

    let snapshot_path = world.batch(batch).join("snapshot.json");
    let snapshot: Value =
        serde_json::from_str(&std::fs::read_to_string(&snapshot_path).expect("the snapshot"))
            .expect("JSON");
    let listed: Vec<&str> = snapshot["tickets"]
        .as_array()
        .expect("tickets")
        .iter()
        .map(|one| one["ticket"].as_str().expect("an id"))
        .collect();
    let expected: Vec<String> = open
        .iter()
        .map(|ticket| format!("followups:{}", ticket.id))
        .collect();
    assert_eq!(
        listed, expected,
        "the snapshot holds every open ticket, no closed one"
    );

    let record = world.batch_record(batch);
    let mut seen = Vec::new();
    for (index, shard) in shards.iter().enumerate() {
        assert_eq!(shard["shard"], format!("{batch}-{}", index + 1));
        let task_path = shard["task"].as_str().expect("a task");
        assert!(std::path::Path::new(task_path).is_absolute());
        let verdicts = shard["verdicts"].as_str().expect("a verdicts path");
        assert!(std::path::Path::new(verdicts).is_absolute());
        let task = std::fs::read_to_string(task_path).expect("the task is written");
        assert!(task.contains(verdicts), "the task names its verdicts file");
        assert!(
            task.contains(&snapshot_path.display().to_string()),
            "the task names the open-ticket snapshot"
        );
        for part in [
            "\"schema_version\": 1, \"ticket\":",
            "\"deciding_commit\": \"<40-hex commit> | null\"",
            "\"seam_repository\": \"<host/owner/name> | null\"",
            "## The rules `groom check` holds your verdicts to",
            "never `low`",
            "is never closed or folded",
            "a claimed ticket first, then a Todo ticket",
        ] {
            assert!(task.contains(part), "the task states {part:?}:\n{task}");
        }
        let tickets: Vec<&str> = record["shards"][index]["tickets"]
            .as_array()
            .expect("tickets")
            .iter()
            .map(|one| one.as_str().expect("an id"))
            .collect();
        assert_eq!(tickets.len() as u64, sizes[index]);
        for ticket in &tickets {
            assert!(
                task.contains(&format!("### {ticket}: ")),
                "{ticket} is in its shard's task"
            );
            let generated = generated
                .tickets
                .iter()
                .find(|one| format!("followups:{}", one.id) == *ticket)
                .expect("a generated ticket");
            let repository = generated.repository.clone();
            let commit = &generated_commit(&generated_repositories(&record), &repository);
            let clone = world
                .root
                .join(".onefollowup/groom/clones")
                .join(&repository);
            assert!(
                task.contains(&format!(
                    "`{repository}`: `{}` at `{commit}`",
                    clone.display()
                )),
                "the task names {repository}'s clone and fetched main commit:\n{task}"
            );
            assert!(
                clone.join("src/module_0.rs").is_file(),
                "the clone is checked out"
            );
        }
        seen.extend(tickets.iter().map(|one| (*one).to_owned()));
    }
    assert_eq!(
        seen, expected,
        "every selected ticket is in exactly one shard"
    );
    for repository in &generated.repositories {
        assert_eq!(
            generated_commit(&generated_repositories(&record), &repository.repository),
            repository.commit,
            "prepare fetched {}'s main",
            repository.repository
        );
    }
}

fn generated_repositories(record: &Value) -> Vec<(String, String)> {
    record["repositories"]
        .as_array()
        .expect("repositories")
        .iter()
        .map(|one| {
            (
                one["repository"].as_str().expect("a repository").to_owned(),
                one["commit"].as_str().expect("a commit").to_owned(),
            )
        })
        .collect()
}

fn generated_commit(repositories: &[(String, String)], repository: &str) -> String {
    repositories
        .iter()
        .find(|(name, _)| name == repository)
        .map(|(_, commit)| commit.clone())
        .unwrap_or_else(|| panic!("{repository} was fetched"))
}

pub fn nothing_to_select_answers_no_shard(runner: &dyn Runner) {
    let world = World::new();
    world.repository("github.com/example/widgets", &[("src/a.rs", "a\n")]);
    world.ticket(
        "closed",
        "widgets: a closed ticket",
        "done",
        &["github.com/example/widgets"],
        "Fixed long ago.",
    );
    let printed = world.json(runner, 0, &["groom", "prepare"]);
    assert_eq!(printed["selected"], 0);
    assert_eq!(printed["unchanged"], 0);
    assert_eq!(printed["shards"], serde_json::json!([]));
    assert!(
        written(&world).is_empty(),
        "nothing selected writes no batch"
    );
}

pub fn a_zero_shard_size_is_refused(runner: &dyn Runner) {
    let world = World::new();
    let ran = runner.run(&world, &["groom", "prepare", "--shard-size", "0"]);
    assert_eq!(ran.code, 2, "{}", ran.stderr);
    assert!(ran.stderr.contains("`--shard-size` is 0"), "{}", ran.stderr);
}

pub fn an_unreachable_repository_fails_naming_it(runner: &dyn Runner) {
    let world = World::new();
    world.ticket(
        "lost",
        "gadgets: a ticket about a repository nobody can fetch",
        "backlog",
        &["github.com/example/missing"],
        "Its repository is not there.",
    );
    let ran = runner.run(&world, &["groom", "prepare"]);
    assert_eq!(ran.code, 1, "{}{}", ran.stderr, ran.stdout);
    assert!(
        ran.stderr
            .contains("cloning https://github.com/example/missing.git failed"),
        "{}",
        ran.stderr
    );
    assert!(ran.stderr.contains("next:"), "{}", ran.stderr);
}
