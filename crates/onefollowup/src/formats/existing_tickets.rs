//! `existing-tickets.json` (schema 1): every ticket that existed before a batch began,
//! recorded once, so a create never files a ticket twice.

use serde::{Deserialize, Serialize};

use super::Versioned;
use super::pyjson::Style;

/// The tickets that existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExistingTickets {
    /// Their qualified ids, sorted.
    pub tickets: Vec<String>,
}

impl Versioned for ExistingTickets {
    const FORMAT: &'static str = "existing tickets";
    const SCHEMA: u64 = 1;
    const STYLE: Style = Style::INDENTED;
}
