//! The receipt journal (schema 3), `receipts/<batch>.jsonl`: one line per outcome of a
//! write to a destination, appended and never rewritten. Each line is spelled with its keys
//! sorted at every depth and non-ASCII written raw.
//!
//! Two line kinds onefollowup adds within schema 3 — `relate`, an edge's outcome, and
//! `assign`, an assignee's — are written only under a configuration with relations or an
//! assignee, so a batch under neither writes exactly the lines it always did.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::pyjson::{self, Style};
use super::{FormatError, Frequency, HeldPriority, Level};

const FORMAT: &str = "receipt journal";

/// The schema a journal line is read and written at.
pub const SCHEMA: u64 = 3;

/// A whole journal, in line order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReceiptJournal {
    /// Its receipts.
    pub receipts: Vec<Receipt>,
}

/// One receipt.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    /// The batch.
    pub batch: String,
    /// The ticket the write was for, qualified, where one was.
    pub ticket: Option<String>,
    /// The destination item written, qualified; `None` only for a copy refused before it
    /// reached one.
    pub card: Option<String>,
    /// What happened.
    pub outcome: Outcome,
}

/// What a write's outcome was, by the command that wrote it.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// A ticket's copy.
    Copy(CopyOutcome),
    /// A comment.
    Comment(CommentOutcome),
    /// A re-estimate.
    ReEstimate(ReEstimateOutcome),
    /// An edge of a relation.
    Relate(RelateOutcome),
    /// An item's assignee.
    Assign(AssignOutcome),
}

impl Outcome {
    fn command(&self) -> &'static str {
        match self {
            Self::Copy(_) => "copy",
            Self::Comment(_) => "comment",
            Self::ReEstimate(_) => "re-estimate",
            Self::Relate(_) => "relate",
            Self::Assign(_) => "assign",
        }
    }
}

/// An edge's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelateOutcome {
    /// The relation's name.
    pub relation: String,
    /// How the store holds it: `parent` or `link`.
    pub store: String,
    /// The `from` end as the batch names it: its ticket, or an item.
    pub from: String,
    /// The `to` end as the batch names it.
    pub to: String,
    /// The `to` item the edge was written to, once there was one.
    pub to_card: Option<String>,
    /// Whether the edge was derived from the tickets' fields rather than asserted.
    pub derived: bool,
    /// What the write did.
    pub action: EdgeAction,
    /// Why it did not land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ReceiptError>,
}

/// What an edge's write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeAction {
    /// Wrote it.
    Written,
    /// Found it already held, and wrote nothing.
    Exists,
    /// Did not write it; `error` says why.
    Failed,
}

/// An assignee's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignOutcome {
    /// The one person written.
    pub assignee: String,
    /// The people the item had to hold for the write to land: what onefollowup last wrote.
    pub when: Vec<String>,
    /// What the write did.
    pub action: AssignAction,
    /// Why it did not land.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ReceiptError>,
}

/// What an assignee's write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AssignAction {
    /// Wrote it.
    Written,
    /// The item held a person's own assignment, which stands; nothing was written.
    Held,
    /// Did not write it; `error` says why.
    Failed,
}

/// A copy's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyOutcome {
    /// What the copy did.
    pub action: CopyAction,
    /// The item as the copy left it.
    pub result: Option<ReceiptResult>,
    /// Whether the status condition held.
    pub status_condition: Option<Condition>,
    /// Whether the priority condition held.
    pub priority_condition: Option<Condition>,
    /// Why it failed.
    pub error: Option<ReceiptError>,
    /// The item as held, on a refusal.
    pub held: Option<ReceiptResult>,
    /// The digest of what was copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// The line of the earlier landed copy this one repeats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unchanged_since: Option<u64>,
}

/// What a copy did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyAction {
    /// Created the item.
    Created,
    /// Updated it.
    Updated,
    /// Found it unchanged.
    Unchanged,
    /// Failed.
    Failed,
    /// Skipped it.
    Skipped,
    /// Left it orphaned.
    Orphaned,
    /// Skipped it because an earlier copy landed the same content.
    SkippedUnchanged,
}

/// An item's state as a write left or found it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptResult {
    /// Its priority.
    pub priority: Option<HeldPriority>,
    /// Its status.
    pub status: Option<ReceiptStatus>,
    /// Its version.
    pub version: Option<String>,
}

/// A status as a destination holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptStatus {
    /// The store category.
    pub category: String,
    /// The destination's own name for it.
    pub name: String,
}

/// Whether a write's condition held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// What the item held.
    pub held: Option<String>,
    /// Whether the condition was met.
    pub met: bool,
}

/// Why a write failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptError {
    /// The failure's kind.
    pub kind: String,
    /// Its message.
    pub message: String,
}

/// A comment's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentOutcome {
    /// The comment's id; omitted when the comment was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The item it was written on.
    pub item: Option<String>,
    /// The root cause it is for.
    pub root_cause: Option<String>,
    /// Its URL: `Some(None)` is a written comment with none, and the member is omitted when
    /// the comment was refused.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    pub url: Option<Option<String>>,
    /// Why it was not written; omitted when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ReceiptError>,
}

/// A member that is present, `null` included: what tells `"url": null` from no `url` at all.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A re-estimate's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReEstimateOutcome {
    /// The estimate line written.
    pub estimate_line: String,
    /// The frequency judged.
    pub frequency: Option<Frequency>,
    /// The occurrences counted.
    pub occurrences: u64,
    /// The priority written.
    pub priority: HeldPriority,
    /// The estimate computed.
    pub priority_estimate: Level,
    /// The root cause.
    pub root_cause: Option<String>,
    /// The severity with the workaround, a rubric level's name.
    pub severity_with_workaround: String,
    /// The item's title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Each write made.
    pub writes: Vec<ReEstimateWrite>,
    /// Why nothing was written; omitted when the re-estimate landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ReceiptError>,
}

/// One write a re-estimate made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReEstimateWrite {
    /// The item as it left it.
    pub result: Option<ReceiptResult>,
    /// What was written.
    pub write: WriteKind,
}

/// What a re-estimate wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WriteKind {
    /// The priority.
    Priority,
    /// The content.
    Content,
    /// The metadata.
    Metadata,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    card: Option<String>,
    command: String,
    outcome: Value,
    run: String,
    ticket: Option<String>,
}

impl ReceiptJournal {
    /// Read a journal.
    ///
    /// # Errors
    ///
    /// [`FormatError::Schema`] for a line of any schema but [`SCHEMA`], and
    /// [`FormatError::Malformed`] for a line that is not a receipt, each naming the line.
    pub fn parse(text: &str) -> Result<Self, FormatError> {
        let mut receipts = Vec::new();
        for (index, line) in text.split('\n').enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            receipts.push(Receipt::parse_line(line, index + 1)?);
        }
        Ok(Self { receipts })
    }

    /// Write the journal, one line per receipt.
    #[must_use]
    pub fn render(&self) -> String {
        self.receipts
            .iter()
            .map(|receipt| receipt.render_line() + "\n")
            .collect()
    }
}

impl Receipt {
    fn parse_line(text: &str, number: usize) -> Result<Self, FormatError> {
        let malformed = |message: String| FormatError::Malformed {
            format: FORMAT,
            message: format!("line {number}: {message}"),
        };
        let value: Value = serde_json::from_str(text)
            .map_err(|error| malformed(format!("it is not JSON: {error}")))?;
        let line: Line = super::from_value(value, FORMAT, SCHEMA).map_err(|error| match error {
            FormatError::Schema { found, reads, .. } => FormatError::Schema {
                format: FORMAT,
                found: format!("{found} (line {number})"),
                reads,
            },
            FormatError::Malformed { message, .. } => malformed(message),
        })?;
        let outcome = |value: Value| -> Result<Outcome, FormatError> {
            let read = |error: serde_path_to_error::Error<serde_json::Error>| {
                malformed(format!(
                    "its {} outcome: `{}`: {}",
                    line.command,
                    error.path(),
                    error.inner()
                ))
            };
            Ok(match line.command.as_str() {
                "copy" => Outcome::Copy(serde_path_to_error::deserialize(value).map_err(read)?),
                "comment" => {
                    Outcome::Comment(serde_path_to_error::deserialize(value).map_err(read)?)
                }
                "re-estimate" => {
                    Outcome::ReEstimate(serde_path_to_error::deserialize(value).map_err(read)?)
                }
                "relate" => Outcome::Relate(serde_path_to_error::deserialize(value).map_err(read)?),
                "assign" => Outcome::Assign(serde_path_to_error::deserialize(value).map_err(read)?),
                other => return Err(malformed(format!("`{other}` is not a command"))),
            })
        };
        let outcome = outcome(line.outcome)?;
        if line.card.is_none() && !matches!(outcome, Outcome::Copy(_) | Outcome::Relate(_)) {
            return Err(malformed(
                "only a copy's or an edge's receipt names no card".to_owned(),
            ));
        }
        Ok(Self {
            batch: line.run,
            ticket: line.ticket,
            card: line.card,
            outcome,
        })
    }

    /// The receipt as its one journal line, without the newline.
    #[must_use]
    pub fn render_line(&self) -> String {
        let outcome = match &self.outcome {
            Outcome::Copy(one) => serde_json::to_value(one),
            Outcome::Comment(one) => serde_json::to_value(one),
            Outcome::ReEstimate(one) => serde_json::to_value(one),
            Outcome::Relate(one) => serde_json::to_value(one),
            Outcome::Assign(one) => serde_json::to_value(one),
        }
        .unwrap_or(Value::Null);
        let line = Line {
            card: self.card.clone(),
            command: self.outcome.command().to_owned(),
            outcome,
            run: self.batch.clone(),
            ticket: self.ticket.clone(),
        };
        pyjson::to_string(&super::to_value(&line, SCHEMA), Style::SORTED_RAW)
    }
}
