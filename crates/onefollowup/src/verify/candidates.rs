//! The duplicate candidates `prepare` finds for each input, and `check` holds the answers to.
//!
//! A duplicate worded differently shares no sentence with the ticket it repeats, so a phrase
//! search never finds it; it shares a few distinctive words and the failing file. So `prepare`
//! asks the destination, inside its one batched `task find`, one short query per distinctive
//! term of each input and one per file name it names, beside the input's title. Terms are the
//! words of its title, then of its body's opening paragraph, less the shipped stoplist
//! (`defaults/duplicate-stoplist.txt`): onefollowup holds no corpus to rank words by rarity,
//! so a fixed list of common English and board-frequent words stands in, and a query that still
//! answers more than [`HIT_CAP`] items keeps only [`RETAINED`] of them and ranks what it found
//! last. Every follow-up ticket another
//! batch filed that the queries found, open or closed, is a candidate, ranked by how many of the
//! input's queries found it, then by how rare those queries' answers were; the first [`BOUND`]
//! are rendered into the task, and `check` refuses
//! an answer that files a ticket while leaving one of them neither related nor dismissed.
//!
//! Everything here is a pure function of the batch state's inputs and reads — each read keeps
//! the keys of the searches that found it — so `check` recomputes exactly what `prepare`
//! rendered without either writing it down.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::formats::batch::{Input, Read};
use crate::store::{ItemStatus, Search, SearchIn};
use crate::verify::context::Batch;
use crate::verify::decide;

/// The distinctive terms searched per input.
pub const TERMS: usize = 5;
/// The file names searched per input.
pub const FILES: usize = 2;
/// The hits above which a query is capped: its candidates rank after every candidate an
/// uncapped query found.
pub const HIT_CAP: usize = 20;
/// The hits a duplicate query keeps: the cap and one more, so a query the store answered past
/// the cap is kept capped — and is seen to be, by `check` too, from the batch state's reads —
/// while what it adds to the reads, the comments read and the candidates stays bounded.
pub const RETAINED: usize = HIT_CAP + 1;
/// The candidates rendered per input. A constant rather than a setting: nothing a host
/// decides depends on it, and a setting would be one more contract to keep.
pub const BOUND: usize = 8;
/// The shortest word searched as a term.
const SHORTEST: usize = 4;
/// The longest word searched as a term: anything longer is a hash or a generated name.
const LONGEST: usize = 40;
/// The body text terms are taken from: its opening paragraph, at most this many characters.
const OPENING: usize = 1000;

static STOPLIST: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    include_str!("../../defaults/duplicate-stoplist.txt")
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .flat_map(str::split_whitespace)
        .collect()
});

/// A file name: a word, a dot and a source extension.
static FILE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b[a-z0-9_][a-z0-9_.-]*\.(?:rs|py|pyi|ts|tsx|js|jsx|mjs|go|java|kt|swift|rb|c|h|cc|cpp|hpp|cs|sh|bash|md|yaml|yml|toml|json|j2|sql|html|css|txt)\b",
    )
    .expect("valid")
});

/// A path, or anything else holding a slash: searched as its file name, never as words.
static PATH_LIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\S*[/\\]\S*").expect("valid"));

/// Where a duplicate query's text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QueryKind {
    /// The input's whole title.
    Title,
    /// One distinctive term.
    Term,
    /// One file name.
    File,
}

impl QueryKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Title => "input",
            Self::Term => "term",
            Self::File => "file",
        }
    }
}

/// One query `prepare` asks for an input's duplicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    /// Its search key, naming the input it is for.
    pub key: String,
    /// What it asks.
    pub text: String,
    /// Where the text came from.
    pub kind: QueryKind,
}

/// What `prepare` asks for one input, and the words the stoplist kept out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The queries, the title's first.
    pub queries: Vec<Query>,
    /// Each word of the input the stoplist removed, once, in the order met.
    pub stoplisted: Vec<String>,
}

fn key(kind: QueryKind, input: &str, at: usize) -> String {
    match kind {
        QueryKind::Title => format!("{}:{input}", kind.prefix()),
        QueryKind::Term | QueryKind::File => format!("{}:{input}:{at}", kind.prefix()),
    }
}

/// Whether `key` is one of `input`'s duplicate queries.
fn is_query_of(key: &str, input: &str) -> bool {
    key == format!("input:{input}")
        || [QueryKind::Term, QueryKind::File].iter().any(|kind| {
            key.strip_prefix(&format!("{}:{input}:", kind.prefix()))
                .is_some_and(|at| at.parse::<usize>().is_ok())
        })
}

/// The words of `text`, lower-cased: runs of letters, digits, `-` and `_`, trimmed of `-`
/// and `_`.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .map(|word| word.trim_matches(|c| c == '-' || c == '_').to_lowercase())
        .filter(|word| !word.is_empty())
}

/// A plural's singular, so the term also matches the singular and every longer form a
/// substring search finds.
fn singular(word: &str) -> String {
    if word.chars().count() > SHORTEST && word.ends_with('s') && !word.ends_with("ss") {
        word[..word.len() - 1].to_owned()
    } else {
        word.to_owned()
    }
}

fn stoplisted(word: &str) -> bool {
    STOPLIST.contains(&word) || STOPLIST.contains(&singular(word).as_str())
}

/// The body's opening paragraph: its first block of text that is not a heading.
fn opening(body: &str) -> String {
    let mut paragraph = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !paragraph.is_empty() {
                break;
            }
            continue;
        }
        if line.starts_with('#') && paragraph.is_empty() {
            continue;
        }
        paragraph.push_str(line);
        paragraph.push(' ');
        if paragraph.len() >= OPENING {
            break;
        }
    }
    paragraph.chars().take(OPENING).collect()
}

/// The queries `prepare` asks for `input`'s duplicates.
#[must_use]
pub fn plan(input: &Input) -> Plan {
    let mut queries = Vec::new();
    let title = input.title.trim();
    if !title.is_empty() {
        queries.push(Query {
            key: key(QueryKind::Title, &input.id, 0),
            text: title.to_owned(),
            kind: QueryKind::Title,
        });
    }
    // The repository's own name is on every ticket about it.
    let repository = input
        .repository
        .as_deref()
        .and_then(|origin| origin.trim_end_matches('/').rsplit('/').next())
        .map(str::to_lowercase);
    let body = opening(&input.body);
    let mut terms: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for part in [title, body.as_str()] {
        let unpathed = PATH_LIKE.replace_all(part, " ");
        let prose = FILE_NAME.replace_all(&unpathed, " ");
        let mut kept: Vec<String> = Vec::new();
        for word in words(&prose) {
            let length = word.chars().count();
            if !(SHORTEST..=LONGEST).contains(&length) || word.chars().any(|c| c.is_ascii_digit()) {
                continue;
            }
            if stoplisted(&word) || repository.as_deref() == Some(word.as_str()) {
                if !removed.contains(&word) {
                    removed.push(word);
                }
                continue;
            }
            let term = singular(&word);
            if !terms.contains(&term) && !kept.contains(&term) {
                kept.push(term);
            }
        }
        // The longer a word, the rarer it tends to be: the nearest a fixed rule comes to the
        // rarity a corpus would give. A stable sort keeps the first-met of equal lengths first.
        kept.sort_by_key(|term| std::cmp::Reverse(term.chars().count()));
        terms.extend(kept.into_iter().take(TERMS - terms.len()));
        if terms.len() == TERMS {
            break;
        }
    }
    for (at, term) in terms.into_iter().enumerate() {
        queries.push(Query {
            key: key(QueryKind::Term, &input.id, at),
            text: term,
            kind: QueryKind::Term,
        });
    }
    let mut files: Vec<String> = Vec::new();
    let named = input
        .paths
        .iter()
        .filter_map(|path| path.trim_end_matches('/').rsplit('/').next())
        .map(str::to_owned)
        .chain([title, input.body.as_str()].into_iter().flat_map(|text| {
            FILE_NAME
                .find_iter(text)
                .map(|found| found.as_str().to_owned())
        }));
    for name in named {
        let name = name.trim().to_lowercase();
        if !name.is_empty() && !files.contains(&name) && files.len() < FILES {
            files.push(name);
        }
    }
    for (at, file) in files.into_iter().enumerate() {
        queries.push(Query {
            key: key(QueryKind::File, &input.id, at),
            text: file,
            kind: QueryKind::File,
        });
    }
    Plan {
        queries,
        stoplisted: removed,
    }
}

/// `plan`'s queries as the store's searches, each in titles and contents, at every status,
/// each keeping at most [`RETAINED`] hits.
#[must_use]
pub fn searches(plan: &Plan) -> Vec<Search> {
    plan.queries
        .iter()
        .map(|query| Search {
            key: query.key.clone(),
            text: Some(query.text.clone()),
            within: SearchIn::Both,
            limit: Some(RETAINED),
            ..Search::default()
        })
        .collect()
}

/// One candidate duplicate, as the task and `prepare`'s result show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// The item's qualified id.
    pub item: String,
    /// Its title.
    pub title: String,
    /// Its status, open or closed.
    pub status: ItemStatus,
    /// Its URL, when it has one.
    pub url: Option<String>,
    /// The root cause its record names.
    pub root_cause: String,
    /// The uncapped queries that found it, by text.
    pub shared: Vec<String>,
    /// The capped queries that found it, by text.
    pub capped: Vec<String>,
}

/// An input's ranked candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    /// The first [`BOUND`] of them, best first.
    pub rendered: Vec<Candidate>,
    /// How many there were before the bound.
    pub matching: usize,
}

/// How many items each search key found, over every read.
#[must_use]
pub fn hits(reads: &[Read]) -> BTreeMap<&str, usize> {
    let mut hits: BTreeMap<&str, usize> = BTreeMap::new();
    for read in reads {
        for key in &read.found_by {
            *hits.entry(key.as_str()).or_default() += 1;
        }
    }
    hits
}

/// `input`'s candidates among `reads`: every follow-up ticket another batch filed that one of
/// its queries found, ranked by the uncapped queries that found it, then the capped ones, then
/// by how few items those uncapped queries answered, then by id. `plan` names each query's text; a key it does not hold is shown as the key.
#[must_use]
pub fn ranked(batch: &Batch<'_>, input: &Input, plan: &Plan, reads: &[Read]) -> Ranked {
    let hits = hits(reads);
    let text_of = |key: &str| {
        plan.queries
            .iter()
            .find(|query| query.key == key)
            .map_or_else(|| key.to_owned(), |query| query.text.clone())
    };
    let mut found: Vec<Candidate> = reads
        .iter()
        .filter_map(|read| {
            let keys: Vec<&String> = read
                .found_by
                .iter()
                .filter(|key| is_query_of(key, &input.id))
                .collect();
            if keys.is_empty() {
                return None;
            }
            let record = decide::item_record(batch, read)?;
            if record.created_by_batch == batch.batch {
                return None;
            }
            let (capped, shared): (Vec<&String>, Vec<&String>) = keys
                .into_iter()
                .partition(|key| hits.get(key.as_str()).copied().unwrap_or_default() > HIT_CAP);
            Some(Candidate {
                item: read.item.id.clone(),
                title: read.item.title.clone(),
                status: read.item.status.clone(),
                url: read.item.url.clone(),
                root_cause: record.root_cause,
                shared: shared.into_iter().map(|key| text_of(key)).collect(),
                capped: capped.into_iter().map(|key| text_of(key)).collect(),
            })
        })
        .collect();
    // Among candidates sharing as many queries, the one found by the rarer ones first: a query
    // answering 2 items says more than one answering 19.
    let rarity = |candidate: &Candidate| -> usize {
        candidate
            .shared
            .iter()
            .filter_map(|text| plan.queries.iter().find(|query| query.text == *text))
            .map(|query| {
                HIT_CAP + 1
                    - hits
                        .get(query.key.as_str())
                        .copied()
                        .unwrap_or_default()
                        .min(HIT_CAP)
            })
            .sum()
    };
    found.sort_by(|a, b| {
        (b.shared.len(), b.capped.len(), rarity(b))
            .cmp(&(a.shared.len(), a.capped.len(), rarity(a)))
            .then_with(|| a.item.cmp(&b.item))
    });
    let matching = found.len();
    found.truncate(BOUND);
    Ranked {
        rendered: found,
        matching,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(title: &str, body: &str, paths: &[&str]) -> Input {
        Input {
            id: "drafts:b/drafts/one".to_owned(),
            source: "draft".to_owned(),
            title: title.to_owned(),
            repository: Some("github.com/example/onejudge".to_owned()),
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
            body: body.to_owned(),
            author: None,
            transcript: None,
        }
    }

    fn texts(plan: &Plan, kind: QueryKind) -> Vec<&str> {
        plan.queries
            .iter()
            .filter(|query| query.kind == kind)
            .map(|query| query.text.as_str())
            .collect()
    }

    #[test]
    fn terms_are_the_longest_unlisted_words_of_the_title_then_the_body() {
        let plan = plan(&input(
            "onejudge panics with min greater than max mid",
            "## Seen\n\nRecording successive interruptions mixes cumulative event positions with a shrinking transcript.\n\nLater paragraphs are not read.",
            &["crates/onejudge/src/engine.rs"],
        ));
        assert_eq!(
            texts(&plan, QueryKind::Term),
            [
                "greater",
                "panic",
                "interruption",
                "successive",
                "cumulative"
            ]
        );
        assert_eq!(texts(&plan, QueryKind::File), ["engine.rs"]);
        assert_eq!(
            texts(&plan, QueryKind::Title),
            ["onejudge panics with min greater than max mid"]
        );
        assert!(plan.stoplisted.contains(&"with".to_owned()));
        assert!(plan.stoplisted.contains(&"onejudge".to_owned()));
        assert!(
            plan.queries
                .iter()
                .all(|query| is_query_of(&query.key, "drafts:b/drafts/one"))
        );
    }

    /// README.md states these constants to a host, so it is held to them.
    #[test]
    fn the_readme_states_the_constants_as_they_are() {
        let readme = include_str!("../../../../README.md")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        for stated in [
            format!("up to {TERMS} words of its title"),
            format!("one per file name it names, up to {FILES}"),
            format!(
                "a query answering more than {HIT_CAP} items is capped: it keeps only {RETAINED} of them"
            ),
            format!("The first {BOUND} are rendered into the task"),
        ] {
            assert!(readme.contains(&stated), "README.md states {stated:?}");
        }
    }

    #[test]
    fn digits_hashes_and_short_words_are_never_terms_and_files_come_from_the_text_too() {
        let plan = plan(&input(
            "a b c",
            "commit 0123456789abcdef broke `src/verify/prepare.rs:313` and docs/contract.md",
            &[],
        ));
        assert_eq!(texts(&plan, QueryKind::Term), ["commit", "broke"]);
        assert_eq!(texts(&plan, QueryKind::File), ["prepare.rs", "contract.md"]);
        assert!(!is_query_of(
            "term:drafts:b/drafts/one:x",
            "drafts:b/drafts/one"
        ));
        assert!(!is_query_of(
            "term:drafts:b/drafts/other:0",
            "drafts:b/drafts/one"
        ));
    }
}
