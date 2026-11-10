//! A batch's dispositions (schema 1), `dispositions/<batch>.json`: what each input became,
//! and the carried dispositions, `dispositions/<batch>.carried.json`, that a re-opened
//! account keeps.

use serde::{Deserialize, Serialize};

use super::Versioned;
use super::pyjson::Style;

/// A batch's dispositions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dispositions {
    /// The batch.
    #[serde(rename = "run")]
    pub batch: String,
    /// Every input the batch was prepared with, by qualified id, sorted.
    pub drafts: Vec<String>,
    /// What each became.
    pub dispositions: Vec<DispositionEntry>,
}

/// What one input became.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispositionEntry {
    /// The input's qualified id.
    pub draft: String,
    /// The disposition's name.
    pub disposition: String,
    /// The root causes it supports, for a disposition that files a ticket.
    pub root_causes: Vec<String>,
    /// What the disposition requires it to say.
    pub detail: String,
    /// The input's kind; `check` refuses an entry without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl Versioned for Dispositions {
    const FORMAT: &'static str = "dispositions";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::INDENTED;
}

/// The dispositions a re-opened account carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarriedDispositions {
    /// The batch.
    #[serde(rename = "run")]
    pub batch: String,
    /// The entries carried.
    pub dispositions: Vec<DispositionEntry>,
}

impl Versioned for CarriedDispositions {
    const FORMAT: &'static str = "carried dispositions";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::INDENTED;
}
