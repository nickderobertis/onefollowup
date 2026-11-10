//! `feedback/pending.json` (schema 1): the comment pass's writes whose outcome is not yet
//! known, kept until a later pass settles them.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Versioned;
use super::pyjson::Style;

/// The pending writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    /// One per write.
    pub entries: Vec<PendingEntry>,
}

/// One write whose outcome is pending.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingEntry {
    /// The snapshot directory the write answers.
    pub snapshot: String,
    /// The batch.
    #[serde(rename = "run")]
    pub batch: String,
    /// The item written, qualified.
    pub item: String,
    /// What the write is.
    pub write: PendingWrite,
    /// Where it stands.
    pub state: PendingState,
    /// The item's version in the snapshot.
    pub snapshot_version: Option<String>,
    /// The version last seen.
    pub seen_version: Option<String>,
    /// What the write meant to set, in the order it was built.
    pub intended: IndexMap<String, Value>,
    /// Each part's outcome.
    pub parts: Vec<WritePart>,
    /// What went wrong.
    pub error: Option<String>,
    /// When it was recorded.
    pub recorded_at: String,
}

/// What a pending write is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PendingWrite {
    /// A ticket's copy.
    TicketCopy,
    /// A priority.
    Priority,
    /// A withdrawal.
    Withdrawal,
}

/// Where a pending write stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PendingState {
    /// Not sent.
    Withheld,
    /// Refused.
    Failed,
    /// Some parts landed.
    Partial,
    /// Its answer was lost.
    Unknown,
}

/// One part of a write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WritePart {
    /// The part.
    pub part: String,
    /// Its outcome.
    pub outcome: String,
    /// What went wrong.
    pub error: Option<String>,
}

impl Versioned for Pending {
    const FORMAT: &'static str = "pending writes";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::INDENTED;
}
