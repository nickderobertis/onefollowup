//! The comment pass over a groomed destination: tickets a follow-up batch filed through the
//! verify loop, then groomed — one folded into the other — and `comments gather` passing over
//! every grooming comment as maintenance while still answering a person's comment beside them.

use serde_json::{Value, json};

use crate::answering::{DESTINATION, comment, file, now};
use crate::world::{REPOSITORY, World};

const BATCH: &str = "nightly-2026-11-21";

/// Point `https://` at a local repository holding `REPOSITORY`'s `main`, through the world's
/// own git configuration, so grooming fetches no network.
fn local_repository(world: &World) {
    let repos = world.root.join("repos");
    let path = repos.join(format!("{REPOSITORY}.git"));
    std::fs::create_dir_all(&path).expect("the repository's directory");
    // A `file:` URL with forward slashes: git's configuration reads a backslash in a quoted
    // section name as an escape.
    let url = crate::groom::world::shell_path(&repos);
    let url = url.strip_prefix('/').unwrap_or(&url).to_owned();
    std::fs::write(
        world.root.join("home/.gitconfig"),
        format!(
            "[user]\n\tname = Groom Journey\n\temail = groom@example.invalid\n\
             [url \"file:///{url}/\"]\n\tinsteadOf = https://\n\
             [uploadpack]\n\tallowFilter = true\n\
             [protocol \"file\"]\n\tallow = always\n"
        ),
    )
    .expect("the git configuration");
    std::fs::write(path.join("layout.rs"), "fn save() {}\n").expect("a file");
    for args in [
        &["init", "--quiet", "--initial-branch=main"][..],
        &["add", "--all"],
        &["commit", "--quiet", "--message", "the first commit"],
    ] {
        let output = crate::isolated::command(
            "git",
            &path,
            [
                ("HOME", world.root.join("home").display().to_string()),
                (
                    "PATH",
                    crate::isolated::search_path(None, crate::isolated::UNIX_TOOLS),
                ),
                ("GIT_CONFIG_NOSYSTEM", "1".to_owned()),
            ],
        )
        .args(args)
        .output()
        .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }
}

#[test]
fn comments_gather_passes_over_grooming_comments_and_answers_a_persons() {
    let mut world = World::new(&[DESTINATION], "host_label: build-host-1\n");
    let cards = file(&mut world, BATCH, &["save-a", "save-b"]);
    local_repository(&world);
    world
        .variables
        .push(("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()));

    let prepared = world.ok("groom", &["groom", "prepare"], "");
    let batch = prepared["batch"].as_str().expect("a batch").to_owned();
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(
            world
                .root
                .join(format!(".onefollowup/groom/{batch}/batch.json")),
        )
        .expect("batch.json"),
    )
    .expect("JSON");
    // The second filed ticket shares the first's root cause; the first rests on more evidence.
    for shard in record["shards"].as_array().expect("shards") {
        let mut text = String::new();
        for ticket in shard["tickets"].as_array().expect("tickets") {
            let duplicate = *ticket == json!(cards[1]);
            let one = json!({
                "schema_version": 1,
                "ticket": ticket,
                "verdict": "keep",
                "confidence": "high",
                "evidence": "Both read the window size twice in `layout.rs`'s save.",
                "deciding_commit": null,
                "evidence_paths": if duplicate { json!(["layout.rs"]) } else { json!(["layout.rs", "README.md"]) },
                "duplicate_of": if duplicate { json!(cards[0]) } else { Value::Null },
                "root_cause": if duplicate { json!("the save reads the window size twice") } else { Value::Null },
                "seam_repository": null,
            });
            text.push_str(&one.to_string());
            text.push('\n');
        }
        std::fs::write(shard["verdicts"].as_str().expect("a path"), text).expect("verdicts");
    }
    let checked = world.ok("groom", &["groom", "check", "--batch", &batch], "");
    assert_eq!(checked["plan"]["survivors"][0]["ticket"], json!(cards[0]));
    let published = world.ok("groom", &["groom", "publish", "--batch", &batch], "");
    assert_eq!(published["mismatched"], json!([]), "{published:#}");

    comment(
        &world,
        &cards[0],
        "c-person",
        "alice",
        &now(),
        "Is this fixed yet?",
    );
    let gathered = world.ok("gather", &["comments", "gather"], "");
    assert_eq!(gathered["selected"], json!({BATCH: 1}), "{gathered:#}");
    let snapshot: Value = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(gathered["snapshot"].as_str().expect("a snapshot"))
                .join("snapshot.json"),
        )
        .expect("snapshot.json"),
    )
    .expect("JSON");
    assert_eq!(
        snapshot["selection"][BATCH],
        json!([{
            "issue": cards[0],
            "comment": "c-person",
            "last_changed": snapshot["selection"][BATCH][0]["last_changed"],
            "body_sha256": snapshot["selection"][BATCH][0]["body_sha256"],
        }]),
        "only the person's comment is selected: {snapshot:#}"
    );
    assert!(
        snapshot["left"]
            .as_array()
            .expect("left")
            .iter()
            .any(|one| {
                one["issue"] == json!(cards[0])
                    && one["reason"] == format!("marked by batch {batch}")
            }),
        "the survivor's consolidation comment is left out as maintenance: {snapshot:#}"
    );
}
