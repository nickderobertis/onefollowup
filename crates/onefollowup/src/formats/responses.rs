//! The responses to one comment-answering gathering (schema 3),
//! `feedback/<batch>/<stamp>.responses.json`: what was done about each quoted comment.

use serde::{Deserialize, Serialize};

use super::pyjson::Style;
use super::{Verdict, Versioned};

/// The responses to one gathering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Responses {
    /// The batch.
    #[serde(rename = "run")]
    pub batch: String,
    /// The feedback file's name.
    pub feedback: String,
    /// One per quoted comment, in quoted order.
    pub responses: Vec<ResponseEntry>,
}

/// What was done about one quoted comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseEntry {
    /// The quoted comment's id.
    pub comment: String,
    /// The issue it sits on, qualified.
    pub issue: String,
    /// What was done.
    pub action: String,
    /// The reply's comment id.
    pub reply: String,
    /// The reply's URL, as the store gave it.
    pub reply_url: Option<String>,
    /// Why the reply's write read the board first, when it did.
    pub reply_fallback: Option<String>,
    /// Whether the comment confirms the ticket's root cause.
    pub verdict: Verdict,
}

impl Versioned for Responses {
    const FORMAT: &'static str = "responses";
    const SCHEMA: u64 = 3;
    const STYLE: Style = Style::INDENTED;
}
