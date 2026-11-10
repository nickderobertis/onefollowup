//! The draft record (schema 1), at the `<namespace>.follow-up-draft` metadata key of a
//! draft, and the drafts project record at `<namespace>.follow-up-drafts`.

use serde::{Deserialize, Serialize};

use super::Versioned;
use super::pyjson::Style;

/// One draft's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftRecord {
    /// The batch the draft belongs to.
    #[serde(rename = "run")]
    pub batch: String,
    /// Who drafted it.
    pub author: Author,
    /// Where it was drafted from.
    pub dispatch: Dispatch,
    /// How to read the drafting session's transcript, when there is one.
    pub transcript: Option<String>,
    /// The repository it is about.
    pub repository: String,
    /// The paths it names.
    pub paths: Vec<String>,
    /// When it was drafted.
    pub drafted_at: String,
    /// The engine's own id for the batch, only where it differs from `run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// Who drafted a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Author {
    /// What kind of author.
    pub kind: AuthorKind,
    /// The node, for a node's draft.
    pub node: Option<String>,
    /// The member, for a monitor's or pacemaker's draft.
    pub member: Option<String>,
}

/// What kind of author drafted a draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthorKind {
    /// A worker on one node.
    Node,
    /// A monitor.
    Monitor,
    /// A pacemaker.
    Pacemaker,
    /// The manager.
    Manager,
}

/// Where a draft was drafted from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dispatch {
    /// The dispatch's scratch directory.
    pub scratch: Option<String>,
    /// The dispatch's session.
    pub session: Option<String>,
    /// The working directory.
    pub cwd: String,
    /// The branch checked out.
    pub branch: Option<String>,
    /// The commit checked out.
    pub head: Option<String>,
}

impl Versioned for DraftRecord {
    const FORMAT: &'static str = "draft record";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::LINE;
    const TRAILING_NEWLINE: bool = false;
}

/// The record of a batch's drafts project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftsProjectRecord {
    /// The batch.
    #[serde(rename = "run")]
    pub batch: String,
    /// The engine's own id for the batch, only where it differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

impl Versioned for DraftsProjectRecord {
    const FORMAT: &'static str = "drafts project record";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::LINE;
    const TRAILING_NEWLINE: bool = false;
}
