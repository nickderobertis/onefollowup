//! The governor through the real `Store` and `Engine` over a loopback GitHub backend: the
//! GitHub Projects source sends real requests to a server on 127.0.0.1, and the governor reads
//! the quota it reports through `GET /rate_limit` there — nothing injects a reading. Only the
//! clock is virtual, so a 30-second hold costs the case no time.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use onefollowup::governor::{self, Clock as _, Parts, Reason};
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
fn a_request_held_on_exhausted_headroom_goes_once_when_a_rate_limit_read_while_held_finds_it_back()
{
    // Floor 1000 and no reserve, so the line is the floor; the loopback says 1002 left in a
    // window resetting in an hour, and never less: it charges nothing.
    let start = DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("now");
    let reset = start.timestamp() + 3600;
    let clock = Virtual::at(start);
    let loopback = Loopback::start(Arc::clone(&clock), 1002, reset);

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

    // A read: one GraphQL request, hinted at 1 point, admitted against the 1002 read first.
    // The loopback charges nothing and its answer carries no reading, so the governor's
    // estimate is 1001 left.
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

    // A two-search find: one GraphQL request hinted at 2 points. The call goes (the boundary
    // estimates a read call at the 1 point the last one spent), but its request is held: 1001
    // estimated is above floor plus reserve, 1000, and under it plus the hint, 1002 —
    // admission headroom exhausted.
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
        "held, a rate-limit read while held, then the held request: {received:?}"
    );
    let (refresh, held) = (&received[2], &received[3]);
    assert!(
        held.body.contains("first search") && held.body.contains("second search"),
        "the held request is the find's: {}",
        held.body
    );
    assert_eq!(
        refresh.at,
        start + chrono::Duration::seconds(30),
        "the rate-limit read reached the loopback while the request was held, at the first 30 s refresh"
    );
    assert_eq!(
        held.at, refresh.at,
        "the held request went on the reading that found headroom back"
    );
    assert!(held.at.timestamp() < reset, "before the reset");
    assert_eq!(
        received
            .iter()
            .filter(|one| one.body.contains("second search"))
            .count(),
        1,
        "the held request was received once"
    );

    let report = store.governor_report().expect("governed");
    assert_eq!(report.waits.len(), 1, "{report:?}");
    assert_eq!(report.waits[0].source, "board");
    assert_eq!(report.waits[0].resource, "graphql");
    assert_eq!(report.waits[0].seconds, 30);
    assert_eq!(report.waits[0].reason, Reason::Floor);
    assert_eq!(
        store.quota_reads(),
        2,
        "one at the first call, one while held"
    );
    let notices = notices.lock().expect("notices").clone();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].starts_with(
            "onefollowup: holding requests to board on graphql: remaining 1001 of 5000, floor 1000, reserve 0;"
        ),
        "{}",
        notices[0]
    );
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
        notices: Arc::new(|line: &str| eprintln!("{line}")),
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
