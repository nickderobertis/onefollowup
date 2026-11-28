//! GitHub on 127.0.0.1, as far as one `onefollowup publish` of evidence comments to a GitHub
//! Projects destination asks of it: `GET /rate_limit`, and `POST /graphql` answering the
//! documents that path sends, told apart by what each document selects. Several boards share
//! one server, each a project of its own owner, as several destinations share one token.
//!
//! Serving generated boards ([`LoopbackGitHub::serving`]), it answers what one `onefollowup
//! prepare` asks instead — the find's batched search rounds, the detail and relation reads —
//! from them (`loopback_boards`), charging each request by GitHub's published calculation
//! (`graphql_cost`) and reporting what is left through the `x-ratelimit-*` headers every
//! response carries and `GET /rate_limit`, its window rolling over to a whole allowance at
//! each reset.
//!
//! Grown from the loopback `crates/onefollowup/tests/governor_loopback.rs` serves in process;
//! onetaskgraph's own loopback board is not published for other repositories to use.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use onetaskgraph_github_projects::graphql as documents;
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

/// What each GraphQL request a loopback serving boards takes from the quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pricing {
    /// Its price by GitHub's published calculation.
    Published,
    /// The first search round at this many points, a board whose find is dear; every other
    /// request at its published price.
    FirstSearchAt(u64),
}

#[derive(Default)]
struct Held {
    received: Vec<Received>,
    comments: u64,
    /// Every request answered, as the boards' pricing charged it.
    charged: Vec<Charged>,
    /// Generated boards answering board-scoped searches and node reads, and how each request
    /// is priced; none for the single-issue loopback the publish journeys use.
    boards: Option<(Arc<Vec<crate::loopback_boards::Board>>, Pricing)>,
    /// Whether a search round was answered yet.
    searched: bool,
    /// How long each window lasts once the first resets, in seconds; an hour when 0.
    period: i64,
}

/// One request the loopback answered, as it charged it.
#[derive(Debug, Clone)]
pub struct Charged {
    /// The onetaskgraph document it sent, by name, or `rate_limit` for the REST read.
    pub document: String,
    /// What it took from the quota, in points (0 for the REST read, which GraphQL's
    /// allowance does not meter).
    pub points: u64,
    /// The issue rows its answer held.
    pub rows: u64,
    /// The searches (aliased search slots) it asked, by their text.
    pub searches: Vec<String>,
}

/// The loopback GitHub.
pub struct LoopbackGitHub {
    pub port: u16,
    held: Arc<Mutex<Held>>,
    quota: Arc<Mutex<Quota>>,
}

impl LoopbackGitHub {
    pub fn start(quota: Quota) -> Self {
        Self::listening(quota, Held::default())
    }

    /// A loopback answering board-scoped searches and node reads from `boards`, each GraphQL
    /// request priced by `pricing` and its window rolling over to a whole allowance, an hour
    /// on, at each reset.
    pub fn serving(
        quota: Quota,
        boards: Vec<crate::loopback_boards::Board>,
        pricing: Pricing,
    ) -> Self {
        Self::listening(
            quota,
            Held {
                boards: Some((Arc::new(boards), pricing)),
                ..Held::default()
            },
        )
    }

    /// The same loopback, each window after the first lasting `seconds` rather than an hour.
    pub fn with_period(self, seconds: i64) -> Self {
        self.held.lock().expect("the log").period = seconds;
        self
    }

    fn listening(quota: Quota, held: Held) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();
        let held = Arc::new(Mutex::new(held));
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

    /// Every request answered so far, as charged: a loopback serving boards only.
    pub fn charged(&self) -> Vec<Charged> {
        self.held.lock().expect("the log").charged.clone()
    }
}

/// The quota as GitHub would report it now: a window past its reset is a fresh one, the whole
/// allowance left until its period on.
fn rolled(quota: &mut Quota, period: i64) -> Quota {
    let now = chrono::Utc::now().timestamp();
    while quota.reset <= now {
        quota.remaining = 5000;
        quota.reset += period;
    }
    *quota
}

/// Serve one connection, request after request, until the client closes it.
fn serve(stream: std::net::TcpStream, held: &Mutex<Held>, quota: &Mutex<Quota>) {
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
        let (boards, period) = {
            let held = held.lock().expect("the log");
            (
                held.boards.clone(),
                if held.period > 0 { held.period } else { 3600 },
            )
        };
        let rate_limit = line.starts_with("GET /rate_limit");
        let answer = if rate_limit {
            let quota = if boards.is_some() {
                let mut held = held.lock().expect("the log");
                held.charged.push(Charged {
                    document: "rate_limit".to_owned(),
                    points: 0,
                    rows: 0,
                    searches: Vec::new(),
                });
                rolled(&mut quota.lock().expect("the quota"), period)
            } else {
                *quota.lock().expect("the quota")
            };
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
            match &boards {
                Some((boards, pricing)) => {
                    let answered = crate::loopback_boards::answer(boards, &received);
                    let mut held = held.lock().expect("the log");
                    let searching = !answered.searches.is_empty();
                    let points = match pricing {
                        Pricing::FirstSearchAt(points) if searching && !held.searched => *points,
                        _ => answered.points,
                    };
                    held.searched |= searching;
                    {
                        let mut quota = quota.lock().expect("the quota");
                        rolled(&mut quota, period);
                        quota.remaining = quota.remaining.saturating_sub(points);
                    }
                    let mut answer = answered.answer;
                    // What GitHub says the request cost, where the document asks.
                    if documents::without_reported_cost(&received.document).is_some() {
                        documents::answer_reported_cost(&mut answer["data"], points);
                    }
                    held.received.push(received);
                    held.charged.push(Charged {
                        document: answered.document,
                        points,
                        rows: answered.rows,
                        searches: answered.searches,
                    });
                    answer
                }
                None => {
                    {
                        let mut quota = quota.lock().expect("the quota");
                        quota.remaining = quota.remaining.saturating_sub(quota.charge);
                    }
                    let mut held = held.lock().expect("the log");
                    held.received.push(received.clone());
                    graphql(&received, &mut held)
                }
            }
        };
        let text = answer.to_string();
        // Serving boards, every GraphQL answer carries the rate-limit headers GitHub sends on
        // each response: the governor learns a request's cost from them.
        let limits = if boards.is_some() && !rate_limit {
            let quota = rolled(&mut quota.lock().expect("the quota"), period);
            format!(
                "x-ratelimit-limit: 5000\r\nx-ratelimit-remaining: {}\r\nx-ratelimit-used: {}\r\nx-ratelimit-reset: {}\r\nx-ratelimit-resource: graphql\r\n",
                quota.remaining,
                5000 - quota.remaining,
                quota.reset
            )
        } else {
            String::new()
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
