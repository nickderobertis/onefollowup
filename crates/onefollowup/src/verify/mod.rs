//! The verify loop: what `draft`, `prepare`, the agent's `ticket` tools, `check`, `publish`
//! and `settle` do. Each verb's public function is in [`crate::api`]; it calls the one here.
//!
//! - [`context`] — the batch, its configuration and its working files.
//! - [`ticket`] — a batch's ticket file, its rules, its estimate and its digest.
//! - [`decide`] — what a ticket's copy, or an item's re-estimate, would write.
//! - [`routes`], [`command`], [`template`] — routes, configured commands and templates.
//! - [`journal`] — what the receipt journal says.

pub mod check;
pub mod command;
pub mod context;
pub mod decide;
pub mod draft;
pub mod journal;
pub mod prepare;
pub mod publish;
pub mod routes;
pub mod settle;
pub mod template;
pub mod ticket;
pub mod tools;
