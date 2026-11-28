//! The loopback GitHub held to recorded real requests: each request `prepare` sent the public
//! `followups` board, as `fixtures/real-board/requests.json` holds it (`recorded`), sent the
//! loopback again — document and variables as recorded — from a board seeded with that
//! recording's sanitized issues, is answered what GitHub answered and charged what GitHub
//! charged and the seconds GitHub took; and the recording holds nothing the board's privacy
//! keeps out of a public tree. Nothing here reaches GitHub.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::Path;

use regex::Regex;
use serde_json::{Value, json};

use crate::loopback_boards::{Board, name_of, offset_of};
use crate::loopback_github::{LoopbackGitHub, Pricing, Quota};
use crate::recorded::{self, Recording};

/// The board the recording was taken from, as its searches name it.
const OWNER: &str = "nickderobertis";
const NUMBER: u64 = 3;

/// `document` and `variables` sent the loopback's GraphQL endpoint, as onetaskgraph sends them:
/// the answer's body.
fn post(github: &LoopbackGitHub, document: &str, variables: &Value) -> Value {
    let body = json!({"query": document, "variables": variables}).to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", github.port)).expect("the loopback");
    write!(
        stream,
        "POST /graphql HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .expect("sent");
    let mut reader = BufReader::new(stream);
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).expect("a header");
        if header == "\r\n" {
            break;
        }
        if let Some(value) = header.to_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().expect("a length");
        }
    }
    let mut answer = vec![0; length];
    reader.read_exact(&mut answer).expect("the answer");
    serde_json::from_slice(&answer).expect("JSON")
}

/// A loopback answering from `board` alone, each request priced as the recording says.
fn loopback(board: Board, pricing: Pricing) -> LoopbackGitHub {
    LoopbackGitHub::serving(
        Quota {
            remaining: 5000,
            reset: chrono::Utc::now().timestamp() + 3600,
            charge: 0,
        },
        vec![board],
        pricing,
    )
}

fn seeded(recording: &Recording<'_>) -> Board {
    Board::recorded(
        OWNER,
        NUMBER,
        recording.document,
        recording.variables,
        &recording.answer["data"],
    )
}

/// The documents onetaskgraph 0.17.3 sends, by name, each selecting its own cost.
fn sent_by_the_release(name: &str) -> String {
    use onetaskgraph_github_projects::graphql;
    let written = match name {
        "SEARCH_BATCH" => graphql::SEARCH_BATCH,
        "ISSUE_DETAILS" => graphql::ISSUE_DETAILS,
        "ISSUE" => graphql::ISSUE,
        other => panic!("no recorded {other}"),
    };
    graphql::with_reported_cost(written).expect("a read selects its cost")
}

#[test]
fn the_loopback_answers_and_charges_every_recorded_real_request_as_github_did() {
    let recordings = recorded::recordings();
    let mut documents = std::collections::BTreeSet::new();
    for recording in &recordings {
        documents.insert(recording.document);
        let seq = recording.seq;
        // Recorded under the adopted release's documents, whole, with what GitHub reported.
        assert_eq!(
            recording.sent,
            sent_by_the_release(recording.document),
            "{seq}: sent as onetaskgraph 0.17.3 sends {}",
            recording.document
        );
        assert_eq!(name_of(recording.sent), recording.document, "{seq}");
        assert!(
            recording.cost > 0
                && recording.rows > 0
                && recording.response_bytes > 0
                && recording.seconds > 0.0,
            "{seq}: GitHub's cost, the rows, the bytes and the seconds it took"
        );
        assert_eq!(
            recording.answer["data"]["actualCost"]["cost"].as_u64(),
            Some(recording.cost),
            "{seq}: the cost the answer reported is the one recorded"
        );

        let github = loopback(seeded(recording), Pricing::Recorded);
        let started = std::time::Instant::now();
        let answered = post(&github, recording.sent, recording.variables);
        let took = started.elapsed().as_secs_f64();
        assert_eq!(
            answered["data"], recording.answer["data"],
            "{seq}: the loopback answers {} as GitHub did",
            recording.document
        );
        let charged = github.charged();
        assert_eq!(charged.len(), 1, "{seq}");
        let binding = recorded::binding(recording.document, recording.variables);
        let price = recorded::price_of(&binding).expect("every binding is priced");
        assert_eq!(
            charged[0].points, recording.cost,
            "{seq}: charged what GitHub charged"
        );
        assert_eq!(
            charged[0].seconds, price.seconds,
            "{seq}: charged the seconds GitHub took for `{binding}`"
        );
        assert!(
            charged[0].seconds > 0.0 && charged[0].seconds != took,
            "{seq}: GitHub's seconds, neither none nor the loopback's own {took} s"
        );
        // GitHub's published calculation, the loopback's own second reading, agrees.
        assert_eq!(
            crate::graphql_cost::points(recording.sent, recording.variables),
            recording.cost,
            "{seq}: GitHub's published calculation prices it as GitHub charged it"
        );
    }
    // Each binding's price is a recorded request of it.
    let prices = recorded::FIXTURE["prices"].as_object().expect("the prices");
    for (binding, price) in prices {
        let recording = recordings
            .iter()
            .find(|one| Some(one.seq) == price["seq"].as_u64())
            .unwrap_or_else(|| panic!("`{binding}` is priced by a request the fixture holds"));
        assert_eq!(
            recorded::binding(recording.document, recording.variables),
            *binding
        );
        assert_eq!(price["seconds"].as_f64(), Some(recording.seconds));
        assert_eq!(price["cost"].as_u64(), Some(recording.cost));
    }
    assert_eq!(
        documents.into_iter().collect::<Vec<_>>(),
        ["ISSUE", "ISSUE_DETAILS", "SEARCH_BATCH"]
    );
}

#[test]
fn following_each_recorded_page_s_cursor_reaches_the_recorded_next_page_and_ends_there() {
    // Every page of a search the recording holds, by its text: where it starts, what it asked,
    // and what GitHub answered.
    let recordings = recorded::recordings();
    let mut pages: std::collections::BTreeMap<String, Vec<(usize, u64, Value)>> =
        std::collections::BTreeMap::new();
    let searches: Vec<&Recording<'_>> = recordings
        .iter()
        .filter(|one| one.document == "SEARCH_BATCH")
        .collect();
    for recording in &searches {
        for slot in 0..onetaskgraph_github_projects::SEARCH_BATCH {
            if recording.variables[format!("ask{slot}")] != true {
                continue;
            }
            let text = recording.variables[format!("search{slot}")]
                .as_str()
                .expect("a search")
                .to_owned();
            pages.entry(text).or_default().push((
                offset_of(&recording.variables[format!("after{slot}")]),
                recording.variables[format!("first{slot}")]
                    .as_u64()
                    .expect("a page size"),
                recording.answer["data"][format!("s{slot}")].clone(),
            ));
        }
    }
    // One board holding every recorded search, so a walk crosses rounds as prepare's did.
    let mut board = seeded(searches[0]);
    for recording in &searches[1..] {
        board.merge(&seeded(recording));
    }
    let github = loopback(board, Pricing::Published);
    let document = sent_by_the_release("SEARCH_BATCH");
    let ask = |text: &str, after: &Value, first: u64| -> Value {
        let mut variables = searches[0].variables.clone();
        for slot in 0..onetaskgraph_github_projects::SEARCH_BATCH {
            variables[format!("ask{slot}")] = json!(false);
            variables[format!("first{slot}")] = json!(1);
            variables[format!("after{slot}")] = Value::Null;
        }
        variables["ask0"] = json!(true);
        variables["search0"] = json!(text);
        variables["first0"] = json!(first);
        variables["after0"] = after.clone();
        post(&github, &document, &variables)["data"]["s0"].clone()
    };
    let mut walked = 0;
    for (text, mut held) in pages {
        held.sort_by_key(|(from, _, _)| *from);
        // A walk the fixture holds whole: each page starting where the one before it ended.
        let whole = held
            .windows(2)
            .all(|pair| offset_of(&pair[0].2["pageInfo"]["endCursor"]) == pair[1].0);
        if held.len() < 2 || !whole || held[0].0 != 0 {
            continue;
        }
        // From the first page, each next page asked at its recorded cursor and size.
        let (_, first, page) = &held[0];
        let mut answered = ask(&text, &Value::Null, *first);
        assert_eq!(answered, *page, "{text}: the first page as recorded");
        for (from, first, page) in &held[1..] {
            let cursor = answered["pageInfo"]["endCursor"].clone();
            assert_eq!(
                offset_of(&cursor),
                *from,
                "{text}: the cursor followed is the one the next page was asked at"
            );
            assert_eq!(answered["pageInfo"]["hasNextPage"], true, "{text}");
            answered = ask(&text, &cursor, *first);
            assert_eq!(answered, *page, "{text}: the page at {from} as recorded");
        }
        assert_eq!(
            answered["pageInfo"],
            held.last().expect("pages").2["pageInfo"],
            "{text}: the walk ends where the recorded one did"
        );
        walked += 1;
    }
    assert!(walked > 0, "a recorded walk crosses two rounds");
}

/// Every string in `value`, with the key it is held under.
fn strings<'a>(value: &'a Value, key: &'a str, out: &mut Vec<(&'a str, &'a str)>) {
    match value {
        Value::String(text) => out.push((key, text)),
        Value::Array(items) => items.iter().for_each(|item| strings(item, key, out)),
        Value::Object(fields) => fields
            .iter()
            .for_each(|(name, item)| strings(item, name, out)),
        _ => {}
    }
}

/// The repositories GitHub reports PUBLIC, as the fixture records its answer.
fn public_repositories() -> std::collections::BTreeSet<String> {
    recorded::repositories()
        .into_iter()
        .filter(|(_, visibility)| visibility == "PUBLIC")
        .map(|(name, _)| name)
        .collect()
}

/// The first privacy violation `bytes`, the recording `label`, holds — a real board or item
/// id, or an issue or a name of a repository not in `public` — or why it cannot be read whole:
/// `None` when it holds none. A gzip-compressed recording is decompressed first, layer by
/// layer, so its payload is what is scanned.
fn privacy_problem(
    label: &str,
    bytes: &[u8],
    public: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let mut bytes = bytes.to_vec();
    while bytes.starts_with(&[0x1f, 0x8b]) {
        let mut inflated = Vec::new();
        if let Err(error) = std::io::Read::read_to_end(
            &mut flate2::read::MultiGzDecoder::new(&bytes[..]),
            &mut inflated,
        ) {
            return Some(format!(
                "{label}: a compressed recording that does not decompress: {error}"
            ));
        }
        bytes = inflated;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Some(format!("{label}: a recording is UTF-8 JSON"));
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return Some(format!("{label}: a recording is JSON: {error}")),
    };
    let board_id = Regex::new(r"\bPVT[A-Z]*_[A-Za-z0-9_-]+").expect("valid");
    let placeholder = Regex::new(r"^PVT[A-Z]*_placeholder_(board|[0-9]{4})$").expect("valid");
    let named = [
        Regex::new(r"github\.com/([A-Za-z0-9-]+/[A-Za-z0-9._-]*[A-Za-z0-9_-])").expect("valid"),
        Regex::new(r"\b([A-Za-z0-9-]+/[A-Za-z0-9._-]*[A-Za-z0-9_-])#[0-9]+").expect("valid"),
    ];
    let mut held = Vec::new();
    strings(&value, "", &mut held);
    for (key, text) in held {
        for id in board_id.find_iter(text) {
            if !placeholder.is_match(id.as_str()) {
                return Some(format!(
                    "{label}: `{}` under `{key}` is a real board id, not a placeholder",
                    id.as_str()
                ));
            }
        }
        if key == "nameWithOwner" && !public.contains(text) {
            return Some(format!(
                "{label}: an issue of {text}, which GitHub does not report PUBLIC"
            ));
        }
        for pattern in &named {
            for name in pattern.captures_iter(text) {
                if !public.contains(&name[1]) {
                    return Some(format!(
                        "{label}: `{}` names {}, which GitHub does not report PUBLIC",
                        &name[0], &name[1]
                    ));
                }
            }
        }
    }
    None
}

#[test]
fn the_recorded_requests_hold_no_board_id_and_no_issue_of_a_repository_github_does_not_report_public()
 {
    let public = public_repositories();
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/e2e/fixtures/real-board");
    let mut scanned = 0;
    for entry in std::fs::read_dir(&directory).expect("the recordings") {
        let path = entry.expect("an entry").path();
        let bytes = std::fs::read(&path).expect("readable");
        if let Some(problem) = privacy_problem(&path.display().to_string(), &bytes, &public) {
            panic!("{problem}");
        }
        scanned += 1;
    }
    assert!(scanned > 0, "the recordings were scanned");
    // The board itself is not public, and its own id is a placeholder everywhere.
    assert!(
        !recorded::FIXTURE
            .to_string()
            .contains("\"(dropped)\",\"number\":3"),
        "the board's project id is a placeholder"
    );
}

/// `text` gzip-compressed, as a recording could be kept.
fn gzipped(text: &str) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, text.as_bytes()).expect("compressed");
    encoder.finish().expect("compressed")
}

#[test]
fn a_compressed_recording_is_scanned_for_what_it_holds_not_refused_for_being_compressed() {
    let public = public_repositories();
    let fixture = recorded::FIXTURE.to_string();
    // The committed, sanitized recording compressed — once, and twice over — holds nothing
    // the board's privacy keeps out of a public tree.
    assert_eq!(
        privacy_problem("sanitized.json.gz", &gzipped(&fixture), &public),
        None
    );
    assert_eq!(
        privacy_problem(
            "sanitized.json.gz.gz",
            &{
                let once = gzipped(&fixture);
                let mut twice =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                std::io::Write::write_all(&mut twice, &once).expect("compressed");
                twice.finish().expect("compressed")
            },
            &public
        ),
        None
    );
    // The same recording with one real board item id, or one issue of a repository GitHub
    // does not report PUBLIC, compressed: refused for what its payload holds.
    let real_item = fixture.replacen("PVTI_placeholder_0001", "PVTI_lADOASiiG84BjXXyzgAAAAA", 1);
    assert_ne!(real_item, fixture, "the fixture holds an item placeholder");
    let problem = privacy_problem("item.json.gz", &gzipped(&real_item), &public)
        .expect("a real board item id is refused");
    assert!(
        problem.contains("`PVTI_lADOASiiG84BjXXyzgAAAAA`") && problem.contains("real board id"),
        "{problem}"
    );
    let real_board = fixture.replacen("PVT_placeholder_board", "PVT_kwHOASiiG84BjXXy", 1);
    let problem = privacy_problem("board.json.gz", &gzipped(&real_board), &public)
        .expect("the board's real project id is refused");
    assert!(problem.contains("`PVT_kwHOASiiG84BjXXy`"), "{problem}");
    let repository = public.iter().next().expect("a public repository").clone();
    let private = fixture.replacen(
        &format!("\"nameWithOwner\":\"{repository}\""),
        "\"nameWithOwner\":\"nickderobertis/a-private-repository\"",
        1,
    );
    assert_ne!(private, fixture, "the fixture names {repository}");
    let problem = privacy_problem("private.json.gz", &gzipped(&private), &public)
        .expect("an issue of a repository not reported PUBLIC is refused");
    assert!(
        problem.contains("nickderobertis/a-private-repository")
            && problem.contains("does not report PUBLIC"),
        "{problem}"
    );
    // A file that claims to be compressed but is not is refused, never skipped.
    let problem = privacy_problem("broken.json.gz", &[0x1f, 0x8b, 0x08, 0x00], &public)
        .expect("an unreadable compressed recording is refused");
    assert!(problem.contains("does not decompress"), "{problem}");
}
