//! Generated GitHub Projects boards the loopback answers from: board-scoped issue searches,
//! one at a time (`SEARCH_ISSUES`) or batched (`SEARCH_BATCH`), issue reads by node id
//! (`ISSUE_DETAILS`, `ISSUE_DETAIL`) and their relations (`ISSUE_RELATIONS`, every issue
//! without sub-issues), each answer shaped as onetaskgraph 0.17.2's documents select it, and
//! each request priced by [`crate::graphql_cost`].
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
        let body_words = normalized(&body);
        Self {
            id,
            number,
            title,
            body,
            repository,
            open,
            state_reason: state_reason.map(str::to_owned),
            status: status.to_owned(),
            title_words,
            body_words,
        }
    }
}

/// One board: a project of its owner, and its issues, newest first.
#[derive(Debug, Clone)]
pub struct Board {
    pub owner: String,
    pub number: u64,
    pub issues: Vec<BoardIssue>,
}

impl Board {
    pub fn project_id(&self) -> String {
        format!("PVT_{}_{}", self.owner, self.number)
    }
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

/// How many issues of `boards` the board-scoped `search` matches, capped as GitHub caps a
/// search: what a walk of every page of it would read.
pub fn matching(boards: &[Board], text: &str) -> usize {
    search(boards, text).len()
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
        "labels": {"nodes": [], "pageInfo": {"hasNextPage": false}},
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

/// The node `id` names, with an empty first page of comments.
fn commented(boards: &[Board], id: &str) -> Option<Value> {
    boards.iter().find_map(|board| {
        board
            .issues
            .iter()
            .find(|issue| issue.id == id)
            .map(|issue| {
                let mut node = node(board, issue);
                node["comments"] =
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

/// One page of `search`, after the cursor `after`, of `first` rows: its connection, and the
/// rows it holds.
fn page(boards: &[Board], text: &str, first: u64, after: &Value) -> (Value, u64) {
    let found = search(boards, text);
    let query = parse_search(text);
    let board = boards
        .iter()
        .find(|board| board.owner == query.owner && board.number == query.number);
    let offset = after
        .as_str()
        .and_then(|cursor| cursor.strip_prefix("cursor:"))
        .and_then(|at| at.parse::<usize>().ok())
        .unwrap_or(0);
    let end = (offset + usize::try_from(first).unwrap_or(0)).min(found.len());
    let nodes: Vec<Value> = match board {
        Some(board) => found[offset.min(end)..end]
            .iter()
            .map(|at| node(board, &board.issues[*at]))
            .collect(),
        None => Vec::new(),
    };
    let rows = nodes.len() as u64;
    let more = end < found.len();
    (
        json!({
            "pageInfo": {"hasNextPage": more, "endCursor": if more { json!(format!("cursor:{end}")) } else { Value::Null }},
            "nodes": nodes,
        }),
        rows,
    )
}

/// What the loopback answered one GraphQL request with, and its price.
pub struct Answered {
    pub answer: Value,
    pub document: String,
    pub points: u64,
    pub rows: u64,
    /// The text of each search it asked.
    pub searches: Vec<String>,
}

/// The name onetaskgraph gives `document`, or a prefix of it.
fn name_of(document: &str) -> String {
    for (name, known) in [
        ("SEARCH_BATCH", documents::SEARCH_BATCH),
        ("SEARCH_ISSUES", documents::SEARCH_ISSUES),
        ("ISSUE_DETAILS", documents::ISSUE_DETAILS),
        ("ISSUE_DETAIL", documents::ISSUE_DETAIL),
        ("ISSUE_RELATIONS", documents::ISSUE_RELATIONS),
    ] {
        if document == known {
            return name.to_owned();
        }
    }
    format!("other:{}", document.chars().take(120).collect::<String>())
}

/// Answer one GraphQL request from `boards`.
pub fn answer(boards: &[Board], received: &Received) -> Answered {
    let variables = &received.variables;
    let document = name_of(&received.document);
    let points = crate::graphql_cost::points(&received.document, variables);
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
                let (connection, held) = page(
                    boards,
                    text,
                    variables[format!("first{slot}")].as_u64().unwrap_or(0),
                    &variables[format!("after{slot}")],
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
            );
            rows = held;
            json!({"search": connection})
        }
        "ISSUE_DETAILS" | "ISSUE_RELATIONS" => {
            let relations = document == "ISSUE_RELATIONS";
            let mut result = serde_json::Map::new();
            let mut distinct = BTreeSet::new();
            for slot in 0..onetaskgraph_github_projects::DETAIL_BATCH {
                let id = variables[format!("id{slot}")].as_str().unwrap_or_default();
                let node = if relations {
                    childless(boards, id)
                } else {
                    commented(boards, id)
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
            let node = commented(boards, id);
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
        points,
        rows,
        searches,
    }
}
