//! THROWAWAY spike harness (`spike-legacy-formats` of `followups-ticket-migration`): never
//! lands. It measures, over the 2026-11-18 read of the `followups` board, how far a
//! deterministic mapping takes each old ticket and evidence-comment format onto onefollowup
//! 0.8.0's current one, using this checkout's own library: the schema-14 record type, the
//! default `ticket-render.md.j2` and `comment-evidence.md.j2`, the default `root-cause` kind's
//! checks and the estimate rule. It also counts what close-stale at 14 days would close and
//! exempt, and the writes the closures and the migration would make.
//!
//! ```text
//! cargo run -q -p onefollowup --example spike_legacy_formats -- \
//!     <board dump dir> crates/onefollowup/examples/spike_legacy_formats/extras.json <out dir> \
//!     [--as-of <RFC 3339>]
//! ```
//!
//! `extras.json` is what `read_extras.py` read beside the dump (comment times, the long item's
//! comments, five edit histories, the plan store's `delivers`). It writes `report.md`,
//! `summary.json` and the before/after texts under `<out dir>`, and reads nothing from GitHub.
//!
//! No test drives it: it is a one-off measurement on a branch that never lands, so a committed
//! test would guard nothing afterwards; its run, quoted in the spike report, is its proof.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::{DateTime, Duration, Utc};
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Map, Value, json};

use onefollowup::config::{Config, Kind, defaults};
use onefollowup::formats::Versioned;
use onefollowup::formats::markers::{self, CommentMarker};
use onefollowup::formats::ticket::TicketRecord;
use onefollowup::verify::render::{self, Common};
use onefollowup::verify::template::Located;
use onefollowup::verify::ticket::{self, Context, EstimateLine};

const NS: &str = "orchestrator";
const STALE_DAYS: i64 = 14;
const WRITES_PER_HOUR: f64 = 250.0;
/// The sentence an older migration's renderer wrote ahead of a one-unit fix's locations.
const RENDERER_SENTENCES: &[&str] =
    &["The original suggested change is retained below; no additional remedy is inferred."];
/// The board's text fields the host's `metadata_fields` project, with the record key each reads.
const METADATA_FIELDS: &[(&str, &str)] = &[
    ("Host", "host"),
    ("Owning runs", "owning_runs"),
    ("Basis", "basis"),
    ("Drafts", "draft_sources"),
    ("Transcripts", "transcripts"),
    ("Witnesses", "witnesses"),
    ("Verified at", "verified_at"),
];

macro_rules! re {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).expect("valid"));
    };
}

re!(ANCHOR, r#"(?m)^[ \t]*<a id="[^"]*"></a>[ \t]*\n?"#);
re!(DETAILS, r"(?s)<details>\n<summary>(?P<summary>[^<]*)</summary>\n\n(?P<content>.*?)\n\n</details>");
re!(LABELLED, r"^[-*] (?P<label>[^:\n]+):(?P<value>[^\n]*)$");
re!(FENCE, r"^ {0,3}(`{3,}|~{3,})");
re!(SUBHEADING, r"^###(?:[ \t][^\n]*)?$");
re!(DRAFT_ID, r"drafts:[A-Za-z0-9._@+/-]+");
re!(COMMIT, r"\b[0-9a-f]{40}\b");
re!(ORIGIN, r"github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)*");
re!(HOST_PHRASE, r"(?i)\b(?:verification host|verified on\b(?: host)?|on host|host)\s*:?\s*`?(?P<host>[A-Za-z0-9][A-Za-z0-9.-]*[A-Za-z0-9])`?");
re!(TRANSCRIPT, r"(?i)\btranscript\s*:?\s*`(?P<command>[^`]+)`");
re!(AUTHOR_DICT, r#"(?i)\bauthor\s*:?\s*\{(?P<dict>[^}]*)\}"#);
re!(AUTHOR, r"(?i)\bauthor\s*:?\s*`?(?P<kind>[A-Za-z]+)`?");
re!(NODE, r"(?i)\bnode\s*:?\s*`?(?P<node>[A-Za-z0-9][A-Za-z0-9._-]*)`?");
re!(MEMBER, r"(?i)\bmember\s*:?\s*`?(?P<member>[A-Za-z0-9][A-Za-z0-9._-]*)`?");
re!(DICT_VALUE, r#"['"](?P<key>kind|node|member)['"]\s*:\s*['"]?(?P<value>[^'",}]+)['"]?"#);
re!(VERIFIED_AT, r"(?i)\bverified at\s+(?P<list>`?[\w./-]+\.\w+:\d+(?:[-,]\d+)*`?(?:\s*[;,]\s*`?[\w./-]+\.\w+:\d+(?:[-,]\d+)*`?)*)(?P<tail>\s*:\s*)?");
re!(PATH_LINE, r"(?P<path>[\w./-]+\.\w+):(?P<line>\d+)");
re!(RUN_CLAUSE, r"(?i)\b(?:from )?run\s+`?[\w.@+-]+`?");
re!(TICKED, r"`[^`]*`");
re!(RUN_START, r"(?i)^\W*(?:from )?run\b");
re!(DRAFT_LEAD, r"(?i)^(?:(?:occurrence from |from )?drafts?(?: evidence)?\s*:?\s*)?`?drafts:");
re!(METADATA_START, r"(?i)^\W*(verification host|verified on\b|host\b|basis\b)");
re!(HOST_START, r"(?i)^\W*(verification host|verified on\b|host\b|on host\b)");
re!(UNITS, r"\d+ units");
re!(NAMED_BASIS, r"(?i)\b(?P<name>[a-z][a-z0-9_.-]+)\s+basis\b\W{0,4}(?:commit\W{0,3})?`?(?P<commit>[0-9a-f]{40})");
re!(WORD, r"[A-Za-z][A-Za-z'/-]*");

/// Words a metadata sentence carries besides its values.
const METADATA_WORDS: &[&str] = &[
    "verification", "verified", "on", "host", "basis", "bases", "commit", "commits", "at", "of",
    "for", "and", "in", "the", "draft", "drafts", "run", "runs", "author", "node", "member",
    "transcript", "transcripts", "none", "manager", "kind", "null", "from", "supporting",
    "this", "occurrence", "evidence", "origin/main", "fetched", "repository", "repositories",
    "recorded", "record", "is", "with", "by", "its", "both", "each", "a", "an", "dispatch",
    "dispatched", "commit", "sha", "head", "main", "origin", "checkout", "local", "verifying",
    "was", "were", "this", "batch", "as",
];

#[derive(Debug, Clone)]
struct Comment {
    id: String,
    body: String,
    created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct Item {
    url: String,
    open: bool,
    title: String,
    status: String,
    fields: BTreeMap<String, String>,
    visible: String,
    record: Option<Value>,
    comments: Vec<Comment>,
}

fn short(url: &str) -> String {
    url.trim_start_matches("https://github.com/nickderobertis/")
        .replace("/issues/", "#")
}

fn load(dump: &Path, extras: &Value) -> Vec<Item> {
    let created: HashMap<String, DateTime<Utc>> = extras["comment_created_at"]
        .as_object()
        .expect("extras.json holds comment_created_at")
        .iter()
        .filter_map(|(id, at)| Some((id.clone(), at.as_str()?.parse().ok()?)))
        .collect();
    let long_url = extras["long_item"]["url"].as_str().unwrap_or_default();
    let mut items = Vec::new();
    let mut pages: Vec<PathBuf> = std::fs::read_dir(dump)
        .expect("the board dump directory reads")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    pages.sort_by_key(|path| {
        path.file_stem()
            .and_then(|stem| stem.to_str()?.strip_prefix("page-")?.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    for page in pages {
        let text = std::fs::read_to_string(&page).expect("a page reads");
        let value: Value = serde_json::from_str(&text).expect("a page is JSON");
        for node in value["data"]["user"]["projectV2"]["items"]["nodes"]
            .as_array()
            .expect("a page holds items")
        {
            let content = &node["content"];
            let url = content["url"].as_str().unwrap_or_default().to_owned();
            let mut fields = BTreeMap::new();
            for field in node["fieldValues"]["nodes"].as_array().into_iter().flatten() {
                let Some(name) = field["field"]["name"].as_str() else {
                    continue;
                };
                let value = field["text"].as_str().or(field["name"].as_str());
                if let Some(value) = value {
                    fields.insert(name.to_owned(), value.to_owned());
                }
            }
            let body = content["body"].as_str().unwrap_or_default();
            let (visible, metadata) = split_metadata(body);
            let record = metadata
                .as_ref()
                .and_then(|metadata| metadata.get(format!("{NS}.follow-up")).cloned());
            let comment_nodes = if url == long_url {
                extras["long_item"]["nodes"].as_array().cloned().unwrap_or_default()
            } else {
                content["comments"]["nodes"].as_array().cloned().unwrap_or_default()
            };
            let comments = comment_nodes
                .iter()
                .map(|one| {
                    let id = one["id"].as_str().unwrap_or_default().to_owned();
                    Comment {
                        created_at: created.get(&id).copied(),
                        id,
                        body: one["body"].as_str().unwrap_or_default().to_owned(),
                    }
                })
                .collect();
            items.push(Item {
                title: fields
                    .get("Title")
                    .cloned()
                    .unwrap_or_else(|| content["title"].as_str().unwrap_or_default().to_owned()),
                status: fields.get("Status").cloned().unwrap_or_default(),
                open: content["state"] == "OPEN",
                url,
                fields,
                visible,
                record,
                comments,
            });
        }
    }
    items
}

fn split_metadata(body: &str) -> (String, Option<Value>) {
    const OPEN: &str = "<!-- onetaskgraph.metadata\n";
    let Some(at) = body.rfind(OPEN) else {
        return (body.to_owned(), None);
    };
    let rest = &body[at + OPEN.len()..];
    let end = rest.find("\n-->").unwrap_or(rest.len());
    (
        body[..at].trim_end().to_owned(),
        serde_json::from_str(&rest[..end]).ok(),
    )
}

fn schema_label(item: &Item) -> String {
    match &item.record {
        None => "no follow-up record".to_owned(),
        Some(record) => match record["schema"].as_u64() {
            Some(schema) => format!("record schema {schema}"),
            None => "record without a schema".to_owned(),
        },
    }
}

fn names(body: &str) -> Vec<String> {
    ticket::headings(body).into_iter().map(|(name, _)| name).collect()
}

fn layout_label(visible: &str) -> String {
    let headings = names(visible);
    let has = |name: &str| headings.iter().any(|one| one == name);
    let at = |name: &str| headings.iter().position(|one| one == name);
    let mut parts = Vec::new();
    if has("Repository") || has("Suggested fixes") {
        parts.push("schema-3 sections (`## Repository`, `## Suggested fixes`)".to_owned());
    }
    if has("Owning runs") {
        parts.push("`## Owning runs` section".to_owned());
    } else if has("Owning batches") {
        parts.push("`## Owning batches` section".to_owned());
    } else {
        parts.push("no metadata section".to_owned());
    }
    if visible.contains("<details>") {
        parts.push("collapsed parts".to_owned());
    } else {
        parts.push("uncollapsed".to_owned());
    }
    if matches!((at("Evidence"), at("Suggested fix").or(at("Suggested fixes"))), (Some(e), Some(f)) if e < f)
    {
        parts.push("Evidence before the fix".to_owned());
    }
    if ANCHOR.is_match(visible) {
        parts.push("heading anchors".to_owned());
    }
    parts.join(", ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CommentLayout {
    EvidenceCurrent,
    Evidence05,
    Withdrawal,
    MarkedNote,
    Reply,
    Unmarked,
}

impl CommentLayout {
    fn label(self) -> &'static str {
        match self {
            Self::EvidenceCurrent => "evidence comment, current layout (collapsed metadata block)",
            Self::Evidence05 => "evidence comment, 0.5 layout (opening line, prose with host/basis/draft lines, optional bears-on, marker)",
            Self::Withdrawal => "marked withdrawal note (`Withdrawn as a duplicate of …`)",
            Self::MarkedNote => "marked filing/correction note (prose and a marker, no opening line)",
            Self::Reply => "reply to a person (marker with `answers=`)",
            Self::Unmarked => "unmarked: a person's comment or a triage note",
        }
    }
}

fn comment_layout(body: &str) -> (CommentLayout, Option<CommentMarker>) {
    let Some(marker) = markers::owner(body, NS) else {
        return (CommentLayout::Unmarked, None);
    };
    let layout = if marker.reply.is_some() {
        CommentLayout::Reply
    } else if body.starts_with("<details>\n<summary>Additional evidence from follow-up run ") {
        CommentLayout::EvidenceCurrent
    } else if body.starts_with("Additional evidence from follow-up run `") {
        CommentLayout::Evidence05
    } else if body.starts_with("Withdrawn as a duplicate of ") {
        CommentLayout::Withdrawal
    } else {
        CommentLayout::MarkedNote
    };
    (layout, Some(marker))
}

/// Each line of `text` with whether it is inside a fence.
fn fenced_lines(text: &str) -> Vec<(&str, bool)> {
    let mut open: Option<String> = None;
    text.split('\n')
        .map(|line| {
            if let Some(captured) = FENCE.captures(line) {
                let marker = captured[1].to_owned();
                match &open {
                    None => open = Some(marker),
                    Some(held) if marker.starts_with(held.as_str()) => open = None,
                    Some(_) => {}
                }
                (line, true)
            } else {
                (line, open.is_some())
            }
        })
        .collect()
}

fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for (line, inside) in fenced_lines(text) {
        if !inside && line.trim().is_empty() {
            if !current.trim().is_empty() {
                out.push(current.trim_end().to_owned());
            }
            current.clear();
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim_end().to_owned());
    }
    out
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tidy(text: &str) -> String {
    paragraphs(text).join("\n\n")
}

/// Split `text` into sentences at `. `, `; ` before a capital, backtick or draft id, never
/// inside backticks.
fn sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut tick = false;
    let mut depth = 0usize;
    let mut at = 0;
    while at < chars.len() {
        let one = chars[at];
        current.push(one);
        if one == '`' {
            tick = !tick;
        }
        if !tick && one == '(' {
            depth += 1;
        }
        if !tick && one == ')' {
            depth = depth.saturating_sub(1);
        }
        if !tick && depth == 0 && (one == '.' || one == ';') && chars.get(at + 1).is_some_and(|c| *c == ' ' || *c == '\n')
        {
            let next = chars[at + 2..].iter().find(|c| !c.is_whitespace());
            if next.is_some_and(|c| c.is_uppercase() || *c == '`' || *c == '-' || *c == '*')
                || (one == ';' && next.is_some())
                || next.is_none()
            {
                out.push(current.trim().to_owned());
                current.clear();
            }
        }
        at += 1;
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_owned());
    }
    out
}

fn clean_token(token: &str) -> String {
    token
        .trim_matches(|c: char| matches!(c, '`' | '.' | ',' | ';' | ')' | '(' | ':' | '\'' | '"'))
        .to_owned()
}

#[derive(Debug, Default, Clone)]
struct Extracted {
    hosts: Vec<String>,
    basis: IndexMap<String, String>,
    drafts: Vec<String>,
    /// Per draft named alone in its block: `author <kind>; node <node>; member <member>`.
    draft_sources: IndexMap<String, String>,
    transcripts: IndexMap<String, String>,
    /// `(origin or None, path, line)` from `Verified at` clauses.
    witnesses: Vec<(Option<String>, String, String)>,
    /// The prose kept, in order.
    prose: Vec<String>,
    /// Sentences kept that still carry a value the record holds.
    leaked: usize,
}

/// Whether `sentence`, its values removed, is metadata only.
fn residue_words(sentence: &str) -> usize {
    let mut text = sentence.to_owned();
    for pattern in [
        &*VERIFIED_AT, &*TRANSCRIPT, &*AUTHOR_DICT, &*DRAFT_ID, &*COMMIT, &*ORIGIN, &*HOST_PHRASE,
        &*RUN_CLAUSE, &*NODE, &*MEMBER, &*AUTHOR,
    ] {
        text = pattern.replace_all(&text, " ").into_owned();
    }
    let text = TICKED.replace_all(&text, " ");
    WORD.find_iter(&text)
        .map(|word| word.as_str().to_lowercase())
        .filter(|word| !METADATA_WORDS.contains(&word.as_str()))
        .count()
}

fn holds_metadata(sentence: &str) -> bool {
    DRAFT_ID.is_match(sentence)
        || COMMIT.is_match(sentence)
        || VERIFIED_AT.is_match(sentence)
        || TRANSCRIPT.is_match(sentence)
        || METADATA_START.is_match(sentence)
}

/// Pair each commit in `text` with the origin nearest before it (or the only origin there).
fn basis_pairs(text: &str) -> IndexMap<String, String> {
    let origins: Vec<(usize, String)> = ORIGIN
        .find_iter(text)
        .map(|found| (found.start(), clean_token(found.as_str())))
        .collect();
    let mut pairs = IndexMap::new();
    for commit in COMMIT.find_iter(text) {
        let before = origins.iter().rev().find(|(at, _)| *at < commit.start());
        let origin = before.or(if origins.len() == 1 { origins.first() } else { None });
        if let Some((_, origin)) = origin {
            pairs.entry(origin.clone()).or_insert_with(|| commit.as_str().to_owned());
        }
    }
    pairs
}

fn extract_block(block: &str, out: &mut Extracted) {
    let bullet = block.starts_with("- ") || block.starts_with("* ");
    let text = if bullet { &block[2..] } else { block };
    let block_drafts: Vec<String> = DRAFT_ID
        .find_iter(text)
        .map(|found| clean_token(found.as_str()))
        .collect();
    let mut kept: Vec<String> = Vec::new();
    let mut headed = false;
    // The same clause opening the whole block, which may run over `; ` (`<id>; run R; author A,
    // node N. Verified at P:L: <prose>`), is taken before the block splits into sentences.
    let mut text = text.to_owned();
    if DRAFT_LEAD.is_match(&text)
        && let Some(colon) = clause_end(&text)
        && residue_words(&text[..colon]) == 0
    {
        let head = text[..colon].to_owned();
        extract_values(&head, &block_drafts, out);
        for found in VERIFIED_AT.captures_iter(&head) {
            for path in PATH_LINE.captures_iter(&found["list"]) {
                out.witnesses.push((None, path["path"].to_owned(), path["line"].to_owned()));
            }
        }
        text = text[colon + 1..].trim().to_owned();
        headed = true;
    }
    for sentence in sentences(&text) {
        let mut sentence = sentence;
        // `<draft id> (<run, author, node, transcript>): <prose>` and `Draft <id>, run R, node N:
        // <prose>` open with a metadata clause up to the first `: ` outside backticks.
        if DRAFT_LEAD.is_match(&sentence)
            && let Some(colon) = clause_end(&sentence)
            && residue_words(&sentence[..colon]) == 0
        {
            let head = sentence[..colon].to_owned();
            extract_values(&head, &block_drafts, out);
            for found in VERIFIED_AT.captures_iter(&head) {
                for path in PATH_LINE.captures_iter(&found["list"]) {
                    out.witnesses.push((None, path["path"].to_owned(), path["line"].to_owned()));
                }
            }
            sentence = sentence[colon + 1..].trim().to_owned();
            headed = true;
        }
        // `Verified at P:L; P:L: prose` keeps the prose after the list.
        if let Some(captured) = VERIFIED_AT.captures(&sentence) {
            for found in PATH_LINE.captures_iter(&captured["list"]) {
                let origin = ORIGIN.find(&sentence).map(|one| clean_token(one.as_str()));
                out.witnesses
                    .push((origin, found["path"].to_owned(), found["line"].to_owned()));
            }
            if captured.name("tail").is_some() {
                let whole = captured.get(0).expect("matched");
                let head = sentence[..whole.start()].to_owned();
                let tail = sentence[whole.end()..].to_owned();
                // The head (draft, run, author …) is read below; the tail is prose.
                extract_values(&head, &block_drafts, out);
                if residue_words(&head) > 0 {
                    kept.push(head.trim().to_owned());
                }
                sentence = tail;
            }
        }
        extract_values(&sentence, &block_drafts, out);
        // A clause naming only the run (`run R`), split off a draft line, is metadata too.
        let run_only = RUN_START.is_match(&sentence) && residue_words(&sentence) == 0;
        if !run_only && (!holds_metadata(&sentence) || residue_words(&sentence) > 0) {
            if holds_metadata(&sentence) {
                out.leaked += 1;
            }
            if !sentence.trim().is_empty() {
                kept.push(sentence.trim().to_owned());
            }
        }
    }
    if !kept.is_empty() {
        let joined = kept.join(" ");
        out.prose.push(if bullet && !headed { format!("- {joined}") } else { joined });
    }
}

/// The first `: ` outside backticks and parentheses.
fn clause_end(text: &str) -> Option<usize> {
    let mut tick = false;
    let mut depth = 0usize;
    let bytes = text.as_bytes();
    for (at, one) in text.char_indices() {
        match one {
            '`' => tick = !tick,
            '(' if !tick => depth += 1,
            ')' if !tick => depth = depth.saturating_sub(1),
            ':' if !tick && depth == 0 && bytes.get(at + 1) == Some(&b' ') => return Some(at),
            _ => {}
        }
    }
    None
}

fn extract_values(sentence: &str, block_drafts: &[String], out: &mut Extracted) {
    if let Some(captured) = HOST_PHRASE.captures(sentence)
        && HOST_START.is_match(sentence)
    {
        let host = clean_token(&captured["host"]);
        if !out.hosts.contains(&host) {
            out.hosts.push(host);
        }
    }
    for (origin, commit) in basis_pairs(sentence) {
        out.basis.entry(origin).or_insert(commit);
    }
    for draft in DRAFT_ID.find_iter(sentence) {
        let draft = clean_token(draft.as_str());
        if !out.drafts.contains(&draft) {
            out.drafts.push(draft);
        }
    }
    let [draft] = block_drafts else {
        return;
    };
    let mut kind = None;
    let mut node = None;
    let mut member = None;
    if let Some(dict) = AUTHOR_DICT.captures(sentence) {
        for found in DICT_VALUE.captures_iter(&dict["dict"]) {
            let value = found["value"].trim().to_owned();
            match &found["key"] {
                "kind" => kind = Some(value),
                "node" => node = Some(value),
                _ => member = Some(value),
            }
        }
    } else if let Some(found) = AUTHOR.captures(sentence) {
        kind = Some(found["kind"].to_lowercase());
        let after = &sentence[found.get(0).expect("matched").end()..];
        node = NODE.captures(after).map(|one| one["node"].to_owned());
        member = MEMBER.captures(after).map(|one| one["member"].to_owned());
    }
    if let Some(kind) = kind.filter(|kind| kind != "none" && kind != "null") {
        let mut source = format!("author {kind}");
        for (label, value) in [("node", node), ("member", member)] {
            if let Some(value) = value.filter(|value| !matches!(value.as_str(), "None" | "none" | "null")) {
                let _ = write!(source, "; {label} {value}");
            }
        }
        out.draft_sources.entry(draft.clone()).or_insert(source);
    }
    if let Some(found) = TRANSCRIPT.captures(sentence) {
        let command = found["command"].trim().to_owned();
        if !command.to_lowercase().starts_with("none") {
            out.transcripts.entry(draft.clone()).or_insert(command);
        }
    }
}

/// Split a section into blocks: paragraphs, each list split into its bullets.
fn blocks(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in paragraphs(content) {
        if FENCE.is_match(&paragraph) || paragraph.starts_with('|') || paragraph.starts_with('>') {
            out.push(paragraph);
            continue;
        }
        let mut current = String::new();
        for line in paragraph.lines() {
            if (line.starts_with("- ") || line.starts_with("* ")) && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

fn extract(content: &str) -> Extracted {
    let mut out = Extracted::default();
    for block in blocks(content) {
        if FENCE.is_match(&block) {
            out.prose.push(block);
        } else {
            extract_block(&block, &mut out);
        }
    }
    out
}

/// Counterfactual rules beyond the decided ones, measured beside them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Variant {
    /// Every `path:line` the Evidence cites becomes a witness at the repository's basis.
    witnesses_from_paths: bool,
    /// A basis commit cited in Evidence prose is replaced by words naming the basis.
    basis_commits_out_of_prose: bool,
}

#[derive(Debug, Clone)]
struct Mapped {
    title: String,
    slug: String,
    body: String,
    record: TicketRecord,
    units: usize,
    notes: Vec<String>,
    occurrences: u64,
}

fn uncollapse(content: &str, impact: &mut IndexMap<String, String>) -> String {
    DETAILS
        .replace_all(content, |captured: &regex::Captures<'_>| {
            let summary = captured["summary"].trim();
            let inner = &captured["content"];
            match summary {
                "Cost of the workaround" => {
                    let mut kept = Vec::new();
                    for line in inner.lines() {
                        if let Some(found) = LABELLED.captures(line)
                            && found["label"].trim() == "Workaround"
                        {
                            impact.insert("Workaround".to_owned(), found["value"].trim().to_owned());
                            continue;
                        }
                        kept.push(line);
                    }
                    format!("**Cost of the workaround.**\n{}", kept.join("\n").trim())
                }
                "Priority calculation" => {
                    for line in inner.lines() {
                        if let Some(found) = LABELLED.captures(line) {
                            let label = found["label"].trim();
                            if label == "Severity" || label == "Severity with the workaround" {
                                impact.insert(label.to_owned(), found["value"].trim().to_owned());
                            }
                        }
                    }
                    String::new()
                }
                "Contributing locations" => format!("**Contributing locations.**\n{}", inner.trim()),
                _ => captured[0].to_owned(),
            }
        })
        .into_owned()
}

/// Remove the rubric's labelled lines (and the estimate line) outside fences, reading them.
fn take_impact_lines(content: &str, impact: &mut IndexMap<String, String>) -> String {
    let mut kept = Vec::new();
    for (line, inside) in fenced_lines(content) {
        if !inside && let Some(found) = LABELLED.captures(line.trim_end()) {
            let label = found["label"].trim();
            if ["Severity", "Workaround", "Severity with the workaround"].contains(&label) {
                impact
                    .entry(label.to_owned())
                    .or_insert_with(|| found["value"].trim().to_owned());
                continue;
            }
            if label == "Priority estimate" {
                continue;
            }
        }
        kept.push(line);
    }
    tidy(&kept.join("\n"))
}

/// The one-unit lead rule over `## Suggested fix`.
fn fix_rule(content: &str, others: &str, notes: &mut Vec<String>) -> (String, usize) {
    let lines = fenced_lines(content);
    let units: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, (line, inside))| !inside && SUBHEADING.is_match(line))
        .map(|(at, _)| at)
        .collect();
    let part_at = lines
        .iter()
        .position(|(line, inside)| !inside && line.starts_with("**Contributing locations.**"));
    let (Some(&first), Some(part_at)) = (units.first(), part_at) else {
        return (content.to_owned(), units.len());
    };
    if units.len() != 1 || part_at > first {
        return (content.to_owned(), units.len());
    }
    let text = |range: std::ops::Range<usize>| {
        lines[range].iter().map(|(line, _)| *line).collect::<Vec<_>>().join("\n")
    };
    let lead = text(0..part_at);
    // The part: its first line and every bullet with its indented continuation; other lines
    // are stray prose.
    let mut part = vec![lines[part_at].0.to_owned()];
    let mut stray = Vec::new();
    let mut bulleted = false;
    for (line, _) in &lines[part_at + 1..first] {
        if line.starts_with("- ") || line.starts_with("* ") {
            bulleted = true;
            part.push((*line).to_owned());
        } else if line.trim().is_empty() {
            if stray.last().is_some_and(|last: &String| !last.is_empty()) {
                stray.push(String::new());
            }
        } else if bulleted && line.starts_with([' ', '\t']) && stray.is_empty() {
            part.push((*line).to_owned());
        } else {
            stray.push((*line).to_owned());
        }
    }
    let unit_heading = lines[first].0;
    let unit_body = text(first + 1..lines.len());
    let haystack = normalized(&format!("{others}\n{unit_body}"));
    let mut moved = Vec::new();
    for paragraph in paragraphs(&lead).into_iter().chain(paragraphs(&stray.join("\n"))) {
        if RENDERER_SENTENCES.contains(&paragraph.trim()) {
            notes.push("dropped an older renderer's lead sentence".to_owned());
        } else if haystack.contains(&normalized(&paragraph)) {
            notes.push("dropped a lead paragraph repeating another section verbatim".to_owned());
        } else {
            notes.push("moved a lead paragraph into the unit's opening".to_owned());
            moved.push(paragraph);
        }
    }
    let mut out = part.join("\n");
    out.push_str("\n\n");
    out.push_str(unit_heading);
    out.push_str("\n\n");
    if !moved.is_empty() {
        out.push_str(&moved.join("\n\n"));
        out.push_str("\n\n");
    }
    out.push_str(unit_body.trim());
    (tidy(&out), 1)
}

fn owning_runs(content: &str) -> Vec<String> {
    let mut runs = Vec::new();
    for line in content.lines() {
        let line = line.trim().trim_start_matches(['-', '*']).trim();
        for token in line.split([',', ' ']) {
            let token = clean_token(token);
            if !token.is_empty() {
                runs.push(token);
            }
        }
    }
    runs
}

fn record_to_14(record: &Value, notes: &mut Vec<String>) -> Result<Map<String, Value>, String> {
    let Value::Object(map) = record else {
        return Err("the record is not an object".to_owned());
    };
    let mut map = map.clone();
    let schema = map.get("schema").and_then(Value::as_u64);
    if schema != Some(14) {
        notes.push(format!("record schema {schema:?} mapped onto 14"));
    }
    if map.shift_remove("board_item").is_some() {
        notes.push("dropped `board_item`, which schema 14 has no key for (the item binds it)".to_owned());
    }
    map.insert("schema".to_owned(), Value::from(14));
    if !map.contains_key("priority_estimate") {
        map.insert("priority_estimate".to_owned(), Value::from("low"));
    }
    Ok(map)
}

fn render_sections(sections: &[(String, String)]) -> String {
    sections
        .iter()
        .map(|(name, content)| format!("## {name}\n\n{}\n", content.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn map_ticket(item: &Item, visible: &str, record: &Value, variant: Variant) -> Result<Mapped, String> {
    let mut notes = Vec::new();
    let mut map = record_to_14(record, &mut notes)?;
    let body = ANCHOR.replace_all(visible, "").into_owned();
    let headings = ticket::headings(&body);
    if headings.is_empty() {
        return Err("the body has no `##` section".to_owned());
    }
    if !body[..headings[0].1].trim().is_empty() {
        notes.push("text before the first heading dropped".to_owned());
    }
    let mut sections: Vec<(String, String)> = headings
        .iter()
        .enumerate()
        .map(|(at, (name, start))| {
            let end = headings.get(at + 1).map_or(body.len(), |(_, offset)| *offset);
            let content = body[*start..end].split_once('\n').map_or("", |(_, rest)| rest);
            (name.clone(), content.trim().to_owned())
        })
        .collect();
    let original = sections.clone();
    let record_basis: IndexMap<String, String> = map
        .get("basis")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let mut impact: IndexMap<String, String> = map
        .get("impact")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let had_impact = !impact.is_empty();
    let mut runs: Vec<String> = map
        .get("owning_runs")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let mut evidence = None;
    sections.retain_mut(|(name, content)| match name.as_str() {
        "Owning runs" | "Owning batches" => {
            for run in owning_runs(content) {
                if !runs.contains(&run) {
                    notes.push(format!("owning run `{run}` added from the section"));
                    runs.push(run);
                }
            }
            false
        }
        "Impact" => {
            let mut found = IndexMap::new();
            let open = uncollapse(content, &mut found);
            *content = take_impact_lines(&open, &mut found);
            for (label, value) in found {
                impact.entry(label).or_insert(value);
            }
            true
        }
        "Suggested fix" => {
            *content = tidy(&uncollapse(content, &mut IndexMap::new()));
            true
        }
        "Evidence" => {
            let mut found = extract(content);
            if variant.witnesses_from_paths {
                for path in PATH_LINE.captures_iter(content) {
                    found.witnesses.push((None, path["path"].to_owned(), path["line"].to_owned()));
                }
            }
            let mut prose = found.prose.join("\n\n");
            if variant.basis_commits_out_of_prose {
                for commit in record_basis.values() {
                    let named = Regex::new(&format!(r"(?i)\bbasis(?: commit)?\s*:?\s*`?{commit}`?")).expect("valid");
                    prose = named.replace_all(&prose, "basis").into_owned();
                    let bare = Regex::new(&format!(r"`?{commit}`?")).expect("valid");
                    prose = bare.replace_all(&prose, "the basis commit").into_owned();
                }
            }
            *content = prose;
            evidence = Some(found);
            true
        }
        _ => true,
    });
    map.insert("owning_runs".to_owned(), json!(runs));
    if !impact.is_empty() {
        let ordered: IndexMap<String, String> = ["Severity", "Workaround", "Severity with the workaround"]
            .iter()
            .filter_map(|label| impact.get(*label).map(|value| ((*label).to_owned(), value.clone())))
            .collect();
        if !had_impact {
            notes.push("impact read from the labelled lines".to_owned());
        }
        map.insert("impact".to_owned(), json!(ordered));
    }
    let repository = map["repository"].as_str().unwrap_or_default().to_owned();
    let basis: IndexMap<String, String> = map
        .get("basis")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    if let Some(found) = &evidence {
        let host = map["host"].as_str().unwrap_or_default();
        if found.hosts.iter().any(|one| one != host) {
            notes.push("the Evidence names a host the record does not; the record's kept".to_owned());
        }
        let drafts: Vec<String> = map
            .get("drafts")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let sources: IndexMap<String, String> = found
            .draft_sources
            .iter()
            .filter(|(draft, _)| drafts.contains(draft))
            .map(|(draft, source)| (draft.clone(), source.clone()))
            .collect();
        if !sources.is_empty() && !map.contains_key("draft_sources") {
            map.insert("draft_sources".to_owned(), json!(sources));
        }
        let transcripts: IndexMap<String, String> = found
            .transcripts
            .iter()
            .filter(|(draft, _)| drafts.contains(draft))
            .map(|(draft, command)| (draft.clone(), command.clone()))
            .collect();
        if !transcripts.is_empty() && !map.contains_key("transcripts") {
            map.insert("transcripts".to_owned(), json!(transcripts));
        }
        let mut witnesses: Vec<String> = Vec::new();
        for (origin, path, line) in &found.witnesses {
            let origin = origin.clone().filter(|one| basis.contains_key(one)).unwrap_or_else(|| repository.clone());
            if let Some(commit) = basis.get(&origin) {
                let witness = format!("{origin} {path}:{line} at {commit}");
                if !witnesses.contains(&witness) {
                    witnesses.push(witness);
                }
            }
        }
        if !witnesses.is_empty() && !map.contains_key("witnesses") {
            map.insert("witnesses".to_owned(), json!(witnesses));
        }
    }
    // Every other section as the board holds it, so a paragraph repeating one is found before
    // the Evidence rule rewrites it.
    let others = original
        .iter()
        .filter(|(name, _)| name != "Suggested fix")
        .map(|(_, content)| content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let mut units = 0;
    for (name, content) in &mut sections {
        if name == "Suggested fix" {
            let (fixed, count) = fix_rule(content, &others, &mut notes);
            *content = fixed;
            units = count;
        }
    }
    let record = TicketRecord::parse(&Value::Object(map).to_string()).map_err(|error| error.to_string())?;
    let occurrences = 1 + item
        .comments
        .iter()
        .filter(|comment| {
            ticket::counts_as_occurrence(&comment.body, NS, &record.created_by_batch, &record.root_cause)
        })
        .count() as u64;
    Ok(Mapped {
        title: item.title.clone(),
        slug: record.root_cause.clone(),
        body: render_sections(&sections),
        record,
        units,
        notes,
        occurrences,
    })
}

struct Env {
    config: Config,
    kind: Kind,
    ticket_template: Located,
    render_template: Located,
    evidence_template: Located,
    line: EstimateLine,
}

impl Env {
    /// The shipped defaults; `structural` holds the kind to `headings`, `title`, `impact` and
    /// `fix-units` only, leaving out `evidence-prose` and `witnesses`.
    fn new(structural: bool) -> Self {
        let mut config = defaults("spike-host");
        if structural {
            for kind in &mut config.kinds {
                kind.checks = kind.checks.take().map(|checks| {
                    checks
                        .into_iter()
                        .filter(|check| {
                            !matches!(
                                check,
                                onefollowup::config::Check::EvidenceProse { .. }
                                    | onefollowup::config::Check::Witnesses { .. }
                            )
                        })
                        .collect()
                });
            }
        }
        let kind = config
            .kinds
            .iter()
            .find(|kind| kind.name == config.default_kind)
            .expect("the default kind")
            .clone();
        let shipped = |configured: &str| Located::of(configured, |path: &str| PathBuf::from(path));
        Self {
            ticket_template: shipped(
                kind.ticket_template
                    .as_deref()
                    .unwrap_or(onefollowup::api::BUILTIN_TICKET_TEMPLATE),
            ),
            render_template: shipped(kind.render_template.as_deref().expect("the default kind renders")),
            evidence_template: shipped(&config.comment_templates.evidence),
            line: EstimateLine {
                label: config.estimate.line_label.clone(),
                template: shipped(&config.estimate.line_template),
            },
            config,
            kind,
        }
    }

    fn common<'a>(&'a self, batch: &'a str) -> Common<'a> {
        Common {
            config: &self.config,
            kind: &self.kind,
            batch,
            destination: &self.config.destination,
        }
    }

    fn ticket_text(mapped: &Mapped) -> String {
        format!(
            "---\ntitle: {}\nmetadata:\n  {NS}.follow-up: {}\n---\n{}",
            serde_json::to_string(&mapped.title).expect("a string"),
            mapped.record.render(),
            mapped.body
        )
    }

    /// The kind's checks over the mapped ticket, as `batch check` holds a ticket.
    fn problems(&self, mapped: &Mapped) -> Vec<String> {
        let batch = mapped.record.owning_batches.last().cloned().unwrap_or_default();
        let file = ticket::parse(&mapped.slug, Path::new("migrated.md"), &Self::ticket_text(mapped), NS);
        ticket::problems(
            &file,
            &self.config,
            &Context {
                batch: &batch,
                inputs: None,
                transcripts: &[],
                template: &self.ticket_template,
                namespace: NS,
                common: Some(self.common(&batch)),
                filed: false,
            },
        )
    }

    fn render(&self, mapped: &Mapped) -> Result<(String, TicketRecord, String), String> {
        let batch = mapped.record.owning_batches.last().cloned().unwrap_or_default();
        let estimated = ticket::estimate(
            &self.config,
            &self.line,
            &self.kind,
            mapped.record.impact.as_ref(),
            &mapped.body,
            mapped.record.frequency,
            mapped.occurrences,
        )?;
        let mut record = mapped.record.clone();
        record.priority_estimate = estimated.level;
        let body = render::destination(
            &self.common(&batch),
            Some(&self.render_template),
            &mapped.title,
            &mapped.slug,
            &mapped.body,
            &mut record,
            &estimated,
        )?;
        Ok((body, record, ticket::level_word(estimated.level).to_owned()))
    }
}

type Evaluated = (Mapped, String, TicketRecord, String);

/// Map, check and render one ticket: its mapping, rendered body, record and estimate, or every
/// problem.
fn evaluate(env: &Env, item: &Item, variant: Variant) -> Result<Evaluated, Vec<String>> {
    let Some(record) = &item.record else {
        return Err(vec!["no follow-up record: nothing to map the body onto".to_owned()]);
    };
    let mapped = map_ticket(item, &item.visible, record, variant).map_err(|error| vec![format!("the mapping refused: {error}")])?;
    let mut problems = env.problems(&mapped);
    match env.render(&mapped) {
        Err(error) => {
            problems.push(format!("render: {error}"));
            Err(problems)
        }
        Ok((body, record, level)) if problems.is_empty() => Ok((mapped, body, record, level)),
        Ok(_) => Err(problems),
    }
}

fn failure_class(problem: &str) -> String {
    let text = TICKED.replace_all(problem, "`…`");
    let text = UNITS.replace_all(&text, "N units");
    text.chars().take(260).collect()
}

#[derive(Debug, Clone)]
struct MappedComment {
    evidence: String,
    bears_on: Option<String>,
    sources: Value,
    leaked: usize,
    /// Whether the basis came from the ticket's basis rather than an origin the comment names.
    fallback: bool,
}

fn map_comment_05(body: &str, ticket_basis: &IndexMap<String, String>) -> Result<MappedComment, String> {
    let mut paragraphs = paragraphs(body);
    paragraphs.remove(0);
    paragraphs.retain(|one| markers::owner(one, NS).is_none());
    let mut bears_on = None;
    let mut fallback = false;
    let mut found = Extracted::default();
    for paragraph in paragraphs {
        if let Some(rest) = paragraph.strip_prefix("Bears on the ticket:") {
            bears_on = Some(rest.trim().to_owned());
            continue;
        }
        if FENCE.is_match(&paragraph) {
            found.prose.push(paragraph);
            continue;
        }
        let before = found.prose.len();
        for block in blocks(&paragraph) {
            extract_block(&block, &mut found);
        }
        // A list kept whole stays one paragraph.
        let added: Vec<String> = found.prose.drain(before..).collect();
        if !added.is_empty() {
            let joined = if added.iter().all(|one| one.starts_with("- ")) {
                added.join("\n")
            } else {
                added.join("\n\n")
            };
            found.prose.push(joined);
        }
    }
    if found.basis.is_empty() {
        // A commit named under a bare repository name the ticket's basis holds, or — when the
        // ticket has one basis origin — a commit naming no origin at all.
        for found_name in NAMED_BASIS.captures_iter(body) {
            if let Some(origin) = ticket_basis.keys().find(|origin| origin.rsplit('/').next() == Some(&found_name["name"])) {
                found.basis.entry(origin.clone()).or_insert_with(|| found_name["commit"].to_owned());
            }
        }
        if found.basis.is_empty() && ticket_basis.len() == 1 && !ORIGIN.is_match(body) {
            let commits: BTreeSet<&str> = COMMIT.find_iter(body).map(|one| one.as_str()).collect();
            if let [commit] = commits.into_iter().collect::<Vec<_>>().as_slice() {
                let origin = ticket_basis.keys().next().expect("one").clone();
                found.basis.insert(origin, (*commit).to_owned());
            }
        }
        if !found.basis.is_empty() {
            fallback = true;
        }
    }
    let mut mapped = comment_from(found, bears_on)?;
    mapped.fallback = fallback;
    Ok(mapped)
}

fn comment_from(found: Extracted, bears_on: Option<String>) -> Result<MappedComment, String> {
    let host = match found.hosts.as_slice() {
        [] => return Err("no host line: the sources' host cannot be read".to_owned()),
        [one] => one.clone(),
        _ => return Err("more than one host named: the sources' host is ambiguous".to_owned()),
    };
    if found.basis.is_empty() {
        return Err("no basis commit paired with an origin: the sources' basis cannot be read".to_owned());
    }
    let witnesses: Vec<String> = found
        .witnesses
        .iter()
        .filter_map(|(origin, path, line)| {
            let origin = origin.clone().or_else(|| (found.basis.len() == 1).then(|| found.basis.keys().next().cloned()).flatten())?;
            let commit = found.basis.get(&origin)?;
            Some(format!("{origin} {path}:{line} at {commit}"))
        })
        .collect();
    let evidence = found.prose.join("\n\n");
    let mut problems = Vec::new();
    if !ticket::is_host(&host) {
        problems.push(format!("the sources' `host` `{host}` is not a host name"));
    }
    problems.extend(ticket::basis_problems(&found.basis));
    problems.extend(render::witness_problems(&witnesses, &found.basis));
    if evidence.trim().is_empty() {
        problems.push("the evidence is empty once the metadata is taken out".to_owned());
    }
    problems.extend(render::reserved_lines(&evidence, NS, "the evidence"));
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    Ok(MappedComment {
        evidence,
        bears_on,
        sources: json!({
            "host": host,
            "basis": found.basis,
            "drafts": found.drafts,
            "witnesses": witnesses,
            "draft_sources": found.draft_sources,
            "transcripts": found.transcripts,
        }),
        leaked: found.leaked,
        fallback: false,
    })
}

/// Read a current-layout comment back into its parts.
fn map_comment_current(body: &str) -> Result<MappedComment, String> {
    let captured = DETAILS.captures(body).ok_or("no collapsed block")?;
    let rest = &body[captured.get(0).expect("matched").end()..];
    let mut found = Extracted::default();
    for line in captured["content"].lines() {
        if let Some(host) = line.strip_prefix("- Host: ") {
            found.hosts.push(host.trim().to_owned());
        } else if let Some(basis) = line.strip_prefix("- Basis: ") {
            if let Some((origin, commit)) = basis.split_once(" at ") {
                found.basis.insert(origin.trim().to_owned(), commit.trim().to_owned());
            }
        } else if let Some(draft) = line.strip_prefix("- Draft: ") {
            let mut parts = draft.split("; ");
            let id = parts.next().unwrap_or_default().to_owned();
            for part in parts {
                if let Some(command) = part.strip_prefix("transcript: ") {
                    found.transcripts.insert(id.clone(), command.to_owned());
                } else {
                    found
                        .draft_sources
                        .entry(id.clone())
                        .and_modify(|held| {
                            held.push_str("; ");
                            held.push_str(part);
                        })
                        .or_insert_with(|| part.to_owned());
                }
            }
            found.drafts.push(id);
        } else if let Some(witness) = line.strip_prefix("- Witness: ") {
            let parsed = render::WITNESS.captures(witness.trim()).ok_or("an unreadable witness")?;
            found.witnesses.push((
                Some(parsed["origin"].to_owned()),
                parsed["path"].to_owned(),
                parsed["line"].to_owned(),
            ));
        }
    }
    let mut bears_on = None;
    for paragraph in paragraphs(rest) {
        if markers::owner(&paragraph, NS).is_some() {
            continue;
        }
        if let Some(text) = paragraph.strip_prefix("Bears on the ticket:") {
            bears_on = Some(text.trim().to_owned());
        } else {
            found.prose.push(paragraph);
        }
    }
    comment_from(found, bears_on)
}

fn render_comment(env: &Env, mapped: &MappedComment, marker: &CommentMarker) -> Result<String, String> {
    let comment = json!({
        "batch": marker.batch,
        "root_cause": marker.root_cause,
        "evidence": mapped.evidence,
        "bears_on": mapped.bears_on,
        "question": null,
        "sources": mapped.sources,
    });
    let rendered = render::comment(&env.common(&marker.batch), &env.evidence_template, &comment, marker)?;
    match markers::owner(&rendered, NS) {
        Some(owner) if owner == *marker => Ok(rendered),
        _ => Err("the rendered comment's marker does not name the old comment's batch and root cause".to_owned()),
    }
}

#[derive(Default)]
struct Tally {
    total: usize,
    open: usize,
    clean: usize,
    open_clean: usize,
    classes: BTreeMap<String, (usize, String)>,
}

impl Tally {
    fn add(&mut self, open: bool, problems: &[String], link: &str) {
        self.total += 1;
        if open {
            self.open += 1;
        }
        if problems.is_empty() {
            self.clean += 1;
            if open {
                self.open_clean += 1;
            }
        }
        let classes: BTreeSet<String> = problems.iter().map(|one| failure_class(one)).collect();
        for class in classes {
            let entry = self.classes.entry(class).or_insert((0, link.to_owned()));
            entry.0 += 1;
        }
    }
}

fn status_role(status: &str) -> &'static str {
    match status {
        "Proposal" => "backlog",
        "Todo" => "todo",
        "Deferred" => "draft",
        "Queued" => "queued",
        "In Progress" => "in-progress",
        "Done" => "done",
        "Cancelled" => "cancelled",
        _ => "unknown",
    }
}

/// The text a `metadata_fields` entry projects from `record[key]`, as the board's own projected
/// values spell it: a string as it is, a list one entry a line in order, a map `key: value` a
/// line sorted by key.
/// `main` holds this rule to every value already on the board before relying on it.
fn field_value(record: &Value, key: &str) -> Option<String> {
    let text = |value: &Value| value.as_str().map_or_else(|| value.to_string(), str::to_owned);
    match record.get(key)? {
        Value::String(one) if !one.is_empty() => Some(one.clone()),
        Value::Array(list) if !list.is_empty() => Some(list.iter().map(text).collect::<Vec<_>>().join("\n")),
        Value::Object(map) if !map.is_empty() => {
            let mut pairs: Vec<(&String, &Value)> = map.iter().collect();
            pairs.sort_by_key(|(name, _)| *name);
            Some(pairs.iter().map(|(name, value)| format!("{name}: {}", text(value))).collect::<Vec<_>>().join("\n"))
        }
        _ => None,
    }
}

/// One migrating ticket's writes.
#[derive(Debug, Clone, Default, serde::Serialize)]
struct Row {
    url: String,
    /// Each board field the migration fills, with the exact value written.
    fields_new: Vec<(String, String)>,
    /// Each board field holding a value other than the one projected: (field, board, new).
    fields_changed: Vec<(String, String, String)>,
    /// The board's Priority and the estimate's, when they differ.
    priority: Option<(String, String)>,
    /// The ids of the evidence comments it edits.
    comment_ids: Vec<String>,
}

impl Row {
    fn writes(&self) -> usize {
        1 + self.fields_new.len() + self.fields_changed.len() + usize::from(self.priority.is_some()) + self.comment_ids.len()
    }
}

fn plan_writes(left: &[&Item], evaluated: &HashMap<String, Result<Evaluated, Vec<String>>>, comment_edits: &HashMap<String, Vec<String>>) -> Plan {
    let mut plan = Plan::default();
    for item in left {
        let Some(Ok((_, _, record, level))) = evaluated.get(&item.url) else {
            continue;
        };
        let mut row = Row { url: item.url.clone(), ..Row::default() };
        plan.tickets += 1;
        plan.body_writes += 1;
        let value: Value = serde_json::from_str(&record.render()).expect("record");
        for (field, key) in METADATA_FIELDS {
            let Some(projected) = field_value(&value, key) else { continue };
            match item.fields.get(*field) {
                None => {
                    plan.fields_new += 1;
                    *plan.fields_new_by.entry((*field).to_owned()).or_default() += 1;
                    row.fields_new.push(((*field).to_owned(), projected));
                }
                Some(board) if *board != projected => {
                    plan.fields_changed += 1;
                    row.fields_changed.push(((*field).to_owned(), board.clone(), projected));
                }
                Some(_) => {}
            }
        }
        let board = item.fields.get("Priority").cloned();
        if board.as_deref().map(str::to_lowercase).as_deref() != Some(level.as_str()) {
            plan.priority_writes += 1;
            let mut new = level.clone();
            new[..1].make_ascii_uppercase();
            row.priority = Some((board.unwrap_or_else(|| "(none)".to_owned()), new));
        }
        let ids = comment_edits.get(&item.url).cloned().unwrap_or_default();
        plan.comment_edits += ids.len();
        row.comment_ids = ids;
        plan.rows.push(row);
    }
    plan
}

/// Every row's writes summed, against the aggregate: the inventory reconciles or the run fails.
fn reconcile(plan: &Plan) -> Result<String, String> {
    let sum = |each: fn(&Row) -> usize| plan.rows.iter().map(each).sum::<usize>();
    let checks = [
        ("tickets", plan.rows.len(), plan.tickets),
        ("body updates", plan.rows.len(), plan.body_writes),
        ("field values newly set", sum(|row| row.fields_new.len()), plan.fields_new),
        ("field values changed", sum(|row| row.fields_changed.len()), plan.fields_changed),
        ("Priority changes", sum(|row| usize::from(row.priority.is_some())), plan.priority_writes),
        ("evidence-comment edits", sum(|row| row.comment_ids.len()), plan.comment_edits),
        ("migration writes", sum(Row::writes), plan.writes()),
    ];
    let line = checks.iter().map(|(name, rows, total)| format!("{name} {rows} = {total}")).collect::<Vec<_>>().join("; ");
    if checks.iter().all(|(_, rows, total)| rows == total) { Ok(line) } else { Err(line) }
}

impl Plan {
    fn writes(&self) -> usize {
        self.body_writes + self.comment_edits + self.fields_new + self.fields_changed + self.priority_writes
    }
}

#[derive(Default)]
struct Plan {
    tickets: usize,
    comment_edits: usize,
    fields_new: usize,
    fields_new_by: BTreeMap<String, usize>,
    priority_writes: usize,
    body_writes: usize,
    fields_changed: usize,
    rows: Vec<Row>,
}

fn table(out: &mut String, header: &[&str], rows: &[Vec<String>]) {
    let _ = writeln!(out, "| {} |", header.join(" | "));
    let _ = writeln!(out, "|{}", " --- |".repeat(header.len()));
    for row in rows {
        let cells: Vec<String> = row.iter().map(|cell| cell.replace('|', "\\|").replace('\n', " ")).collect();
        let _ = writeln!(out, "| {} |", cells.join(" | "));
    }
    out.push('\n');
}

#[allow(clippy::too_many_lines)]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dump, extras_path, out_dir, rest @ ..] = args.as_slice() else {
        eprintln!("usage: spike_legacy_formats <board dump dir> <extras.json> <out dir> [--as-of <RFC 3339>]");
        std::process::exit(2);
    };
    let as_of: DateTime<Utc> = match rest {
        [flag, at] if flag == "--as-of" => at.parse().expect("--as-of is an RFC 3339 time"),
        _ => Utc::now(),
    };
    let extras: Value =
        serde_json::from_str(&std::fs::read_to_string(extras_path).expect("extras.json reads")).expect("extras.json is JSON");
    let out_dir = PathBuf::from(out_dir);
    std::fs::create_dir_all(out_dir.join("before-after")).expect("the out dir is writable");
    let items = load(Path::new(dump), &extras);
    let env = Env::new(false);
    let structural = Env::new(true);
    let scenarios: [(&str, &Env, Variant); 4] = [
        ("decided rules, default checks", &env, Variant::default()),
        ("+ a witness for every `path:line` the Evidence cites", &env, Variant { witnesses_from_paths: true, basis_commits_out_of_prose: false }),
        ("+ basis commits taken out of Evidence prose", &env, Variant { witnesses_from_paths: true, basis_commits_out_of_prose: true }),
        ("decided rules, without the `evidence-prose` and `witnesses` checks", &structural, Variant::default()),
    ];
    let mut scenario_clean: BTreeMap<(usize, String), (usize, usize)> = BTreeMap::new();
    let mut scenario_ok: Vec<HashMap<String, Result<Evaluated, Vec<String>>>> = vec![HashMap::new(); scenarios.len()];
    let delivered: BTreeSet<String> = extras["delivers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|one| one["url"].as_str().map(str::to_owned))
        .collect();

    // Formats and the ticket mapping.
    let mut formats: BTreeMap<(String, String), Tally> = BTreeMap::new();
    let mut by_schema: BTreeMap<String, Tally> = BTreeMap::new();
    let mut notes: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut not_idempotent: Vec<String> = Vec::new();
    let mut current_changed: Vec<String> = Vec::new();
    let mut witnesses_only: usize = 0;
    let mut witnesses_only_open: usize = 0;
    let mut title_refusals: Vec<(String, String)> = Vec::new();
    let mut outcomes: HashMap<String, (bool, Option<(Mapped, String, TicketRecord, String)>, Vec<String>)> = HashMap::new();
    for item in &items {
        let key = (schema_label(item), layout_label(&item.visible));
        let link = item.url.clone();
        let result = evaluate(&env, item, Variant::default());
        for (at, (_, scenario_env, variant)) in scenarios.iter().enumerate() {
            let evaluated = evaluate(scenario_env, item, *variant);
            let entry = scenario_clean.entry((at, key.0.clone())).or_default();
            entry.0 += 1;
            entry.1 += usize::from(evaluated.is_ok());
            scenario_ok[at].insert(item.url.clone(), evaluated);
        }
        let problems = match &result {
            Ok(_) => Vec::new(),
            Err(problems) => problems.clone(),
        };
        if std::env::var("SPIKE_DEBUG").is_ok_and(|wanted| item.url.ends_with(&wanted)) {
            if let Some(record) = &item.record
                && let Ok(mapped) = map_ticket(item, &item.visible, record, Variant::default())
            {
                eprintln!("=== MAPPED {}\n{}\n=== RECORD {}\n=== NOTES {:?}", item.url, mapped.body, mapped.record.render(), mapped.notes);
                if let Ok((body, record, _)) = env.render(&mapped) {
                    eprintln!("=== RENDERED\n{body}");
                    let again = map_ticket(item, &body, &serde_json::from_str(&record.render()).expect("record"), Variant::default())
                        .and_then(|mapped| env.render(&mapped));
                    if let Ok((twice, ..)) = again {
                        eprintln!("=== RENDERED TWICE\n{twice}");
                    }
                }
            }
            eprintln!("=== PROBLEMS {problems:#?}");
        }
        if !problems.is_empty()
            && problems.iter().all(|one| one.contains("carries no `witnesses`"))
        {
            witnesses_only += 1;
            if item.open {
                witnesses_only_open += 1;
            }
        }
        for problem in &problems {
            if problem.starts_with("the title") {
                title_refusals.push((short(&item.url), format!("{} — {}", problem, item.title)));
            }
        }
        formats.entry(key.clone()).or_default().add(item.open, &problems, &link);
        by_schema.entry(key.0.clone()).or_default().add(item.open, &problems, &link);
        let ok = result.as_ref().ok().cloned();
        if let Some(Ok(mapped)) = item.record.as_ref().map(|record| map_ticket(item, &item.visible, record, Variant::default())) {
            let distinct: BTreeSet<String> = mapped.notes.iter().map(|note| failure_class(note)).collect();
            for note in distinct {
                let entry = notes.entry(note).or_default();
                entry.0 += 1;
                entry.1 += usize::from(item.open);
            }
        }
        if let Some((_, body, record, _)) = &ok {
            // Idempotency: the rendered body, mapped again, renders the same.
            let again = map_ticket(item, body, &serde_json::from_str(&record.render()).expect("record"), Variant::default())
                .and_then(|mapped| env.render(&mapped));
            match again {
                Ok((twice, _, _)) if twice == *body => {}
                _ => not_idempotent.push(short(&item.url)),
            }
            if key.1.starts_with("no metadata section, collapsed parts")
                && normalized(body) != normalized(&ANCHOR.replace_all(&item.visible, ""))
            {
                current_changed.push(short(&item.url));
            }
        }
        outcomes.insert(item.url.clone(), (item.open, ok, problems));
    }

    // Comments.
    let mut comment_layouts: BTreeMap<CommentLayout, (usize, usize)> = BTreeMap::new();
    let mut comment_tally: BTreeMap<CommentLayout, Tally> = BTreeMap::new();
    let mut comment_leaked = 0usize;
    let mut comment_edits_by_item: HashMap<String, Vec<String>> = HashMap::new();
    let mut renders: HashMap<String, (String, String)> = HashMap::new();
    let mut unmappable_notes: Vec<(String, String)> = Vec::new();
    let mut comment_fallback = 0usize;
    for item in &items {
        let ticket_basis: IndexMap<String, String> = item
            .record
            .as_ref()
            .and_then(|record| serde_json::from_value(record["basis"].clone()).ok())
            .unwrap_or_default();
        for comment in &item.comments {
            let (layout, marker) = comment_layout(&comment.body);
            let entry = comment_layouts.entry(layout).or_default();
            entry.0 += 1;
            if item.open {
                entry.1 += 1;
            }
            let link = format!("{}#issuecomment-{}", item.url, comment.id);
            let mapped = match (layout, &marker) {
                (CommentLayout::Evidence05, Some(marker)) => Some(
                    map_comment_05(&comment.body, &ticket_basis).and_then(|mapped| render_comment(&env, &mapped, marker).map(|text| (mapped, text))),
                ),
                (CommentLayout::EvidenceCurrent, Some(marker)) => Some(
                    map_comment_current(&comment.body)
                        .and_then(|mapped| render_comment(&env, &mapped, marker).map(|text| (mapped, text)))
                        .and_then(|(mapped, text)| {
                            if normalized(&text) == normalized(&comment.body) {
                                Ok((mapped, text))
                            } else {
                                Err("a current-layout comment does not render back unchanged".to_owned())
                            }
                        }),
                ),
                (CommentLayout::Withdrawal | CommentLayout::MarkedNote, _) => {
                    unmappable_notes.push((link.clone(), layout.label().to_owned()));
                    Some(Err("no opening line, sources or evidence part: nothing the evidence template's parts map from".to_owned()))
                }
                _ => None,
            };
            if let Some(mapped) = mapped {
                let problems = match &mapped {
                    Ok(_) => Vec::new(),
                    Err(error) => error.split("; ").map(str::to_owned).collect(),
                };
                comment_tally.entry(layout).or_default().add(item.open, &problems, &link);
                if let Ok((mapped, text)) = &mapped {
                    // Idempotency: the rendered comment reads back to itself.
                    if layout == CommentLayout::Evidence05 {
                        comment_leaked += usize::from(mapped.leaked > 0);
                        comment_fallback += usize::from(mapped.fallback);
                        let back = map_comment_current(text).and_then(|again| render_comment(&env, &again, marker.as_ref().expect("marked")));
                        if back.as_deref() != Ok(text.as_str()) {
                            comment_tally.entry(layout).or_default().classes.entry("NOT IDEMPOTENT: the rendered comment does not read back to itself".to_owned()).or_insert((0, link.clone())).0 += 1;
                        }
                        comment_edits_by_item.entry(item.url.clone()).or_default().push(comment.id.clone());
                    }
                    renders.insert(comment.id.clone(), (comment.body.clone(), text.clone()));
                }
            }
        }
    }

    // Closures at 14 days.
    let window = as_of - Duration::days(STALE_DAYS);
    let mut open_total = 0usize;
    let mut stale = 0usize;
    let mut fresh = 0usize;
    let mut no_time = 0usize;
    let mut exempt_by: BTreeMap<&str, usize> = BTreeMap::new();
    let mut exempt_first: BTreeMap<&str, usize> = BTreeMap::new();
    let mut closes: Vec<String> = Vec::new();
    let mut exempt_urls: BTreeSet<String> = BTreeSet::new();
    let mut verified_only_stale = 0usize;
    for item in items.iter().filter(|item| item.open) {
        open_total += 1;
        let verified: Option<DateTime<Utc>> = item
            .record
            .as_ref()
            .and_then(|record| record["verified_at"].as_str()?.parse().ok());
        if verified.is_some_and(|at| at <= window) {
            verified_only_stale += 1;
        }
        let newest_evidence = item
            .comments
            .iter()
            .filter(|comment| {
                matches!(comment_layout(&comment.body).0, CommentLayout::Evidence05 | CommentLayout::EvidenceCurrent)
            })
            .filter_map(|comment| comment.created_at)
            .max();
        let Some(latest) = verified.into_iter().chain(newest_evidence).max() else {
            no_time += 1;
            continue;
        };
        if latest > window {
            fresh += 1;
            continue;
        }
        stale += 1;
        let role = status_role(&item.status);
        let mut reasons = Vec::new();
        if ["todo", "queued", "in-progress"].contains(&role) {
            reasons.push("status role todo/queued/in-progress");
        }
        if delivered.contains(&item.url) {
            reasons.push("a task's `delivers` names it");
        }
        if item.comments.iter().any(|comment| {
            comment_layout(&comment.body).0 == CommentLayout::Unmarked
                && comment.created_at.is_some_and(|at| at > window)
        }) {
            reasons.push("a person's comment in the window");
        }
        if reasons.is_empty() {
            closes.push(item.url.clone());
        } else {
            exempt_urls.insert(item.url.clone());
            for reason in &reasons {
                *exempt_by.entry(reason).or_default() += 1;
            }
            *exempt_first.entry(reasons[0]).or_default() += 1;
        }
    }
    let closed_now: BTreeSet<&String> = closes.iter().collect();

    // Migration writes, over the open tickets close-stale leaves.
    let left: Vec<&Item> = items.iter().filter(|item| item.open && !closed_now.contains(&item.url)).collect();
    let mut migrate_tally = Tally::default();
    for item in &left {
        let (_, _, problems) = &outcomes[&item.url];
        migrate_tally.add(true, problems, &item.url);
    }
    let plans: Vec<(usize, Plan)> = (0..scenarios.len())
        .map(|at| (at, plan_writes(&left, &scenario_ok[at], &comment_edits_by_item)))
        .collect();
    let plan = &plans[0].1;
    let closure_writes = closes.len() * 3;
    let migration_writes = plan.writes();

    // The report.
    let mut out = String::new();
    let _ = writeln!(out, "# spike-legacy-formats measurement\n");
    let _ = writeln!(out, "As of {as_of} (the 14-day window opens {window}); {} board items read from the 2026-11-18 dump, comment times and the long item's comments from `extras.json` (read {}).\n", items.len(), extras["read_at"].as_str().unwrap_or("?"));

    let _ = writeln!(out, "## Ticket formats and the mapping\n");
    let mut rows = Vec::new();
    for ((schema, layout), tally) in &formats {
        rows.push(vec![
            schema.clone(),
            layout.clone(),
            tally.total.to_string(),
            format!("{} / {}", tally.open, tally.total - tally.open),
            format!("{} ({} open)", tally.clean, tally.open_clean),
            tally.classes.iter().map(|(class, (count, link))| format!("{count} × {class} (e.g. {})", short(link))).collect::<Vec<_>>().join("; "),
        ]);
    }
    rows.sort_by(|a, b| b[2].parse::<usize>().unwrap_or(0).cmp(&a[2].parse::<usize>().unwrap_or(0)));
    table(&mut out, &["Record schema", "Body layout", "Tickets", "Open / closed", "Map and validate", "Failure classes (tickets, example)"], &rows);
    let mut rows = Vec::new();
    for (schema, tally) in &by_schema {
        rows.push(vec![schema.clone(), tally.total.to_string(), format!("{} ({} open)", tally.clean, tally.open_clean)]);
    }
    table(&mut out, &["Record schema", "Tickets", "Map and validate"], &rows);
    let total_clean: usize = by_schema.values().map(|tally| tally.clean).sum();
    let total_open_clean: usize = by_schema.values().map(|tally| tally.open_clean).sum();
    let _ = writeln!(out, "All: {total_clean} of {} tickets map and validate ({total_open_clean} of {open_total} open). Tickets failing on `witnesses` alone: {witnesses_only} ({witnesses_only_open} open).\n", items.len());
    let _ = writeln!(out, "Tickets that map and validate under each rule set (all / open):\n");
    let schemas: BTreeSet<String> = scenario_clean.keys().map(|(_, schema)| schema.clone()).collect();
    let mut header = vec!["Rule set".to_owned()];
    header.extend(schemas.iter().cloned());
    header.push("All".to_owned());
    header.push("Open".to_owned());
    header.push("Open, left after closures".to_owned());
    let mut rows = Vec::new();
    for (at, (name, _, _)) in scenarios.iter().enumerate() {
        let mut row = vec![(*name).to_owned()];
        let mut all = 0;
        for schema in &schemas {
            let (total, clean) = scenario_clean.get(&(at, schema.clone())).copied().unwrap_or_default();
            all += clean;
            row.push(format!("{clean} / {total}"));
        }
        row.push(all.to_string());
        let open_clean = items.iter().filter(|item| item.open && scenario_ok[at].get(&item.url).is_some_and(Result::is_ok)).count();
        let left_clean = left.iter().filter(|item| scenario_ok[at].get(&item.url).is_some_and(Result::is_ok)).count();
        row.push(open_clean.to_string());
        row.push(format!("{left_clean} / {}", left.len()));
        rows.push(row);
    }
    let header: Vec<&str> = header.iter().map(String::as_str).collect();
    table(&mut out, &header, &rows);
    let _ = writeln!(out, "Not idempotent (the rendered body, mapped again, renders differently), of the {} validating under the decided checks: {} {:?}\n", total_clean, not_idempotent.len(), not_idempotent.iter().take(10).collect::<Vec<_>>());
    let structural_at = scenarios.len() - 1;
    let mut structural_checked = 0usize;
    let mut structural_drift: Vec<String> = Vec::new();
    for item in &items {
        if let Some(Ok((_, body, record, _))) = scenario_ok[structural_at].get(&item.url) {
            structural_checked += 1;
            let again = map_ticket(item, body, &serde_json::from_str(&record.render()).expect("record"), Variant::default())
                .and_then(|mapped| structural.render(&mapped));
            if !matches!(again, Ok((twice, _, _)) if twice == *body) {
                structural_drift.push(short(&item.url));
            }
        }
    }
    let _ = writeln!(out, "Not idempotent, of the {structural_checked} validating without `evidence-prose` and `witnesses`: {} {:?}\n", structural_drift.len(), structural_drift.iter().take(10).collect::<Vec<_>>());
    let _ = writeln!(out, "Current-shape tickets whose mapped render differs from the board body (whitespace aside): {} {:?}\n", current_changed.len(), current_changed.iter().take(10).collect::<Vec<_>>());
    let _ = writeln!(out, "What the decided mapping did, over every ticket it maps (checks aside):\n");
    let rows: Vec<Vec<String>> = notes.iter().map(|(note, (count, open))| vec![note.clone(), count.to_string(), open.to_string()]).collect();
    table(&mut out, &["Note", "Tickets", "Open"], &rows);
    let _ = writeln!(out, "Title refusals: {}\n", title_refusals.len());
    for (link, problem) in title_refusals.iter().take(40) {
        let _ = writeln!(out, "- {link}: {problem}");
    }
    out.push('\n');

    let _ = writeln!(out, "## Comment layouts and the evidence-comment mapping\n");
    let mut rows = Vec::new();
    for (layout, (count, open)) in &comment_layouts {
        let tally = comment_tally.get(layout);
        rows.push(vec![
            layout.label().to_owned(),
            count.to_string(),
            open.to_string(),
            tally.map_or("never touched".to_owned(), |tally| format!("{} ({} on open tickets)", tally.clean, tally.open_clean)),
            tally.map_or(String::new(), |tally| tally.classes.iter().map(|(class, (count, link))| format!("{count} × {class} (e.g. {})", link.replace("https://github.com/nickderobertis/", ""))).collect::<Vec<_>>().join("; ")),
        ]);
    }
    table(&mut out, &["Layout", "Comments", "On open tickets", "Map and validate", "Failure classes (comments, example)"], &rows);
    let _ = writeln!(out, "0.5-layout comments that map but keep a value the sources hold in a prose sentence: {comment_leaked}\n");
    let _ = writeln!(out, "0.5-layout comments that map only by taking the basis origin from the ticket's basis (the comment names a commit under a bare repository name, or under no origin while the ticket has one): {comment_fallback}\n");
    let _ = writeln!(out, "Marked notes no deterministic rule maps:\n");
    for (link, layout) in &unmappable_notes {
        let _ = writeln!(out, "- {} — {layout}", link.replace("https://github.com/nickderobertis/", ""));
    }
    out.push('\n');

    let _ = writeln!(out, "## Closures at {STALE_DAYS} days\n");
    let _ = writeln!(out, "- Open tickets: {open_total}");
    let _ = writeln!(out, "- `verified_at` alone {STALE_DAYS}+ days old: {verified_only_stale}");
    let _ = writeln!(out, "- Newest evidence (later of `verified_at` and newest evidence comment) {STALE_DAYS}+ days old: {stale}; fresher: {fresh}; no evidence time: {no_time}");
    let _ = writeln!(out, "- Exempt: {} — by reason (a ticket may hold several): {:?}; by first reason: {:?}", exempt_urls.len(), exempt_by, exempt_first);
    let delivered_open = items.iter().filter(|item| item.open && delivered.contains(&item.url)).count();
    let _ = writeln!(out, "- Tickets a task's `delivers` names in the local plan store: {} ({delivered_open} open, none of them stale unless counted above)", delivered.len());
    let _ = writeln!(out, "- Close-stale closes: {}", closes.len());
    let _ = writeln!(out, "- Open tickets left to migrate: {}\n", open_total - closes.len());
    let _ = writeln!(out, "Sample closures: {:?}\n", closes.iter().take(8).map(|one| short(one)).collect::<Vec<_>>());

    let _ = writeln!(out, "## Writes\n");
    let left_count = open_total - closes.len();
    let _ = writeln!(out, "Of the {left_count} open tickets left, {} map and validate (the rest are skipped and written nothing). Their failure classes:\n", migrate_tally.clean);
    let rows: Vec<Vec<String>> = migrate_tally.classes.iter().map(|(class, (count, link))| vec![class.clone(), count.to_string(), short(link)]).collect();
    table(&mut out, &["Failure class", "Open tickets left", "Example"], &rows);
    for (at, (name, _, _)) in scenarios.iter().enumerate().skip(1) {
        let mut tally = Tally::default();
        for item in &left {
            let problems = match &scenario_ok[at][&item.url] {
                Ok(_) => Vec::new(),
                Err(problems) => problems.clone(),
            };
            tally.add(true, &problems, &item.url);
        }
        let _ = writeln!(out, "Under \"{name}\", {} of the {} left map and validate; the rest:\n", tally.clean, left.len());
        let rows: Vec<Vec<String>> = tally.classes.iter().map(|(class, (count, link))| vec![class.clone(), count.to_string(), short(link)]).collect();
        table(&mut out, &["Failure class", "Open tickets left", "Example"], &rows);
    }
    let _ = writeln!(out, "Closure writes: {} closures × 3 (marked comment, Status field, issue state) = {closure_writes}.\n", closes.len());
    // The projection rule, held to every value the board already carries.
    let mut agree = 0usize;
    let mut disagree: Vec<String> = Vec::new();
    for item in &items {
        let Some(record) = &item.record else { continue };
        for (field, key) in METADATA_FIELDS {
            if let (Some(board), Some(projected)) = (item.fields.get(*field), field_value(record, key)) {
                if *board == projected {
                    agree += 1;
                } else {
                    disagree.push(format!("{} {field}", short(&item.url)));
                }
            }
        }
    }
    let _ = writeln!(out, "The projection rule (string as is, list one entry a line in order, map `key: value` a line sorted by key) against every projected value already on the board: {agree} agree, {} differ {:?}.\n", disagree.len(), disagree.iter().take(10).collect::<Vec<_>>());
    let mut rows = Vec::new();
    for (at, plan) in &plans {
        let writes = plan.writes();
        rows.push(vec![
            format!("{}{}", if *at == 0 { "DECIDED: " } else { "COUNTERFACTUAL: " }, scenarios[*at].0),
            plan.tickets.to_string(),
            plan.body_writes.to_string(),
            format!("{} {:?}", plan.fields_new, plan.fields_new_by),
            plan.fields_changed.to_string(),
            plan.priority_writes.to_string(),
            plan.comment_edits.to_string(),
            format!("{writes} ({:.2} a ticket)", writes as f64 / plan.tickets.max(1) as f64),
            (writes + closure_writes).to_string(),
            format!("{:.1}", (writes + closure_writes) as f64 / WRITES_PER_HOUR),
        ]);
    }
    table(&mut out, &["Rule set", "Tickets migrated", "Body updates", "Field values newly set (by field)", "Field values changed", "Priority changes", "Evidence-comment edits", "Migration writes", "With closures", "Hours at 250/h"], &rows);
    // The per-ticket inventory of each rule set, reconciled with its aggregate row above.
    let mut unreconciled = false;
    for (at, plan) in &plans {
        let label = if *at == 0 { "DECIDED" } else { "COUNTERFACTUAL" };
        let name = format!("inventory-{at}.md");
        let mut text = format!("# Per-ticket writes — {label}: {}\n\n", scenarios[*at].0);
        let mut jsonl = String::new();
        for row in &plan.rows {
            let _ = writeln!(text, "## {}\n\n- writes: {}", short(&row.url), row.writes());
            let _ = writeln!(text, "- body and record update: 1");
            for (field, value) in &row.fields_new {
                let _ = writeln!(text, "- set `{field}`:\n\n````text\n{value}\n````");
            }
            for (field, board, new) in &row.fields_changed {
                let _ = writeln!(text, "- change `{field}` from `{board}` to `{new}`");
            }
            match &row.priority {
                Some((board, new)) => { let _ = writeln!(text, "- Priority: {board} → {new}"); }
                None => { let _ = writeln!(text, "- Priority: unchanged"); }
            }
            let _ = writeln!(text, "- evidence comments edited: {}\n", if row.comment_ids.is_empty() { "none".to_owned() } else { row.comment_ids.join(", ") });
            jsonl.push_str(&serde_json::to_string(row).expect("json"));
            jsonl.push('\n');
        }
        let verdict = reconcile(plan);
        let line = match &verdict { Ok(line) => format!("reconciled: {line}"), Err(line) => { unreconciled = true; format!("DOES NOT RECONCILE: {line}") } };
        let _ = writeln!(text, "{line}");
        std::fs::write(out_dir.join(&name), text).expect("writes");
        std::fs::write(out_dir.join(format!("inventory-{at}.jsonl")), jsonl).expect("writes");
        let _ = writeln!(out, "- {label} `{}` — `{name}` (+ `.jsonl`), {} rows; {line}", scenarios[*at].0, plan.rows.len());
    }
    out.push('\n');
    if unreconciled {
        eprintln!("the per-ticket inventory does not reconcile with the aggregate; see report.md");
        std::fs::write(out_dir.join("report.md"), &out).expect("writes");
        std::process::exit(1);
    }
    let total = migration_writes + closure_writes;

    let _ = writeln!(out, "## Edit histories\n");
    for history in extras["history"].as_array().into_iter().flatten() {
        let _ = writeln!(out, "- {}: {} revisions, {} with a record, schemas {}; revisions holding `draft_sources`/`transcripts`/`witnesses`: {}", short(history["url"].as_str().unwrap_or_default()), history["revisions"], history["with_record"], history["schemas"], history["holding"]);
    }
    let _ = writeln!(out, "\nGraphQL points the spike's reads spent: {}\n", extras["graphql_points"]);

    // Before and after.
    let _ = writeln!(out, "## Before and after\n");
    let trial = |path: &str| items.iter().find(|item| item.url.ends_with(path));
    for (name, path) in [("onepipeline-823", "onepipeline/issues/823"), ("ai-orchestrator-1796", "ai-orchestrator/issues/1796")] {
        let item = trial(path).expect("a trial ticket is on the board");
        let (_, ok, problems) = &outcomes[&item.url];
        let before = format!("{}\n", item.visible);
        std::fs::write(out_dir.join("before-after").join(format!("{name}-before.md")), &before).expect("writes");
        match ok {
            Some((mapped, body, record, level)) => {
                std::fs::write(out_dir.join("before-after").join(format!("{name}-after.md")), body).expect("writes");
                std::fs::write(out_dir.join("before-after").join(format!("{name}-after-record.json")), record.render()).expect("writes");
                let _ = writeln!(out, "- {name}: maps and validates; {} unit(s); estimate {level}; notes {:?}", mapped.units, mapped.notes);
            }
            None => {
                let units = item
                    .record
                    .as_ref()
                    .and_then(|record| map_ticket(item, &item.visible, record, Variant::default()).ok())
                    .map_or(0, |mapped| mapped.units);
                let _ = writeln!(out, "- {name}: FAILS ({units} unit(s)): {problems:?}");
                // What the mapping renders, though the checks refuse it, so the plan can read it.
                if let Some(Ok((body, record, _))) = item.record.as_ref().map(|record| {
                    map_ticket(item, &item.visible, record, Variant::default()).and_then(|mapped| env.render(&mapped))
                }) {
                    std::fs::write(out_dir.join("before-after").join(format!("{name}-after-refused.md")), body).expect("writes");
                    std::fs::write(out_dir.join("before-after").join(format!("{name}-after-refused-record.json")), record.render()).expect("writes");
                }
                for (at, (scenario, _, _)) in scenarios.iter().enumerate() {
                    let _ = writeln!(out, "  - under \"{scenario}\": {}", if scenario_ok[at][&item.url].is_ok() { "maps and validates" } else { "fails" });
                }
            }
        }
    }
    if let Some((before, after)) = renders.get("IC_kwDOTx8aUc8AAAABhjme-g") {
        std::fs::write(out_dir.join("before-after").join("onepipeline-774-comment-before.md"), before).expect("writes");
        std::fs::write(out_dir.join("before-after").join("onepipeline-774-comment-after.md"), after).expect("writes");
        let _ = writeln!(out, "- onepipeline#774 comment IC_kwDOTx8aUc8AAAABhjme-g: maps and validates");
    } else {
        let _ = writeln!(out, "- onepipeline#774 comment IC_kwDOTx8aUc8AAAABhjme-g: FAILS");
    }
    // Substitutes of each kind, should a trial ticket fail: open, left after the closures, with a
    // mapped 0.5 evidence comment and a person's comment.
    for (at, (scenario, _, _)) in scenarios.iter().enumerate() {
        for several in [false, true] {
            let found: Vec<String> = left
                .iter()
                .filter(|item| {
                    let Some(Ok((mapped, ..))) = scenario_ok[at].get(&item.url) else {
                        return false;
                    };
                    (if several { mapped.units >= 2 } else { mapped.units == 1 })
                        && item.comments.iter().any(|comment| renders.contains_key(&comment.id) && comment_layout(&comment.body).0 == CommentLayout::Evidence05)
                        && item.comments.iter().any(|comment| comment_layout(&comment.body).0 == CommentLayout::Unmarked)
                })
                .map(|item| short(&item.url))
                .collect();
            let _ = writeln!(out, "- substitutes under \"{scenario}\" with {}, a mapped evidence comment and a person's comment: {} {:?}", if several { "two or more units" } else { "one unit" }, found.len(), found.iter().take(6).collect::<Vec<_>>());
        }
        let several: Vec<String> = left
            .iter()
            .filter(|item| {
                matches!(scenario_ok[at].get(&item.url), Some(Ok((mapped, ..))) if mapped.units >= 2)
                    && item.comments.iter().any(|comment| renders.contains_key(&comment.id) && comment_layout(&comment.body).0 == CommentLayout::Evidence05)
            })
            .map(|item| short(&item.url))
            .collect();
        let _ = writeln!(out, "  - with two or more units and a mapped evidence comment (#823 carries the person's comment): {} {:?}", several.len(), several.iter().take(6).collect::<Vec<_>>());
    }
    // The multi-unit substitute's before and after under the decided mapping (the last rule set
    // keeps its prose as the board holds it), else the first rule set that validates it.
    if let Some(item) = trial("oneharness-ui/issues/82") {
        std::fs::write(out_dir.join("before-after").join("oneharness-ui-82-before.md"), format!("{}\n", item.visible)).expect("writes");
        for (at, (scenario, _, _)) in scenarios.iter().enumerate().rev() {
            if let Ok((mapped, body, record, level)) = &scenario_ok[at][&item.url] {
                std::fs::write(out_dir.join("before-after").join("oneharness-ui-82-after.md"), body).expect("writes");
                std::fs::write(out_dir.join("before-after").join("oneharness-ui-82-after-record.json"), record.render()).expect("writes");
                let _ = writeln!(out, "- oneharness-ui#82 validates under \"{scenario}\": {} units; estimate {level}; notes {:?}", mapped.units, mapped.notes);
                break;
            }
        }
        for comment in &item.comments {
            if let Some((before, after)) = renders.get(&comment.id)
                && comment_layout(before).0 == CommentLayout::Evidence05
            {
                std::fs::write(out_dir.join("before-after").join("oneharness-ui-82-comment-before.md"), before).expect("writes");
                std::fs::write(out_dir.join("before-after").join("oneharness-ui-82-comment-after.md"), after).expect("writes");
                let _ = writeln!(out, "- oneharness-ui#82 evidence comment {} maps and validates", comment.id);
                break;
            }
        }
    }
    let _ = writeln!(out, "\nThe six provenance-lost tickets of ai-orchestrator#2548:\n");
    for path in ["onetaskgraph/issues/3696", "onepipeline/issues/1319", "onepipeline/issues/820", "onepipeline/issues/648", "ai-orchestrator/issues/2319", "ai-orchestrator/issues/1886"] {
        let Some(item) = trial(path) else { continue };
        let verdicts: Vec<String> = scenarios
            .iter()
            .enumerate()
            .map(|(at, _)| match &scenario_ok[at][&item.url] {
                Ok(_) => "validates".to_owned(),
                Err(problems) => format!("fails: {}", problems.iter().map(|one| failure_class(one)).collect::<Vec<_>>().join("; ")),
            })
            .collect();
        let _ = writeln!(out, "- {} ({}, {}): {}", short(&item.url), if item.open { "open" } else { "closed" }, if closed_now.contains(&item.url) { "close-stale closes it" } else { "left" }, verdicts.join(" | "));
    }
    out.push('\n');

    std::fs::write(out_dir.join("report.md"), &out).expect("writes");
    let summary = json!({
        "as_of": as_of.to_rfc3339(),
        "items": items.len(),
        "open": open_total,
        "clean": total_clean,
        "open_clean": total_open_clean,
        "stale": stale,
        "exempt": exempt_urls.len(),
        "closes": closes.len(),
        "left_to_migrate": open_total - closes.len(),
        "migrated": plan.tickets,
        "migration_writes": migration_writes,
        "closure_writes": closure_writes,
        "hours": total as f64 / WRITES_PER_HOUR,
        "not_idempotent": not_idempotent.len(),
    });
    std::fs::write(out_dir.join("summary.json"), serde_json::to_string_pretty(&summary).expect("json") + "\n").expect("writes");
    print!("{out}");
}
