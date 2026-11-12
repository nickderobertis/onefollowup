//! A batch's state (schema 1, new in onefollowup), `state/<batch>.json`: the input set
//! `prepare` snapshotted, what it read of the destination, the drafts-source item each of the
//! batch's tickets is stored as, the outbox the latest passing `check` held, and each
//! `publish`'s writes until `settle` closes them out.
//!
//! It is onefollowup's own working file: the verifying agent reads `prepare`'s task and
//! answers, never this.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::Versioned;
use super::outbox::StagedWrite;
use super::pyjson::Style;
use crate::store::{ItemRelations, StoreComment, StoreItem};

/// A batch's state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchState {
    /// The batch.
    pub batch: String,
    /// Other ids the batch's drafts were recorded under.
    pub aliases: Vec<String>,
    /// The drafts source's project the batch's drafts and tickets are filed under.
    pub project: String,
    /// The input set, in the order it was snapshotted: the drafts, then the hook's inputs.
    pub inputs: Vec<Input>,
    /// What `prepare` read of the destination, and what `ticket search` added.
    pub reads: Vec<Read>,
    /// How many receipts the journal held when `prepare` read: a receipt past these is newer
    /// than the read.
    pub read_at_receipt: usize,
    /// Each field value `prepare`'s search asked about, as `<field>=<value>`.
    pub searched_values: Vec<String>,
    /// Each ticket's root cause, and the drafts-source item it is stored as.
    pub tickets: IndexMap<String, String>,
    /// Each ticket `ticket copy` treated as an existing item of its kind.
    pub merged: Vec<Merged>,
    /// What the latest passing `check` held: each staged write's digest, in outbox order;
    /// `None` when none has passed since the batch was last prepared or published.
    pub checked: Option<Vec<String>>,
    /// Each `publish`'s writes, oldest first.
    pub publishes: Vec<PublishRecord>,
    /// What `prepare`'s one `task relations` read of the found tickets held, under a
    /// configuration with relations; omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<ItemRelations>,
}

/// One input of the batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Its id: a draft's qualified id, or the id the hook gave.
    pub id: String,
    /// Where it came from: `draft`, or the hook's `source`.
    pub source: String,
    /// Its title or summary.
    pub title: String,
    /// The repository a draft is about.
    pub repository: Option<String>,
    /// The paths a draft names.
    pub paths: Vec<String>,
    /// Its body or detail.
    pub body: String,
}

/// One destination item read, with its comments when they were read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    /// The item as read.
    pub item: StoreItem,
    /// Its comments, when they were read.
    pub comments: Option<Vec<StoreComment>>,
    /// The searches that found it.
    pub found_by: Vec<String>,
}

/// A ticket `ticket copy` treated as an existing item, its evidence staged there instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Merged {
    /// The ticket's root cause.
    pub ticket: String,
    /// The item it was treated as, or the root cause of the staged ticket it was.
    pub into: String,
}

/// One `publish`'s writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishRecord {
    /// How many receipts the journal held before it.
    pub journal_from: usize,
    /// The writes it sent, in outbox order.
    pub writes: Vec<StagedWrite>,
    /// Each copied ticket's digest, by its staged write's index.
    pub digests: IndexMap<String, String>,
    /// Whether `settle` closed it out.
    pub settled: bool,
}

impl Versioned for BatchState {
    const FORMAT: &'static str = "batch state";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::INDENTED;
}
