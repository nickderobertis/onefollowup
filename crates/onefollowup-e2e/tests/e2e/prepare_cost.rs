//! What one `onefollowup prepare` of a 50-draft batch costs in GitHub GraphQL points and in
//! seconds, measured by running the built binary's `prepare` end to end over the loopback
//! GitHub (`loopback_github`, `loopback_boards`): the telemetry of
//! `prepare-graphql-points-per-draft` and `prepare-seconds-per-batch` (`budgets.yaml`).
//!
//! The workload is the one those budgets state (`real_workload`): 50 drafts under the shipped
//! `root-cause` kind, which judges duplicates, so each searches its title, up to 5 terms and up
//! to 2 file names, keeping 21 hits a query — 40 routed to a public board at the real
//! `followups` board's shape, 10 to a 400-ticket private board. The loopback charges each
//! request the points GitHub charged and the seconds GitHub took for the recorded real request
//! of the same document and page sizes (`recorded`), and answers from one token's two
//! interleaved 5,000-point windows, the first resetting 1,800 s after the start, which other
//! work spends at 105 points a minute. The governor keeps its 20% floor and seeded reserve,
//! from a host state carried over from many earlier windows, about the 910 KB this host held.
//!
//! Points are prepare's own: what its progress lines count, each request at the cost its
//! response reported, held equal to what the loopback charged its GraphQL requests. Seconds
//! are the process's wall clock from spawn to exit, timed here with a monotonic clock, any
//! governor hold inside it as real time, plus the recorded seconds charged to each request,
//! once each; nothing sleeps for them. The journey asserts no point or time threshold — the
//! budgets are onebudgetspec's to judge — only where each draft was searched, what the
//! progress lines said, that every hold was the own request's, and exact counts.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use serde_json::{Value, json};

use crate::duplicate_boards::{
    Crowd, CrowdTicket, REPOSITORIES, clusters, crowd_tickets, origin, record, slug_of,
};
use crate::loopback_boards::{Board, BoardIssue};
use crate::loopback_github::{Charged, LoopbackGitHub, Pricing, Shared};
use crate::real_workload;
use crate::telemetry;
use crate::world::World;

pub const PUBLIC_OWNER: &str = "example-org";
pub const PRIVATE_OWNER: &str = "example-private-org";
/// The private repositories, `github.com/example-private/<name>`, the route sends away.
pub const PRIVATE_REPOSITORIES: [&str; 3] = ["ledger", "payroll", "vault"];
const PUBLIC_TICKETS: usize = 2000;

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
        recorded: None,
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
            // Each interval fell in the window of its hour, as one window at a time answered.
            let reset = to.timestamp() - to.timestamp().rem_euclid(3600) + 3600;
            let window = chrono::DateTime::from_timestamp(reset, 0).expect("an instant");
            held.push(json!({
                "from": format(at), "to": format(to), "spend": spend, "window": format(window),
            }));
            at = to;
        }
        held
    };
    let mean = json!([{"sum": 0.0, "count": 0}, {"sum": 0.0, "count": 0}]);
    let resource = |spend: f64| {
        json!({
            "windows": {},
            "unread": 0.0,
            "history": history(spend),
            "request_cost": mean,
            "call_cost": [{"sum": 5319.0, "count": 1}, {"sum": 0.0, "count": 0}],
        })
    };
    let state = json!({
        "schema_version": 2,
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
        "ISSUE_DETAILS" | "ISSUE_DETAIL" | "ISSUE" => "detail reads",
        "ISSUE_RELATIONS" => "relation reads",
        "rate_limit" => "quota reads",
        _ => "other",
    }
}

/// One hold the governor's line on standard error stated: the destination, the remaining of
/// the window that held it, the floor and reserve, the own request's estimated cost, and that
/// window by its reset.
#[derive(Debug)]
struct Hold {
    remaining: f64,
    floor: f64,
    reserve: f64,
    cost: f64,
    resets_at: i64,
}

fn holds(stderr: &str) -> Vec<Hold> {
    let line = regex::Regex::new(
        r"holding requests to \S+ on graphql: remaining (\d+) of \d+, floor (\d+), reserve (\d+); this request ([0-9.]+) of the window resetting (\S+);",
    )
    .expect("valid");
    line.captures_iter(stderr)
        .map(|held| Hold {
            remaining: held[1].parse().expect("a figure"),
            floor: held[2].parse().expect("a figure"),
            reserve: held[3].parse().expect("a figure"),
            cost: held[4].parse().expect("a figure"),
            resets_at: chrono::DateTime::parse_from_rfc3339(&held[5])
                .expect("an instant")
                .timestamp(),
        })
        .collect()
}

/// The quantile `at` of `sizes`, sorted.
fn quantile(sizes: &[usize], at: f64) -> usize {
    sizes[((sizes.len() - 1) as f64 * at).round() as usize]
}

#[test]
fn a_50_draft_prepare_at_the_real_board_s_shape_spends_and_waits_for_its_own_requests_alone() {
    let boards = vec![
        real_workload::public_board(),
        real_workload::private_board(),
    ];
    // The public board at the real board's shape.
    let public = &boards[0];
    let public_open = public.issues.iter().filter(|one| one.open).count();
    assert_eq!(
        (public.issues.len(), public_open),
        (real_workload::PUBLIC_TICKETS, real_workload::PUBLIC_OPEN)
    );
    assert_eq!(
        (
            boards[1].issues.len(),
            boards[1].issues.iter().filter(|one| one.open).count()
        ),
        (real_workload::PRIVATE_TICKETS, real_workload::PRIVATE_OPEN)
    );
    let mut sizes: Vec<usize> = public.issues.iter().map(|one| one.body.len()).collect();
    sizes.sort_unstable();
    for (at, size) in real_workload::BODY_QUANTILES {
        let measured = quantile(&sizes, at);
        assert!(
            measured.abs_diff(size) * 100 <= size,
            "body bytes at {at}: {measured}, the real board's {size}"
        );
    }
    let labelled = public.issues.iter().filter(|one| one.labels == 1).count();
    assert_eq!(
        (
            labelled,
            public.issues.iter().filter(|one| one.labels == 0).count()
        ),
        (
            real_workload::LABELLED,
            real_workload::PUBLIC_TICKETS - real_workload::LABELLED
        ),
        "one label on 22 issues, none on the rest"
    );

    let start = chrono::Utc::now().timestamp();
    let resets = [start + 1800, start + 1800 + 1550];
    let github = LoopbackGitHub::sharing(
        &Shared {
            windows: vec![(2600, resets[0]), (4700, resets[1])],
            others_per_minute: 105.0,
        },
        boards.clone(),
        Pricing::Recorded,
    );
    let (mut world, state) = loopback_world(&github);
    let drafts = real_workload::record_drafts(&mut world);
    assert_eq!(drafts.len(), 50);
    assert!(github.charged().is_empty(), "drafting reached GitHub");
    // A host that has governed many earlier windows: about this host's 910 KB.
    let carried = carry_governor_state(state.path(), 25);
    assert!((880_000..950_000).contains(&carried), "{carried} bytes");
    let windows_before = github.windows();

    // One prepare, timed from spawn to exit.
    let mut command =
        crate::isolated::command(crate::isolated::binary(), &world.root, world.environment());
    command.args(["prepare", "--batch", real_workload::BATCH]);
    let started = Instant::now();
    let output = command.output().expect("the binary runs");
    let elapsed = started.elapsed().as_secs_f64();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "prepare: {stderr}");
    let prepared: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let after = governor_state_size(state.path());
    let windows_after = github.windows();

    let charged = github.charged();
    let requests = charged.len() as u64;
    let graphql: Vec<&Charged> = charged
        .iter()
        .filter(|one| one.document != "rate_limit")
        .collect();
    let points: u64 = graphql.iter().map(|one| one.points).sum();
    let mut by_phase: BTreeMap<&str, (u64, u64, f64)> = BTreeMap::new();
    for one in &charged {
        let held = by_phase.entry(phase_of(one)).or_default();
        held.0 += 1;
        held.1 += one.points;
        held.2 += one.seconds;
    }
    assert_eq!(by_phase.get("other"), None, "{by_phase:?}");
    assert!(
        charged
            .iter()
            .filter(|one| one.document == "rate_limit")
            .all(|one| one.points == 0),
        "a rate-limit read costs no GraphQL points"
    );

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
    let sent: Vec<&String> = charged.iter().flat_map(|one| &one.searches).collect();
    let scoped = |owner: &str, number: u64| -> BTreeSet<&String> {
        sent.iter()
            .copied()
            .filter(|text| text.contains(&format!("project:{owner}/{number}")))
            .collect()
    };
    let (public_sent, private_sent) = (scoped(PUBLIC_OWNER, 7), scoped(PRIVATE_OWNER, 8));
    let distinct: BTreeSet<&String> = sent.iter().copied().collect();
    assert_eq!(
        public_sent.len() + private_sent.len(),
        distinct.len(),
        "every search is scoped to one of the boards"
    );
    assert_eq!(
        distinct.len(),
        sent.len(),
        "each distinct text is sent once"
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
    // The texts searched on the public board match as many issues as the real board's did.
    let hits: Vec<usize> = public_sent
        .iter()
        .map(|text| crate::loopback_boards::counted(&boards, text))
        .collect();
    let share = |from: usize, to: usize, of: usize, total: usize| -> (f64, f64) {
        let measured = hits.iter().filter(|hit| (from..to).contains(hit)).count();
        (
            measured as f64 / hits.len() as f64,
            of as f64 / total as f64,
        )
    };
    let real: usize = real_workload::REAL_HITS.iter().map(|(_, _, of)| of).sum();
    let mut spread = Vec::new();
    for (from, to, of) in real_workload::REAL_HITS {
        let (measured, wanted) = share(from, to, of, real);
        let range = if to == usize::MAX {
            format!("{from} or more")
        } else {
            format!("{from} to {}", to - 1)
        };
        spread.push(format!(
            "{range}: {:.1}% (real {:.1}%)",
            measured * 100.0,
            wanted * 100.0
        ));
    }
    for (from, to, of) in real_workload::REAL_HITS {
        let (measured, wanted) = share(from, to, of, real);
        assert!(
            (measured - wanted).abs() <= 0.06,
            "texts matching {from} to {to} issues: {:.1}% of {}, the real board's {:.1}%",
            measured * 100.0,
            hits.len(),
            wanted * 100.0
        );
    }
    assert_eq!(hits.iter().max(), Some(&real_workload::MOST_HITS));
    let mut ordered = hits.clone();
    ordered.sort_unstable();
    let median = (ordered[(ordered.len() - 1) / 2] + ordered[ordered.len() / 2]) as f64 / 2.0;
    assert!(
        (median - real_workload::MEDIAN_HITS).abs() <= real_workload::MEDIAN_HITS / 2.0,
        "the median text matched {median} issues, the real board's {}",
        real_workload::MEDIAN_HITS
    );
    spread.push(format!(
        "median {median} (real {})",
        real_workload::MEDIAN_HITS
    ));
    let limited = hits.iter().filter(|hit| **hit > 21).count()
        + private_sent
            .iter()
            .filter(|text| crate::loopback_boards::counted(&boards, text) > 21)
            .count();
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
    assert_eq!(
        detail("snapshot read"),
        format!("batch {}", real_workload::BATCH)
    );
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
        &std::fs::read_to_string(
            world
                .work()
                .join(format!("state/{}.json", real_workload::BATCH)),
        )
        .expect("the batch state"),
    )
    .expect("JSON");
    // Read again with comments: every read an answer can relate, each draft's rendered
    // candidates among them; every other hit keeps the find's answer.
    let items = state_reads["reads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|read| !read["comments"].is_null())
        .count();
    assert!(
        detail("kept-hit detail read").starts_with(&format!("{items} items (")),
        "{stderr}"
    );
    let mut ticked = 0;
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
        ticked += ticks.len();
    }
    assert!(ticked >= 1, "a phase sent 20 requests or more: {stderr}");
    let (said_requests, said_points) = said
        .finished
        .values()
        .fold((0, 0), |(requests, points), (one, two)| {
            (requests + one, points + two)
        });
    // The progress lines count every request prepare sent, its rate-limit reads among them,
    // and the points GitHub reported its own GraphQL requests cost — nothing for a reading.
    assert_eq!(
        said_requests, requests,
        "every request in a phase: {stderr}"
    );
    assert_eq!(
        said_points, points,
        "the points the progress lines count are what the loopback charged prepare's own GraphQL requests: {stderr}"
    );

    // Every hold, and what caused it: the own request's projected spend crossing floor plus
    // reserve in the window that answered, as that window last read.
    let held = holds(&stderr);
    for hold in &held {
        let window = resets
            .iter()
            .position(|reset| *reset == hold.resets_at)
            .unwrap_or_else(|| panic!("{hold:?} names one of the token's windows"));
        assert!(
            hold.remaining - hold.cost < hold.floor + hold.reserve + 1.0,
            "held while its own request left room: {hold:?}"
        );
        assert!(
            charged
                .iter()
                .any(|one| one.window == window && one.remaining as f64 == hold.remaining),
            "{hold:?}: the remaining the window that answered reported"
        );
    }
    let waits = prepared["governor"]["waits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(waits.len(), held.len(), "one wait a hold line: {stderr}");
    let held_seconds: u64 = waits
        .iter()
        .filter_map(|wait| wait["seconds"].as_u64())
        .sum();
    assert!(
        held_seconds as f64 <= elapsed + 1.0,
        "every hold is real time inside the process's elapsed"
    );
    // Others' burn the governor booked, window by window, is no more than other work spent.
    let host: Value = serde_json::from_str(
        &std::fs::read_to_string(
            state
                .path()
                .join("onefollowup/governor/github-projects.json"),
        )
        .expect("the host state"),
    )
    .expect("JSON");
    let booked: Vec<f64> = resets
        .iter()
        .map(|reset| {
            host["resources"]["graphql"]["history"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|interval| {
                    interval["window"]
                        .as_str()
                        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                        .is_some_and(|at| at.timestamp() == *reset)
                })
                .map(|interval| interval["spend"].as_f64().unwrap_or_default())
                .sum()
        })
        .collect();
    let others: Vec<u64> = windows_after
        .iter()
        .zip(&windows_before)
        .map(|(after, before)| after.others - before.others)
        .collect();
    for (window, (booked, spent)) in booked.iter().zip(&others).enumerate() {
        assert!(
            *booked <= *spent as f64 + 1.0,
            "window {window}: the governor booked {booked} of others' burn, others spent {spent}"
        );
    }
    let own: Vec<u64> = (0..resets.len())
        .map(|window| {
            graphql
                .iter()
                .filter(|one| one.window == window)
                .map(|one| one.points)
                .sum()
        })
        .collect();

    // The seconds: the process's elapsed, holds inside it, plus each request's recorded
    // seconds, charged once — recomputed here from the recording, request by request.
    let charged_seconds: f64 = charged.iter().map(|one| one.seconds).sum();
    let recorded_seconds: f64 = charged
        .iter()
        .map(|one| {
            if one.document == "rate_limit" {
                crate::recorded::rate_limit_seconds()
            } else {
                crate::recorded::price(&one.document, &one.variables).seconds
            }
        })
        .sum();
    assert_eq!(
        requests,
        github.received().len() as u64 + by_phase.get("quota reads").map_or(0, |held| held.0),
        "one charge a request"
    );
    assert!(
        charged.iter().all(|one| one.seconds > 0.0),
        "every request takes GitHub's time"
    );
    assert_eq!(
        charged_seconds, recorded_seconds,
        "each request charged the seconds recorded for its binding"
    );
    let seconds = elapsed + charged_seconds;
    assert_eq!(
        seconds - elapsed,
        recorded_seconds,
        "the figure adds them once"
    );

    let per_draft = points as f64 / drafts.len() as f64;
    let routed = format!(
        "followups {} drafts, private-followups {} drafts",
        drafts.iter().filter(|(_, private)| !private).count(),
        drafts.iter().filter(|(_, private)| *private).count()
    );
    let phase_points = by_phase
        .iter()
        .map(|(phase, (sent, spent, _))| format!("{phase} {spent} points in {sent} requests"))
        .collect::<Vec<_>>()
        .join(", ");
    let falls = windows_before
        .iter()
        .zip(&windows_after)
        .enumerate()
        .map(|(at, (before, after))| {
            format!(
                "window {} fell {} ({} of it prepare's own, {} other work's)",
                ["A", "B"][at],
                before.remaining - after.remaining,
                own[at],
                others[at]
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    telemetry::record(
        "prepare-graphql-points-per-draft",
        per_draft.ceil() as usize,
        &format!(
            "{per_draft:.2} of prepare's own points a draft, rounded up: {points} points, each request at the cost its response reported, in {requests} requests for {} drafts; by phase: {phase_points}; {} searches asked, {} distinct searches sent, {rows} rows read, {limited} searches ended limited at 21 hits; drafts per routed destination: {routed}; beside them, {falls}; boards: followups {} tickets ({public_open} open), private-followups {} tickets; hits per searched public text: {}",
            drafts.len(),
            public_searches + private_searches,
            distinct.len(),
            boards[0].issues.len(),
            boards[1].issues.len(),
            spread.join(", "),
        ),
    );
    let phase_seconds = by_phase
        .iter()
        .map(|(phase, (sent, _, seconds))| format!("{phase} {sent} requests, {seconds:.1} s"))
        .collect::<Vec<_>>()
        .join(", ");
    let caused = held
        .iter()
        .map(|hold| {
            format!(
                "{:.1} own points against {} left, floor {} and reserve {} of the window resetting at {}",
                hold.cost, hold.remaining, hold.floor, hold.reserve, hold.resets_at - start
            )
        })
        .collect::<Vec<_>>();
    telemetry::record(
        "prepare-seconds-per-batch",
        seconds.ceil() as usize,
        &format!(
            "{seconds:.1} s, rounded up: the process's measured elapsed {elapsed:.1} s plus the recorded seconds of its {requests} requests in sequence ({charged_seconds:.1} s); governor holds: {} totalling {held_seconds} s, inside the elapsed{}; other work's burn the governor attributed: {:.0} points of window A, {:.0} of window B, beside the {} and {} other work spent; governor state {carried} bytes at start, {after} at end; requests and seconds by phase: {phase_seconds}; two interleaved windows, 2,600 and 4,700 of 5,000 points left, resetting 1,800 s and 3,350 s after the start, other work spending 105 points a minute",
            waits.len(),
            if caused.is_empty() {
                String::new()
            } else {
                format!(", each caused by {}", caused.join("; "))
            },
            booked[0],
            booked[1],
            others[0],
            others[1],
        ),
    );
    telemetry::record_journey(
        "prepare-cost",
        &json!({
            "points": points,
            "requests": requests,
            "elapsed_seconds": elapsed,
            "request_seconds": charged_seconds,
            "seconds": seconds,
            "by_phase": by_phase.iter().map(|(phase, (sent, spent, seconds))| json!({"phase": phase, "requests": sent, "points": spent, "seconds": seconds})).collect::<Vec<_>>(),
            "windows": {"before": windows_before.iter().map(|one| one.remaining).collect::<Vec<_>>(), "after": windows_after.iter().map(|one| one.remaining).collect::<Vec<_>>(), "own": own, "others": others, "booked_as_others": booked},
            "governor": prepared["governor"],
            "quota_reads": prepared["quota_reads"],
            "governor_state_bytes": {"start": carried, "end": after},
            "store_operations": prepared["store_operations"],
            "stderr": stderr,
        }),
    );
}
