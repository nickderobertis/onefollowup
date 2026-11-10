//! A comment snapshot's record (schema 2), `feedback/.snapshots/<stamp>/snapshot.json`, and
//! its `launched.json` (schema 1), written once when the snapshot's dispatches launch.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::Versioned;
use super::pyjson::{Layout, Style};

/// What one gathering read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// When it read.
    pub queried_at: String,
    /// The board read first.
    pub board: String,
    /// Every board read, the first being `board`.
    pub boards: Vec<String>,
    /// Whether the board was named rather than defaulted.
    pub named_board: bool,
    /// The batch it was scoped to.
    pub scope: Option<String>,
    /// The one issue it was scoped to.
    pub issue: Option<String>,
    /// From when each board was read, and why.
    pub starts: IndexMap<String, Start>,
    /// The store's version line.
    pub store: String,
    /// Whether it was a dry run.
    pub dry_run: bool,
    /// Every item copied into the snapshot.
    pub items: Vec<SnapshotItem>,
    /// Each batch's boundary.
    pub boundaries: IndexMap<String, Boundary>,
    /// The comments selected for each batch to answer.
    pub selection: IndexMap<String, Vec<Chosen>>,
    /// What was left out, and why.
    pub left: Vec<LeftOut>,
}

/// From when a board was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    /// The moment.
    pub since: String,
    /// Where it came from.
    pub origin: String,
}

/// One item copied into the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotItem {
    /// The item, qualified.
    pub id: String,
    /// Its copy in the snapshot source.
    pub snapshot_id: String,
    /// Its version.
    pub version: Option<String>,
    /// Its title.
    pub title: String,
    /// The batch that owns it.
    pub owner: Option<String>,
    /// The host that filed it.
    pub host: Option<String>,
    /// Its URL.
    pub url: Option<String>,
}

/// A batch's boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    /// The moment, if any.
    pub boundary: Option<String>,
    /// Whether it was computed rather than recorded.
    pub computed: bool,
}

/// One comment selected to answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Chosen {
    /// The issue, qualified.
    pub issue: String,
    /// The comment's id.
    pub comment: String,
    /// When it last changed.
    pub last_changed: String,
    /// The SHA-256 of its body.
    pub body_sha256: String,
}

/// One thing left out of the selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeftOut {
    /// The issue, qualified.
    pub issue: String,
    /// The comment, for a comment left out.
    pub comment: Option<String>,
    /// Why.
    pub reason: String,
    /// What was left out.
    pub outcome: LeftOutcome,
}

/// What was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeftOutcome {
    /// A comment.
    Comment,
    /// An item the snapshot could not copy.
    NotCaptured,
    /// An item no comment of which is answered.
    NotAnswered,
}

impl Versioned for Snapshot {
    const FORMAT: &'static str = "snapshot";
    const SCHEMA: u64 = 2;
    const STYLE: Style = Style::INDENTED;
}

/// A snapshot's launch, recorded once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Launched {
    /// When it launched.
    pub claimed_at: String,
    /// How.
    pub launch: Launch,
}

/// How a snapshot launched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Launch {
    /// From a fresh gathering.
    Fresh,
    /// From an existing snapshot.
    From,
}

impl Versioned for Launched {
    const FORMAT: &'static str = "launched record";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style {
        layout: Layout::Line,
        ascii: true,
        sorted: false,
    };
    const TRAILING_NEWLINE: bool = false;
}
