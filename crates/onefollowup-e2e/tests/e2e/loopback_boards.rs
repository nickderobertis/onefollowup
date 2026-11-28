//! Generated GitHub Projects boards the loopback answers from: board-scoped issue searches,
//! one at a time (`SEARCH_ISSUES`) or batched (`SEARCH_BATCH`), issue reads by node id
//! (`ISSUE_DETAILS`, `ISSUE_DETAIL`, `ISSUE`) and their relations (`ISSUE_RELATIONS`, every
//! issue without sub-issues), each answer shaped as onetaskgraph's documents select it — under
//! 0.17.3 a batched search's rows its scalar fields alone, under the 0.17.0 an earlier release
//! sent each row the whole issue. What a request costs is the loopback GitHub's to say.
//!
//! A board seeded from a recorded real request ([`Board::recorded`]) answers what GitHub
//! answered it: its search index each recorded page's rows in the order GitHub listed them,
//! the rows the recording dropped as not public kept as holes a cursor still counts, and each
//! issue read as the recording holds it.
//!
//! Search follows GitHub's own rules as far as the walk needs them: the `project:<owner>/<n>`
//! and `is:` qualifiers scope it, `in:title`, `in:body` or `in:title,body` pick the fields, each
//! quoted phrase must appear in one of them (word-start match, case-insensitive, so a term
//! finds its plural as GitHub's stemming does), results come newest first, and no search
//! answers past its 1,000th result.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex};

use serde_json::{Value, json};

use onetaskgraph_github_projects::graphql as documents;

use crate::loopback_github::Received;

/// GitHub's issue search answers at most this many results of one query.
pub const SEARCH_CAP: usize = 1000;

/// One issue on a board.
#[derive(Debug, Clone)]
pub struct BoardIssue {
    /// Its node id.
    pub id: String,
    pub number: u64,
    pub title: String,
    /// Its whole body: visible text and the onetaskgraph metadata slot.
    pub body: String,
    /// `owner/name`.
    pub repository: String,
    pub open: bool,
    /// `COMPLETED`, `DUPLICATE` or `NOT_PLANNED` for a closed issue.
    pub state_reason: Option<String>,
    /// Its Status option on the board.
    pub status: String,
    /// How many labels it carries: each answered as one of the board's labels.
    pub labels: usize,
    /// Normalized title and body, for search.
    title_words: String,
    body_words: String,
}

impl BoardIssue {
    #[allow(
        clippy::too_many_arguments,
        reason = "one argument per field of the issue GitHub answers, each a different value the board generator sets"
    )]
    pub fn new(
        id: String,
        number: u64,
        title: String,
        body: String,
        repository: String,
        open: bool,
        state_reason: Option<&str>,
        status: &str,
    ) -> Self {
        let title_words = normalized(&title);
        let body_words = normalized(visible(&body));
        Self {
            id,
            number,
            title,
            body,
            repository,
            open,
            state_reason: state_reason.map(str::to_owned),
            status: status.to_owned(),
            labels: 0,
            title_words,
            body_words,
        }
    }
}

impl BoardIssue {
    /// The same issue, its whole body `body`.
    #[must_use]
    pub fn with_body(mut self, body: String) -> Self {
        self.body_words = normalized(visible(&body));
        self.body = body;
        self
    }

    /// The same issue, carrying `labels` labels.
    #[must_use]
    pub fn with_labels(mut self, labels: usize) -> Self {
        self.labels = labels;
        self
    }
}

/// How many issues of `boards` the board-scoped `search` matches, uncapped: GitHub's count of
/// them, which no walk past its 1,000th result reads.
pub fn counted(boards: &[Board], text: &str) -> usize {
    let query = parse_search(text);
    boards
        .iter()
        .find(|board| board.owner == query.owner && board.number == query.number)
        .map_or(0, |board| {
            board
                .issues
                .iter()
                .filter(|issue| matches(issue, &query))
                .count()
        })
}

/// One board: a project of its owner, and its issues, newest first — or, seeded from a
/// recorded real request, what GitHub answered that request.
#[derive(Debug, Clone)]
pub struct Board {
    pub owner: String,
    pub number: u64,
    pub issues: Vec<BoardIssue>,
    pub recorded: Option<Arc<Recorded>>,
}

impl Board {
    pub fn project_id(&self) -> String {
        format!("PVT_{}_{}", self.owner, self.number)
    }

    /// The board one recorded real request was answered from, `owner`'s project `number`: its
    /// search index and its issues as the sanitized answer holds them.
    pub fn recorded(
        owner: &str,
        number: u64,
        document: &str,
        variables: &Value,
        data: &Value,
    ) -> Self {
        let mut recorded = Recorded::default();
        match document {
            "SEARCH_BATCH" => {
                for slot in 0..onetaskgraph_github_projects::SEARCH_BATCH {
                    if variables[format!("ask{slot}")] != true {
                        continue;
                    }
                    let text = variables[format!("search{slot}")]
                        .as_str()
                        .unwrap_or_default();
                    let page = &data[format!("s{slot}")];
                    let from = offset_of(&variables[format!("after{slot}")]);
                    let to = if page["pageInfo"]["endCursor"].is_string() {
                        offset_of(&page["pageInfo"]["endCursor"])
                    } else {
                        from
                    };
                    let nodes = page["nodes"].as_array().cloned().unwrap_or_default();
                    let (positions, more) = recorded.searches.entry(text.to_owned()).or_default();
                    // Whether GitHub has more is what its furthest page said.
                    if positions.len() <= to {
                        positions.resize(to, None);
                        *more = page["pageInfo"]["hasNextPage"] == true;
                    }
                    // GitHub's rows in its order, then the rows the recording dropped as not
                    // public: a hole is no row of the answer, though a cursor counts it.
                    for (at, node) in nodes.iter().enumerate() {
                        let id = node["id"].as_str().unwrap_or_default().to_owned();
                        positions[from + at] = Some(id.clone());
                        recorded.hits.insert(id, node.clone());
                    }
                }
            }
            "ISSUE_DETAILS" => {
                let comments = variables["comments"] == true;
                for slot in 0..onetaskgraph_github_projects::DETAIL_BATCH {
                    let node = &data[format!("i{slot}")];
                    if let Some(id) = variables[format!("id{slot}")].as_str()
                        && !node.is_null()
                    {
                        recorded
                            .details
                            .insert((id.to_owned(), comments), node.clone());
                    }
                }
            }
            "ISSUE" => {
                if let Some(id) = variables["id"].as_str() {
                    recorded.issues.insert(id.to_owned(), data["node"].clone());
                }
            }
            other => panic!("no recorded {other} seeds a board"),
        }
        Self {
            owner: owner.to_owned(),
            number,
            issues: Vec::new(),
            recorded: Some(Arc::new(recorded)),
        }
    }
}

impl Board {
    /// This recorded board, holding `other`'s recorded answers beside its own: a walk across
    /// rounds, each page as GitHub answered it.
    pub fn merge(&mut self, other: &Self) {
        let (Some(mine), Some(theirs)) = (&self.recorded, &other.recorded) else {
            panic!("only recorded boards merge");
        };
        let mut merged = Recorded::clone(mine);
        for (text, (positions, more)) in &theirs.searches {
            let (held, beyond) = merged.searches.entry(text.clone()).or_default();
            if held.len() <= positions.len() {
                held.resize(positions.len(), None);
                *beyond = *more;
            }
            for (at, id) in positions.iter().enumerate() {
                if id.is_some() {
                    held[at] = id.clone();
                }
            }
        }
        merged.hits.extend(theirs.hits.clone());
        merged.details.extend(theirs.details.clone());
        merged.issues.extend(theirs.issues.clone());
        self.recorded = Some(Arc::new(merged));
    }
}

/// What a board seeded from a recorded real request answers.
#[derive(Debug, Default, Clone)]
pub struct Recorded {
    /// Each search text's rows by position, `None` for a row the recording dropped, and
    /// whether GitHub said there was more past the last.
    searches: HashMap<String, (Vec<Option<String>>, bool)>,
    /// Each issue as a search row carried it.
    hits: HashMap<String, Value>,
    /// Each issue as a detail read carried it, by whether that read asked its comments.
    details: HashMap<(String, bool), Value>,
    /// Each issue as a single-issue read carried it.
    issues: HashMap<String, Value>,
}

/// What of `body` a search reads: its text before onetaskgraph's metadata slot, the HTML
/// comment no rendered body shows and onetaskgraph's own text match does not read either — so
/// a search's hits are the ones onetaskgraph keeps, and a record's own words (`intermittent`,
/// `onetaskgraph`) match no issue that does not say them.
fn visible(body: &str) -> &str {
    body.find("<!-- onetaskgraph.metadata")
        .map_or(body, |slot| &body[..slot])
}

/// Lower-case words joined by single spaces, wrapped in spaces.
fn normalized(text: &str) -> String {
    let mut out = String::from(" ");
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        out.push_str(&word.to_lowercase());
        out.push(' ');
    }
    out
}

/// One parsed board-scoped search.
struct Query {
    owner: String,
    number: u64,
    open_only: bool,
    title: bool,
    body: bool,
    phrases: Vec<String>,
}

fn parse_search(search: &str) -> Query {
    let mut query = Query {
        owner: String::new(),
        number: 0,
        open_only: false,
        title: true,
        body: true,
        phrases: Vec::new(),
    };
    let chars: Vec<char> = search.chars().collect();
    let mut at = 0;
    let mut words: Vec<(String, bool)> = Vec::new();
    while at < chars.len() {
        if chars[at].is_whitespace() {
            at += 1;
        } else if chars[at] == '"' {
            let mut phrase = String::new();
            at += 1;
            while at < chars.len() && chars[at] != '"' {
                if chars[at] == '\\' && at + 1 < chars.len() {
                    at += 1;
                }
                phrase.push(chars[at]);
                at += 1;
            }
            at += 1;
            words.push((phrase, true));
        } else {
            let start = at;
            while at < chars.len() && !chars[at].is_whitespace() {
                at += 1;
            }
            words.push((chars[start..at].iter().collect(), false));
        }
    }
    for (word, quoted) in words {
        if !quoted {
            if let Some(project) = word.strip_prefix("project:") {
                let (owner, number) = project.split_once('/').expect("project:<owner>/<n>");
                query.owner = owner.to_owned();
                query.number = number.parse().expect("a project number");
                continue;
            }
            if word == "is:issue" {
                continue;
            }
            if word == "is:open" || word == "state:open" {
                query.open_only = true;
                continue;
            }
            if let Some(fields) = word.strip_prefix("in:") {
                query.title = fields.split(',').any(|field| field == "title");
                query.body = fields.split(',').any(|field| field == "body");
                continue;
            }
        }
        let phrase = normalized(&word);
        if phrase.trim().is_empty() {
            continue;
        }
        // A word-start match: the trailing space is dropped so a term finds its longer forms.
        query.phrases.push(phrase.trim_end().to_owned());
    }
    query
}

fn matches(issue: &BoardIssue, query: &Query) -> bool {
    if query.open_only && !issue.open {
        return false;
    }
    query.phrases.iter().all(|phrase| {
        (query.title && issue.title_words.contains(phrase.as_str()))
            || (query.body && issue.body_words.contains(phrase.as_str()))
    })
}

/// The node id of every issue of `boards` the board-scoped `search` matches, in the order a walk
/// of every page of it lists them.
pub fn matched(boards: &[Board], text: &str) -> Vec<String> {
    let query = parse_search(text);
    let Some(board) = boards
        .iter()
        .find(|board| board.owner == query.owner && board.number == query.number)
    else {
        return Vec::new();
    };
    search(boards, text)
        .iter()
        .map(|at| board.issues[*at].id.clone())
        .collect()
}

/// The positions on its board of every issue `search` finds, newest first, capped as GitHub
/// caps a search; remembered per set of boards and search text, so a walk's later pages cost
/// the loopback nothing. A loopback's boards live as long as the process, so no two sets share
/// an address.
fn search(boards: &[Board], search: &str) -> Arc<Vec<usize>> {
    type Found = HashMap<(usize, String), Arc<Vec<usize>>>;
    static FOUND: LazyLock<Mutex<Found>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    let key = (boards.as_ptr() as usize, search.to_owned());
    if let Some(found) = FOUND.lock().expect("the cache").get(&key) {
        return Arc::clone(found);
    }
    let query = parse_search(search);
    let found: Vec<usize> = boards
        .iter()
        .find(|board| board.owner == query.owner && board.number == query.number)
        .map(|board| {
            board
                .issues
                .iter()
                .enumerate()
                .filter(|(_, issue)| matches(issue, &query))
                .map(|(at, _)| at)
                .take(SEARCH_CAP)
                .collect()
        })
        .unwrap_or_default();
    let found = Arc::new(found);
    FOUND
        .lock()
        .expect("the cache")
        .insert(key, Arc::clone(&found));
    found
}

fn status_field() -> Value {
    json!({"__typename": "ProjectV2SingleSelectField", "id": "F_status", "name": "Status", "options": [
        {"id": "O_backlog", "name": "Backlog"},
        {"id": "O_todo", "name": "Todo"},
        {"id": "O_progress", "name": "In Progress"},
        {"id": "O_done", "name": "Done"},
        {"id": "O_cancelled", "name": "Cancelled"},
    ]})
}

/// `issue` of `board`, as `BoardIssue` selects it.
fn node(board: &Board, issue: &BoardIssue) -> Value {
    json!({
        "__typename": "Issue",
        "id": issue.id,
        "number": issue.number,
        "title": issue.title,
        "body": issue.body,
        "url": format!("https://github.com/{}/issues/{}", issue.repository, issue.number),
        "createdAt": "2026-10-01T00:00:00Z",
        "updatedAt": "2026-10-01T00:00:00Z",
        "state": if issue.open { "OPEN" } else { "CLOSED" },
        "stateReason": issue.state_reason,
        "repository": {"nameWithOwner": issue.repository},
        "parent": null,
        "subIssuesSummary": {"total": 0},
        "commentCount": {"totalCount": 0},
        "relationshipEvents": {"totalCount": 0},
        "labels": {
            "nodes": (0..issue.labels)
                .map(|at| json!({"id": format!("LA_{}_{at}", issue.id), "name": format!("area-{at}"), "color": "ededed"}))
                .collect::<Vec<_>>(),
            "pageInfo": {"hasNextPage": false},
        },
        "projectItems": {
            "nodes": [{
                "id": format!("PVTI_{}", issue.id),
                "updatedAt": "2026-10-01T00:00:00Z",
                "project": {"id": board.project_id(), "number": board.number},
                "fieldValues": {"nodes": [{
                    "name": issue.status,
                    "field": {"id": "F_status", "name": "Status", "options": status_field()["options"].clone()},
                }], "pageInfo": {"hasNextPage": false}},
            }],
            "pageInfo": {"hasNextPage": false, "endCursor": null},
        },
    })
}

/// The node `id` names, with an empty first page of comments when `comments` asks for them:
/// what a detail read selects of it — on a recorded board, as the recording holds it.
fn commented(boards: &[Board], id: &str, comments: bool) -> Option<Value> {
    boards.iter().find_map(|board| {
        if let Some(recorded) = &board.recorded {
            return recorded.details.get(&(id.to_owned(), comments)).cloned();
        }
        board
            .issues
            .iter()
            .find(|issue| issue.id == id)
            .map(|issue| {
                let mut node = node(board, issue);
                if comments {
                    node["comments"] =
                        json!({"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}});
                }
                node
            })
    })
}

/// The node `id` names as a single-issue read selects it: with its board's fields and no
/// blockers — on a recorded board, as the recording holds it.
fn single(boards: &[Board], id: &str) -> Option<Value> {
    boards.iter().find_map(|board| {
        if let Some(recorded) = &board.recorded {
            return recorded.issues.get(id).cloned();
        }
        board
            .issues
            .iter()
            .find(|issue| issue.id == id)
            .map(|issue| {
                let mut node = node(board, issue);
                node["boards"] = json!({"nodes": [{"project": {
                    "id": board.project_id(),
                    "number": board.number,
                    "fields": {"nodes": [status_field()], "pageInfo": {"hasNextPage": false}},
                }}]});
                node["blockedBy"] =
                    json!({"nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}});
                node
            })
    })
}

/// The node `id` names, with no sub-issues: what a relations read selects of it.
fn childless(boards: &[Board], id: &str) -> Option<Value> {
    boards.iter().find_map(|board| {
        board
            .issues
            .iter()
            .find(|issue| issue.id == id)
            .map(|issue| {
                let mut node = node(board, issue);
                node["subIssues"] = json!({"nodes": [], "pageInfo": {"hasNextPage": false}});
                node
            })
    })
}

/// `issue` as one row of a batched search slot selects it: its scalar fields alone.
fn hit(issue: &BoardIssue) -> Value {
    json!({
        "__typename": "Issue",
        "id": issue.id,
        "number": issue.number,
        "title": issue.title,
        "state": if issue.open { "OPEN" } else { "CLOSED" },
        "updatedAt": "2026-10-01T00:00:00Z",
        "repository": {"nameWithOwner": issue.repository},
    })
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// A search cursor as GitHub spells one: `cursor:<rows so far>`, base64-encoded.
pub fn cursor(at: usize) -> String {
    let bytes = format!("cursor:{at}").into_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0_u32, |word, (at, byte)| {
            word | u32::from(*byte) << (16 - 8 * at)
        });
        for at in 0..4 {
            if at <= chunk.len() {
                out.push(char::from(BASE64[(word >> (18 - 6 * at) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The rows before the cursor `after` names, as [`cursor`] spells it.
pub fn offset_of(after: &Value) -> usize {
    let Some(text) = after.as_str() else {
        return 0;
    };
    let mut bytes = Vec::new();
    let (mut word, mut bits) = (0_u32, 0);
    for one in text.bytes().take_while(|one| *one != b'=') {
        let Some(value) = BASE64.iter().position(|known| *known == one) else {
            return 0;
        };
        word = word << 6 | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((word >> bits & 255) as u8);
        }
    }
    String::from_utf8(bytes)
        .ok()
        .and_then(|text| text.strip_prefix("cursor:")?.parse().ok())
        .unwrap_or(0)
}

/// One page of `search`, after the cursor `after`, of `first` rows, each row as `row` shapes
/// it: its connection, and the rows it holds. Its cursor counts rows as GitHub does, a
/// recorded board's holes among them.
fn page(
    boards: &[Board],
    text: &str,
    first: u64,
    after: &Value,
    row: fn(&Board, &BoardIssue) -> Value,
) -> (Value, u64) {
    let query = parse_search(text);
    let Some(board) = boards
        .iter()
        .find(|board| board.owner == query.owner && board.number == query.number)
    else {
        return (
            json!({"pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": []}),
            0,
        );
    };
    let offset = offset_of(after);
    let first = usize::try_from(first).unwrap_or(0);
    let (nodes, end, more): (Vec<Value>, usize, bool) = match &board.recorded {
        Some(recorded) => {
            let (positions, beyond) = recorded
                .searches
                .get(text)
                .map_or((&[][..], false), |(positions, beyond)| {
                    (&positions[..], *beyond)
                });
            let end = (offset + first).min(positions.len());
            let nodes = positions[offset.min(end)..end]
                .iter()
                .flatten()
                .map(|id| recorded.hits[id].clone())
                .collect();
            (nodes, end, end < positions.len() || beyond)
        }
        None => {
            let found = search(boards, text);
            let end = (offset + first).min(found.len());
            let nodes = found[offset.min(end)..end]
                .iter()
                .map(|at| row(board, &board.issues[*at]))
                .collect();
            (nodes, end, end < found.len())
        }
    };
    let rows = nodes.len() as u64;
    (
        json!({
            "pageInfo": {"hasNextPage": more, "endCursor": if end > offset { json!(cursor(end)) } else { Value::Null }},
            "nodes": nodes,
        }),
        rows,
    )
}

/// What the loopback answered one GraphQL request with.
pub struct Answered {
    pub answer: Value,
    pub document: String,
    pub rows: u64,
    /// The text of each search it asked.
    pub searches: Vec<String>,
}

/// The name onetaskgraph gives `document` — as it was written, without the `actualCost` every
/// read it sends selects — or a prefix of it. A document of an earlier release is named by
/// what it selects, as the recording forwarder names one.
pub fn name_of(document: &str) -> String {
    let written = documents::without_reported_cost(document);
    let document = written.as_deref().unwrap_or(document);
    for (name, known) in [
        ("SEARCH_BATCH", documents::SEARCH_BATCH),
        ("SEARCH_ISSUES", documents::SEARCH_ISSUES),
        ("ISSUE_DETAILS", documents::ISSUE_DETAILS),
        ("ISSUE_DETAIL", documents::ISSUE_DETAIL),
        ("ISSUE_RELATIONS", documents::ISSUE_RELATIONS),
        ("ISSUE", documents::ISSUE),
    ] {
        if document == known {
            return name.to_owned();
        }
    }
    let named = if document.contains("s0:search(") {
        Some("SEARCH_BATCH")
    } else if document.starts_with("query($search:String!") {
        Some("SEARCH_ISSUES")
    } else if document.contains("$id23:ID!") {
        Some(if document.contains("subIssues") {
            "ISSUE_RELATIONS"
        } else {
            "ISSUE_DETAILS"
        })
    } else if document.contains("node(id:$id)") && document.contains("blockedBy(") {
        Some("ISSUE")
    } else if document.contains("node(id:$id)") && document.contains("comments(first:") {
        Some("ISSUE_DETAIL")
    } else {
        None
    };
    named.map_or_else(
        || format!("other:{}", document.chars().take(120).collect::<String>()),
        str::to_owned,
    )
}

/// Answer one GraphQL request from `boards`.
pub fn answer(boards: &[Board], received: &Received) -> Answered {
    let variables = &received.variables;
    let document = name_of(&received.document);
    let mut rows = 0;
    let mut searches = Vec::new();
    let data = match document.as_str() {
        "SEARCH_BATCH" => {
            let mut result = serde_json::Map::new();
            for slot in 0..onetaskgraph_github_projects::SEARCH_BATCH {
                if variables[format!("ask{slot}")] != true {
                    continue;
                }
                let text = variables[format!("search{slot}")]
                    .as_str()
                    .unwrap_or_default();
                searches.push(text.to_owned());
                // 0.17.3 reads which issues match, each row its scalar fields; an earlier
                // release read each row whole.
                let whole = received
                    .document
                    .contains("nodes{__typename ...BoardIssue}}");
                let (connection, held) = page(
                    boards,
                    text,
                    variables[format!("first{slot}")].as_u64().unwrap_or(0),
                    &variables[format!("after{slot}")],
                    if whole { node } else { |_, issue| hit(issue) },
                );
                rows += held;
                result.insert(format!("s{slot}"), connection);
            }
            for slot in 0..onetaskgraph_github_projects::SEARCH_BATCH_ORIGINS {
                assert_ne!(
                    variables[format!("origin{slot}")],
                    true,
                    "the loopback boards answer no origin lookup"
                );
            }
            Value::Object(result)
        }
        "SEARCH_ISSUES" => {
            searches.push(variables["search"].as_str().unwrap_or_default().to_owned());
            let (connection, held) = page(
                boards,
                variables["search"].as_str().unwrap_or_default(),
                variables["first"].as_u64().unwrap_or(0),
                &variables["after"],
                node,
            );
            rows = held;
            json!({"search": connection})
        }
        "ISSUE_DETAILS" | "ISSUE_RELATIONS" => {
            let relations = document == "ISSUE_RELATIONS";
            let comments = variables["comments"] != false;
            let mut result = serde_json::Map::new();
            let mut distinct = BTreeSet::new();
            for slot in 0..onetaskgraph_github_projects::DETAIL_BATCH {
                let id = variables[format!("id{slot}")].as_str().unwrap_or_default();
                let node = if relations {
                    childless(boards, id)
                } else {
                    commented(boards, id, comments)
                };
                if node.is_some() && distinct.insert(id.to_owned()) {
                    rows += 1;
                }
                result.insert(format!("i{slot}"), node.unwrap_or(Value::Null));
            }
            Value::Object(result)
        }
        "ISSUE_DETAIL" => {
            let id = variables["id"].as_str().unwrap_or_default();
            let node = commented(boards, id, true);
            rows = u64::from(node.is_some());
            json!({"node": node})
        }
        "ISSUE" => {
            let id = variables["id"].as_str().unwrap_or_default();
            let node = single(boards, id);
            rows = u64::from(node.is_some());
            json!({"node": node})
        }
        other => panic!(
            "the loopback boards answer no {other} (variables {variables}); grow loopback_boards.rs"
        ),
    };
    Answered {
        answer: json!({"data": data}),
        document,
        rows,
        searches,
    }
}
