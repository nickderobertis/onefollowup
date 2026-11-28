//! GitHub on 127.0.0.1, as far as one `onefollowup publish` of evidence comments to a GitHub
//! Projects destination asks of it: `GET /rate_limit`, and `POST /graphql` answering the
//! documents that path sends, told apart by what each document selects. Several boards share
//! one server, each a project of its own owner, as several destinations share one token.
//!
//! Serving generated boards ([`LoopbackGitHub::serving`]), it answers what one `onefollowup
//! prepare` asks instead — the find's batched search rounds, the detail and relation reads —
//! from them (`loopback_boards`), charging each request as its [`Pricing`] says, telling the
//! request what it cost in the `actualCost` it selects, and reporting what is left through the
//! `x-ratelimit-*` headers every response carries and `GET /rate_limit`, each window rolling
//! over to a whole allowance at its reset.
//!
//! A token can be shared ([`LoopbackGitHub::sharing`]) as GitHub was measured answering one:
//! from two windows of 5,000 points with different resets, interleaved request by request —
//! every response and every rate-limit read answered by the next window in turn — while other
//! work spends both at a steady rate in real time.
//!
//! Grown from the loopback `crates/onefollowup/tests/governor_loopback.rs` serves in process;
//! onetaskgraph's own loopback board is not published for other repositories to use.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

/// One token GitHub answers from several GraphQL windows in turn, `(remaining, reset)` each,
/// which other work spends at `others_per_minute` points a minute, shared evenly among them.
#[derive(Debug, Clone)]
pub struct Shared {
    pub windows: Vec<(u64, i64)>,
    pub others_per_minute: f64,
}

/// What each GraphQL request a loopback serving boards takes from the quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pricing {
    /// Its price by GitHub's published calculation.
    Published,
    /// The first search round at this many points, a board whose find is dear; every other
    /// request at its published price.
    FirstSearchAt(u64),
    /// The points GitHub charged, and the seconds it took, for the recorded real request of the
    /// same document and page sizes (`recorded`); a rate-limit read at the seconds GitHub took
    /// for one.
    Recorded,
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
    /// The variables it sent; none for the REST read.
    pub variables: Value,
    /// What it took from the quota, in points (0 for the REST read, which GraphQL's
    /// allowance does not meter).
    pub points: u64,
    /// The seconds GitHub takes to answer it, as its pricing says: charged, never slept.
    pub seconds: f64,
    /// The issue rows its answer held.
    pub rows: u64,
    /// The searches (aliased search slots) it asked, by their text.
    pub searches: Vec<String>,
    /// The GraphQL window that answered it, and what that window had left once it had.
    pub window: usize,
    pub remaining: u64,
}

/// One GraphQL window as the loopback holds it.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub remaining: u64,
    pub reset: i64,
    /// What other work has spent of it, in all.
    pub others: u64,
}

/// The token's windows, the next to answer, and other work's spend.
struct Token {
    windows: Vec<Window>,
    turn: usize,
    others_per_minute: f64,
    /// When other work's spend was last taken, and what of a point each window still owes.
    burned_at: Instant,
    owed: Vec<f64>,
    /// What every GraphQL request takes, on the single-issue loopback.
    charge: u64,
}

impl Token {
    fn single(quota: Quota) -> Self {
        Self {
            windows: vec![Window {
                remaining: quota.remaining,
                reset: quota.reset,
                others: 0,
            }],
            turn: 0,
            others_per_minute: 0.0,
            burned_at: Instant::now(),
            owed: vec![0.0],
            charge: quota.charge,
        }
    }

    fn shared(shared: &Shared) -> Self {
        Self {
            windows: shared
                .windows
                .iter()
                .map(|(remaining, reset)| Window {
                    remaining: *remaining,
                    reset: *reset,
                    others: 0,
                })
                .collect(),
            turn: 0,
            others_per_minute: shared.others_per_minute,
            burned_at: Instant::now(),
            owed: vec![0.0; shared.windows.len()],
            charge: 0,
        }
    }

    /// The windows as GitHub would report them now: other work's spend since it was last
    /// taken taken from each, and — where `roll` — a window past its reset a fresh one, the
    /// whole allowance left until its period on.
    fn now(&mut self, period: i64, roll: bool) {
        let spent = self.burned_at.elapsed().as_secs_f64() * self.others_per_minute / 60.0;
        self.burned_at = Instant::now();
        let share = spent / self.windows.len() as f64;
        let now = chrono::Utc::now().timestamp();
        for (window, owed) in self.windows.iter_mut().zip(&mut self.owed) {
            while roll && window.reset <= now {
                window.remaining = 5000;
                window.reset += period;
            }
            *owed += share;
            let whole = owed.floor();
            *owed -= whole;
            let whole = whole as u64;
            window.others += whole.min(window.remaining);
            window.remaining = window.remaining.saturating_sub(whole);
        }
    }

    /// The window that answers next, in turn.
    fn next(&mut self) -> usize {
        let window = self.turn % self.windows.len();
        self.turn += 1;
        window
    }
}

/// The loopback GitHub.
pub struct LoopbackGitHub {
    pub port: u16,
    held: Arc<Mutex<Held>>,
    token: Arc<Mutex<Token>>,
}

impl LoopbackGitHub {
    pub fn start(quota: Quota) -> Self {
        Self::listening(Token::single(quota), Held::default())
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
            Token::single(quota),
            Held {
                boards: Some((Arc::new(boards), pricing)),
                ..Held::default()
            },
        )
    }

    /// [`Self::serving`] `boards` on a token `shared` with other work, answered from its
    /// windows in turn.
    pub fn sharing(
        shared: &Shared,
        boards: Vec<crate::loopback_boards::Board>,
        pricing: Pricing,
    ) -> Self {
        Self::listening(
            Token::shared(shared),
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

    fn listening(token: Token, held: Held) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let port = listener.local_addr().expect("its address").port();
        let held = Arc::new(Mutex::new(held));
        let token = Arc::new(Mutex::new(token));
        let (serving, quoting) = (Arc::clone(&held), Arc::clone(&token));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (serving, quoting) = (Arc::clone(&serving), Arc::clone(&quoting));
                std::thread::spawn(move || serve(stream, &serving, &quoting));
            }
        });
        Self { port, held, token }
    }

    /// The GraphQL endpoint a source of this loopback names.
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/graphql", self.port)
    }

    pub fn received(&self) -> Vec<Received> {
        self.held.lock().expect("the log").received.clone()
    }

    pub fn set_quota(&self, quota: Quota) {
        *self.token.lock().expect("the token") = Token::single(quota);
    }

    /// Every request answered so far, as charged: a loopback serving boards only.
    pub fn charged(&self) -> Vec<Charged> {
        self.held.lock().expect("the log").charged.clone()
    }

    /// Each window of the token now, other work's spend so far included.
    pub fn windows(&self) -> Vec<Window> {
        let period = {
            let held = self.held.lock().expect("the log");
            if held.period > 0 { held.period } else { 3600 }
        };
        let mut token = self.token.lock().expect("the token");
        token.now(period, true);
        token.windows.clone()
    }
}

/// Serve one connection, request after request, until the client closes it.
fn serve(stream: std::net::TcpStream, held: &Mutex<Held>, token: &Mutex<Token>) {
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
        // The window that answers this request, and what it has left once it has.
        let (answer, window) = if rate_limit {
            let mut token = token.lock().expect("the token");
            token.now(period, boards.is_some());
            let at = token.next();
            let window = token.windows[at];
            if let Some((_, pricing)) = &boards {
                held.lock().expect("the log").charged.push(Charged {
                    document: "rate_limit".to_owned(),
                    variables: Value::Null,
                    points: 0,
                    seconds: if *pricing == Pricing::Recorded {
                        crate::recorded::rate_limit_seconds()
                    } else {
                        0.0
                    },
                    rows: 0,
                    searches: Vec::new(),
                    window: at,
                    remaining: window.remaining,
                });
            }
            (
                json!({"resources": {
                    "graphql": {"limit": 5000, "used": 5000 - window.remaining, "remaining": window.remaining, "reset": window.reset},
                    "core": {"limit": 5000, "used": 0, "remaining": 5000, "reset": window.reset},
                }}),
                window,
            )
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
                    let (points, seconds) = match pricing {
                        Pricing::FirstSearchAt(points) if searching && !held.searched => {
                            (*points, 0.0)
                        }
                        Pricing::Recorded => {
                            let price =
                                crate::recorded::price(&answered.document, &received.variables);
                            (price.points, price.seconds)
                        }
                        _ => (
                            crate::graphql_cost::points(&received.document, &received.variables),
                            0.0,
                        ),
                    };
                    held.searched |= searching;
                    let (at, window) = {
                        let mut token = token.lock().expect("the token");
                        token.now(period, true);
                        let at = token.next();
                        let window = &mut token.windows[at];
                        window.remaining = window.remaining.saturating_sub(points);
                        (at, *window)
                    };
                    let mut answer = answered.answer;
                    // What GitHub says the request cost, where the document asks.
                    if documents::without_reported_cost(&received.document).is_some() {
                        documents::answer_reported_cost(&mut answer["data"], points);
                    }
                    held.charged.push(Charged {
                        document: answered.document,
                        variables: received.variables.clone(),
                        points,
                        seconds,
                        rows: answered.rows,
                        searches: answered.searches,
                        window: at,
                        remaining: window.remaining,
                    });
                    held.received.push(received);
                    (answer, window)
                }
                None => {
                    let window = {
                        let mut token = token.lock().expect("the token");
                        let charge = token.charge;
                        let window = &mut token.windows[0];
                        window.remaining = window.remaining.saturating_sub(charge);
                        *window
                    };
                    let mut held = held.lock().expect("the log");
                    held.received.push(received.clone());
                    (graphql(&received, &mut held), window)
                }
            }
        };
        let text = answer.to_string();
        // Serving boards, every GraphQL answer carries the rate-limit headers GitHub sends on
        // each response, of the window that answered it: the governor reads its windows there.
        let limits = if boards.is_some() && !rate_limit {
            format!(
                "x-ratelimit-limit: 5000\r\nx-ratelimit-remaining: {}\r\nx-ratelimit-used: {}\r\nx-ratelimit-reset: {}\r\nx-ratelimit-resource: graphql\r\n",
                window.remaining,
                5000 - window.remaining,
                window.reset
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
