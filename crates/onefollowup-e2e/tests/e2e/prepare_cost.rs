//! What one `onefollowup prepare` of a 50-draft batch costs in GitHub GraphQL points and in
//! seconds, measured by running the built binary's `prepare` end to end over the loopback
//! GitHub (`loopback_github`, `loopback_boards`), which charges every request by GitHub's
//! published calculation (`graphql_cost`): the telemetry of `prepare-graphql-points-per-draft`
//! and `prepare-seconds-per-batch` (`budgets.yaml`).
//!
//! The workload is the one those budgets state: 50 drafts under the shipped `root-cause` kind,
//! which judges duplicates, so each searches its title, up to 5 terms and up to 2 file names.
//! 40 are about public repositories and land on the public board `followups` (2,000 tickets
//! across 6 repositories, about 470 open); 10 are about private repositories, which a route by
//! repository sends to the private board `private-followups` (400 tickets). The 13 real
//! clusters of `fixtures/filing-duplicates.json` give 13 of the public drafts and their earlier
//! members on the public board; the rest of both boards come from `duplicate_boards`' crowd
//! generator, and at least three unstoplisted terms the drafts search each match 1,000 public
//! tickets or more. The loopback reports a fresh window — 5,000 of 5,000 points, resetting
//! 1,800 s after the start — and the governor starts from a host state carried over from many
//! earlier windows, about the 910 KB this host's held on 2026-11-25.
//!
//! Seconds are the process's wall clock from spawn to exit, timed here with a monotonic clock,
//! plus every request it sent, in sequence, times a stated 0.5 s GitHub round trip; nothing
//! sleeps for it. The journey asserts no point or time threshold — the budgets are
//! onebudgetspec's to judge — only where each draft was searched, what the progress lines
//! said, and exact counts of what prepare did.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde_json::{Value, json};

use crate::duplicate_boards::{
    Crowd, CrowdTicket, REPOSITORIES, clusters, crowd_tickets, origin, record, slug_of,
};
use crate::loopback_boards::{Board, BoardIssue};
use crate::loopback_github::{Charged, LoopbackGitHub, Pricing, Quota};
use crate::telemetry;
use crate::world::World;

const BATCH: &str = "nightly-2026-11-25";
/// GitHub's own round trip per request, which the loopback cannot reproduce.
const ROUND_TRIP: f64 = 0.5;
pub const PUBLIC_OWNER: &str = "example-org";
pub const PRIVATE_OWNER: &str = "example-private-org";
/// The private repositories, `github.com/example-private/<name>`, the route sends away.
pub const PRIVATE_REPOSITORIES: [&str; 3] = ["ledger", "payroll", "vault"];
const PUBLIC_TICKETS: usize = 2000;
const PUBLIC_OPEN: usize = 470;
const PRIVATE_TICKETS: usize = 400;
const PRIVATE_OPEN: usize = 95;

/// Board vocabulary the generated drafts search, with how many public crowd tickets carry
/// each: the first three are the common unstoplisted terms the workload requires.
pub const PUBLIC_WORDS: &[(&str, usize)] = &[
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
    "unicode",
    "tombstone",
    "backslash",
    "symlink",
    "newline",
    "ampersand",
    "emoji",
    "tabstop",
    "hostname",
    "locale",
    "umask",
    "inode",
    "zombie",
    "deadline",
    "checksum",
    "heartbeat",
    "watermark",
    "backoff",
    "rollover",
    "overflow",
    "underflow",
    "truncation",
    "padding",
    "endianness",
    "keepalive",
    "handshake",
    "tarball",
    "manpage",
    "shebang",
    "dotfile",
    "lockfile",
    "sidecar",
    "preamble",
    "epilogue",
    "trailer",
    "footnote",
    "breadcrumb",
];
const FILES: [&str; 6] = [
    "mod.rs",
    "lib.rs",
    "main.rs",
    "config.rs",
    "cli.rs",
    "engine.rs",
];

/// A private repository's origin.
pub fn private_origin(name: &str) -> String {
    format!("github.com/example-private/{name}")
}

/// One ticket of a generated board.
pub struct BoardTicket {
    /// Its repository's origin.
    pub origin: String,
    pub title: String,
    pub body: String,
    pub root_cause: String,
    pub batch: String,
    /// `open`, `fixed` or `duplicate`.
    pub status: String,
}

impl BoardTicket {
    /// A crowd ticket about the repository `origin_of` its name gives.
    pub fn of(ticket: CrowdTicket, origin_of: impl Fn(&str) -> String) -> Self {
        Self {
            origin: origin_of(&ticket.repository),
            title: ticket.title,
            body: ticket.body,
            root_cause: ticket.root_cause,
            batch: ticket.batch,
            status: ticket.status.to_owned(),
        }
    }
}

/// A board of `tickets`, newest first, its issues numbered in the order given, each body its
/// visible text then onetaskgraph's metadata slot holding the follow-up record.
pub fn board(owner: &str, number: u64, tickets: Vec<BoardTicket>) -> Board {
    let mut issues: Vec<BoardIssue> = tickets
        .into_iter()
        .enumerate()
        .map(|(at, ticket)| {
            let (open, reason, status) = match ticket.status.as_str() {
                "open" => (true, None, "Backlog"),
                "fixed" => (false, Some("COMPLETED"), "Done"),
                _ => (false, Some("DUPLICATE"), "Cancelled"),
            };
            let name_with_owner = ticket
                .origin
                .strip_prefix("github.com/")
                .unwrap_or(&ticket.origin)
                .to_owned();
            let metadata = json!({
                "orchestrator.follow-up": record(&ticket.origin, &ticket.root_cause, &ticket.batch)
            });
            BoardIssue::new(
                format!("I_{owner}_{at:05}"),
                at as u64 + 1,
                ticket.title,
                format!(
                    "{}\n\n<!-- onetaskgraph.metadata\n{metadata}\n-->",
                    ticket.body
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

/// The public board, project 7 of [`PUBLIC_OWNER`]: the 13 clusters' earlier members among a
/// crowd of `tickets` in all across the six repositories, `open` of them open, each word of
/// `words` carried by as many crowd tickets as it says.
pub fn public_board(tickets: usize, open: usize, words: &[(&str, usize)]) -> Board {
    let mut earlier: Vec<BoardTicket> = Vec::new();
    for cluster in clusters() {
        for member in cluster.earlier {
            earlier.push(BoardTicket {
                origin: origin(&member.repository),
                root_cause: slug_of(&member.title),
                title: member.title,
                body: member.body,
                batch: format!("groomed-{}", member.key.replace('#', "-")),
                status: member.status,
            });
        }
    }
    let earlier_open = earlier.iter().filter(|one| one.status == "open").count();
    let mut public: Vec<BoardTicket> = crowd_tickets(
        &REPOSITORIES,
        &Crowd {
            count: tickets - earlier.len(),
            open: open - earlier_open,
            words,
        },
    )
    .into_iter()
    .map(|ticket| BoardTicket::of(ticket, origin))
    .collect();
    // The clusters' members sit among the crowd, spread through its history.
    let stride = public.len() / (earlier.len() + 1);
    for (at, member) in earlier.into_iter().enumerate() {
        public.insert((at + 1) * stride + at, member);
    }
    board(PUBLIC_OWNER, 7, public)
}

/// The private board, project 8 of [`PRIVATE_OWNER`]: a crowd of `tickets` across the private
/// repositories, `open` of them open, carrying `words` in proportion to the public board's.
pub fn private_board(tickets: usize, open: usize, words: &[(&str, usize)]) -> Board {
    let words: Vec<(&str, usize)> = words
        .iter()
        .map(|(word, many)| (*word, many * tickets / PUBLIC_TICKETS))
        .collect();
    let private = crowd_tickets(
        &PRIVATE_REPOSITORIES,
        &Crowd {
            count: tickets,
            open,
            words: &words,
        },
    )
    .into_iter()
    .map(|ticket| BoardTicket::of(ticket, private_origin))
    .collect();
    board(PRIVATE_OWNER, 8, private)
}

/// A world whose store holds the local-md `drafts` and the two loopback boards, `followups`
/// routing every `example-private` repository's ticket to `private-followups`, with the
/// governor's host state under a state home of its own: `(world, state home)`.
pub fn loopback_world(github: &LoopbackGitHub) -> (World, tempfile::TempDir) {
    let mut world = World::new(&[], "host_label: build-host-1\n");
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
        // A test driving the real GitHub plugin at a fake host (AGENTS.md).
        (
            "DBUS_SESSION_BUS_ADDRESS".to_owned(),
            "disabled:".to_owned(),
        ),
    ]);
    (world, state)
}

/// The governor's host state for GitHub Projects sources, carried over from many earlier
/// windows: every resource's last day of others' burn, one interval every `every` seconds up to
/// ten minutes before `now`, beside the per-call means an earlier release kept. Written under
/// `state_home`; answers its size in bytes.
pub fn carry_governor_state(state_home: &std::path::Path, every: i64) -> u64 {
    let now = chrono::Utc::now();
    let format =
        |at: chrono::DateTime<chrono::Utc>| at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let history = |spend: f64| -> Vec<Value> {
        let last = now - chrono::Duration::seconds(600);
        let mut at = now - chrono::Duration::seconds(86_000);
        let mut held = Vec::new();
        while at < last - chrono::Duration::seconds(every) {
            let to = at + chrono::Duration::seconds(every);
            held.push(json!({"from": format(at), "to": format(to), "spend": spend}));
            at = to;
        }
        held
    };
    let mean = json!([{"sum": 0.0, "count": 0}, {"sum": 0.0, "count": 0}]);
    let resource = |spend: f64| {
        json!({
            "windows": {},
            "own": 6298.0,
            "history": history(spend),
            "request_cost": mean,
            "call_cost": [{"sum": 5319.0, "count": 1}, {"sum": 0.0, "count": 0}],
        })
    };
    let state = json!({
        "schema_version": 1,
        "resources": {"graphql": resource(1.0), "core": resource(0.5)},
        "in_flight": [],
        "content_writes": [],
    });
    let directory = state_home.join("onefollowup/governor");
    std::fs::create_dir_all(&directory).expect("the governor's state directory");
    let file = directory.join("github-projects.json");
    std::fs::write(&file, state.to_string()).expect("the carried state");
    std::fs::metadata(&file).expect("written").len()
}

/// The size of the governor's host state for GitHub Projects sources under `state_home`.
pub fn governor_state_size(state_home: &std::path::Path) -> u64 {
    std::fs::metadata(state_home.join("onefollowup/governor/github-projects.json"))
        .map_or(0, |held| held.len())
}

/// A generated draft: `(repository origin, title, paths, body)`.
pub type Draft = (String, String, Vec<String>, String);

/// The `at`th generated draft about `repository`: two common terms, a mid-frequency one, a
/// rare one of its own, and one or two file names.
pub fn generated(at: usize, repository: &str) -> Draft {
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
    if at.is_multiple_of(2) {
        paths.push(format!("src/{}", FILES[(at + 3) % FILES.len()]));
    }
    (repository.to_owned(), title, paths, body)
}

/// Record the 50 drafts: the 13 clusters' later members, 27 generated public ones and 10
/// private ones; answers each id with whether it is private.
fn record_drafts(world: &mut World) -> Vec<(String, bool)> {
    let mut drafts = Vec::new();
    for cluster in clusters() {
        let paths: Vec<&str> = cluster.draft.paths.iter().map(String::as_str).collect();
        let id = world.draft_in(
            BATCH,
            &origin(&cluster.repository),
            &cluster.draft.title,
            &paths,
            &format!("{}\n", cluster.draft.body),
        );
        drafts.push((id, false));
    }
    for at in 0..37 {
        let private = at >= 27;
        let repository = if private {
            private_origin(PRIVATE_REPOSITORIES[at % PRIVATE_REPOSITORIES.len()])
        } else {
            origin(REPOSITORIES[at % REPOSITORIES.len()])
        };
        let (repository, title, paths, body) = generated(at, &repository);
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let id = world.draft_in(BATCH, &repository, &title, &paths, &body);
        drafts.push((id, private));
    }
    drafts
}

/// The phases prepare's stderr named: each one's `started` detail and its `finished` counts,
/// and every progress line's counts, by phase, in order.
pub struct Phases {
    pub started: Vec<(String, String)>,
    pub finished: BTreeMap<String, (u64, u64)>,
    pub progress: BTreeMap<String, Vec<(u64, u64)>>,
}

/// Read prepare's progress lines off `stderr`.
pub fn phases(stderr: &str) -> Phases {
    let started = regex::Regex::new(r"^onefollowup prepare: (.+?): started: (.*)$").expect("ok");
    let finished =
        regex::Regex::new(r"^onefollowup prepare: (.+?): finished: (\d+) requests, (\d+) points$")
            .expect("ok");
    let progress =
        regex::Regex::new(r"^onefollowup prepare: (.+?): (\d+) requests, (\d+) points so far$")
            .expect("ok");
    let mut phases = Phases {
        started: Vec::new(),
        finished: BTreeMap::new(),
        progress: BTreeMap::new(),
    };
    let count = |text: &str| text.parse::<u64>().expect("a count");
    for line in stderr.lines() {
        if let Some(held) = started.captures(line) {
            phases
                .started
                .push((held[1].to_owned(), held[2].to_owned()));
        } else if let Some(held) = finished.captures(line) {
            phases
                .finished
                .insert(held[1].to_owned(), (count(&held[2]), count(&held[3])));
        } else if let Some(held) = progress.captures(line) {
            phases
                .progress
                .entry(held[1].to_owned())
                .or_default()
                .push((count(&held[2]), count(&held[3])));
        }
    }
    phases
}

/// The phase a request the loopback answered belongs to.
fn phase_of(charged: &Charged) -> &'static str {
    match charged.document.as_str() {
        "SEARCH_BATCH" | "SEARCH_ISSUES" => "duplicate find rounds",
        "ISSUE_DETAILS" | "ISSUE_DETAIL" => "kept-hit detail reads",
        "ISSUE_RELATIONS" => "relation reads",
        "rate_limit" => "quota reads",
        _ => "other",
    }
}

#[test]
fn a_50_draft_prepare_searches_each_draft_where_it_lands_and_records_its_points_and_seconds() {
    let boards = vec![
        public_board(PUBLIC_TICKETS, PUBLIC_OPEN, PUBLIC_WORDS),
        private_board(PRIVATE_TICKETS, PRIVATE_OPEN, PUBLIC_WORDS),
    ];
    let public_open = boards[0].issues.iter().filter(|one| one.open).count();
    let reset_in = 1800;
    let github = LoopbackGitHub::serving(
        Quota {
            remaining: 5000,
            reset: chrono::Utc::now().timestamp() + reset_in,
            charge: 0,
        },
        boards.clone(),
        Pricing::Published,
    );
    let (mut world, state) = loopback_world(&github);
    let drafts = record_drafts(&mut world);
    assert_eq!(drafts.len(), 50);
    assert!(github.charged().is_empty(), "drafting reached GitHub");
    // A host that has governed many earlier windows: about this host's 910 KB.
    let carried = carry_governor_state(state.path(), 17);
    assert!(carried > 900_000, "{carried} bytes");

    // One prepare, timed from spawn to exit.
    let mut command =
        crate::isolated::command(crate::isolated::binary(), &world.root, world.environment());
    command.args(["prepare", "--batch", BATCH]);
    let started = Instant::now();
    let output = command.output().expect("the binary runs");
    let elapsed = started.elapsed().as_secs_f64();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "prepare: {stderr}");
    let prepared: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let after = governor_state_size(state.path());

    let charged = github.charged();
    let requests = charged.len() as u64;
    let points: u64 = charged.iter().map(|one| one.points).sum();
    let mut by_phase: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for one in &charged {
        let held = by_phase.entry(phase_of(one)).or_default();
        held.0 += 1;
        held.1 += one.points;
    }
    assert_eq!(by_phase.get("other"), None, "{by_phase:?}");

    // Where each draft was searched: one find a board, carrying exactly the searches of the
    // drafts routed there, and every search the private board was asked scoped to it alone.
    let queries = |private: bool| -> u64 {
        drafts
            .iter()
            .filter(|(_, is_private)| *is_private == private)
            .map(|(id, _)| {
                prepared["duplicate_searches"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|search| search["input"] == id.as_str())
                    .and_then(|search| search["queries"].as_array())
                    .map_or(0, Vec::len) as u64
            })
            .sum()
    };
    let (public_searches, private_searches) = (queries(false), queries(true));
    let operations: Vec<(String, String, u64)> = prepared["store_operations"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|operation| {
            (
                operation["op"].as_str().unwrap_or_default().to_owned(),
                operation["source"].as_str().unwrap_or_default().to_owned(),
                operation["searches"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        operations,
        [
            ("find".to_owned(), "drafts".to_owned(), 1),
            ("find".to_owned(), "followups".to_owned(), public_searches),
            (
                "find".to_owned(),
                "private-followups".to_owned(),
                private_searches
            ),
            ("read".to_owned(), "followups".to_owned(), 0),
            ("read".to_owned(), "private-followups".to_owned(), 0),
        ],
        "{}",
        prepared["store_operations"]
    );
    let asked = |owner: &str, number: u64| -> Vec<&String> {
        charged
            .iter()
            .flat_map(|one| &one.searches)
            .filter(|text| text.contains(&format!("project:{owner}/{number}")))
            .collect()
    };
    let all_searches: usize = charged.iter().map(|one| one.searches.len()).sum();
    assert_eq!(
        asked(PUBLIC_OWNER, 7).len() + asked(PRIVATE_OWNER, 8).len(),
        all_searches,
        "every search is scoped to one of the boards"
    );
    // Each search stopped once it held the 21 hits prepare keeps.
    let first_asked: Vec<u64> = github
        .received()
        .iter()
        .flat_map(|received| {
            (0..onetaskgraph_github_projects::SEARCH_BATCH).filter_map(move |slot| {
                (received.variables[format!("ask{slot}")] == true)
                    .then(|| received.variables[format!("first{slot}")].as_u64())
                    .flatten()
            })
        })
        .collect();
    assert!(
        !first_asked.is_empty() && first_asked.iter().all(|first| *first <= 21),
        "every page asks at most the 21 hits kept: {first_asked:?}"
    );
    let distinct: BTreeSet<&String> = charged.iter().flat_map(|one| &one.searches).collect();
    let limited = distinct
        .iter()
        .filter(|text| crate::loopback_boards::matching(&boards, text) > 21)
        .count();
    let common = distinct
        .iter()
        .filter(|text| text.contains(&format!("project:{PUBLIC_OWNER}/7")))
        .filter(|text| crate::loopback_boards::matching(&boards, text) >= 1000)
        .count();
    assert!(
        common >= 3,
        "at least 3 searched terms match 1,000 public tickets or more"
    );
    let rows: u64 = charged
        .iter()
        .filter(|one| !one.searches.is_empty())
        .map(|one| one.rows)
        .sum();

    // What the progress lines said: each phase that ran, as it started and ended, with its
    // drafts, searches or items, and a line every 20 requests it sent.
    let said = phases(&stderr);
    let named: Vec<&str> = said
        .started
        .iter()
        .map(|(phase, _)| phase.as_str())
        .collect();
    assert_eq!(
        named,
        [
            "snapshot read",
            "duplicate find on followups",
            "duplicate find on private-followups",
            "kept-hit detail read"
        ],
        "{stderr}"
    );
    let detail = |phase: &str| {
        said.started
            .iter()
            .find(|(named, _)| named == phase)
            .map(|(_, detail)| detail.clone())
            .unwrap_or_default()
    };
    assert_eq!(detail("snapshot read"), format!("batch {BATCH}"));
    assert!(
        detail("duplicate find on followups").starts_with("40 drafts (")
            && detail("duplicate find on followups")
                .ends_with(&format!("), {public_searches} searches")),
        "{stderr}"
    );
    assert!(
        detail("duplicate find on private-followups").starts_with("10 drafts (")
            && detail("duplicate find on private-followups")
                .ends_with(&format!("), {private_searches} searches")),
        "{stderr}"
    );
    let state_reads: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join(format!("state/{BATCH}.json")))
            .expect("the batch state"),
    )
    .expect("JSON");
    let items = state_reads["reads"].as_array().map_or(0, Vec::len);
    assert!(
        detail("kept-hit detail read").starts_with(&format!("{items} items (")),
        "{stderr}"
    );
    for (phase, (sent, _)) in &said.finished {
        let ticks: Vec<u64> = said
            .progress
            .get(phase)
            .map(|held| held.iter().map(|(at, _)| *at).collect())
            .unwrap_or_default();
        let expected: Vec<u64> = (1..=sent / 20).map(|at| at * 20).collect();
        assert_eq!(
            ticks, expected,
            "{phase}: a line every 20 requests: {stderr}"
        );
    }
    let (said_requests, said_points) = said
        .finished
        .values()
        .fold((0, 0), |(requests, points), (one, two)| {
            (requests + one, points + two)
        });
    assert_eq!(
        said_requests, requests,
        "every request in a phase: {stderr}"
    );
    assert_eq!(said_points, points, "every point in a phase: {stderr}");

    // The figures, and how they were counted.
    let waits = prepared["governor"]["waits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let held_seconds: u64 = waits
        .iter()
        .filter_map(|wait| wait["seconds"].as_u64())
        .sum();
    let seconds = elapsed + requests as f64 * ROUND_TRIP;
    let per_draft = points as f64 / drafts.len() as f64;
    let routed = format!(
        "followups {} drafts, private-followups {} drafts",
        drafts.iter().filter(|(_, private)| !private).count(),
        drafts.iter().filter(|(_, private)| *private).count()
    );
    let phase_points = by_phase
        .iter()
        .map(|(phase, (sent, spent))| format!("{phase} {spent} points in {sent} requests"))
        .collect::<Vec<_>>()
        .join(", ");
    telemetry::record(
        "prepare-graphql-points-per-draft",
        per_draft.ceil() as usize,
        &format!(
            "{per_draft:.2} points a draft, rounded up: {points} points in {requests} requests for {} drafts; by phase: {phase_points}; {} searches asked, {rows} rows read, {limited} searches ended limited at 21 hits; drafts per routed destination: {routed}; boards: followups {} tickets ({public_open} open), private-followups {} tickets",
            drafts.len(),
            public_searches + private_searches,
            boards[0].issues.len(),
            boards[1].issues.len(),
        ),
    );
    let phase_requests = by_phase
        .iter()
        .map(|(phase, (sent, _))| format!("{phase} {sent}"))
        .collect::<Vec<_>>()
        .join(", ");
    telemetry::record(
        "prepare-seconds-per-batch",
        seconds.ceil() as usize,
        &format!(
            "{seconds:.1} s, rounded up: the process's measured elapsed {elapsed:.1} s plus {requests} requests in sequence at a stated {ROUND_TRIP} s round trip ({:.1} s); governor holds reported: {} totalling {held_seconds} s, inside the elapsed; governor state {carried} bytes at start, {after} at end; requests by phase: {phase_requests}; a fresh window, 5,000 of 5,000 points, resetting {reset_in} s after the start",
            requests as f64 * ROUND_TRIP,
            waits.len(),
        ),
    );
    telemetry::record_journey(
        "prepare-cost",
        &json!({
            "points": points,
            "requests": requests,
            "elapsed_seconds": elapsed,
            "seconds": seconds,
            "by_phase": by_phase.iter().map(|(phase, (sent, spent))| json!({"phase": phase, "requests": sent, "points": spent})).collect::<Vec<_>>(),
            "governor": prepared["governor"],
            "quota_reads": prepared["quota_reads"],
            "governor_state_bytes": {"start": carried, "end": after},
            "store_operations": prepared["store_operations"],
            "stderr": stderr,
        }),
    );
}
