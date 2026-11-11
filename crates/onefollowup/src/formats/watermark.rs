//! A destination's comment watermark (schema 1), `feedback/.watermark-<board>.json`: from
//! when the next gathering reads comments.

use serde::{Deserialize, Serialize};

use super::Versioned;
use super::pyjson::Style;

/// A board's watermark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watermark {
    /// The board (source) it is for.
    pub board: String,
    /// From when the next gathering reads.
    pub since: String,
    /// When the last gathering read.
    pub queried_at: String,
}

impl Versioned for Watermark {
    const FORMAT: &'static str = "watermark";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::INDENTED;
}
