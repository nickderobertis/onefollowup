//! A batch's outbox (schema 1, new in onefollowup v1), `outbox/<batch>.json`: the writes
//! the verifying agent staged with `ticket copy|comment|re-estimate|relate`, which `publish`
//! sends and nothing before it does.

use serde::{Deserialize, Serialize};

use super::pyjson::Style;
use super::{Level, Versioned};

/// A batch's staged writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outbox {
    /// The batch.
    pub batch: String,
    /// Every staged write, in the order it was staged.
    pub writes: Vec<StagedWrite>,
}

/// One staged write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StagedWrite {
    /// Copy a ticket to its destination.
    Copy {
        /// The ticket, qualified.
        ticket: String,
        /// The source it is copied to.
        destination: String,
    },
    /// Comment a batch's evidence or reply on an item.
    Comment {
        /// The item, qualified.
        item: String,
        /// The root cause the comment is for.
        root_cause: String,
        /// The comment's whole body, marker included.
        body: String,
    },
    /// Re-estimate an item's priority.
    ReEstimate {
        /// The item, qualified.
        item: String,
        /// The estimate to write.
        priority_estimate: Level,
    },
    /// Relate two tickets by a configured relation.
    Relate {
        /// The relation's name.
        relation: String,
        /// The `from` ticket, qualified.
        from: String,
        /// The `to` ticket, qualified.
        to: String,
    },
}

impl Versioned for Outbox {
    const FORMAT: &'static str = "outbox";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::INDENTED;
}
