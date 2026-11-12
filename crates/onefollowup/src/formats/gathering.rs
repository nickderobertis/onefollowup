//! A comment snapshot's gathering (schema 1, new in onefollowup),
//! `feedback/.snapshots/<stamp>/gathering.json`: what `comments gather` read beside the
//! snapshot's own record — each destination's comments-since listing as the store answered
//! it, the ticket onefollowup last wrote to each item a selected comment sits on, and, under a
//! relation with a rollup, the relations and related tickets of those items.
//!
//! `snapshot.json` says what was selected and why; this holds what every later step reads in
//! place of the destination, so nothing after the gathering reads it again until settle.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Versioned;
use super::batch::Read;
use super::pyjson::Style;
use crate::store::{ItemRelations, Listing};

/// What one gathering read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gathering {
    /// Each destination's listing, by its source.
    pub listings: IndexMap<String, Listing>,
    /// The ticket onefollowup last wrote to each item a selected comment sits on.
    pub last_written: Vec<LastWritten>,
    /// What the one `task relations` read of those items held, under a relation with a
    /// rollup; omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<ItemRelations>,
    /// The tickets at those relations' other ends, with their comments; omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Read>,
}

/// The ticket onefollowup last wrote to one destination item: the batch's own copy of it in
/// the drafts source, which the item's copy carried. It is not the destination's current text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LastWritten {
    /// The destination item, qualified.
    pub item: String,
    /// The batch that owns it.
    pub batch: String,
    /// The ticket's root cause, which names its file.
    pub ticket: String,
    /// The drafts-source item the ticket is stored as.
    pub stored_as: String,
    /// The version the batch's receipts last recorded for the item.
    pub version: Option<String>,
    /// The ticket's title.
    pub title: String,
    /// Its content.
    pub content: String,
    /// Its record, as stored.
    pub record: Value,
}

impl Versioned for Gathering {
    const FORMAT: &'static str = "gathering";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::INDENTED;
}
