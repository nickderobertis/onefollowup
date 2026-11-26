//! `spike-prepare-cost`: what one `onefollowup prepare` of a 50-draft batch costs in GitHub
//! GraphQL points and in seconds, measured by running the built binary's `prepare` end to end
//! over the loopback GitHub (`loopback_github`, `loopback_boards`), which charges every request
//! by GitHub's published calculation (`graphql_cost`).
//!
//! The workload is the one plan `authoring:followup-duplicate-search-cost`'s budgets hold at:
//! 50 drafts under the shipped `root-cause` kind, which judges duplicates, so each searches its
//! title, up to 5 terms and up to 2 file names. 40 are about public repositories and are filed
//! on the public board `followups` (2,000 tickets across 6 repositories, about 470 open); 10
//! are about private repositories, which a repository route sends to the private board
//! `private-followups` (400 tickets). The 13 real clusters of `fixtures/filing-duplicates.json`
//! give 13 of the public drafts and their earlier members on the public board; the rest of
//! both boards and the other 37 drafts are generated, and at least three unstoplisted terms the
//! drafts search each match 1,000 public tickets or more.
//!
//! One measurement per run, selected by `SPIKE_DESIGN`:
//!
//! - `base` (the default): the loopback reports a full allowance whatever is spent, so the
//!   governor never holds and the run ends.
//! - `bounded`: the binary is run with `ONEFOLLOWUP_SPIKE_BOUNDED=1`, which the spike's
//!   throwaway patch reads (each draft's searches only to the source its ticket would land in,
//!   and each search's walk ended once it holds 21 hits); the loopback reports 2,000 of 5,000
//!   points left, takes each request's price from it, and the governor keeps its 20% floor.
//!
//! Seconds are the process's wall clock from spawn to exit plus every request it sent, in
//! sequence, times a stated 0.5 s GitHub round trip; nothing sleeps for it. The figures, their
//! breakdown and every rendered candidate are printed and written to
//! `target/spike-prepare-cost/<design>.json`; a `bounded` run compares its candidates with the
//! `base` file's when there is one.
//!
//! Run with `scripts/spike-prepare-cost.sh <base|bounded>`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::duplicate_boards::{REPOSITORIES, clusters, origin, slug_of};
use crate::loopback_boards::{Board, BoardIssue};
use crate::loopback_github::{LoopbackGitHub, Quota};
use crate::world::{COMMIT, HOST, World};

const BATCH: &str = "nightly-2026-11-25";
/// GitHub's own round trip per request, which the loopback cannot reproduce.
const ROUND_TRIP: f64 = 0.5;
const PUBLIC_OWNER: &str = "example-org";
const PRIVATE_OWNER: &str = "example-private-org";
const PRIVATE_REPOSITORIES: [&str; 3] = ["ledger", "payroll", "vault"];
const PUBLIC_TICKETS: usize = 2000;
const PUBLIC_OPEN: usize = 470;
const PRIVATE_TICKETS: usize = 400;
const PRIVATE_OPEN: usize = 95;

/// Board vocabulary the generated drafts search, with how many public crowd tickets carry
/// each: the first three are the common unstoplisted terms the workload requires.
const PUBLIC_WORDS: &[(&str, usize)] = &[
    ("publish", 1300),
    ("session", 1150),
    ("template", 1050),
    ("comment", 600),
    ("merge", 520),
    ("branch", 500),
    ("search", 480),
    ("release", 450),
    ("governor", 420),
    ("schema", 400),
    ("budget", 380),
    ("binary", 350),
    ("prepare", 330),
    ("judge", 300),
    ("progress", 280),
    ("worktree", 260),
    ("cancel", 240),
    ("timeout", 220),
    ("snapshot", 200),
    ("quota", 180),
    ("launcher", 160),
    ("manifest", 140),
    ("wheel", 120),
    ("lease", 110),
    ("cursor", 90),
    ("webhook", 60),
    ("mod.rs", 420),
    ("lib.rs", 300),
    ("main.rs", 260),
    ("config.rs", 200),
    ("cli.rs", 180),
    ("engine.rs", 150),
];

/// The common words a generated draft pairs, and the mid-frequency words it adds.
const COMMON: [&str; 3] = ["publish", "session", "template"];
const MID: [&str; 12] = [
    "governor", "worktree", "launcher", "manifest", "snapshot", "progress", "timeout", "webhook",
    "cursor", "budget", "release", "branch",
];
/// One word only its own draft uses.
const RARE: [&str; 37] = [
    "unicode", "tombstone", "backslash", "symlink", "newline", "ampersand", "emoji", "tabstop",
    "hostname", "locale", "umask", "inode", "zombie", "deadline", "checksum", "heartbeat",
    "watermark", "backoff", "rollover", "overflow", "underflow", "truncation", "padding",
    "endianness", "keepalive", "handshake", "tarball", "manpage", "shebang", "dotfile",
    "lockfile", "sidecar", "preamble", "epilogue", "trailer", "footnote", "breadcrumb",
];
const FILES: [&str; 6] = [
    "mod.rs",
    "lib.rs",
    "main.rs",
    "config.rs",
    "cli.rs",
    "engine.rs",
];
const FILLER: [&str; 24] = [
    "gizmo",
    "calibration",
    "sprocket",
    "widget",
    "lantern",
    "harbour",
    "meadow",
    "pebble",
    "quartz",
    "saffron",
    "tundra",
    "velvet",
    "walnut",
    "zephyr",
    "orchard",
    "granite",
    "juniper",
    "kettle",
    "lagoon",
    "marble",
    "nectar",
    "oyster",
    "parsley",
    "thistle",
];

/// The body a board ticket is stored with: its visible text, then onetaskgraph's metadata slot
/// holding onefollowup's record.
fn stored_body(visible: &str, repository: &str, root_cause: &str, batch: &str) -> String {
    let record = json!({
        "schema": 14,
        "root_cause": root_cause,
        "repository": repository,
        "created_by_run": batch,
        "owning_runs": [batch],
        "drafts": [format!("drafts:{batch}/drafts/x")],
        "basis": {repository: COMMIT},
        "verified_at": "2026-10-01T11:30:00Z",
        "host": HOST,
        "priority_estimate": "medium",
        "board_estimate": "medium",
        "frequency": "intermittent",
        "impact": {
            "Severity": "high",
            "Workaround": "re-run the failed step",
            "Severity with the workaround": "medium",
        },
    });
    let metadata = json!({"orchestrator.follow-up": record});
    format!("{visible}\n\n<!-- onetaskgraph.metadata\n{metadata}\n-->")
}

/// A ticket of the boards: `(repository origin, title, visible body, root cause, status)`.
struct Ticket {
    repository: String,
    title: String,
    body: String,
    root_cause: String,
    status: &'static str,
}

/// `count` generated tickets across `repositories`, `open` of them open, each word of `words`
/// carried by as many of them as it says.
fn crowd(
    repositories: &[String],
    count: usize,
    open: usize,
    words: &[(&str, usize)],
    tag: &str,
) -> Vec<Ticket> {
    let mut carried: Vec<Vec<&str>> = vec![Vec::new(); count];
    let mut offset = 0;
    for (word, many) in words {
        let many = (*many).min(count);
        for at in 0..many {
            carried[(offset + at * 7) % count].push(word);
        }
        offset += 13;
    }
    carried
        .iter()
        .enumerate()
        .map(|(at, words)| {
            let repository = &repositories[at % repositories.len()];
            let name = repository.rsplit('/').next().unwrap_or_default();
            let one = FILLER[at % FILLER.len()];
            let two = FILLER[(at / FILLER.len() + 7) % FILLER.len()];
            let (prose, files): (Vec<&str>, Vec<&str>) =
                words.iter().partition(|word| !word.contains('.'));
            let extra = prose.join(" and ");
            let file = files.first().copied().unwrap_or("drift.rs");
            Ticket {
                repository: repository.clone(),
                title: format!("{name}: the {one} {two} drifts by {at}"),
                body: format!(
                    "## Root cause\n\nThe {one} {two} {extra} drifts by {at}.\n\n## Suggested fix\n\n- {name} `src/{one}_{two}/{file}`: recompute the {two}."
                ),
                root_cause: format!("{tag}-{one}-{two}-drifts-{at}"),
                status: if at < open { "open" } else { "fixed" },
            }
        })
        .collect()
}

/// A board of `tickets`, newest first, its issues numbered in the order given.
fn board(owner: &str, number: u64, tickets: Vec<Ticket>) -> Board {
    let mut issues: Vec<BoardIssue> = tickets
        .into_iter()
        .enumerate()
        .map(|(at, ticket)| {
            let (open, reason, status) = match ticket.status {
                "open" => (true, None, "Backlog"),
                "fixed" => (false, Some("COMPLETED"), "Done"),
                _ => (false, Some("DUPLICATE"), "Cancelled"),
            };
            let name_with_owner = ticket
                .repository
                .strip_prefix("github.com/")
                .unwrap_or(&ticket.repository)
                .to_owned();
            BoardIssue::new(
                format!("I_{owner}_{at:05}"),
                at as u64 + 1,
                ticket.title,
                stored_body(
                    &ticket.body,
                    &ticket.repository,
                    &ticket.root_cause,
                    &format!("groomed-{at}"),
                ),
                name_with_owner,
                open,
                reason,
                status,
            )
        })
        .collect();
    issues.reverse();
    Board {
        owner: owner.to_owned(),
        number,
        issues,
    }
}

/// A generated draft: `(repository origin, title, paths, body)`.
type Draft = (String, String, Vec<String>, String);

fn generated(at: usize, repository: &str) -> Draft {
    let name = repository.rsplit('/').next().unwrap_or_default();
    let common = COMMON[at % COMMON.len()];
    let second = COMMON[(at + 1) % COMMON.len()];
    let mid = MID[at % MID.len()];
    let rare = RARE[at % RARE.len()];
    let title = format!("{name}: {common} {mid} loses {rare} on {second}");
    let body = format!(
        "When the {mid} {common} runs, the {rare} is lost and the {second} keeps going as if nothing happened, so a person sees a stale result.\n"
    );
    let mut paths = vec![format!("crates/{name}/src/{}", FILES[at % FILES.len()])];
    if at % 2 == 0 {
        paths.push(format!("src/{}", FILES[(at + 3) % FILES.len()]));
    }
    (repository.to_owned(), title, paths, body)
}

fn output_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/spike-prepare-cost")
}

#[test]
#[ignore = "spike-prepare-cost measurement: run with scripts/spike-prepare-cost.sh"]
fn spike_prepare_cost() {
    let design = std::env::var("SPIKE_DESIGN").unwrap_or_else(|_| "base".to_owned());
    assert!(
        design == "base" || design == "bounded",
        "SPIKE_DESIGN is base or bounded, not {design}"
    );
    let bounded = design == "bounded";

    // The public board: the 13 clusters' earlier members, then the crowd around them.
    let clusters = clusters();
    let mut earlier: Vec<Ticket> = Vec::new();
    for cluster in &clusters {
        for member in &cluster.earlier {
            earlier.push(Ticket {
                repository: origin(&member.repository),
                title: member.title.clone(),
                body: member.body.clone(),
                root_cause: slug_of(&member.title),
                status: match member.status.as_str() {
                    "open" => "open",
                    "fixed" => "fixed",
                    _ => "duplicate",
                },
            });
        }
    }
    let earlier_open = earlier.iter().filter(|one| one.status == "open").count();
    let public_repositories: Vec<String> =
        REPOSITORIES.iter().map(|name| origin(name)).collect();
    let mut public = crowd(
        &public_repositories,
        PUBLIC_TICKETS - earlier.len(),
        PUBLIC_OPEN - earlier_open,
        PUBLIC_WORDS,
        "public",
    );
    // The clusters' members sit among the crowd, spread through its history.
    let stride = public.len() / (earlier.len() + 1);
    for (at, member) in earlier.into_iter().enumerate() {
        public.insert((at + 1) * stride + at, member);
    }
    let private_repositories: Vec<String> = PRIVATE_REPOSITORIES
        .iter()
        .map(|name| format!("github.com/example-private/{name}"))
        .collect();
    let private_words: Vec<(&str, usize)> = PUBLIC_WORDS
        .iter()
        .map(|(word, many)| (*word, many * PRIVATE_TICKETS / PUBLIC_TICKETS))
        .collect();
    let private = crowd(
        &private_repositories,
        PRIVATE_TICKETS,
        PRIVATE_OPEN,
        &private_words,
        "private",
    );
    let public_open = public.iter().filter(|one| one.status == "open").count();
    let boards = vec![
        board(PUBLIC_OWNER, 7, public),
        board(PRIVATE_OWNER, 8, private),
    ];
    let board_sizes = json!({
        "followups": {"tickets": boards[0].issues.len(), "open": public_open},
        "private-followups": {"tickets": boards[1].issues.len(), "open": PRIVATE_OPEN},
    });

    let reset = chrono::Utc::now().timestamp() + 3600;
    let quota = if bounded {
        Quota {
            remaining: 2000,
            reset,
            charge: 0,
        }
    } else {
        Quota {
            remaining: 5000,
            reset,
            charge: 0,
        }
    };
    let github = LoopbackGitHub::serving(quota, boards, bounded);

    let mut world = World::new(&[], "host_label: build-host-1\ngovernor:\n  floor_percent: 20\n");
    std::fs::write(
        world.root.join("onetaskgraph.yaml"),
        format!(
            "sources:
  drafts:
    plugin: local-md
    config:
      root: drafts-root
  followups:
    plugin: github-projects
    config:
      owner: {PUBLIC_OWNER}
      project_number: 7
      endpoint: {endpoint}
      token_env: LOOPBACK_TOKEN
    routes:
      - repositories: [\"github.com/example-private/*\"]
        to: private-followups
  private-followups:
    plugin: github-projects
    config:
      owner: {PRIVATE_OWNER}
      project_number: 8
      endpoint: {endpoint}
      token_env: LOOPBACK_TOKEN
",
            endpoint = github.endpoint()
        ),
    )
    .expect("the store");
    let state = tempfile::tempdir().expect("a state home");
    world.variables.extend([
        // A loopback board's token: it reaches nothing but this test's server.
        ("LOOPBACK_TOKEN".to_owned(), "loopback".to_owned()),
        (
            "XDG_STATE_HOME".to_owned(),
            state.path().display().to_string(),
        ),
    ]);
    if bounded {
        world
            .variables
            .push(("ONEFOLLOWUP_SPIKE_BOUNDED".to_owned(), "1".to_owned()));
    }

    // The 50 drafts: the 13 clusters' later members, 27 generated public ones and 10 private.
    let mut drafts: Vec<(String, Option<String>)> = Vec::new();
    for cluster in &clusters {
        let paths: Vec<&str> = cluster.draft.paths.iter().map(String::as_str).collect();
        let id = world.draft_in(
            BATCH,
            &origin(&cluster.repository),
            &cluster.draft.title,
            &paths,
            &format!("{}\n", cluster.draft.body),
        );
        drafts.push((id, Some(cluster.cluster.clone())));
    }
    for at in 0..27 {
        let (repository, title, paths, body) =
            generated(at, &public_repositories[at % public_repositories.len()]);
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let id = world.draft_in(BATCH, &repository, &title, &paths, &body);
        drafts.push((id, None));
    }
    for at in 27..37 {
        let (repository, title, paths, body) =
            generated(at, &private_repositories[at % private_repositories.len()]);
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let id = world.draft_in(BATCH, &repository, &title, &paths, &body);
        drafts.push((id, None));
    }
    assert_eq!(drafts.len(), 50);
    let before = github.charged().len();
    assert_eq!(before, 0, "drafting reached GitHub: {:?}", github.charged());

    // One prepare, timed from spawn to exit.
    let mut command =
        crate::isolated::command(crate::isolated::binary(), &world.root, world.environment());
    command.args(["prepare", "--batch", BATCH]);
    let started = Instant::now();
    let output = command.output().expect("the binary runs");
    let elapsed = started.elapsed();
    assert!(
        output.status.success(),
        "prepare exited {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let prepared: Value = serde_json::from_slice(&output.stdout).expect("prepare printed JSON");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    let charged = github.charged();
    let graphql: Vec<_> = charged
        .iter()
        .filter(|one| one.document != "rate_limit")
        .collect();
    let total_points: u64 = graphql.iter().map(|one| one.points).sum();
    let requests = charged.len();
    let seconds = elapsed.as_secs_f64() + requests as f64 * ROUND_TRIP;
    let mut phases: BTreeMap<String, (u64, u64, u64, u64, u64)> = BTreeMap::new();
    for one in &charged {
        let phase = match one.document.as_str() {
            "SEARCH_BATCH" | "SEARCH_ISSUES" => format!("find {}", one.board),
            "ISSUE_DETAILS" | "ISSUE_DETAIL" => format!("read {}", one.board),
            "rate_limit" => "quota reads (REST, unmetered)".to_owned(),
            other => format!("other {other}"),
        };
        let entry = phases.entry(phase).or_default();
        entry.0 += 1;
        entry.1 += one.rows;
        entry.2 += one.points;
        entry.3 += one.searches;
        entry.4 += one.connection_requests;
    }
    let loopback_busy: Duration = charged.iter().map(|one| one.busy).sum();

    // What the batch searched: every query's hits as the store answered them.
    let searches = prepared["duplicate_searches"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let queries: usize = searches
        .iter()
        .map(|one| one["queries"].as_array().map_or(0, Vec::len))
        .sum();
    let mut term_hits: BTreeMap<String, u64> = BTreeMap::new();
    for search in &searches {
        for query in search["queries"].as_array().into_iter().flatten() {
            if query["kind"] == "term" {
                let hits = query["hits"].as_u64().unwrap_or(0);
                let text = query["text"].as_str().unwrap_or_default().to_owned();
                let held = term_hits.entry(text).or_default();
                *held = (*held).max(hits);
            }
        }
    }
    let common_terms: Vec<(&String, &u64)> =
        term_hits.iter().filter(|(_, hits)| **hits >= 1000).collect();
    let candidates: BTreeMap<String, Value> = drafts
        .iter()
        .map(|(id, cluster)| {
            let search = searches
                .iter()
                .find(|one| one["input"] == id.as_str())
                .cloned()
                .unwrap_or_default();
            let label = match cluster {
                Some(cluster) => format!("cluster {cluster}"),
                None => {
                    let title = search["queries"][0]["text"].as_str().unwrap_or_default();
                    format!("generated {title}")
                }
            };
            (
                label,
                json!({
                    "candidates": search["candidates"],
                    "matching": search["matching"],
                }),
            )
        })
        .collect();

    let report = json!({
        "design": design,
        "drafts": drafts.len(),
        "queries": queries,
        "boards": board_sizes,
        "terms_matching_1000_or_more": common_terms,
        "points_total": total_points,
        "points_per_draft": total_points as f64 / drafts.len() as f64,
        "requests": requests,
        "graphql_requests": graphql.len(),
        "process_wall_seconds": elapsed.as_secs_f64(),
        "loopback_busy_seconds": loopback_busy.as_secs_f64(),
        "round_trip_seconds": requests as f64 * ROUND_TRIP,
        "seconds": seconds,
        "phases": phases.iter().map(|(phase, (requests, rows, points, searches, connection_requests))| {
            json!({"phase": phase, "requests": requests, "rows": rows, "points": points,
                   "searches": searches, "connection_requests": connection_requests})
        }).collect::<Vec<_>>(),
        "quota_left": github.quota().remaining,
        "governed": prepared["governed"],
        "store_operations": prepared["store_operations"],
        "stderr": stderr,
        "candidates": candidates,
    });
    std::fs::create_dir_all(output_dir()).expect("the output directory");
    std::fs::write(
        output_dir().join(format!("{design}.json")),
        serde_json::to_string_pretty(&report).expect("JSON"),
    )
    .expect("the report");

    println!("\n== spike-prepare-cost: {design} ==");
    println!(
        "workload: {} drafts, {queries} searches; boards {board_sizes}",
        drafts.len()
    );
    println!("terms matching >= 1000 public tickets: {common_terms:?}");
    println!(
        "points: {total_points} total, {:.2} a draft (target <= 10)",
        total_points as f64 / drafts.len() as f64
    );
    println!(
        "seconds: {seconds:.1} = process wall {:.1} s + {requests} requests x {ROUND_TRIP} s (target <= 300); the loopback itself was busy {:.1} s of the wall",
        elapsed.as_secs_f64(),
        loopback_busy.as_secs_f64()
    );
    println!(
        "{:<44} {:>8} {:>8} {:>8} {:>9} {:>11}",
        "phase", "requests", "rows", "points", "searches", "conn.req"
    );
    for (phase, (requests, rows, points, searches, connection_requests)) in &phases {
        println!(
            "{phase:<44} {requests:>8} {rows:>8} {points:>8} {searches:>9} {connection_requests:>11}"
        );
    }
    println!(
        "quota left at the end: {} of 5000; governor: {}",
        github.quota().remaining,
        prepared["governed"]
    );

    assert!(
        common_terms.len() >= 3,
        "at least 3 unstoplisted terms match 1,000 tickets or more: {term_hits:?}"
    );

    // The bounded design keeps every candidate the base rendered for the real clusters.
    if bounded {
        let base: Option<Value> = std::fs::read_to_string(output_dir().join("base.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok());
        match base {
            None => println!("no base.json beside this run: candidates not compared"),
            Some(base) => {
                let mut differ = BTreeSet::new();
                for (label, held) in &candidates {
                    if base["candidates"][label] != *held {
                        differ.insert(label.clone());
                    }
                }
                let clusters_differ: Vec<&String> = differ
                    .iter()
                    .filter(|label| label.starts_with("cluster "))
                    .collect();
                println!(
                    "candidates differing from base: {} of {} drafts; of the 13 clusters: {clusters_differ:?}",
                    differ.len(),
                    candidates.len()
                );
                for label in &differ {
                    let items = |value: &Value| -> Vec<String> {
                        value["candidates"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|one| one["item"].as_str().unwrap_or_default().to_owned())
                            .collect()
                    };
                    println!(
                        "  {label}\n    base:    {:?}\n    bounded: {:?}",
                        items(&base["candidates"][label]),
                        items(&candidates[label])
                    );
                }
            }
        }
    }
}
