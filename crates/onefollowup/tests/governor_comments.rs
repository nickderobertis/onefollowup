//! The content-write caps over GitHub's batched comments: two onefollowup processes, each a
//! store of its own work directory publishing to a GitHub Projects board of its own, share one
//! `governor.state_dir` on one simulated clock. GitHub is a loopback on 127.0.0.1 that the real
//! GitHub Projects source sends its real requests to — several `addComment`s aliased into one
//! request, as `comment add-many` sends them — and GitHub's rate-limit state is the only thing
//! doubled. Every comment is stamped with the instant the loopback received it, so the caps are
//! checked per comment, never per request.

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use onefollowup::config::GovernorSettings;
use onefollowup::governor::{self, Clock as _, Parts, Pending, Reading, Readings};
use onefollowup::store::{CommentPost, Store};
use serde_json::{Value, json};

mod support;
use support::{Killed, Participant, Simulated, T0, instant, reading, shipped};

/// GitHub's rate-limit state as ample throughout: full windows resetting on the hour.
struct Ample(Arc<Simulated>);

impl Readings for Ample {
    fn read<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        let hour = (self.0.now().timestamp() - T0) / 3600;
        let reset = T0 + (hour + 1) * 3600;
        let readings = vec![
            reading("graphql", 5000, reset),
            reading("core", 5000, reset),
        ];
        Box::pin(async move { Ok(readings) })
    }
}

/// One comment the loopback added: its body, and when — on the simulated clock — it arrived.
#[derive(Debug, Clone)]
struct Added {
    body: String,
    at: DateTime<Utc>,
}

/// GitHub on 127.0.0.1, as far as a `comment add-many` on one issue asks of it: the issue's
/// read by `node(id:$id)`, and `addComment`s, alone or aliased `comment<n>` beside each other
/// in one request, each sent while its `$send<n>` holds.
struct Loopback {
    port: u16,
    added: Arc<Mutex<Vec<Added>>>,
    /// How many comments each content-creating request carried.
    per_request: Arc<Mutex<Vec<usize>>>,
}

impl Loopback {
    fn start(clock: Arc<Simulated>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();
        let added = Arc::new(Mutex::new(Vec::new()));
        let per_request = Arc::new(Mutex::new(Vec::new()));
        let (adding, counting) = (Arc::clone(&added), Arc::clone(&per_request));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (clock, adding, counting) = (
                    Arc::clone(&clock),
                    Arc::clone(&adding),
                    Arc::clone(&counting),
                );
                std::thread::spawn(move || serve(stream, &clock, &adding, &counting));
            }
        });
        Self {
            port,
            added,
            per_request,
        }
    }

    fn added(&self) -> Vec<Added> {
        self.added.lock().unwrap().clone()
    }

    fn per_request(&self) -> Vec<usize> {
        self.per_request.lock().unwrap().clone()
    }
}

/// Serve one connection, request after request, until the client closes it.
fn serve(
    stream: std::net::TcpStream,
    clock: &Simulated,
    added: &Mutex<Vec<Added>>,
    per_request: &Mutex<Vec<usize>>,
) {
    let mut writer = stream.try_clone().expect("a stream");
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut length = 0;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            if header == "\r\n" {
                break;
            }
            if let Some(value) = header.to_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let document = request["query"].as_str().unwrap_or_default();
        let variables = &request["variables"];
        let answer = if document.contains("addComment(") {
            let inputs: Vec<(String, &Value)> = if document.contains("comment0:addComment(") {
                (0..)
                    .map_while(|slot| {
                        variables
                            .get(format!("send{slot}"))
                            .map(|send| (slot, send))
                    })
                    .filter(|(_, send)| send.as_bool() == Some(true))
                    .map(|(slot, _)| {
                        (
                            format!("comment{slot}"),
                            &variables[format!("comment{slot}")],
                        )
                    })
                    .collect()
            } else {
                vec![("addComment".to_owned(), &variables["input"])]
            };
            per_request.lock().unwrap().push(inputs.len());
            let mut data = serde_json::Map::new();
            let mut added = added.lock().unwrap();
            for (alias, input) in inputs {
                let body = input["body"].as_str().unwrap_or_default().to_owned();
                added.push(Added {
                    body: body.clone(),
                    at: clock.now(),
                });
                let number = added.len();
                data.insert(
                    alias,
                    json!({
                        "subject": {"id": input["subjectId"]},
                        "commentEdge": {"node": {
                            "id": format!("IC_{number}"),
                            "databaseId": number,
                            "url": format!("https://github.com/example/widgets/issues/1#issuecomment-{number}"),
                            "author": {"login": "follow-up-bot"},
                            "body": body,
                            "createdAt": "2026-11-22T00:00:00Z",
                            "updatedAt": "2026-11-22T00:00:00Z",
                            "lastEditedAt": null,
                        }},
                    }),
                );
            }
            json!({"data": data})
        } else if document.contains("node(id:$id)") {
            json!({"data": {"node": issue(variables["id"].as_str().unwrap_or_default())}})
        } else {
            json!({"data": null, "errors": [{"type": "NOT_FOUND", "message": "Could not resolve to a node"}]})
        };
        let text = answer.to_string();
        if write!(
            writer,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{text}",
            text.len()
        )
        .is_err()
        {
            return;
        }
    }
}

fn status_options() -> Value {
    json!([
        {"id": "O_backlog", "name": "Backlog"},
        {"id": "O_todo", "name": "Todo"},
        {"id": "O_progress", "name": "In Progress"},
        {"id": "O_done", "name": "Done"},
    ])
}

/// The issue `id`, open, in the Backlog of project 7.
fn issue(id: &str) -> Value {
    let field = json!({"__typename": "ProjectV2SingleSelectField", "id": "F_status", "name": "Status", "options": status_options()});
    json!({
        "__typename": "Issue",
        "id": id,
        "number": 1,
        "title": "widgets: the export dialog drops unicode titles",
        "body": "The export dialog drops unicode titles.",
        "url": "https://github.com/example/widgets/issues/1",
        "createdAt": "2026-10-01T00:00:00Z",
        "updatedAt": "2026-10-01T00:00:00Z",
        "state": "OPEN",
        "stateReason": null,
        "repository": {"nameWithOwner": "example/widgets"},
        "parent": null,
        "subIssuesSummary": {"total": 0},
        "commentCount": {"totalCount": 0},
        "relationshipEvents": {"totalCount": 0},
        "labels": {"nodes": [], "pageInfo": {"hasNextPage": false}},
        "projectItems": {
            "nodes": [{
                "id": format!("PVTI_{id}"),
                "updatedAt": "2026-10-01T00:00:00Z",
                "project": {"id": "PVT_board", "number": 7},
                "fieldValues": {"nodes": [{
                    "name": "Backlog",
                    "field": {"id": "F_status", "name": "Status", "options": status_options()},
                }], "pageInfo": {"hasNextPage": false}},
            }],
            "pageInfo": {"hasNextPage": false, "endCursor": null},
        },
        "boards": {"nodes": [{"project": {
            "id": "PVT_board",
            "number": 7,
            "fields": {"nodes": [field], "pageInfo": {"hasNextPage": false}},
        }}]},
        "blockedBy": {"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}},
    })
}

/// One onefollowup process's world: a work directory whose destination `board` is the GitHub
/// Projects board of the owner `name` at the loopback, its evidence comments all on one issue.
/// The source's own pacing is off (`min_mutation_interval_ms: 0`, its setting for a loopback),
/// so the governor's caps are all that hold a comment, and its real-time windows cost no wait.
struct Process {
    _directory: tempfile::TempDir,
    root: PathBuf,
    name: String,
}

impl Process {
    fn new(name: &str, loopback: &Loopback) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        std::fs::create_dir_all(root.join("drafts-root/tasks")).unwrap();
        std::fs::write(
            root.join("onetaskgraph.yaml"),
            format!(
                "sources:\n  drafts:\n    plugin: local-md\n    config:\n      root: drafts-root\n  board:\n    plugin: github-projects\n    config:\n      owner: {name}-org\n      project_number: 7\n      endpoint: http://127.0.0.1:{}/graphql\n      token_env: LOOPBACK_TOKEN\n      pacing:\n        min_mutation_interval_ms: 0\n",
                loopback.port
            ),
        )
        .unwrap();
        Self {
            _directory: directory,
            root,
            name: name.to_owned(),
        }
    }

    fn store(&self, clock: &Arc<Simulated>, settings: &GovernorSettings) -> Store {
        let home = self.root.join("home");
        let environment = vec![
            ("HOME".to_owned(), home.display().to_string()),
            (
                "XDG_CONFIG_HOME".to_owned(),
                home.join("config").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".to_owned(),
                home.join("cache").display().to_string(),
            ),
            // A loopback board's token: it reaches nothing but this test's server.
            ("LOOPBACK_TOKEN".to_owned(), "loopback".to_owned()),
        ];
        let parts = Parts {
            clock: clock.clone(),
            readings: Some(Arc::new(Ample(Arc::clone(clock)))),
            notices: Arc::new(|_: &str| {}),
        };
        governor::with_parts(parts, || {
            Store::open(Some(&self.root), &self.root, &environment)
                .unwrap()
                .with_local("drafts")
                .with_governor(settings)
        })
    }

    fn post(&self, number: usize) -> CommentPost {
        CommentPost {
            item: format!("board:I_{}", self.name),
            body: format!("evidence {} {number:03}.", self.name),
        }
    }

    /// This process's comments the loopback added, by body, with how often each was.
    fn written(&self, loopback: &Loopback) -> BTreeMap<String, usize> {
        let mine = format!("evidence {} ", self.name);
        let mut written = BTreeMap::new();
        for added in loopback.added() {
            if added.body.starts_with(&mine) {
                *written.entry(added.body).or_insert(0) += 1;
            }
        }
        written
    }

    /// The posts of `numbers` the loopback has not added: what a restarted publish sends.
    fn unwritten(&self, loopback: &Loopback, numbers: std::ops::Range<usize>) -> Vec<CommentPost> {
        let written = self.written(loopback);
        numbers
            .map(|number| self.post(number))
            .filter(|post| !written.contains_key(&post.body))
            .collect()
    }
}

/// Run `posts` of `process` as one `comment add-many` on a thread of its own taking part in
/// `clock`: `Ok` once it answered every comment written, `Err` when the case killed it.
fn publish(
    process: &Arc<Process>,
    clock: &Arc<Simulated>,
    settings: &GovernorSettings,
    posts: Vec<CommentPost>,
    killed_at: Option<DateTime<Utc>>,
) -> std::thread::JoinHandle<Result<(), ()>> {
    let process = Arc::clone(process);
    let clock_of = Arc::clone(clock);
    let settings = settings.clone();
    // Joined before the thread starts, so the clock never moves past a process not yet begun.
    let participant: Participant = clock.join();
    std::thread::spawn(move || {
        let _taking_part = participant;
        if let Some(at) = killed_at {
            clock_of.kill(std::thread::current().id(), at);
        }
        let ran = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let store = process.store(&clock_of, &settings);
            let answers = store.add_comments("board", &posts).unwrap();
            assert_eq!(answers.len(), posts.len());
            assert!(
                answers.iter().all(|answer| answer.outcome == "written"),
                "{answers:?}"
            );
        }));
        match ran {
            Ok(()) => Ok(()),
            Err(payload) if payload.is::<Killed>() => Err(()),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

/// The most of `writes` in any rolling `span` seconds.
fn most_in_any(writes: &[DateTime<Utc>], span: i64) -> usize {
    let mut sorted = writes.to_vec();
    sorted.sort();
    (0..sorted.len())
        .map(|first| {
            sorted[first..]
                .iter()
                .take_while(|at| (**at - sorted[first]).num_seconds() < span)
                .count()
        })
        .max()
        .unwrap_or(0)
}

/// How many of `writes` fall in each instant, in seconds from the start.
fn per_instant(writes: &[DateTime<Utc>]) -> Vec<(i64, usize)> {
    let mut counted: BTreeMap<i64, usize> = BTreeMap::new();
    for at in writes {
        *counted.entry(at.timestamp() - T0).or_default() += 1;
    }
    counted.into_iter().collect()
}

#[test]
fn two_processes_of_150_batched_comments_and_a_restart_stay_inside_the_shipped_caps_per_comment() {
    let state = tempfile::tempdir().unwrap();
    let settings = GovernorSettings {
        state_dir: Some(state.path().to_path_buf()),
        ..shipped()
    };
    assert_eq!(settings.max_content_writes_per_minute, 40);
    assert_eq!(settings.max_content_writes_per_hour, 250);
    let clock = Simulated::at(instant(T0));
    let loopback = Loopback::start(Arc::clone(&clock));
    let alpha = Arc::new(Process::new("alpha", &loopback));
    let beta = Arc::new(Process::new("beta", &loopback));
    assert_ne!(alpha.root, beta.root);

    // Both publish 150 at once; whichever is waiting for the hour at its half is killed there,
    // and restarted with what it had not written.
    let runs: Vec<_> = [&alpha, &beta]
        .into_iter()
        .map(|process| {
            let posts = (0..150).map(|number| process.post(number)).collect();
            let run = publish(process, &clock, &settings, posts, Some(instant(T0 + 1800)));
            (Arc::clone(process), run)
        })
        .collect();
    let killed: Vec<Arc<Process>> = runs
        .into_iter()
        .filter_map(|(process, run)| run.join().unwrap().is_err().then_some(process))
        .collect();
    let killed_at = clock.now();
    for victim in &killed {
        let left = victim.unwritten(&loopback, 0..150);
        assert!(!left.is_empty(), "killed with comments left to write");
        publish(victim, &clock, &settings, left, None)
            .join()
            .unwrap()
            .unwrap();
    }

    // GitHub's source batched the comments: a request carried several, so a cap counted per
    // request would have let several times the cap through.
    let per_request = loopback.per_request();
    assert!(
        per_request.iter().any(|carried| *carried > 1),
        "comments were batched into requests: {per_request:?}"
    );
    let writes: Vec<DateTime<Utc>> = loopback.added().iter().map(|added| added.at).collect();
    assert_eq!(writes.len(), 300, "every comment written");
    assert!(
        most_in_any(&writes, 60) <= 40,
        "40 a minute, counted per comment: {:?}",
        per_instant(&writes)
    );
    assert!(
        most_in_any(&writes, 3600) <= 250,
        "250 an hour, counted per comment: {:?}",
        per_instant(&writes)
    );
    for process in [&alpha, &beta] {
        let written = process.written(&loopback);
        assert_eq!(
            written.len(),
            150,
            "every comment of {} landed",
            process.name
        );
        assert!(
            written.values().all(|times| *times == 1),
            "every comment written exactly once: {written:?}"
        );
    }
    assert_eq!(killed.len(), 1, "one process was killed mid-hour");
    assert_eq!(killed_at, instant(T0 + 1800));
    assert!(
        clock.now() >= instant(T0 + 3600),
        "the last comments waited for the hour"
    );
}
