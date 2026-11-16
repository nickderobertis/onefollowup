//! The ticket record (schema 14): the metadata a ticket is stored under, at the
//! `<namespace>.follow-up` key of the store's metadata slot (the hidden trailing comment on
//! GitHub and Linear, which onetaskgraph's metadata search finds).
//!
//! Its optional keys onefollowup adds within schema 14 — `draft_sources`, `transcripts`,
//! `witnesses`, `kind`, `fields`, `edges`, `board_assignee`, `impact` and `projected` — are
//! omitted when empty, so a host using only the default kind, no declared fields, no relations,
//! no assignee and no projections, whose drafts name no author, writes byte-identical
//! schema-14 records. `draft_sources`, `transcripts` and `witnesses` take the names, shapes and
//! places of the schema-15 records an orchestrator's `followups` board held, whose
//! `metadata_fields` read them: never rename one.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::pyjson::Style;
use super::{Frequency, Level, Versioned};

/// One ticket's record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketRecord {
    /// The root cause's slug.
    pub root_cause: String,
    /// The repository the ticket is about, as a normalized origin.
    pub repository: String,
    /// The batch that created the ticket.
    #[serde(rename = "created_by_run")]
    pub created_by_batch: String,
    /// Every batch that owns the ticket.
    #[serde(rename = "owning_runs")]
    pub owning_batches: Vec<String>,
    /// The qualified ids of the drafts the ticket stands on.
    pub drafts: Vec<String>,
    /// Where each draft came from, by its qualified id: `author <kind>`, then `; node <node>`
    /// and `; member <member>` where the draft names them. onefollowup writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_sources: Option<IndexMap<String, String>>,
    /// The command that reads each draft's turns, by its qualified id, for each draft naming
    /// one. onefollowup writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcripts: Option<IndexMap<String, String>>,
    /// Each repository verified, and the commit it was verified at.
    pub basis: IndexMap<String, String>,
    /// Where each verified claim was seen: `<origin> <path>:<line> at <commit>`, the origin a
    /// key of `basis` and the commit the one `basis` holds for it. The verifying agent writes
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub witnesses: Option<Vec<String>>,
    /// When it was verified.
    pub verified_at: String,
    /// The host that verified it.
    pub host: String,
    /// The priority its facts estimate.
    pub priority_estimate: Level,
    /// Whether it fires consistently; absent when not judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency: Option<Frequency>,
    /// The budget a budget overrun was filed for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<OverrunBudget>,
    /// The priority onefollowup last wrote to the ticket's item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_estimate: Option<Level>,
    /// The ticket's kind; absent means the configured `default_kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Each declared field the ticket records, with its value as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<IndexMap<String, String>>,
    /// The relations' edges this ticket is the `from` end of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<Edge>>,
    /// The one person onefollowup last wrote as the ticket's item's assignee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_assignee: Option<String>,
    /// The rubric's impact-line values, by the line's label, as the verifying agent records
    /// them; read and kept whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<IndexMap<String, String>>,
    /// Each of the kind's `projections`, rendered, by its name: what a destination's
    /// `metadata_fields` reads at `[projected, <name>]`. onefollowup alone writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected: Option<IndexMap<String, String>>,
}

/// The budget a ticket filed for an overrun names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverrunBudget {
    /// The repository holding the budgets file.
    pub repository: String,
    /// The budgets file, relative to the repository.
    pub file: String,
    /// The budget's id.
    pub id: String,
    /// The threshold, spelled as it was written: an integer stays an integer (`123`) and a
    /// float a float (`300.0`, `123.5`), so a rewrite changes no record's bytes.
    pub threshold: serde_json::Number,
    /// Whether the threshold is a `max` or a `min`.
    pub direction: Direction,
}

/// A budget's direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    /// The measure must stay at or under the threshold.
    Max,
    /// The measure must stay at or over it.
    Min,
}

/// One edge of a relation, recorded on its `from` ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    /// The relation's name.
    pub relation: String,
    /// The `to` ticket, as `<source>:<id>`.
    pub to: String,
}

impl Versioned for TicketRecord {
    const FORMAT: &'static str = "ticket record";
    const SCHEMA: u64 = 14;
    const STYLE: Style = Style::LINE;
    const TRAILING_NEWLINE: bool = false;
}
