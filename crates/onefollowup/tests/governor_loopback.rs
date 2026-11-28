//! The governor through the real `Store` and `Engine` over a loopback GitHub backend: the
//! GitHub Projects source sends real requests to a server on 127.0.0.1, and the governor reads
//! the quota it reports through `GET /rate_limit` there — nothing injects a reading. Only the
//! clock is virtual, so a 30-second hold costs the case no time.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use onefollowup::governor::{self, Clock as _, Parts};
use onefollowup::store::{Search, Store};

mod support;
use support::{Virtual, shipped};

/// One request the loopback received: its method and path, its body, and when — on the
/// case's virtual clock — it arrived.
#[derive(Debug, Clone)]
struct Received {
    line: String,
    body: String,
    at: DateTime<Utc>,
}

/// GitHub on 127.0.0.1: `GET /rate_limit` answers `graphql` at `remaining`, resetting at
/// `reset`; every GraphQL request is answered with no `x-ratelimit-*` headers — so no response
/// refreshes the governor's reading — and charges nothing. With `drop_first`, the first GraphQL
/// request is read and its connection closed before any header: a transport failure.
struct Loopback {
    port: u16,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Loopback {
    fn start(clock: Arc<Virtual>, remaining: u64, reset: i64) -> Self {
        Self::serving(clock, remaining, reset, false)
    }

    fn serving(clock: Arc<Virtual>, remaining: u64, reset: i64, drop_first: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            let mut dropped = !drop_first;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().expect("a stream"));
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err() || header == "\r\n" {
                        break;
                    }
                    if let Some(value) = header.to_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let line = line.trim().to_owned();
                let answer = if line.starts_with("GET /rate_limit") {
                    format!(
                        r#"{{"resources":{{"graphql":{{"limit":5000,"used":{},"remaining":{remaining},"reset":{reset}}},"core":{{"limit":5000,"used":0,"remaining":5000,"reset":{reset}}}}}}}"#,
                        5000 - remaining
                    )
                } else {
                    r#"{"data":null,"errors":[{"type":"NOT_FOUND","message":"Could not resolve to a node"}]}"#.to_owned()
                };
                let graphql = line.starts_with("POST /graphql");
                log.lock().expect("the log").push(Received {
                    line,
                    body: String::from_utf8_lossy(&body).into_owned(),
                    at: clock.now(),
                });
                if graphql && !std::mem::replace(&mut dropped, true) {
                    // Closed with nothing written: no status line, no header.
                    drop(reader);
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    continue;
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        Self { port, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().expect("the log").clone()
    }
}

#[test]
fn a_call_its_estimate_leaves_no_headroom_is_read_again_and_goes_once_on_the_reading_that_finds_it_back()
 {
    // Floor 1000 and no reserve, so the line is the floor; the loopback says 1001 left in a
    // window resetting in an hour, and never less: it charges nothing.
    let start = DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("now");
    let reset = start.timestamp() + 3600;
    let clock = Virtual::at(start);
    let loopback = Loopback::start(Arc::clone(&clock), 1001, reset);

    let directory = tempfile::tempdir().expect("a directory");
    let root = directory.path();
    std::fs::create_dir_all(root.join("drafts-root")).expect("the drafts root");
    std::fs::write(
        root.join("onetaskgraph.yaml"),
        format!(
            "sources:\n  drafts:\n    plugin: local-md\n    config:\n      root: drafts-root\n  board:\n    plugin: github-projects\n    config:\n      owner: example\n      project_number: 7\n      endpoint: http://127.0.0.1:{}/graphql\n      token_env: LOOPBACK_TOKEN\n",
            loopback.port
        ),
    )
    .expect("the store");
    let environment = vec![
        ("HOME".to_owned(), root.display().to_string()),
        (
            "XDG_CONFIG_HOME".to_owned(),
            root.join("config").display().to_string(),
        ),
        (
            "XDG_CACHE_HOME".to_owned(),
            root.join("cache").display().to_string(),
        ),
        // A loopback board's token: it reaches nothing but this test's server.
        ("LOOPBACK_TOKEN".to_owned(), "loopback".to_owned()),
    ];
    let notices = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&notices);
    let parts = Parts {
        clock: clock.clone(),
        readings: None,
        notices: Arc::new(move |line: &str| sink.lock().expect("notices").push(line.to_owned())),
    };
    let mut settings = shipped();
    settings.reserve_factor = 0.0;
    let store = governor::with_parts(parts, || {
        Store::open(Some(root), root, &environment).expect("the store opens")
    })
    .with_local("drafts")
    .with_governor(&settings);

    // A read: one GraphQL request, hinted at 1 point, admitted against the 1001 read first.
    // The loopback charges nothing and its answer carries no reading nor any reported cost, so
    // the governor counts its hint as spent and estimates 1000 left.
    store
        .read(&["board:I_kwDOAAAA".to_owned()], false)
        .expect("the read is answered");
    let after_read = loopback.received();
    assert_eq!(
        after_read
            .iter()
            .map(|one| one.line.as_str())
            .collect::<Vec<_>>(),
        ["GET /rate_limit HTTP/1.1", "POST /graphql HTTP/1.1"],
        "{after_read:?}"
    );

    // A two-search find: one GraphQL request hinted at 1 point, its search slots selecting
    // scalar fields alone. At its boundary the estimate, 1000 less the 1 point a read call was
    // admitted at, is under floor plus reserve, so the governor reads again before sending:
    // 1001, the read's spend shown to have been less than its hint. The find goes on that
    // reading, at once.
    let found = store
        .find(
            "board",
            &[
                Search {
                    key: "first".to_owned(),
                    text: Some("first search".to_owned()),
                    ..Search::default()
                },
                Search {
                    key: "second".to_owned(),
                    text: Some("second search".to_owned()),
                    ..Search::default()
                },
            ],
        )
        .expect("the find is answered");
    assert_eq!(found.len(), 2);

    let received = loopback.received();
    let lines: Vec<&str> = received.iter().map(|one| one.line.as_str()).collect();
    assert_eq!(
        lines,
        [
            "GET /rate_limit HTTP/1.1",
            "POST /graphql HTTP/1.1",
            "GET /rate_limit HTTP/1.1",
            "POST /graphql HTTP/1.1",
        ],
        "a rate-limit read before the find, then the find: {received:?}"
    );
    let (refresh, find) = (&received[2], &received[3]);
    assert!(
        find.body.contains("first search") && find.body.contains("second search"),
        "the last request is the find's: {}",
        find.body
    );
    assert_eq!(refresh.at, start, "read again at once, with no wait");
    assert_eq!(find.at, refresh.at, "the find went on that reading");
    assert_eq!(
        received
            .iter()
            .filter(|one| one.body.contains("second search"))
            .count(),
        1,
        "the find was received once"
    );

    let report = store.governor_report().expect("governed");
    assert_eq!(report.waits, Vec::new(), "{report:?}");
    assert_eq!(
        store.quota_reads(),
        2,
        "one at the first call, one before the find"
    );
    let notices = notices.lock().expect("notices").clone();
    assert!(notices.is_empty(), "{notices:?}");
    // Two store operations, the quota reads counted in neither.
    let operations: Vec<(String, String)> = store
        .operations()
        .into_iter()
        .map(|op| (op.op, op.source))
        .collect();
    assert_eq!(
        operations,
        [
            ("read".to_owned(), "board".to_owned()),
            ("find".to_owned(), "board".to_owned())
        ]
    );
}

/// A store of a local-md `drafts` source and two GitHub Projects boards, `board` and
/// `other-board`, both at the loopback on `port`, governed on `clock` by the store's own
/// readings and the shipped defaults.
fn two_board_store(root: &std::path::Path, port: u16, clock: &Arc<Virtual>) -> Store {
    two_board_store_noting(
        root,
        port,
        clock,
        Arc::new(|line: &str| eprintln!("{line}")),
    )
}

/// [`two_board_store`], its governor's and its phases' lines going to `notices`.
fn two_board_store_noting(
    root: &std::path::Path,
    port: u16,
    clock: &Arc<Virtual>,
    notices: governor::Notices,
) -> Store {
    std::fs::create_dir_all(root.join("drafts-root")).expect("the drafts root");
    let mut store = String::from(
        "sources:\n  drafts:\n    plugin: local-md\n    config:\n      root: drafts-root\n",
    );
    for (name, number) in [("board", 7), ("other-board", 8)] {
        store.push_str(&format!(
            "  {name}:\n    plugin: github-projects\n    config:\n      owner: example\n      project_number: {number}\n      endpoint: http://127.0.0.1:{port}/graphql\n      token_env: LOOPBACK_TOKEN\n"
        ));
    }
    std::fs::write(root.join("onetaskgraph.yaml"), store).expect("the store");
    let environment = vec![
        ("HOME".to_owned(), root.display().to_string()),
        (
            "XDG_CONFIG_HOME".to_owned(),
            root.join("config").display().to_string(),
        ),
        (
            "XDG_CACHE_HOME".to_owned(),
            root.join("cache").display().to_string(),
        ),
        // A loopback board's token: it reaches nothing but this test's server.
        ("LOOPBACK_TOKEN".to_owned(), "loopback".to_owned()),
    ];
    let parts = Parts {
        clock: clock.clone(),
        readings: None,
        notices,
    };
    governor::with_parts(parts, || {
        Store::open(Some(root), root, &environment).expect("the store opens")
    })
    .with_local("drafts")
    .with_governor(&shipped())
}

#[test]
fn a_request_failed_in_transport_before_any_header_ends_and_the_next_of_the_same_call_goes() {
    // One read of an item on each of two boards: one request a board. The first request to
    // reach the loopback is closed before any header — a transport failure, which reports no
    // reading — and the other board's request comes in the same call. Quota is ample, so only
    // the one in-flight slot could hold the second: it goes once the first has ended, and the
    // call returns.
    let start = DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("now");
    let clock = Virtual::at(start);
    let loopback = Loopback::serving(Arc::clone(&clock), 5000, start.timestamp() + 3600, true);
    let directory = tempfile::tempdir().expect("a directory");
    let root = directory.path().to_path_buf();
    let port = loopback.port;
    let (sender, answered) = std::sync::mpsc::channel();
    // On a thread of its own, so a second request waiting forever on the slot fails the case
    // at the deadline below rather than hanging it.
    std::thread::spawn(move || {
        let store = two_board_store(&root, port, &clock);
        let read = store.read(
            &[
                "board:I_kwDOAAAA".to_owned(),
                "other-board:I_kwDOBBBB".to_owned(),
            ],
            false,
        );
        let _ = sender.send((
            read.map_err(|error| error.to_string()),
            store.governor_report(),
            store.operations().len(),
        ));
    });
    let (read, report, operations) = answered
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the call returned: the failed request ended and the next one went");
    let read = read.expect("the read is answered item by item");
    let errors: Vec<&str> = read
        .iter()
        .map(|item| item.error.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.contains("connection closed before message completed"))
            .count(),
        1,
        "one item's request failed in transport: {errors:?}"
    );
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.contains("no task with the id"))
            .count(),
        1,
        "the other's was sent and answered: {errors:?}"
    );
    let posts: Vec<Received> = loopback
        .received()
        .into_iter()
        .filter(|one| one.line.starts_with("POST /graphql"))
        .collect();
    assert_eq!(posts.len(), 2, "one request a board: {posts:?}");
    let ids = |one: &Received| {
        ["I_kwDOAAAA", "I_kwDOBBBB"]
            .into_iter()
            .filter(|id| one.body.contains(id))
            .collect::<Vec<_>>()
    };
    let (first, second) = (ids(&posts[0]), ids(&posts[1]));
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(second.len(), 1, "{second:?}");
    assert_ne!(first, second, "the second request is the other board's");
    assert_eq!(
        report.expect("governed").waits,
        Vec::new(),
        "nothing waited on quota"
    );
    assert_eq!(operations, 2, "one read a board");
}

#[test]
fn a_phase_says_when_it_starts_every_20_requests_it_sends_and_when_it_ends() {
    // A read of 30 items the loopback holds none of: one batched request of 24 and one of 6,
    // each refused for a node that does not resolve, then each item read again on its own —
    // 32 requests in all, each priced by the source's own hint, no response reporting a
    // reading. The phase's lines name it, the 20th request and the points so far, and its end.
    let start = DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("now");
    let clock = Virtual::at(start);
    let loopback = Loopback::start(Arc::clone(&clock), 5000, start.timestamp() + 3600);
    let directory = tempfile::tempdir().expect("a directory");
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let store = two_board_store_noting(
        directory.path(),
        loopback.port,
        &clock,
        Arc::new(move |line: &str| sink.lock().expect("the lines").push(line.to_owned())),
    );
    let ids: Vec<String> = (0..30).map(|at| format!("board:I_kwDO{at:04}")).collect();
    let phase = store.phase("prepare", "kept-hit detail read", "30 items");
    store.read(&ids, true).expect("answered item by item");
    phase.finish();
    let posts = loopback
        .received()
        .iter()
        .filter(|one| one.line.starts_with("POST /graphql"))
        .count();
    let lines = lines.lock().expect("the lines").clone();
    let progress: Vec<&String> = lines
        .iter()
        .filter(|line| line.starts_with("onefollowup prepare: kept-hit detail read: "))
        .collect();
    assert_eq!(
        progress.first().map(|line| line.as_str()),
        Some("onefollowup prepare: kept-hit detail read: started: 30 items"),
        "{lines:?}"
    );
    let ticks = regex::Regex::new(
        r"^onefollowup prepare: kept-hit detail read: (\d+) requests, (\d+) points so far$",
    )
    .expect("a pattern");
    let ticked: Vec<(u64, u64)> = progress
        .iter()
        .filter_map(|line| ticks.captures(line))
        .map(|held| {
            (
                held[1].parse().expect("a count"),
                held[2].parse().expect("a count"),
            )
        })
        .collect();
    let finished = regex::Regex::new(
        r"^onefollowup prepare: kept-hit detail read: finished: (\d+) requests, (\d+) points$",
    )
    .expect("a pattern");
    let last = progress
        .last()
        .and_then(|line| finished.captures(line))
        .expect("its end");
    let (requests, points): (u64, u64) = (
        last[1].parse().expect("a count"),
        last[2].parse().expect("a count"),
    );
    // Every request the phase sent is counted: the GraphQL ones and the quota reads with them.
    let sent = loopback.received().len() as u64;
    assert_eq!(requests, sent, "{lines:?}");
    assert!(posts >= 26, "{posts} requests: one per item at least");
    let expected: Vec<u64> = (1..=requests / 20).map(|at| at * 20).collect();
    assert_eq!(
        ticked.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
        expected,
        "one line every 20 requests: {lines:?}"
    );
    assert!(
        ticked
            .iter()
            .all(|(_, so_far)| *so_far >= 1 && *so_far <= points),
        "{ticked:?} of {points}"
    );
    assert_eq!(progress.len(), 2 + expected.len(), "{lines:?}");
}
