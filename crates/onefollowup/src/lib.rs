//! `onefollowup`: the follow-up loop for any orchestrator repository, over onetaskgraph.
//!
//! - [`api`] — one function per verb of the `onefollowup` command, the library's surface.
//! - [`config`] — the configuration, its defaults and its layered resolution.
//! - [`formats`] — the record formats, each with its parse and render.
//! - [`store`] — the one module that calls onetaskgraph.
//! - [`verify`] — the verify loop the batch verbs run: tickets, templates, routes, commands.
//! - [`work`] — where a batch's working files live.
//! - [`cli`] — the command line, a thin layer over [`api`].
//!
//! `README.md` documents the command surface, the configuration and the formats.

pub mod api;
pub mod cli;
pub mod config;
pub mod error;
pub mod formats;
pub mod store;
pub mod verify;
pub mod work;

pub use config::{Config, Inputs, Settings};
pub use error::Error;
