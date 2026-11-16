//! A batch's ticket: the Markdown file the verifying agent writes under
//! `tasks/<batch>/tickets/<root-cause>.md`, held to its kind's ticket template, the rubric and
//! the ticket record's format, and what the loop derives from it — its priority estimate,
//! its occurrences and its copy digest.
//!
//! The file is front matter and a body:
//!
//! ```text
//! ---
//! title: "widgets: a layout save reads one window size"
//! status: proposal            # optional: a status role; `cancelled` withdraws the ticket
//! metadata:
//!   orchestrator.follow-up: {"schema": 14, "root_cause": "…", …}
//! ---
//! ## Root cause
//! …
//! ```
//!
//! The record's `priority_estimate` and `board_estimate` are the tool's to write: a record
//! without a `priority_estimate` is read as estimating `low` until `publish` writes the one
//! its facts estimate.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use indexmap::IndexMap;

use crate::config::{
    Check, Config, Kind, Priority, PriorityModel, Rubric, StatusRole, TitlePrefix,
};
use crate::formats::markers::CommentMarker;
use crate::formats::ticket::TicketRecord;
use crate::formats::{Frequency, Level, Verdict, Versioned};
use crate::verify::render::{self, Common, TicketData};
use crate::verify::template::{self, Located};

/// The longest title a draft carries, and the `title` check's default limit.
pub const TITLE_LIMIT: usize = 120;

/// A root cause's slug, which names the ticket's file.
pub static SLUG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("valid"));
/// A normalized repository origin, `host/owner/name`.
pub static ORIGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9.-]*(?:/[A-Za-z0-9._-]+)+$").expect("valid"));
/// A commit a claim was verified at.
static COMMIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{40}$").expect("valid"));
/// A host label, as `hostname` prints one.
static HOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*$")
        .expect("valid")
});
/// A level-2 heading.
static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^##[ \t]+(?P<name>.*?)[ \t]*$").expect("valid"));
/// A fence line.
static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})").expect("valid"));
/// A labelled line, `- Label: value` or Linear's `* Label: value`.
static LABELLED_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-*] (?P<label>[^:\n]+):(?P<value>[^\n]*)$").expect("valid"));
/// A level-3 heading, as a unit subsection opens.
static SUBHEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^###(?:[ \t][^\n]*)?$").expect("valid"));
/// A unit subsection's heading: the unit, an em dash, its repository, and the package or path
/// only when the unit is part of a repository.
static UNIT_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^### (?P<unit>\S[^\n]*?) — `(?P<repository>[^`\n]+)`(?: \(`[^`\n]+`\))?[ \t]*$")
        .expect("valid")
});
/// One contributing location: its repository, its path in backticks, then what it does.
static LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^- [^`\n]*\S[ \t]+`[^`\n]+`:[ \t]*\S").expect("valid"));
/// A `Host:` or `Basis:` line, which the record holds.
static METADATA_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[ \t]*(?:[-*+][ \t]+)?(?:\*\*)?(?P<label>Host|Basis)(?:\*\*)?:").expect("valid")
});
/// A 40-character commit standing alone.
static BARE_COMMIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[^0-9a-f])[0-9a-f]{40}(?:[^0-9a-f]|$)").expect("valid"));

/// The section 0.5.0 placed the estimate line in, after its severity with the workaround: the
/// one fixed place `publish` puts a line into an authored body that carries none, when no
/// `render_template` renders the body.
const LEGACY_IMPACT: &str = "Impact";
/// How a location bullet reads, as every refusal names it.
const LOCATION_SHAPE: &str =
    "`- <repository> `<path>`: <what this location does that breaks the guarantee>`";
/// How a unit subsection is headed, as every refusal names it.
const UNIT_SHAPE: &str = "`### <unit> — `<repository>``, with `` (`<package or path>`)`` after it when the unit is part of a repository";

/// One ticket file, read as far as it reads.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketFile {
    /// Its root cause, from its file name.
    pub slug: String,
    /// The file.
    pub path: PathBuf,
    /// Its title.
    pub title: String,
    /// The status role it asks for.
    pub status: Option<StatusRole>,
    /// Its record as written, a JSON object.
    pub record_value: Option<Map<String, Value>>,
    /// Its record, when it reads as the ticket record format.
    pub record: Option<TicketRecord>,
    /// Its body.
    pub body: String,
    /// What keeps it from reading as a ticket.
    pub problems: Vec<String>,
}

impl TicketFile {
    /// The kind it records, or the configured default.
    #[must_use]
    pub fn kind_name<'a>(&'a self, config: &'a Config) -> &'a str {
        self.record
            .as_ref()
            .and_then(|record| record.kind.as_deref())
            .unwrap_or(&config.default_kind)
    }

    /// Its kind's configuration, when the kind is configured.
    #[must_use]
    pub fn kind<'a>(&self, config: &'a Config) -> Option<&'a Kind> {
        let name = self.kind_name(config);
        config.kinds.iter().find(|kind| kind.name == name)
    }

    /// Whether it asks to be withdrawn.
    #[must_use]
    pub fn withdrawn(&self) -> bool {
        self.status == Some(StatusRole::Cancelled)
    }

    /// The value of the declared field `name` it records.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&str> {
        self.record
            .as_ref()?
            .fields
            .as_ref()?
            .get(name)
            .map(String::as_str)
    }

    /// The text of its `## <name>` section, when it has one.
    #[must_use]
    pub fn section(&self, name: &str) -> Option<String> {
        section(&self.body, name)
    }
}

/// The text of `body`'s `## <name>` section, trimmed, when it has one.
#[must_use]
pub fn section(body: &str, name: &str) -> Option<String> {
    let headings = headings(body);
    let at = headings.iter().position(|(heading, _)| heading == name)?;
    let start = headings[at].1;
    let end = headings
        .get(at + 1)
        .map_or(body.len(), |(_, offset)| *offset);
    let content = &body[start..end];
    let content = content.split_once('\n').map_or("", |(_, rest)| rest);
    Some(content.trim().to_owned())
}

/// Every level-2 heading of `body` outside a fence, with the byte offset its line starts at.
#[must_use]
pub fn headings(body: &str) -> Vec<(String, usize)> {
    let mut found = Vec::new();
    let mut fence: Option<String> = None;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if let Some(captured) = FENCE.captures(bare) {
            let marker = captured[1].to_owned();
            match &fence {
                None => fence = Some(marker),
                Some(open) if marker.starts_with(open.as_str()) => fence = None,
                Some(_) => {}
            }
        } else if fence.is_none()
            && let Some(captured) = HEADING.captures(bare)
        {
            found.push((captured["name"].to_owned(), offset));
        }
        offset += line.len();
    }
    found
}

/// Split `text` into its front matter and body.
fn front_matter(text: &str) -> Option<(&str, &str)> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

/// Read the ticket `slug`'s `text` as written.
#[must_use]
pub fn parse(slug: &str, path: &Path, text: &str, namespace: &str) -> TicketFile {
    let mut ticket = TicketFile {
        slug: slug.to_owned(),
        path: path.to_path_buf(),
        title: String::new(),
        status: None,
        record_value: None,
        record: None,
        body: String::new(),
        problems: Vec::new(),
    };
    let Some((front, body)) = front_matter(text) else {
        ticket.problems.push(
            "the file opens with no front matter: a `---` line, `title`, `metadata` and a closing `---` line"
                .to_owned(),
        );
        ticket.body = text.to_owned();
        return ticket;
    };
    ticket.body = body.trim_start_matches(['\n', '\r']).to_owned();
    let front: Value = match serde_saphyr::from_str(front) {
        Ok(Value::Object(map)) => Value::Object(map),
        Ok(_) | Err(_) => {
            ticket
                .problems
                .push("the front matter is not a YAML mapping".to_owned());
            return ticket;
        }
    };
    let Value::Object(front) = front else {
        return ticket;
    };
    for key in front.keys() {
        if !["title", "status", "metadata"].contains(&key.as_str()) {
            ticket.problems.push(format!(
                "the front matter carries `{key}`, which a ticket does not; it carries `title`, `metadata` and optionally `status`"
            ));
        }
    }
    match front.get("title") {
        Some(Value::String(title)) => ticket.title = title.clone(),
        _ => ticket
            .problems
            .push("the front matter carries no `title` string".to_owned()),
    }
    match front.get("status") {
        None | Some(Value::Null) => {}
        Some(value) => match serde_json::from_value::<StatusRole>(value.clone()) {
            Ok(role) => ticket.status = Some(role),
            Err(_) => ticket.problems.push(format!(
                "the front matter's `status` {value} is not a status role"
            )),
        },
    }
    let key = format!("{namespace}.follow-up");
    match front.get("metadata") {
        Some(Value::Object(metadata)) => {
            for other in metadata.keys().filter(|other| **other != key) {
                ticket.problems.push(format!(
                    "the metadata carries `{other}`; a ticket's metadata is its `{key}` record alone"
                ));
            }
            match metadata.get(&key) {
                Some(Value::Object(record)) => {
                    let mut record = record.clone();
                    if !record.contains_key("priority_estimate") {
                        record.insert("priority_estimate".to_owned(), Value::from("low"));
                    }
                    ticket.record_value = Some(record.clone());
                    match TicketRecord::parse(&Value::Object(record).to_string()) {
                        Ok(parsed) => ticket.record = Some(parsed),
                        Err(error) => ticket.problems.push(format!("its `{key}` record: {error}")),
                    }
                }
                _ => ticket
                    .problems
                    .push(format!("the metadata carries no `{key}` record object")),
            }
        }
        _ => ticket.problems.push(format!(
            "the front matter carries no `metadata` mapping holding its `{key}` record"
        )),
    }
    ticket
}

/// Whether `host` is a host name as `hostname` prints one.
#[must_use]
pub fn is_host(host: &str) -> bool {
    HOST.is_match(host)
}

/// Every way `basis` does not name a commit per origin.
#[must_use]
pub fn basis_problems(basis: &indexmap::IndexMap<String, String>) -> Vec<String> {
    let mut problems = Vec::new();
    for (origin, commit) in basis {
        if !ORIGIN.is_match(origin) {
            problems.push(format!(
                "the basis names `{origin}`, which is not an origin"
            ));
        }
        if !COMMIT.is_match(commit) {
            problems.push(format!(
                "the basis commit `{commit}` of `{origin}` is not a 40-character commit"
            ));
        }
    }
    problems
}

/// What a ticket is held to besides the configuration.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The batch.
    pub batch: &'a str,
    /// The batch's input set, by id; `None` when it was not prepared.
    pub inputs: Option<&'a [String]>,
    /// The transcript commands the batch's inputs name, which `## Evidence` never carries.
    pub transcripts: &'a [String],
    /// The kind's ticket template, resolved.
    pub template: &'a Located,
    /// The configured namespace, whose marker lines a ticket never carries.
    pub namespace: &'a str,
    /// What every template sees besides the ticket: the batch's data.
    pub common: Option<Common<'a>>,
    /// Whether the ticket was already copied to an item: a ticket 0.5.0 wrote may carry no
    /// `impact`, its values read from its body's labelled lines.
    pub filed: bool,
}

/// The template headings a ticket of the kind carries, each with whether it is optional.
///
/// # Errors
///
/// A template that cannot be rendered.
pub fn template_headings(
    template: &Located,
    data: minijinja::Value,
) -> Result<Vec<(String, bool)>, template::TemplateError> {
    let rendered = template::render_value(template, data)?;
    let found = headings(&rendered);
    Ok(found
        .iter()
        .map(|(name, _)| {
            let optional = section(&rendered, name)
                .is_some_and(|content| content.trim_start().starts_with("<optional"));
            (name.clone(), optional)
        })
        .collect())
}

/// Every rule `ticket` breaks; empty when it is sound. A `{command: [...]}` check is not run
/// here: it is given the rendered body, which the caller renders.
#[must_use]
pub fn problems(ticket: &TicketFile, config: &Config, context: &Context<'_>) -> Vec<String> {
    let mut problems = ticket.problems.clone();
    if !SLUG.is_match(&ticket.slug) {
        problems.push(format!(
            "the file name `{}` is not a root-cause slug: lowercase words joined by `-`",
            ticket.slug
        ));
    }
    let title = ticket.title.trim();
    if title.is_empty() && ticket.problems.is_empty() {
        problems.push("the title is empty".to_owned());
    }
    if title.chars().any(char::is_control) {
        problems.push("the title carries a control character".to_owned());
    }
    problems.extend(render::reserved_lines(
        title,
        context.namespace,
        "the title",
    ));
    problems.extend(render::reserved_lines(
        &ticket.body,
        context.namespace,
        "the body",
    ));
    let Some(record) = &ticket.record else {
        return problems;
    };
    if record.root_cause != ticket.slug {
        problems.push(format!(
            "the record's root cause `{}` is not the file's, `{}`",
            record.root_cause, ticket.slug
        ));
    }
    let origin = ORIGIN.is_match(&record.repository);
    if !origin {
        problems.push(format!(
            "the record's repository `{}` is not a normalized origin, `host/owner/name`",
            record.repository
        ));
    }
    if !record.owning_batches.iter().any(|one| one == context.batch) {
        problems.push(format!(
            "the record's `owning_runs` does not name this batch, `{}`",
            context.batch
        ));
    }
    if record.drafts.is_empty() {
        problems.push("the record names no draft or input it stands on".to_owned());
    }
    if let Some(inputs) = context.inputs {
        for draft in &record.drafts {
            if !inputs.contains(draft) {
                problems.push(format!(
                    "the record names `{draft}`, which is not one of the batch's inputs"
                ));
            }
        }
    }
    if !record.basis.contains_key(&record.repository) {
        problems.push(format!(
            "the record's basis names no commit for its repository `{}`",
            record.repository
        ));
    }
    problems.extend(basis_problems(&record.basis));
    if let Some(witnesses) = &record.witnesses {
        problems.extend(render::witness_problems(witnesses, &record.basis));
    }
    if record.projected.is_some() {
        problems.push(
            "the record carries `projected`, which onefollowup alone writes from the kind's `projections`; leave it out".to_owned(),
        );
    }
    if chrono::DateTime::parse_from_rfc3339(&record.verified_at).is_err() {
        problems.push(format!(
            "`verified_at` `{}` is not an RFC 3339 time",
            record.verified_at
        ));
    }
    if !HOST.is_match(&record.host) {
        problems.push(format!(
            "`host` `{}` is not a host name as `hostname` prints one",
            record.host
        ));
    }
    let Some(kind) = ticket.kind(config) else {
        problems.push(format!(
            "the record's kind `{}` is not a configured kind",
            ticket.kind_name(config)
        ));
        return problems;
    };
    let mut fields = Map::new();
    for (name, value) in record.fields.iter().flatten() {
        if config.fields.contains_key(name) {
            fields.insert(name.clone(), Value::from(value.as_str()));
        } else {
            problems.push(format!(
                "the ticket records the field `{name}`, which `fields` does not declare"
            ));
        }
    }
    if kind.priority == PriorityModel::Estimate {
        problems.extend(impact_problems(
            record.impact.as_ref(),
            &ticket.body,
            &config.rubric,
            context.filed,
        ));
    }
    for check in kind.checks() {
        match check {
            Check::Headings => {
                let mut held = record_value(record);
                if let Value::Object(map) = &mut held {
                    if fields.is_empty() {
                        map.shift_remove("fields");
                    } else {
                        map.insert("fields".to_owned(), Value::Object(fields.clone()));
                    }
                }
                let data = match context.common {
                    Some(common) => Common { kind, ..common }.ticket(&TicketData {
                        title: &ticket.title,
                        slug: &ticket.slug,
                        body: &ticket.body,
                        record: &held,
                        estimate: None,
                    }),
                    None => minijinja::Value::from_serialize(
                        serde_json::json!({"rubric": config.rubric, "fields": fields}),
                    ),
                };
                match template_headings(context.template, data) {
                    Ok(expected) => problems.extend(body_problems(&ticket.body, &expected)),
                    Err(error) => problems.push(error.to_string()),
                }
            }
            Check::Title { prefix, limit } => {
                if title.chars().count() > limit {
                    problems.push(format!("the title is longer than {limit} characters"));
                }
                let name = record.repository.rsplit('/').next().unwrap_or_default();
                if prefix == TitlePrefix::RepositoryName
                    && origin
                    && !title.starts_with(&format!("{name}: "))
                {
                    problems.push(format!(
                        "the title does not open with `{name}: `, the name of the repository its item is filed in"
                    ));
                }
            }
            Check::Impact { section } => {
                problems.extend(impact_part_problems(&ticket.body, &section, &config.rubric));
            }
            Check::FixUnits { section, locations } => {
                problems.extend(fix_problems(&ticket.body, &section, &locations));
            }
            Check::EvidenceProse { section } => {
                problems.extend(evidence_problems(
                    &ticket.body,
                    &section,
                    &config.drafts_source,
                    record,
                    context.transcripts,
                ));
            }
            Check::Witnesses { required } => {
                if required && record.witnesses.as_ref().is_none_or(Vec::is_empty) {
                    problems.push(
                        "the record carries no `witnesses`: name at least one `<origin> <path>:<line> at <commit>` where the claim was seen, at its basis commit".to_owned(),
                    );
                }
            }
            Check::Command(_) => {}
        }
    }
    problems
}

/// What `body` breaks of the headings its template states, in order.
fn body_problems(body: &str, expected: &[(String, bool)]) -> Vec<String> {
    let mut problems = Vec::new();
    let found = headings(body);
    let mut at = 0;
    for (name, _) in &found {
        match expected[at.min(expected.len())..]
            .iter()
            .position(|(wanted, _)| wanted == name)
        {
            Some(offset) => {
                for (skipped, optional) in &expected[at..at + offset] {
                    if !optional {
                        problems.push(format!(
                            "the body is missing the `## {skipped}` section its ticket template requires, before `## {name}`"
                        ));
                    }
                }
                at += offset + 1;
            }
            None => problems.push(if expected.iter().any(|(wanted, _)| wanted == name) {
                format!("`## {name}` is out of the order its ticket template states")
            } else {
                format!("`## {name}` is not a section its ticket template has")
            }),
        }
    }
    for (missing, optional) in &expected[at.min(expected.len())..] {
        if !optional {
            problems.push(format!(
                "the body is missing the `## {missing}` section its ticket template requires"
            ));
        }
    }
    for (name, _) in &found {
        if section(body, name).is_some_and(|content| content.is_empty()) {
            problems.push(format!("`## {name}` is empty"));
        }
    }
    problems
}

/// `text` with every fenced line blanked to spaces, so an offset in it is the same in `text`.
fn unfenced(text: &str) -> String {
    let mut fence: Option<String> = None;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        let inside = if let Some(captured) = FENCE.captures(bare) {
            let marker = captured[1].to_owned();
            match &fence {
                None => fence = Some(marker),
                Some(open) if marker.starts_with(open.as_str()) => fence = None,
                Some(_) => {}
            }
            true
        } else {
            fence.is_some()
        };
        if inside {
            for one in line.chars() {
                if one == '\n' {
                    out.push('\n');
                } else {
                    out.extend(std::iter::repeat_n(' ', one.len_utf8()));
                }
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The legacy read of a body's impact values, for a record carrying no `impact` — every ticket
/// 0.5.0 wrote: each rubric impact line's `- <label>: <value>` (or Linear's `* `), wherever it
/// sits outside fenced code, a collapsed block's content included. `None` when a label is
/// missing or repeated, or a severity value is no rubric level: the lines cannot be read.
#[must_use]
pub fn legacy_impact(body: &str, rubric: &Rubric) -> Option<IndexMap<String, String>> {
    let mut found: IndexMap<String, Vec<String>> = IndexMap::new();
    for line in unfenced(body).lines() {
        if let Some(captured) = LABELLED_LINE.captures(line.trim_end()) {
            let label = captured["label"].trim();
            if rubric.impact_lines.iter().any(|wanted| wanted == label) {
                found
                    .entry(label.to_owned())
                    .or_default()
                    .push(captured["value"].trim().to_owned());
            }
        }
    }
    let mut values = IndexMap::new();
    for label in &rubric.impact_lines {
        match found.get(label).map(Vec::as_slice) {
            Some([value]) => {
                values.insert(label.clone(), value.clone());
            }
            _ => return None,
        }
    }
    let severities = [rubric.impact_lines.first(), rubric.impact_lines.last()];
    severities
        .into_iter()
        .flatten()
        .all(|label| level_index(rubric, &values[label]).is_some())
        .then_some(values)
}

/// A ticket's impact values: its record's `impact`, or for a record carrying none, the legacy
/// read of its body.
#[must_use]
pub fn impact_values(
    impact: Option<&IndexMap<String, String>>,
    body: &str,
    rubric: &Rubric,
) -> Option<IndexMap<String, String>> {
    impact.cloned().or_else(|| legacy_impact(body, rubric))
}

/// One line 0.5.0 read from `body`'s `## Impact`: the first `- <label>:` line there.
fn legacy_line(body: &str, label: &str) -> Option<String> {
    section(body, LEGACY_IMPACT)?.lines().find_map(|line| {
        LABELLED_LINE
            .captures(line.trim_end())
            .filter(|captured| captured["label"].trim() == label)
            .map(|captured| captured["value"].trim().to_owned())
    })
}

/// The rubric severity a ticket records, by the rubric's first impact line: `impact`'s, or for a
/// record carrying none, its body's.
#[must_use]
pub fn severity(
    impact: Option<&IndexMap<String, String>>,
    body: &str,
    rubric: &Rubric,
) -> Option<String> {
    let label = rubric.impact_lines.first()?;
    match impact_values(impact, body, rubric) {
        Some(values) => values.get(label).cloned(),
        None => legacy_line(body, label),
    }
}

/// The severity with the workaround a ticket records, by the rubric's last impact line:
/// `impact`'s, or for a record carrying none, its body's, read as 0.5.0 read it where the
/// legacy read cannot read the body.
#[must_use]
pub fn with_workaround(
    impact: Option<&IndexMap<String, String>>,
    body: &str,
    rubric: &Rubric,
) -> Option<String> {
    let label = rubric.impact_lines.last()?;
    match impact_values(impact, body, rubric) {
        Some(values) => values.get(label).cloned(),
        None => legacy_line(body, label),
    }
}

fn level_index(rubric: &Rubric, name: &str) -> Option<usize> {
    rubric.levels.iter().position(|level| level.name == name)
}

fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|one| format!("`{one}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What an estimated ticket's impact values break of the rubric: its record's `impact`, which a
/// ticket this release writes carries, or — for a ticket already `filed` whose record carries
/// none, as 0.5.0 wrote it — the legacy read of its body.
fn impact_problems(
    impact: Option<&IndexMap<String, String>>,
    body: &str,
    rubric: &Rubric,
    filed: bool,
) -> Vec<String> {
    let labels: Vec<&str> = rubric.impact_lines.iter().map(String::as_str).collect();
    let values = match impact {
        Some(values) => values.clone(),
        None => match legacy_impact(body, rubric).filter(|_| filed) {
            Some(values) => values,
            None => {
                return vec![format!(
                    "the record carries no `impact`: it records each of the rubric's impact lines {} with its value, `impact: {{<label>: <value>}}`, which the priority is estimated from",
                    quoted(&labels)
                )];
            }
        },
    };
    let mut problems = Vec::new();
    for label in &labels {
        if !values.contains_key(*label) {
            problems.push(format!(
                "the record's `impact` carries no `{label}`; it records each of the rubric's impact lines {}",
                quoted(&labels)
            ));
        }
    }
    for label in values.keys() {
        if !labels.contains(&label.as_str()) {
            problems.push(format!(
                "the record's `impact` carries `{label}`, which is not one of the rubric's impact lines {}",
                quoted(&labels)
            ));
        }
    }
    let names: Vec<&str> = rubric
        .levels
        .iter()
        .map(|level| level.name.as_str())
        .collect();
    let (Some(first), Some(last)) = (labels.first(), labels.last()) else {
        return problems;
    };
    let mut levels = Vec::new();
    for label in [*first, *last] {
        let Some(value) = values.get(label) else {
            continue;
        };
        match level_index(rubric, value) {
            Some(at) => levels.push(at),
            None => problems.push(format!(
                "the record's `impact` gives `{label}` as `{value}`, which is not one of the rubric's levels {}",
                quoted(&names)
            )),
        }
    }
    if !problems.is_empty() {
        return problems;
    }
    let (raw, left) = (levels[0], levels[levels.len() - 1]);
    let severity = &values[*first];
    let mitigated = &values[*last];
    let workaround = labels
        .get(1)
        .filter(|_| labels.len() > 2)
        .and_then(|label| values.get(*label))
        .map(String::as_str);
    if workaround.is_some_and(str::is_empty) {
        problems.push(format!(
            "the record's `impact` gives `{}` empty: the workaround in place, or `{}`",
            labels[1], rubric.no_workaround
        ));
    }
    if left < raw {
        problems.push(format!(
            "the severity with the workaround, `{mitigated}`, is above the severity, `{severity}`"
        ));
    } else if workaround == Some(rubric.no_workaround.as_str()) && left != raw {
        problems.push(format!(
            "with a workaround of `{}` the severity with the workaround is the severity, `{severity}`",
            rubric.no_workaround
        ));
    } else if workaround.is_some_and(|one| one != rubric.no_workaround)
        && left == raw
        && raw + 1 < rubric.levels.len()
    {
        problems.push(format!(
            "an acceptable workaround lowers the severity `{severity}` at least one level; a workaround too costly to lower it is `{}`",
            rubric.no_workaround
        ));
    }
    problems
}

/// What `body`'s `## <name>` breaks of the rubric's labelled parts, each once, in order.
fn impact_part_problems(body: &str, name: &str, rubric: &Rubric) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(content) = section(body, name) else {
        return vec![format!(
            "the body has no `## {name}` section, which carries the rubric's labelled parts"
        )];
    };
    let mut from = 0;
    for part in &rubric.impact_parts {
        let label = format!("**{part}.**");
        match content[from..].find(&label) {
            Some(at) => from += at + label.len(),
            None => problems.push(format!(
                "`## {name}` does not carry its `{label}` part, in the rubric's order"
            )),
        }
    }
    problems
}

/// What `body`'s `## <name>` breaks of a fix's form: a non-empty `**<locations>.**` part of one
/// bullet per location, then its unit subsections. With exactly one unit nothing but that part
/// precedes it; with two or more an overview paragraph opens the section.
fn fix_problems(body: &str, name: &str, locations: &str) -> Vec<String> {
    let Some(content) = section(body, name) else {
        return vec![format!(
            "the body has no `## {name}` section, which carries the fix and its units"
        )];
    };
    let where_ = format!("`## {name}`");
    let part = format!("**{locations}.**");
    let plain = unfenced(&content);
    let mut offset = 0;
    let mut units: Vec<(usize, &str)> = Vec::new();
    for line in plain.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if SUBHEADING.is_match(bare) {
            units.push((offset, bare));
        }
        offset += line.len();
    }
    let Some(&(first, _)) = units.first() else {
        return vec![format!(
            "{where_} carries no unit subsection. With one unit it is the {part} part then the unit, headed {UNIT_SHAPE}, whose opening sentence or two says why the change removes the root cause; with two or more units an overview paragraph saying how they fit together comes first"
        )];
    };
    let placed = plain
        .match_indices(&part)
        .map(|(at, _)| at)
        .find(|at| *at == 0 || plain[..*at].ends_with('\n'));
    let Some(placed) = placed.filter(|at| *at < first) else {
        return vec![format!(
            "{where_} carries no {part} part before its first unit; list there every location that contributes to the root cause, one bullet each, {LOCATION_SHAPE}"
        )];
    };
    if !plain[placed + part.len()..first]
        .lines()
        .any(|line| LOCATION.is_match(line))
    {
        return vec![format!(
            "{where_}'s {part} part carries no bullet {LOCATION_SHAPE}; list every location that contributes to the root cause, one bullet each"
        )];
    }
    let mut problems = Vec::new();
    let overview = plain[..placed].trim();
    if units.len() == 1 && !overview.is_empty() {
        problems.push(format!(
            "{where_} has one unit, so nothing but its {part} part comes before it: drop the opening paragraph, and move what it says that the unit does not, such as why the change removes the root cause, into the unit's opening sentence or two"
        ));
    }
    if units.len() > 1 && overview.is_empty() {
        problems.push(format!(
            "{where_} has {} units, so it opens with an overview paragraph saying how they fit together and why the change removes the root cause, before its {part} part",
            units.len()
        ));
    }
    let ends: Vec<usize> = units
        .iter()
        .skip(1)
        .map(|(at, _)| *at)
        .chain(std::iter::once(plain.len()))
        .collect();
    for ((at, heading), end) in units.iter().zip(ends) {
        if !UNIT_HEADING.is_match(heading) {
            problems.push(format!(
                "{where_}'s subsection `{}` is not headed {UNIT_SHAPE}",
                heading.trim()
            ));
        } else if content[at + heading.len()..end].trim().is_empty() {
            problems.push(format!(
                "{where_}'s subsection `{}` is empty; say what changes in that unit and show each contract that changes",
                heading.trim()
            ));
        }
    }
    problems
}

/// What `body`'s `## <name>` carries of what the record holds: a `Host:` or `Basis:` line, a
/// qualified draft id, a 40-character commit or a transcript command.
fn evidence_problems(
    body: &str,
    name: &str,
    drafts_source: &str,
    record: &TicketRecord,
    transcripts: &[String],
) -> Vec<String> {
    let Some(content) = section(body, name) else {
        return vec![format!(
            "the body has no `## {name}` section, which says what happened"
        )];
    };
    let mut found: Vec<String> = Vec::new();
    for line in content.lines() {
        if let Some(captured) = METADATA_LINE.captures(line) {
            let phrase = format!("a `{}:` line", &captured["label"]);
            if !found.contains(&phrase) {
                found.push(phrase);
            }
        }
    }
    let draft_id = format!("{drafts_source}:");
    if record
        .drafts
        .iter()
        .any(|draft| content.contains(draft.as_str()))
        || content.split_whitespace().any(|word| {
            word.trim_start_matches(['`', '(', '['])
                .starts_with(&draft_id)
                && word.contains('/')
        })
    {
        found.push("a qualified draft id".to_owned());
    }
    if BARE_COMMIT.is_match(&content) {
        found.push("a 40-character commit".to_owned());
    }
    if record
        .transcripts
        .iter()
        .flat_map(IndexMap::values)
        .chain(transcripts)
        .any(|command| !command.trim().is_empty() && content.contains(command.trim()))
    {
        found.push("a transcript command".to_owned());
    }
    if found.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "`## {name}` carries {}, which the record holds; say there only what happened",
        found.join(", ")
    )]
}

/// A configured priority as an estimate level, `none` reading as the lowest.
#[must_use]
pub fn level_of(priority: Priority) -> Level {
    match priority {
        Priority::Urgent => Level::Urgent,
        Priority::High => Level::High,
        Priority::Medium => Level::Medium,
        Priority::Low | Priority::None => Level::Low,
    }
}

/// A level's word.
#[must_use]
pub fn level_word(level: Level) -> &'static str {
    match level {
        Level::Urgent => "urgent",
        Level::High => "high",
        Level::Medium => "medium",
        Level::Low => "low",
    }
}

/// A configured priority's word.
#[must_use]
pub fn priority_word(priority: Priority) -> &'static str {
    match priority {
        Priority::Urgent => "urgent",
        Priority::High => "high",
        Priority::Medium => "medium",
        Priority::Low => "low",
        Priority::None => "none",
    }
}

const LEVELS: [Level; 4] = [Level::Urgent, Level::High, Level::Medium, Level::Low];

/// The estimate line's label and template, as `estimate.line_label` and
/// `estimate.line_template` configure them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EstimateLine {
    /// The label the line is found by.
    pub label: String,
    /// The template it is rendered from.
    pub template: Located,
}

/// What a ticket's facts estimate: its level, and the estimate line for an estimated kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Estimated {
    /// The level.
    pub level: Level,
    /// The estimate line, for the estimate model.
    pub line: Option<String>,
    /// The severity with the workaround the ticket records, when it records one.
    pub mitigated: Option<String>,
    /// Whether it fires consistently, when judged.
    pub frequency: Option<Frequency>,
    /// The occurrences the estimate counted.
    pub occurrences: u64,
    /// How the estimate was raised, or why not, for the estimate model.
    pub raise: Option<String>,
    /// The level its related tickets' highest holds it at, above its own, when one does.
    pub held: Option<Level>,
}

/// The priority a ticket's impact values and `frequency` estimate under `kind` with
/// `occurrences` occurrences. The values are `impact`'s, or for a record carrying none (as
/// 0.5.0 wrote it), its `body`'s.
///
/// # Errors
///
/// An estimated kind's ticket recording no severity with the workaround the rubric names, or
/// an estimate line template that does not render one line opening with its label.
pub fn estimate(
    config: &Config,
    line: &EstimateLine,
    kind: &Kind,
    impact: Option<&IndexMap<String, String>>,
    body: &str,
    frequency: Option<Frequency>,
    occurrences: u64,
) -> Result<Estimated, String> {
    let mitigated = with_workaround(impact, body, &config.rubric);
    match &kind.priority {
        PriorityModel::Fixed(priority) => Ok(Estimated {
            level: level_of(*priority),
            line: None,
            mitigated,
            frequency,
            occurrences,
            raise: None,
            held: None,
        }),
        PriorityModel::Estimate => {
            let mitigated = mitigated.ok_or_else(|| {
                "the ticket records no severity with the workaround: its record carries no `impact`, and its body no line to read one from".to_owned()
            })?;
            let level = config
                .rubric
                .levels
                .iter()
                .find(|level| level.name == mitigated)
                .ok_or_else(|| {
                    format!("the severity with the workaround `{mitigated}` is no rubric level")
                })?;
            let base = level_of(level.priority);
            let at = |wanted: Level| LEVELS.iter().position(|one| *one == wanted).unwrap_or(3);
            let cap = at(level_of(config.estimate.raise_cap));
            let base_at = at(base);
            let raise_at = u64::from(config.estimate.raise_at);
            let consistent = frequency == Some(Frequency::Consistent);
            let many = occurrences >= raise_at;
            let raised = consistent || many;
            let level = if raised && base_at > cap {
                LEVELS[(base_at - 1).max(cap)]
            } else {
                base
            };
            let mut causes = Vec::new();
            if consistent {
                causes.push("it fires consistently".to_owned());
            }
            if many {
                causes.push(format!("it has {raise_at} or more occurrences"));
            }
            let held = causes.join(" and ");
            let raise = if causes.is_empty() {
                "not raised".to_owned()
            } else if base_at == 0 {
                format!("already {}, so not raised though {held}", level_word(base))
            } else if base_at <= cap {
                format!(
                    "already {}, where a raise is capped, so not raised though {held}",
                    level_word(base)
                )
            } else {
                format!("raised one level because {held}")
            };
            let mut estimated = Estimated {
                level,
                line: None,
                mitigated: Some(mitigated),
                frequency,
                occurrences,
                raise: Some(raise),
                held: None,
            };
            estimated.line = Some(render_line(line, &estimated)?);
            Ok(estimated)
        }
    }
}

/// Hold `estimated` at `level`, its related tickets' highest, rendering its line again.
///
/// # Errors
///
/// An estimate line template that does not render one line opening with its label.
pub fn hold(line: &EstimateLine, estimated: &mut Estimated, level: Level) -> Result<(), String> {
    estimated.level = level;
    estimated.held = Some(level);
    if estimated.line.is_some() {
        estimated.line = Some(render_line(line, estimated)?);
    }
    Ok(())
}

/// The estimate line `estimated` renders through `estimate.line_template`.
fn render_line(line: &EstimateLine, estimated: &Estimated) -> Result<String, String> {
    let data = serde_json::json!({
        "label": line.label,
        "level": level_word(estimated.level),
        "mitigated": estimated.mitigated,
        "frequency": estimated.frequency.map(|frequency| match frequency {
            Frequency::Consistent => "consistent",
            Frequency::Intermittent => "intermittent",
        }),
        "occurrences": estimated.occurrences,
        "raise": estimated.raise,
        "held": estimated.held.map(level_word),
    });
    let rendered = template::render(&line.template, &data).map_err(|error| error.to_string())?;
    let rendered = rendered.trim();
    if rendered.contains('\n') || !is_estimate_line(rendered, &line.label) {
        return Err(format!(
            "`estimate.line_template` rendered `{rendered}`, which is not one line opening with `- {}:`, the label the line is found by",
            line.label
        ));
    }
    Ok(rendered.to_owned())
}

/// Whether `line` is an estimate line labelled `label`: after an optional `- ` or `* ` bullet, it
/// opens with `<label>:`.
#[must_use]
pub fn is_estimate_line(line: &str, label: &str) -> bool {
    let line = line.trim_end();
    let bare = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line);
    bare.strip_prefix(label)
        .is_some_and(|rest| rest.starts_with(':'))
}

/// `body` with its estimate line, found by `label` anywhere outside fenced code (a collapsed
/// block's content included, Linear's `* ` bullet accepted), replaced in place by `line`, which
/// keeps the bullet the body wrote; `None` when it carries none.
#[must_use]
pub fn replace_estimate_line(body: &str, line: &str, label: &str) -> Option<String> {
    let plain = unfenced(body);
    let mut offset = 0;
    for (held, original) in plain.split_inclusive('\n').zip(body.split_inclusive('\n')) {
        let bare = held.trim_end_matches(['\n', '\r']);
        if is_estimate_line(bare, label) {
            let ending = &original[bare.len()..];
            let line = match (bare.strip_prefix("* "), line.strip_prefix("- ")) {
                (Some(_), Some(rest)) => format!("* {rest}"),
                _ => line.to_owned(),
            };
            return Some(format!(
                "{}{line}{ending}{}",
                &body[..offset],
                &body[offset + original.len()..]
            ));
        }
        offset += original.len();
    }
    None
}

/// `body` with every estimate line labelled `label` outside fenced code removed: what a
/// person's edit is read from, onefollowup's own writes moving that line.
#[must_use]
pub fn without_estimate_line(body: &str, label: &str) -> String {
    let plain = unfenced(body);
    plain
        .split('\n')
        .zip(body.split('\n'))
        .filter(|(held, _)| !is_estimate_line(held.trim_end_matches('\r'), label))
        .map(|(_, original)| original)
        .collect::<Vec<_>>()
        .join("\n")
}

/// An authored `body` with its estimate line set to `line`, as `publish` writes it when no
/// `render_template` renders the body: replaced in place where it carries one, and otherwise
/// placed where 0.5.0 placed it, after its `## Impact`'s severity with the workaround.
#[must_use]
pub fn with_estimate_line(body: &str, line: &str, label: &str, rubric: &Rubric) -> String {
    if let Some(replaced) = replace_estimate_line(body, line, label) {
        return replaced;
    }
    let mut lines: Vec<String> = body.split('\n').map(str::to_owned).collect();
    let start = lines.iter().position(|one| {
        HEADING
            .captures(one)
            .is_some_and(|c| &c["name"] == LEGACY_IMPACT)
    });
    let Some(start) = start else {
        return body.to_owned();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|one| one.starts_with("## "))
        .map_or(lines.len(), |offset| start + 1 + offset);
    if let Some(last) = rubric.impact_lines.last() {
        let mitigated = format!("- {last}:");
        let starred_mitigated = format!("* {last}:");
        if let Some(at) = (start..end).find(|at| {
            lines[*at].starts_with(&mitigated) || lines[*at].starts_with(&starred_mitigated)
        }) {
            lines.insert(at + 1, line.to_owned());
        }
    }
    lines.join("\n")
}

/// Whether a comment on an item counts as one more occurrence of `root_cause`: an evidence
/// comment of any batch but `created_by`, or a reply whose verdict confirms, each only while
/// its marker names the item's own root cause.
#[must_use]
pub fn counts_as_occurrence(
    body: &str,
    namespace: &str,
    created_by: &str,
    root_cause: &str,
) -> bool {
    let Some(last) = body
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
    else {
        return false;
    };
    let Ok(marker) = CommentMarker::parse(last, namespace) else {
        return false;
    };
    if marker.root_cause != root_cause {
        return false;
    }
    match marker.reply {
        Some(reply) => reply.verdict == Some(Verdict::Confirms),
        None => marker.batch != created_by,
    }
}

/// `value` with every object's keys sorted, at every depth.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonical(&map[key])))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The sha256, as hex, of what a ticket's copy carries onto its item: its title, its body and
/// its record, less the keys a copy stamps afterwards (`schema`, `board_estimate`).
#[must_use]
pub fn digest(title: &str, body: &str, record: &Value) -> String {
    let mut held = record.clone();
    if let Value::Object(map) = &mut held {
        map.shift_remove("schema");
        map.shift_remove("board_estimate");
    }
    let document = canonical(&serde_json::json!({
        "title": title,
        "body": body.trim_end(),
        "record": held,
    }));
    let hash = Sha256::digest(document.to_string().as_bytes());
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `record` as the JSON object it is stored as.
#[must_use]
pub fn record_value(record: &TicketRecord) -> Value {
    serde_json::from_str(&record.render()).unwrap_or(Value::Null)
}
