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

use crate::config::{Config, Kind, Priority, PriorityModel, Rubric, StatusRole};
use crate::formats::markers::CommentMarker;
use crate::formats::ticket::TicketRecord;
use crate::formats::{Frequency, Level, Verdict, Versioned};
use crate::verify::template::{self, Located};

/// The longest title a ticket carries.
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
/// A line of an `## Impact` section's tail.
static IMPACT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-*] (?P<label>[^:\n]+):(?P<value>[^\n]*)$").expect("valid"));

/// The label of the estimate line `publish` writes into an estimated ticket's `## Impact`.
pub const ESTIMATE_LINE: &str = "Priority estimate";
/// The section an estimate model's severity lines sit in.
pub const IMPACT: &str = "Impact";
/// The section a ticket's evidence is in.
pub const EVIDENCE: &str = "Evidence";

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

/// What a ticket is held to besides the configuration.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The batch.
    pub batch: &'a str,
    /// The batch's input set, by id; `None` when it was not prepared.
    pub inputs: Option<&'a [String]>,
    /// The kind's ticket template, resolved.
    pub template: &'a Located,
}

/// The template headings a ticket of the kind carries, each with whether it is optional.
///
/// # Errors
///
/// A template that cannot be rendered.
pub fn template_headings(
    template: &Located,
    rubric: &Rubric,
    fields: &Map<String, Value>,
) -> Result<Vec<(String, bool)>, template::TemplateError> {
    let rendered = template::render(
        template,
        &serde_json::json!({"rubric": rubric, "fields": fields}),
    )?;
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

/// Every rule `ticket` breaks; empty when it is sound.
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
    if title.chars().count() > TITLE_LIMIT {
        problems.push(format!("the title is longer than {TITLE_LIMIT} characters"));
    }
    if title.chars().any(char::is_control) {
        problems.push("the title carries a control character".to_owned());
    }
    let Some(record) = &ticket.record else {
        return problems;
    };
    if record.root_cause != ticket.slug {
        problems.push(format!(
            "the record's root cause `{}` is not the file's, `{}`",
            record.root_cause, ticket.slug
        ));
    }
    if !ORIGIN.is_match(&record.repository) {
        problems.push(format!(
            "the record's repository `{}` is not a normalized origin, `host/owner/name`",
            record.repository
        ));
    } else {
        let name = record.repository.rsplit('/').next().unwrap_or_default();
        if !title.starts_with(&format!("{name}: ")) {
            problems.push(format!(
                "the title does not open with `{name}: `, the name of the repository its item is filed in"
            ));
        }
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
    for (origin, commit) in &record.basis {
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
    match template_headings(context.template, &config.rubric, &fields) {
        Ok(expected) => problems.extend(body_problems(&ticket.body, &expected)),
        Err(error) => problems.push(error.to_string()),
    }
    if kind.priority == PriorityModel::Estimate {
        problems.extend(impact_problems(&ticket.body, &config.rubric));
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

/// One `## Impact` line's label and value.
fn impact_lines(body: &str) -> Vec<(String, String)> {
    section(body, IMPACT)
        .map(|content| {
            content
                .lines()
                .filter_map(|line| IMPACT_LINE.captures(line.trim_end()))
                .map(|captured| {
                    (
                        captured["label"].trim().to_owned(),
                        captured["value"].trim().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The value `body`'s `## Impact` states for the line `label`.
#[must_use]
pub fn impact_value(body: &str, label: &str) -> Option<String> {
    impact_lines(body)
        .into_iter()
        .find(|(found, _)| found == label)
        .map(|(_, value)| value)
}

/// The rubric severity `body` records, by the rubric's first impact line.
#[must_use]
pub fn severity(body: &str, rubric: &Rubric) -> Option<String> {
    impact_value(body, rubric.impact_lines.first()?)
}

/// The severity with the workaround `body` records, by the rubric's last impact line.
#[must_use]
pub fn with_workaround(body: &str, rubric: &Rubric) -> Option<String> {
    impact_value(body, rubric.impact_lines.last()?)
}

fn level_index(rubric: &Rubric, name: &str) -> Option<usize> {
    rubric.levels.iter().position(|level| level.name == name)
}

/// What `body`'s `## Impact` breaks of the rubric.
fn impact_problems(body: &str, rubric: &Rubric) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(content) = section(body, IMPACT) else {
        return vec![format!(
            "the body has no `## {IMPACT}` section, so there is no severity to estimate a priority from"
        )];
    };
    let mut from = 0;
    for part in &rubric.impact_parts {
        let label = format!("**{part}.**");
        match content[from..].find(&label) {
            Some(at) => from += at + label.len(),
            None => problems.push(format!(
                "`## {IMPACT}` does not carry its `{label}` part, in the rubric's order"
            )),
        }
    }
    let lines: Vec<(String, String)> = impact_lines(body)
        .into_iter()
        .filter(|(label, _)| rubric.impact_lines.contains(label))
        .collect();
    let labels: Vec<&String> = lines.iter().map(|(label, _)| label).collect();
    let wanted: Vec<&String> = rubric.impact_lines.iter().collect();
    if labels != wanted {
        problems.push(format!(
            "`## {IMPACT}` carries its lines {}, where the rubric states each of {} exactly once, in that order",
            labels.iter().map(|one| format!("`{one}`")).collect::<Vec<_>>().join(", "),
            wanted.iter().map(|one| format!("`{one}`")).collect::<Vec<_>>().join(", ")
        ));
        return problems;
    }
    let names: Vec<&str> = rubric
        .levels
        .iter()
        .map(|level| level.name.as_str())
        .collect();
    let severity = &lines[0].1;
    let mitigated = &lines[lines.len() - 1].1;
    for (label, value) in [
        (&lines[0].0, severity),
        (&lines[lines.len() - 1].0, mitigated),
    ] {
        if level_index(rubric, value).is_none() {
            problems.push(format!(
                "`- {label}:` is `{value}`, which is not one of the rubric's levels {}",
                names
                    .iter()
                    .map(|one| format!("`{one}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if let (Some(raw), Some(left)) = (
        level_index(rubric, severity),
        level_index(rubric, mitigated),
    ) {
        let workaround = lines
            .get(1)
            .map(|(_, value)| value.as_str())
            .unwrap_or_default();
        if workaround.is_empty() {
            problems.push("the workaround line is empty".to_owned());
        }
        if left < raw {
            problems.push(format!(
                "the severity with the workaround, `{mitigated}`, is above the severity, `{severity}`"
            ));
        } else if workaround == rubric.no_workaround && left != raw {
            problems.push(format!(
                "with a workaround of `{}` the severity with the workaround is the severity, `{severity}`",
                rubric.no_workaround
            ));
        } else if workaround != rubric.no_workaround && left == raw && raw + 1 < rubric.levels.len()
        {
            problems.push(format!(
                "an acceptable workaround lowers the severity `{severity}` at least one level; a workaround too costly to lower it is `{}`",
                rubric.no_workaround
            ));
        }
    }
    problems
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

/// What a ticket's facts estimate: its level, and the estimate line `## Impact` carries for
/// an estimated kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Estimated {
    /// The level.
    pub level: Level,
    /// The estimate line, for the estimate model.
    pub line: Option<String>,
}

/// The priority `body` and `frequency` estimate under `kind` with `occurrences` occurrences.
///
/// # Errors
///
/// An estimated kind's body stating no severity with the workaround the rubric names.
pub fn estimate(
    config: &Config,
    kind: &Kind,
    body: &str,
    frequency: Option<Frequency>,
    occurrences: u64,
) -> Result<Estimated, String> {
    match &kind.priority {
        PriorityModel::Fixed(priority) => Ok(Estimated {
            level: level_of(*priority),
            line: None,
        }),
        PriorityModel::Estimate => {
            let mitigated = with_workaround(body, &config.rubric).ok_or_else(|| {
                format!("the body's `## {IMPACT}` states no severity with the workaround")
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
            let frequency = match frequency {
                Some(Frequency::Consistent) => "fires consistently",
                Some(Frequency::Intermittent) => "fires intermittently",
                None => "frequency not judged",
            };
            let plural = if occurrences == 1 { "" } else { "s" };
            Ok(Estimated {
                level,
                line: Some(format!(
                    "- {ESTIMATE_LINE}: {} (severity with the workaround {mitigated}; {frequency}; {occurrences} occurrence{plural}; {raise})",
                    level_word(level)
                )),
            })
        }
    }
}

/// `body` with its `## Impact` estimate line set to `line`: replaced where it has one, and
/// otherwise placed after the rubric's last impact line.
#[must_use]
pub fn with_estimate_line(body: &str, line: &str, rubric: &Rubric) -> String {
    let mut lines: Vec<String> = body.split('\n').map(str::to_owned).collect();
    let start = lines
        .iter()
        .position(|one| HEADING.captures(one).is_some_and(|c| &c["name"] == IMPACT));
    let Some(start) = start else {
        return body.to_owned();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|one| one.starts_with("## "))
        .map_or(lines.len(), |offset| start + 1 + offset);
    let estimate = format!("- {ESTIMATE_LINE}:");
    let starred = format!("* {ESTIMATE_LINE}:");
    if let Some(at) = (start..end)
        .find(|at| lines[*at].starts_with(&estimate) || lines[*at].starts_with(&starred))
    {
        lines[at] = line.to_owned();
    } else if let Some(last) = rubric.impact_lines.last() {
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
