//! GitHub on 127.0.0.1, as far as one `onefollowup publish` of evidence comments to a GitHub
//! Projects destination asks of it: `GET /rate_limit`, and `POST /graphql` answering the
//! documents that path sends, told apart by what each document selects. Several boards share
//! one server, each a project of its own owner, as several destinations share one token.
//!
//! Grown from the loopback `crates/onefollowup/tests/governor_loopback.rs` serves in process;
//! onetaskgraph's own loopback board is not published for other repositories to use.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// One GraphQL request the loopback answered: its document and variables.
#[derive(Debug, Clone)]
pub struct Received {
    pub document: String,
    pub variables: Value,
}

impl Received {
    /// Whether it adds a comment: a content-creating write.
    pub fn adds_comment(&self) -> bool {
        self.document.contains("addComment(")
    }
}

/// GitHub's rate-limit state the loopback reports: GraphQL `remaining` of 5,000 until `reset`
/// (seconds since the epoch), every GraphQL request it answers taking `charge` from it.
#[derive(Debug, Clone, Copy)]
pub struct Quota {
    pub remaining: u64,
    pub reset: i64,
    pub charge: u64,
}

#[derive(Default)]
struct Held {
    received: Vec<Received>,
    comments: u64,
    /// Every request answered, as the boards' pricing charged it.
    charged: Vec<Charged>,
    /// Generated boards answering board-scoped searches and node reads; empty for the
    /// single-issue loopback the publish journeys use.
    boards: Arc<Vec<crate::loopback_boards::Board>>,
    /// Whether each GraphQL request takes its own price from the quota, rather than `charge`.
    priced: bool,
}

/// One request the loopback answered, as GitHub's published calculation prices it.
#[derive(Debug, Clone)]
pub struct Charged {
    /// The onetaskgraph document it sent, by name, or `rate_limit` for the REST read.
    pub document: String,
    /// Its price in points (0 for the REST read, which GraphQL's allowance does not meter).
    pub points: u64,
    /// The connection requests behind that price.
    pub connection_requests: u64,
    /// The issue rows its answer held.
    pub rows: u64,
    /// The searches (aliased search slots) it asked.
    pub searches: u64,
    /// The board's owner it read, when it read one.
    pub board: String,
    /// How long the loopback took to answer it.
    pub busy: std::time::Duration,
}

/// The loopback GitHub.
pub struct LoopbackGitHub {
    pub port: u16,
    held: Arc<Mutex<Held>>,
    quota: Arc<Mutex<Quota>>,
}

impl LoopbackGitHub {
    pub fn start(quota: Quota) -> Self {
        Self::serving(quota, Vec::new(), false)
    }

    /// A loopback answering board-scoped searches and node reads from `boards`, each GraphQL
    /// request taking its own price from the quota when `priced`.
    pub fn serving(
        quota: Quota,
        boards: Vec<crate::loopback_boards::Board>,
        priced: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();
        let held = Arc::new(Mutex::new(Held {
            boards: Arc::new(boards),
            priced,
            ..Held::default()
        }));
        let quota = Arc::new(Mutex::new(quota));
        let (serving, quoting) = (Arc::clone(&held), Arc::clone(&quota));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (serving, quoting) = (Arc::clone(&serving), Arc::clone(&quoting));
                std::thread::spawn(move || serve(stream, &serving, &quoting));
            }
        });
        Self { port, held, quota }
    }

    /// Every request answered so far, priced.
    pub fn charged(&self) -> Vec<Charged> {
        self.held.lock().expect("the log").charged.clone()
    }

    /// The quota as it stands.
    pub fn quota(&self) -> Quota {
        *self.quota.lock().expect("the quota")
    }

    /// The GraphQL endpoint a source of this loopback names.
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/graphql", self.port)
    }

    pub fn received(&self) -> Vec<Received> {
        self.held.lock().expect("the log").received.clone()
    }

    pub fn set_quota(&self, quota: Quota) {
        *self.quota.lock().expect("the quota") = quota;
    }
}

/// Serve one connection, request after request, until the client closes it.
fn serve(
    stream: std::net::TcpStream,
    held: &Mutex<Held>,
    quota: &Mutex<Quota>,
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
        let answer = if line.starts_with("GET /rate_limit") {
            let quota = *quota.lock().expect("the quota");
            held.lock().expect("the log").charged.push(Charged {
                document: "rate_limit".to_owned(),
                points: 0,
                connection_requests: 0,
                rows: 0,
                searches: 0,
                board: String::new(),
                busy: std::time::Duration::ZERO,
            });
            json!({"resources": {
                "graphql": {"limit": 5000, "used": 5000 - quota.remaining, "remaining": quota.remaining, "reset": quota.reset},
                "core": {"limit": 5000, "used": 0, "remaining": 5000, "reset": quota.reset},
            }})
        } else {
            let request: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let received = Received {
                document: request["query"].as_str().unwrap_or_default().to_owned(),
                variables: request["variables"].clone(),
            };
            let boards = Arc::clone(&held.lock().expect("the log").boards);
            let priced = held.lock().expect("the log").priced;
            let began = std::time::Instant::now();
            let answered = if boards.is_empty() {
                None
            } else {
                Some(crate::loopback_boards::answer(&boards, &received))
            };
            let busy = began.elapsed();
            let mut held = held.lock().expect("the log");
            held.received.push(received.clone());
            match answered {
                Some(answered) => {
                    let mut quota = quota.lock().expect("the quota");
                    let take = if priced { answered.points } else { quota.charge };
                    quota.remaining = quota.remaining.saturating_sub(take);
                    held.charged.push(Charged {
                        document: answered.document,
                        points: answered.points,
                        connection_requests: answered.connection_requests,
                        rows: answered.rows,
                        searches: answered.searches,
                                board: answered.board,
                        busy,
                    });
                    answered.answer
                }
                None => {
                    {
                        let mut quota = quota.lock().expect("the quota");
                        quota.remaining = quota.remaining.saturating_sub(quota.charge);
                    }
                    graphql(&received, &mut held)
                }
            }
        };
        let text = answer.to_string();
        // Every GraphQL answer carries the rate-limit headers GitHub sends on each response:
        // the governor learns a request's cost from them and releases its place in flight.
        let limits = if line.starts_with("GET /rate_limit") {
            String::new()
        } else {
            let quota = *quota.lock().expect("the quota");
            format!(
                "x-ratelimit-limit: 5000\r\nx-ratelimit-remaining: {}\r\nx-ratelimit-used: {}\r\nx-ratelimit-reset: {}\r\nx-ratelimit-resource: graphql\r\n",
                quota.remaining,
                5000 - quota.remaining,
                quota.reset
            )
        };
        if write!(
            writer,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{limits}Content-Length: {}\r\n\r\n{text}",
            text.len()
        )
        .is_err()
        {
            return;
        }
    }
}

/// The answer to one GraphQL request.
fn graphql(received: &Received, held: &mut Held) -> Value {
    if received.adds_comment() {
        let subject = received.variables["input"]["subjectId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        held.comments += 1;
        let body = received.variables["input"]["body"].clone();
        return json!({"data": {"addComment": {
            "subject": {"id": subject},
            "commentEdge": {"node": {
                "id": format!("IC_{}", held.comments),
                "databaseId": held.comments,
                "url": format!("https://github.com/example/widgets/issues/1#issuecomment-{}", held.comments),
                "author": {"login": "follow-up-bot"},
                "body": body,
                "createdAt": "2026-11-22T00:00:00Z",
                "updatedAt": "2026-11-22T00:00:00Z",
                "lastEditedAt": null,
            }},
        }}});
    }
    if received.document.contains("node(id:$id)") {
        let id = received.variables["id"].as_str().unwrap_or_default();
        return json!({"data": {"node": issue(id)}});
    }
    json!({"data": null, "errors": [{"type": "NOT_FOUND", "message": "Could not resolve to a node"}]})
}

/// The board every issue of the loopback is on: project 7, with a Status field.
fn board() -> Value {
    json!({"id": "PVT_board", "number": 7})
}

fn status_field() -> Value {
    json!({"__typename": "ProjectV2SingleSelectField", "id": "F_status", "name": "Status", "options": [
        {"id": "O_backlog", "name": "Backlog"},
        {"id": "O_todo", "name": "Todo"},
        {"id": "O_progress", "name": "In Progress"},
        {"id": "O_done", "name": "Done"},
    ]})
}

/// The issue `id`, open, in the Backlog of the board.
fn issue(id: &str) -> Value {
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
                "project": board(),
                "fieldValues": {"nodes": [{
                    "name": "Backlog",
                    "field": {"id": "F_status", "name": "Status", "options": status_field()["options"].clone()},
                }], "pageInfo": {"hasNextPage": false}},
            }],
            "pageInfo": {"hasNextPage": false, "endCursor": null},
        },
        "boards": {"nodes": [{"project": {
            "id": "PVT_board",
            "number": 7,
            "fields": {"nodes": [status_field()], "pageInfo": {"hasNextPage": false}},
        }}]},
        "blockedBy": {"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}},
    })
}
