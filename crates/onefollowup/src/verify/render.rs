//! What every ticket-facing template is given, and the renders onefollowup makes with it: a
//! ticket's destination body and projections, and the visible part of each comment a batch
//! writes, after which onefollowup alone appends the marker line.
//!
//! Every template — a kind's `ticket_template` and `render_template`, both comment templates,
//! each projection and the task template's ticket example — sees the same data (README.md,
//! "Templates"): `rubric`, `estimate`, `fields`, `declared_fields`, `kind`, `batch`, `destination`
//! and `namespace`;
//! for a ticket `ticket`, `record` and `estimate_line`; for a comment `comment`.

use std::sync::{Arc, LazyLock};

use indexmap::IndexMap;
use minijinja::Value as Jinja;
use minijinja::value::{Enumerator, Object};
use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::config::{Config, Kind};
use crate::formats::markers::CommentMarker;
use crate::formats::ticket::TicketRecord;
use crate::formats::{Frequency, Versioned};
use crate::verify::template::{self, Located};
use crate::verify::ticket::{self, Estimated};

/// A fence line.
static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})").expect("valid"));
/// A level-3 heading.
static SUBHEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^###[ \t]+(?P<heading>.*?)[ \t]*$").expect("valid"));
/// A labelled part's first line, `**Label.** text`.
static PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\*\*(?P<label>[^*\n]+?)\.\*\*[ \t]*(?P<rest>.*)$").expect("valid")
});
/// A `- Label: value` line.
static LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[-*] (?P<label>[^:\n]+):(?P<value>[^\n]*)$").expect("valid"));
/// A `<details>` or `</details>` line, as a block opens or closes.
static DETAILS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[ \t]*</?details\b").expect("valid"));
/// One witness: `<origin> <path>:<line> at <commit>`.
pub static WITNESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<origin>\S+) (?P<path>\S+):(?P<line>[1-9][0-9]*) at (?P<commit>\S+)$")
        .expect("valid")
});

/// One `**Label.**` part of a section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Part {
    /// Its label, without the bold markers and the full stop.
    pub label: String,
    /// What follows the label, every paragraph of it: to the next part, the next `###`
    /// subsection, or the end of the section, trimmed.
    pub text: String,
}

/// One `- Label: value` line of a section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Line {
    /// Its label, trimmed.
    pub label: String,
    /// Its value, trimmed.
    pub value: String,
    /// The whole line as written.
    pub text: String,
}

/// One `###` subsection of a section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Subsection {
    /// Its heading.
    pub heading: String,
    /// Its content, trimmed.
    pub content: String,
}

/// One `##` section of a ticket's authored body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Section {
    /// Its heading.
    pub name: String,
    /// Its content, trimmed.
    pub content: String,
    /// Its content before the first `**Label.**` part, `- Label: value` line or `###`
    /// subsection, trimmed.
    pub lead: String,
    /// Every `**Label.**` part of it, those within its subsections included, in order.
    pub parts: Vec<Part>,
    /// Every `- Label: value` line of its content, in order.
    pub lines: Vec<Line>,
    /// Every `###` subsection, in order.
    pub subsections: Vec<Subsection>,
}

/// Each line of `text` with whether it is inside a fence (a fence line counting as inside).
fn fenced(text: &str) -> Vec<(&str, bool)> {
    let mut open: Option<String> = None;
    text.split('\n')
        .map(|line| {
            let bare = line.trim_end_matches('\r');
            if let Some(captured) = FENCE.captures(bare) {
                let marker = captured[1].to_owned();
                match &open {
                    None => open = Some(marker),
                    Some(held) if marker.starts_with(held.as_str()) => open = None,
                    Some(_) => {}
                }
                (bare, true)
            } else {
                (bare, open.is_some())
            }
        })
        .collect()
}

/// Every `##` section of `body`, in order.
#[must_use]
pub fn sections(body: &str) -> Vec<Section> {
    let headings = ticket::headings(body);
    headings
        .iter()
        .enumerate()
        .map(|(at, (name, start))| {
            let end = headings
                .get(at + 1)
                .map_or(body.len(), |(_, offset)| *offset);
            let text = &body[*start..end];
            let content = text.split_once('\n').map_or("", |(_, rest)| rest).trim();
            section(name, content)
        })
        .collect()
}

fn section(name: &str, content: &str) -> Section {
    let lines = fenced(content);
    let mut lead: Vec<&str> = Vec::new();
    // The lead ends at the first part, labelled line or subsection outside a fence.
    let mut lead_open = true;
    let mut parts: Vec<(String, Vec<&str>)> = Vec::new();
    // Whether the last part found still takes lines: until the next part or subsection.
    let mut part_open = false;
    let mut subsections: Vec<(String, Vec<&str>)> = Vec::new();
    let mut labelled: Vec<Line> = Vec::new();
    for (line, inside) in &lines {
        let heading = (!inside).then(|| SUBHEADING.captures(line)).flatten();
        if let Some(captured) = heading {
            lead_open = false;
            part_open = false;
            subsections.push((captured["heading"].to_owned(), Vec::new()));
            continue;
        }
        if let Some((_, held)) = subsections.last_mut() {
            held.push(line);
        }
        let part = (!inside).then(|| PART.captures(line)).flatten();
        let pair = (!inside).then(|| LINE.captures(line)).flatten();
        if let Some(captured) = &pair {
            labelled.push(Line {
                label: captured["label"].trim().to_owned(),
                value: captured["value"].trim().to_owned(),
                text: (*line).to_owned(),
            });
        }
        if part.is_some() || pair.is_some() {
            lead_open = false;
        }
        if lead_open {
            lead.push(line);
        }
        if let Some(captured) = part {
            let rest = captured.name("rest").map_or("", |rest| rest.as_str());
            parts.push((captured["label"].trim().to_owned(), vec![rest]));
            part_open = true;
        } else if part_open && let Some((_, text)) = parts.last_mut() {
            text.push(line);
        }
    }
    Section {
        name: name.to_owned(),
        content: content.to_owned(),
        lead: lead.join("\n").trim().to_owned(),
        parts: parts
            .into_iter()
            .map(|(label, text)| Part {
                label,
                text: text.join("\n").trim().to_owned(),
            })
            .collect(),
        lines: labelled,
        subsections: subsections
            .into_iter()
            .map(|(heading, held)| Subsection {
                heading,
                content: held.join("\n").trim().to_owned(),
            })
            .collect(),
    }
}

/// Every line of authored text only the renderer or onefollowup writes: a `<details>` or
/// `</details>` line outside a fence (only the renderer writes a block) and a comment-marker
/// line (only onefollowup writes a marker). `what` names the text in each refusal.
#[must_use]
pub fn reserved_lines(text: &str, namespace: &str, what: &str) -> Vec<String> {
    let marker = format!("<!-- {namespace}:follow-up-comment");
    let mut problems = Vec::new();
    for (line, inside) in fenced(text) {
        if !inside && DETAILS.is_match(line) {
            problems.push(format!(
                "{what} carries the line `{}`: only the renderer writes a collapsed block, through its template's `collapsed(summary, content)`",
                line.trim()
            ));
        }
        if line.trim_start().starts_with(&marker) {
            problems.push(format!(
                "{what} carries the comment-marker line `{}`: only onefollowup writes a marker",
                line.trim()
            ));
        }
    }
    problems
}

/// Every way `witnesses` is not one claim each seen in `basis`, at its basis commit.
#[must_use]
pub fn witness_problems(witnesses: &[String], basis: &IndexMap<String, String>) -> Vec<String> {
    let mut problems = Vec::new();
    for witness in witnesses {
        let Some(captured) = WITNESS.captures(witness) else {
            problems.push(format!(
                "the witness `{witness}` is not `<origin> <path>:<line> at <commit>`"
            ));
            continue;
        };
        match basis.get(&captured["origin"]) {
            None => problems.push(format!(
                "the witness `{witness}` names `{}`, which the basis does not",
                &captured["origin"]
            )),
            Some(commit) if *commit != captured["commit"] => problems.push(format!(
                "the witness `{witness}` is at `{}`, where the basis records `{commit}` for `{}`",
                &captured["commit"], &captured["origin"]
            )),
            Some(_) => {}
        }
    }
    problems
}

/// What every template is given about where it renders.
#[derive(Debug, Clone, Copy)]
pub struct Common<'a> {
    /// The effective configuration.
    pub config: &'a Config,
    /// The kind of the ticket rendered, or the default kind.
    pub kind: &'a Kind,
    /// The batch.
    pub batch: &'a str,
    /// The destination the ticket goes to, or the comment's item is on.
    pub destination: &'a str,
}

impl Common<'_> {
    /// `rubric`, `estimate`, `fields`, `declared_fields`, `kind`, `batch`, `destination` and
    /// `namespace`, with `fields` as given.
    fn data(&self, fields: Value) -> Map<String, Value> {
        let mut data = Map::new();
        data.insert("rubric".to_owned(), json!(self.config.rubric));
        data.insert("estimate".to_owned(), json!(self.config.estimate));
        data.insert("fields".to_owned(), fields);
        data.insert("declared_fields".to_owned(), self.declarations());
        data.insert("kind".to_owned(), json!(self.kind));
        data.insert("batch".to_owned(), Value::from(self.batch));
        data.insert("destination".to_owned(), Value::from(self.destination));
        data.insert(
            "namespace".to_owned(),
            Value::from(self.config.namespace.as_str()),
        );
        data
    }

    /// Each declared field, mapped to `{description}`: `fields` where no ticket gives values.
    #[must_use]
    pub fn declarations(&self) -> Value {
        Value::Object(
            self.config
                .fields
                .iter()
                .map(|(name, field)| (name.clone(), json!({"description": field.description})))
                .collect(),
        )
    }

    /// The data a template rendered for no particular ticket or comment sees: the task
    /// template's ticket example, with each declared field's description.
    #[must_use]
    pub fn example(&self) -> Jinja {
        context(self.data(self.declarations()), None)
    }

    /// The data a ticket's templates see.
    #[must_use]
    pub fn ticket(&self, ticket: &TicketData<'_>) -> Jinja {
        let mut data = self.data(ticket.fields());
        data.insert("record".to_owned(), ticket.record.clone());
        data.insert(
            "estimate_line".to_owned(),
            ticket.estimate.map_or(Value::Null, estimate_line),
        );
        let mut fields = Map::new();
        fields.insert("title".to_owned(), Value::from(ticket.title));
        fields.insert("slug".to_owned(), Value::from(ticket.slug));
        fields.insert("body".to_owned(), Value::from(ticket.body));
        let sections = sections(ticket.body);
        fields.insert("sections".to_owned(), json!(sections));
        context(data, Some(TicketObject { fields, sections }))
    }

    /// The data a comment's template sees.
    #[must_use]
    pub fn comment(&self, comment: &Value) -> Jinja {
        let mut data = self.data(self.declarations());
        data.insert("comment".to_owned(), comment.clone());
        context(data, None)
    }
}

/// A ticket as its templates see it.
#[derive(Debug, Clone, Copy)]
pub struct TicketData<'a> {
    /// Its title.
    pub title: &'a str,
    /// Its root cause.
    pub slug: &'a str,
    /// Its authored body, verbatim.
    pub body: &'a str,
    /// Its record, as it will be stored.
    pub record: &'a Value,
    /// Its estimate, once known.
    pub estimate: Option<&'a Estimated>,
}

impl TicketData<'_> {
    /// The ticket's declared field values, each name mapped to its value.
    fn fields(&self) -> Value {
        self.record
            .get("fields")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| Value::Object(Map::new()))
    }
}

/// An estimate as `estimate_line` is given it.
fn estimate_line(estimated: &Estimated) -> Value {
    json!({
        "level": ticket::level_word(estimated.level),
        "line": estimated.line,
        "mitigated": estimated.mitigated,
        "frequency": estimated.frequency.map(|frequency| match frequency {
            Frequency::Consistent => "consistent",
            Frequency::Intermittent => "intermittent",
        }),
        "occurrences": estimated.occurrences,
        "raise": estimated.raise,
    })
}

/// The `ticket` a template sees: its fields, and `section(name)`.
#[derive(Debug)]
struct TicketObject {
    fields: Map<String, Value>,
    sections: Vec<Section>,
}

impl Object for TicketObject {
    fn get_value(self: &Arc<Self>, key: &Jinja) -> Option<Jinja> {
        self.fields.get(key.as_str()?).map(Jinja::from_serialize)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Values(
            self.fields
                .keys()
                .map(|key| Jinja::from(key.as_str()))
                .collect(),
        )
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Jinja],
    ) -> Result<Jinja, minijinja::Error> {
        if method != "section" {
            return Err(minijinja::Error::from(minijinja::ErrorKind::UnknownMethod));
        }
        let (name,): (String,) = minijinja::value::from_args(args)?;
        Ok(self
            .sections
            .iter()
            .find(|section| section.name == name)
            .map_or_else(Jinja::default, Jinja::from_serialize))
    }
}

fn context(data: Map<String, Value>, ticket: Option<TicketObject>) -> Jinja {
    let mut pairs: Vec<(String, Jinja)> = data
        .into_iter()
        .map(|(key, value)| (key, Jinja::from_serialize(&value)))
        .collect();
    if let Some(ticket) = ticket {
        pairs.push(("ticket".to_owned(), Jinja::from_object(ticket)));
    }
    Jinja::from_iter(pairs)
}

/// A ticket's destination body and its record's `projected`, rendered.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    /// The body the destination is written.
    pub body: String,
    /// Each projection, by name, rendered; `None` when the kind declares none.
    pub projected: Option<IndexMap<String, String>>,
}

/// Render a ticket's projections into `record.projected`, then its destination body: its kind's
/// `render_template` given every datum, or — with none configured — the authored body with
/// its estimate line, as 0.5.0 wrote it.
///
/// # Errors
///
/// A template that cannot be read or rendered, named.
pub fn destination(
    common: &Common<'_>,
    render_template: Option<&Located>,
    title: &str,
    slug: &str,
    authored: &str,
    record: &mut TicketRecord,
    estimate: &Estimated,
) -> Result<String, String> {
    record.projected = None;
    if let Some(projections) = &common.kind.projections {
        let value = record_value(record);
        let data = common.ticket(&TicketData {
            title,
            slug,
            body: authored,
            record: &value,
            estimate: Some(estimate),
        });
        let mut projected = IndexMap::new();
        for (name, source) in projections {
            let rendered = template::render_text(
                &format!("kinds[{}].projections.{name}", common.kind.name),
                source,
                data.clone(),
            )
            .map_err(|error| error.to_string())?;
            projected.insert(name.clone(), rendered.trim().to_owned());
        }
        record.projected = (!projected.is_empty()).then_some(projected);
    }
    let Some(located) = render_template else {
        return Ok(match &estimate.line {
            Some(line) => ticket::with_estimate_line(authored, line, &common.config.rubric),
            None => authored.to_owned(),
        });
    };
    let value = record_value(record);
    let rendered = template::render_value(
        located,
        common.ticket(&TicketData {
            title,
            slug,
            body: authored,
            record: &value,
            estimate: Some(estimate),
        }),
    )
    .map_err(|error| error.to_string())?;
    Ok(format!(
        "{}\n",
        rendered.trim_start_matches(['\n', '\r']).trim_end()
    ))
}

/// `record` as the JSON object it is stored as.
fn record_value(record: &TicketRecord) -> Value {
    serde_json::from_str(&record.render()).unwrap_or(Value::Null)
}

/// A comment's whole body: its template rendered with `comment`, then a blank line and the
/// marker as its last line.
///
/// # Errors
///
/// A template that cannot be read or rendered, or one whose text writes a marker itself.
pub fn comment(
    common: &Common<'_>,
    located: &Located,
    comment: &Value,
    marker: &CommentMarker,
) -> Result<String, String> {
    let visible = visible(common, located, comment)?;
    Ok(format!(
        "{visible}\n\n{}\n",
        marker.render(&common.config.namespace)
    ))
}

/// A comment's visible part: its template rendered with `comment`, trimmed.
///
/// # Errors
///
/// A template that cannot be read or rendered, or one whose text writes a marker itself.
pub fn visible(common: &Common<'_>, located: &Located, comment: &Value) -> Result<String, String> {
    let rendered = template::render_value(located, common.comment(comment))
        .map_err(|error| error.to_string())?;
    let visible = rendered.trim_start_matches(['\n', '\r']).trim_end();
    let reserved = reserved_lines(visible, &common.config.namespace, "the rendered comment");
    if let Some(marker_line) = reserved
        .iter()
        .find(|problem| problem.contains("comment-marker"))
    {
        return Err(marker_line.clone());
    }
    Ok(visible.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{Line, Part, Subsection, sections};

    const BODY: &str = "## Suggested fix

Read the window size once per save.

It is one change.

**Contributing locations.** The save path.

It reads the size twice.
- Layer: widgets `src/layout.rs`

**Why now.** Saves are lost.

```text
**Fenced.** not a part
- Fenced: not a line
```

### widgets — `github.com/example/widgets`

Read the size once.

**Interface.** None changes.

Behaviour only.

### gadgets — `github.com/example/gadgets`

Nothing to do.

## Evidence

**Seen.** Twice.
";

    fn part(label: &str, text: &str) -> Part {
        Part {
            label: label.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn the_lead_ends_at_the_first_part_line_or_subsection() {
        let found = sections(BODY);
        assert_eq!(
            found[0].lead,
            "Read the window size once per save.\n\nIt is one change."
        );
        assert_eq!(
            found[1].lead, "",
            "a section opening with a part has no lead"
        );
        let lined = sections("## Impact\n\nIntro.\n- Severity: high\n\nAfter.\n");
        assert_eq!(lined[0].lead, "Intro.", "a labelled line ends the lead");
        let sub = sections("## Fix\n\nIntro.\n\n### unit\n\nBody.\n");
        assert_eq!(sub[0].lead, "Intro.", "a subsection ends the lead");
    }

    #[test]
    fn every_part_keeps_each_paragraph_and_parts_in_subsections_are_collected() {
        let found = sections(BODY);
        assert_eq!(
            found[0].parts,
            [
                part(
                    "Contributing locations",
                    "The save path.\n\nIt reads the size twice.\n- Layer: widgets `src/layout.rs`"
                ),
                part(
                    "Why now",
                    "Saves are lost.\n\n```text\n**Fenced.** not a part\n- Fenced: not a line\n```"
                ),
                part("Interface", "None changes.\n\nBehaviour only."),
            ],
            "a part runs to the next part or subsection, fenced lines its own text"
        );
        assert_eq!(found[1].parts, [part("Seen", "Twice.")]);
    }

    #[test]
    fn labelled_lines_and_subsections_are_read_outside_fences() {
        let found = sections(BODY);
        assert_eq!(
            found[0].lines,
            [Line {
                label: "Layer".to_owned(),
                value: "widgets `src/layout.rs`".to_owned(),
                text: "- Layer: widgets `src/layout.rs`".to_owned(),
            }]
        );
        assert_eq!(
            found[0].subsections,
            [
                Subsection {
                    heading: "widgets — `github.com/example/widgets`".to_owned(),
                    content:
                        "Read the size once.\n\n**Interface.** None changes.\n\nBehaviour only."
                            .to_owned(),
                },
                Subsection {
                    heading: "gadgets — `github.com/example/gadgets`".to_owned(),
                    content: "Nothing to do.".to_owned(),
                },
            ]
        );
    }
}
