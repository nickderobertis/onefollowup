//! The store: every access onefollowup makes to drafts, tickets and destinations.
//!
//! **This is the one module that calls onetaskgraph**, and it calls only the public API of
//! its crates (`tests/architecture.rs` holds that). Nothing here names a destination's API,
//! a plugin's internals, or branches on a plugin or destination name: a destination is a
//! source onetaskgraph's configuration declares, and what it can do is onetaskgraph's to say.

use std::path::{Path, PathBuf};

use onetaskgraph_core::config::{self, Layer};
use onetaskgraph_core::{Engine, Environment, SourceState};
use onetaskgraph_plugin_api::StatusCategory;

/// The store status categories, in onetaskgraph's spelling, that a status role may map to.
/// `tests/store.rs` reconciles this list with onetaskgraph's own vocabulary.
pub const STATUS_CATEGORIES: &[&str] = &[
    "draft",
    "backlog",
    "todo",
    "queued",
    "in-progress",
    "done",
    "cancelled",
];

/// The name of the onetaskgraph configuration document a `store` directory holds.
pub const STORE_DOCUMENT: &str = "onetaskgraph.yaml";

/// Whether `name` is a store status category a status role may map to: one onetaskgraph's
/// status vocabulary names, other than the `unknown` it reports for a status it cannot place.
#[must_use]
pub fn is_status_category(name: &str) -> bool {
    serde_json::from_value::<StatusCategory>(serde_json::Value::from(name))
        .is_ok_and(|category| category != StatusCategory::Unknown)
}

/// A store that could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The `store` setting names something that is not an onetaskgraph configuration.
    #[error(
        "the store `{0}` is not an onetaskgraph configuration: name an onetaskgraph.yaml or the directory holding one"
    )]
    NotAConfiguration(PathBuf),
    /// onetaskgraph refused its configuration.
    #[error("the store's configuration was refused: {0}")]
    Refused(String),
}

/// One source the store offers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Source {
    /// The source's name.
    pub name: String,
    /// Whether it built.
    pub available: bool,
}

/// An opened store.
pub struct Store {
    engine: Engine,
}

impl Store {
    /// Open the store: the onetaskgraph configuration at `store` (an `onetaskgraph.yaml` or
    /// the directory holding one), or else the one onetaskgraph discovers from
    /// `working_directory`, read with `environment` as onetaskgraph reads its own.
    ///
    /// # Errors
    ///
    /// Refused when `store` is not an onetaskgraph configuration, or onetaskgraph refuses it.
    pub fn open(
        store: Option<&Path>,
        working_directory: &Path,
        environment: &[(String, String)],
    ) -> Result<Self, StoreError> {
        let directory = match store {
            None => working_directory.to_path_buf(),
            Some(path) if path.is_dir() && path.join(STORE_DOCUMENT).is_file() => {
                path.to_path_buf()
            }
            Some(path)
                if path.is_file()
                    && path.file_name().is_some_and(|name| name == STORE_DOCUMENT) =>
            {
                path.parent()
                    .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
            }
            Some(path) => return Err(StoreError::NotAConfiguration(path.to_path_buf())),
        };
        let environment = Environment::from_pairs(environment.iter().cloned());
        let loaded = config::load(&directory, &environment, &Layer::new(Vec::new()))
            .map_err(|error| StoreError::Refused(error.to_string()))?;
        Ok(Self {
            engine: Engine::build(&loaded.config, &loaded.secrets),
        })
    }

    /// Every source the store's configuration declares, in name order.
    #[must_use]
    pub fn sources(&self) -> Vec<Source> {
        self.engine
            .listing()
            .into_iter()
            .map(|listing| Source {
                name: listing.source.to_string(),
                available: matches!(listing.state, SourceState::Available { .. }),
            })
            .collect()
    }
}
