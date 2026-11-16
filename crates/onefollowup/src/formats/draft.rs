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

impl Author {
    /// Where the draft came from, as a ticket record's `draft_sources` holds it: `author
    /// <kind>`, then `; node <node>` and `; member <member>` where it names them.
    #[must_use]
    pub fn source(&self) -> String {
        let mut line = format!("author {}", self.kind.as_str());
        if let Some(node) = &self.node {
            line.push_str(&format!("; node {node}"));
        }
        if let Some(member) = &self.member {
            line.push_str(&format!("; member {member}"));
        }
        line
    }
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

impl AuthorKind {
    /// The kind as a record spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Monitor => "monitor",
            Self::Pacemaker => "pacemaker",
            Self::Manager => "manager",
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{Author, AuthorKind};

    #[test]
    fn a_drafts_source_names_its_kind_then_its_node_and_member() {
        for (kind, node, member, source) in [
            (
                AuthorKind::Node,
                Some("verify-cursor"),
                None,
                "author node; node verify-cursor",
            ),
            (
                AuthorKind::Monitor,
                None,
                Some("m-1"),
                "author monitor; member m-1",
            ),
            (
                AuthorKind::Pacemaker,
                Some("n"),
                Some("p-2"),
                "author pacemaker; node n; member p-2",
            ),
            (AuthorKind::Manager, None, None, "author manager"),
        ] {
            let author = Author {
                kind,
                node: node.map(str::to_owned),
                member: member.map(str::to_owned),
            };
            assert_eq!(author.source(), source);
        }
    }
}
