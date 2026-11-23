//! The quota governor's host state between real onefollowup processes: two built binaries,
//! each in a work directory of its own publishing to a GitHub Projects board of its own, share
//! one `XDG_STATE_HOME`. GitHub — its rate-limit state and the GraphQL a publish of one
//! evidence comment sends — is the loopback of `loopback_github`; nothing else is doubled.
//!
//! Each world's batch is prepared, staged and checked over a local-md `followups`, then its
//! store names `followups` as the GitHub board, so its `publish` sends the comment to GitHub:
//! one node read, then one `addComment`, a content-creating write. A process the governor holds
//! says so on standard error; the journey reads that line, then kills the process, so no case
//! waits out a real minute or hour.

use std::io::BufRead as _;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::loopback_github::{LoopbackGitHub, Quota};
use crate::world::{World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-22";
const ROOT_CAUSE: &str = "export-dialog-drops-unicode";

/// A world whose batch has one evidence comment staged and checked on its ticket `item`, its
/// destination `followups` then the GitHub Projects board `owner`/7 of `github`, its governor
/// state in `state_home`, configured by `variables`.
fn staged(
    github: &LoopbackGitHub,
    owner: &str,
    item: &str,
    state_home: &Path,
    variables: &[(&str, &str)],
) -> World {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    world.item(
        "followups",
        item,
        &filed_item(
            "widgets: the export dialog drops unicode titles",
            "nightly-2026-10-01",
            ROOT_CAUSE,
            &json!({}),
            "medium",
        ),
        &filed_body("The export dialog drops unicode titles."),
    );
    let draft = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "café became caf.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let qualified = format!("followups:{item}");
    world.ok(
        "agent",
        &["ticket", "comment", "--batch", BATCH, "--item", &qualified],
        "Host build-host-1: café exported as caf again.\n",
    );
    world.answer(
        BATCH,
        &[answer(
            &draft,
            "root-cause",
            "filed",
            &[ROOT_CAUSE],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    std::fs::write(
        world.root.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  drafts:\n    plugin: local-md\n    config:\n      root: drafts-root\n  followups:\n    plugin: github-projects\n    config:\n      owner: {owner}\n      project_number: 7\n      endpoint: {}\n      token_env: LOOPBACK_TOKEN\n",
            github.endpoint()
        ),
    )
    .expect("the store");
    world.variables.extend(
        [
            // A loopback board's token: it reaches nothing but this test's server.
            ("LOOPBACK_TOKEN", "loopback"),
            ("XDG_STATE_HOME", &*state_home.display().to_string()),
        ]
        .into_iter()
        .chain(variables.iter().copied())
        .map(|(name, value)| (name.to_owned(), value.to_owned())),
    );
    world
}

/// The host's governor state file for GitHub boards, under `state_home`.
fn state_file(state_home: &Path) -> std::path::PathBuf {
    state_home.join("onefollowup/governor/github-projects.json")
}

/// Run `world`'s publish to its end: what it printed.
fn published(world: &World) -> Value {
    let output = world.run(&["publish", "--batch", BATCH], "");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("publish printed JSON")
}

/// Start `world`'s publish, wait for the first standard-error line holding `held`, kill the
/// process (started here, by its own PID) and answer that line.
fn held_then_killed(world: &World, held: &str) -> String {
    let mut command = crate::isolated::command(
        assert_cmd::cargo::cargo_bin("onefollowup"),
        &world.root,
        world.environment(),
    );
    command
        .args(["publish", "--batch", BATCH])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child: Child = command.spawn().expect("the binary starts");
    let stderr = child.stderr.take().expect("its standard error");
    let (lines, read) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    let mut seen = Vec::new();
    let line = loop {
        match read.recv_timeout(Duration::from_secs(60)) {
            Ok(line) if line.contains(held) => break line,
            Ok(line) => seen.push(line),
            Err(error) => {
                let _ = child.kill();
                panic!("no line holding {held:?} ({error}): {seen:?}");
            }
        }
    };
    child.kill().expect("the held publish is killed");
    child.wait().expect("it ends");
    line
}

/// Every comment GitHub was asked to add, by the issue it went on.
fn comments_on(github: &LoopbackGitHub) -> Vec<String> {
    github
        .received()
        .iter()
        .filter(|received| received.adds_comment())
        .map(|received| {
            received.variables["input"]["subjectId"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

fn ample() -> Quota {
    Quota {
        remaining: 5000,
        reset: chrono::Utc::now().timestamp() + 3600,
        charge: 0,
    }
}

/// Two processes and one cap of 1 a `window`, the other cap not binding: the first one's
/// comment fills the window, and the second is held by it.
fn one_process_s_write_holds_the_other_s(window: &str, variables: &[(&str, &str)]) {
    let state = tempfile::tempdir().expect("a state home");
    let github = LoopbackGitHub::start(ample());
    let alpha = staged(
        &github,
        "alpha-org",
        "alpha-ticket",
        state.path(),
        variables,
    );
    let beta = staged(&github, "beta-org", "beta-ticket", state.path(), variables);
    assert_ne!(alpha.root, beta.root);

    let first = published(&alpha);
    assert_eq!(first["outcomes"][0]["outcome"], "written", "{first}");
    assert_eq!(first["governor"]["waits"], json!([]), "{first}");
    assert_eq!(comments_on(&github), vec!["alpha-ticket"]);

    let line = held_then_killed(&beta, "holding content-creating writes");
    assert!(
        line.contains(&format!(
            "holding content-creating writes to followups on graphql: 1 in the last {window} across this host, max_content_writes_per_{window} 1; resuming by"
        )),
        "{line}"
    );
    assert_eq!(
        comments_on(&github),
        vec!["alpha-ticket"],
        "beta's comment was never sent"
    );
    assert!(state_file(state.path()).is_file(), "the host's state");
}

#[test]
fn one_process_s_comment_counts_against_another_s_per_minute_cap_through_the_host_state() {
    one_process_s_write_holds_the_other_s(
        "minute",
        &[
            ("ONEFOLLOWUP_GOVERNOR__MAX_CONTENT_WRITES_PER_MINUTE", "1"),
            ("ONEFOLLOWUP_GOVERNOR__MAX_CONTENT_WRITES_PER_HOUR", "100"),
        ],
    );
}

#[test]
fn one_process_s_comment_counts_against_another_s_per_hour_cap_through_the_host_state() {
    one_process_s_write_holds_the_other_s(
        "hour",
        &[
            ("ONEFOLLOWUP_GOVERNOR__MAX_CONTENT_WRITES_PER_MINUTE", "100"),
            ("ONEFOLLOWUP_GOVERNOR__MAX_CONTENT_WRITES_PER_HOUR", "1"),
        ],
    );
}

#[test]
fn a_fresh_process_starts_from_the_persisted_burn_estimate_and_a_corrupted_state_is_no_history() {
    // Every reading is taken afresh (`sample_seconds` 0) and nothing is seeded, so a reserve is
    // only ever what a process measured or read from the host state. While alpha publishes,
    // others spend 300 points per request it sends, which the readings around its requests
    // show: a burn near 60 points a minute.
    let variables = [
        ("ONEFOLLOWUP_GOVERNOR__SAMPLE_SECONDS", "0"),
        ("ONEFOLLOWUP_GOVERNOR__MIN_PEAK_PER_MINUTE__GRAPHQL", "0"),
    ];
    for corrupt in [false, true] {
        let state = tempfile::tempdir().expect("a state home");
        let now = chrono::Utc::now().timestamp();
        let github = LoopbackGitHub::start(Quota {
            remaining: 4800,
            reset: now + 600,
            charge: 300,
        });
        let alpha = staged(
            &github,
            "alpha-org",
            "alpha-ticket",
            state.path(),
            &variables,
        );
        let beta = staged(&github, "beta-org", "beta-ticket", state.path(), &variables);
        let first = published(&alpha);
        assert_eq!(first["outcomes"][0]["outcome"], "written", "{first}");
        let recorded = std::fs::read_to_string(state_file(state.path())).expect("the state");
        assert!(recorded.contains("\"history\""), "{recorded}");
        if corrupt {
            std::fs::write(state_file(state.path()), "{\"schema_version\": 1, \"resour")
                .expect("corrupted");
        }

        // Beta starts fresh on another window, 1,300 left of it, others spending nothing:
        // above the floor (1,000) by more than its request, under it plus the reserve the
        // recorded burn sizes (about 60 a minute for the 600 s to the reset).
        github.set_quota(Quota {
            remaining: 1300,
            reset: now + 660,
            charge: 0,
        });
        if corrupt {
            let second = published(&beta);
            assert_eq!(second["outcomes"][0]["outcome"], "written", "{second}");
            assert_eq!(
                second["governor"]["waits"],
                json!([]),
                "no history: nothing reserves for others"
            );
            assert_eq!(comments_on(&github), vec!["alpha-ticket", "beta-ticket"]);
        } else {
            let line = held_then_killed(&beta, "holding requests to followups on graphql");
            assert!(
                line.contains("remaining 1300 of 5000, floor 1000"),
                "{line}"
            );
            let peak: f64 = line
                .split("/min peak")
                .next()
                .and_then(|before| before.rsplit(' ').next())
                .and_then(|rate| rate.parse().ok())
                .unwrap_or_else(|| panic!("a peak rate in {line}"));
            assert!(
                (50.0..=60.0).contains(&peak),
                "the burn alpha recorded: {line}"
            );
            assert_eq!(
                comments_on(&github),
                vec!["alpha-ticket"],
                "beta sent nothing"
            );
        }
    }
}
