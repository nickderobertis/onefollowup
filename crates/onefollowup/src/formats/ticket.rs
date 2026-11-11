//! The ticket record (schema 14): the metadata a ticket is stored under, at the
//! `<namespace>.follow-up` key of the store's metadata slot (the hidden trailing comment on
//! GitHub and Linear, which onetaskgraph's metadata search finds).
//!
//! Its four optional keys onefollowup adds within schema 14 — `kind`, `fields`, `edges` and
//! `board_assignee` — are omitted when empty, so a host using only the default kind, no
//! declared fields, no relations and no assignee writes byte-identical schema-14 records.

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
    /// Each repository verified, and the commit it was verified at.
    pub basis: IndexMap<String, String>,
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
    /// The threshold, always written as a float (`300.0`).
    pub threshold: f64,
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
