//! The grooming pass: `groom prepare`, `groom check` and `groom publish`, which verify,
//! deduplicate and close a destination's open tickets — prepare, the verifying agents, check
//! until it passes, then publish sending the plan.
//!
//! **The agent only judges**, by filling a verdict record: whether a ticket is fixed or
//! obsolete, and whether two tickets share one root cause. Selection, checking that judgement's
//! evidence, clustering, choosing survivors and protecting claimed and accepted work are code.
//!
//! ```text
//! .onefollowup/groom/
//!   clones/<host>/<owner>/<name>/   each repository's blob-less clone (`groom.clones`)
//!   <batch>/batch.json              what prepare listed, fetched and selected
//!   <batch>/snapshot.json           every open ticket, for duplicate detection
//!   <batch>/<shard>/task.md         one shard's task
//!   <batch>/<shard>/verdicts.jsonl  the verdicts its agent writes
//!   <batch>/plan.json               what the latest passing check planned
//!   <batch>/receipts.json           what publish sent and heard back
//!   <batch>/report.md, report.json  what publish did, each ticket left for a person among it
//! ```
//!
//! Once publish ran, the batch holds only the plan, the receipts and the reports.

pub mod check;
pub mod git;
pub mod prepare;
pub mod publish;

use std::path::{Path, PathBuf};

use crate::config::{Config, Settings, StatusRole};
use crate::error::Error;
use crate::formats::groom::Protection;
use crate::store::Store;
use crate::verify::context::store_directory;
use crate::work::WORK_DIRECTORY;

/// The directory under the work root every grooming batch is in.
pub const GROOM_DIRECTORY: &str = "groom";
/// A batch's record.
pub const BATCH_RECORD: &str = "batch.json";
/// A batch's snapshot of every open ticket.
pub const SNAPSHOT_RECORD: &str = "snapshot.json";
/// A batch's plan.
pub const PLAN_RECORD: &str = "plan.json";
/// A shard's task.
pub const TASK: &str = "task.md";
/// A shard's verdicts.
pub const VERDICTS: &str = "verdicts.jsonl";

/// The store categories of an open ticket.
pub const OPEN_CATEGORIES: [&str; 7] = [
    "draft",
    "backlog",
    "todo",
    "queued",
    "in-progress",
    "in-review",
    "unknown",
];

/// What both grooming verbs share: the settings and where the batches are.
pub struct Groom<'a> {
    /// The settings.
    pub settings: &'a Settings,
    /// The configuration.
    pub config: &'a Config,
    /// The store's directory.
    pub store_directory: PathBuf,
}

impl<'a> Groom<'a> {
    /// The pass's context.
    ///
    /// # Errors
    ///
    /// No store configuration.
    pub fn new(settings: &'a Settings) -> Result<Self, Error> {
        Ok(Self {
            settings,
            config: settings.config(),
            store_directory: store_directory(settings)?,
        })
    }

    /// Open the store, counting the drafts source as local.
    ///
    /// # Errors
    ///
    /// A store that cannot be opened.
    pub fn store(&self) -> Result<Store, Error> {
        let named = self
            .config
            .store
            .as_deref()
            .map(|path| self.settings.path("store", path));
        Ok(Store::open(
            named.as_deref(),
            self.settings.working_directory(),
            self.settings.environment(),
        )?
        .with_local(&self.config.drafts_source)
        .with_governor(&self.config.governor))
    }

    /// The `git` the verb runs.
    #[must_use]
    pub fn git(&self) -> git::Git<'_> {
        git::Git {
            working_directory: self.settings.working_directory(),
            environment: self.settings.environment(),
        }
    }

    /// Where every grooming batch is.
    #[must_use]
    pub fn root(&self) -> PathBuf {
        self.store_directory
            .join(WORK_DIRECTORY)
            .join(GROOM_DIRECTORY)
    }

    /// The batch `batch`'s directory.
    #[must_use]
    pub fn batch(&self, batch: &str) -> PathBuf {
        self.root().join(batch)
    }

    /// Where each repository's clone is kept: `groom.clones`, against the store's directory
    /// when relative.
    #[must_use]
    pub fn clones(&self) -> PathBuf {
        let clones = Path::new(&self.config.groom.clones);
        if clones.is_absolute() {
            clones.to_path_buf()
        } else {
            self.store_directory.join(clones)
        }
    }

    /// The protection a ticket at the store category `category` has, by the status roles.
    #[must_use]
    pub fn protection(&self, category: &str) -> Protection {
        let role_of = |role: StatusRole| {
            self.config
                .statuses
                .iter()
                .into_iter()
                .find(|(each, _)| *each == role)
                .is_some_and(|(_, mapping)| mapping.category == category)
        };
        if role_of(StatusRole::Queued)
            || role_of(StatusRole::InProgress)
            || role_of(StatusRole::InReview)
        {
            Protection::Claimed
        } else if role_of(StatusRole::Accepted) {
            Protection::Todo
        } else {
            Protection::Unprotected
        }
    }
}

/// A batch id `groom check` is given: `groom-` and word characters, never a path.
///
/// # Errors
///
/// Anything else, refused.
pub fn batch_id(batch: &str) -> Result<&str, Error> {
    let valid = batch.strip_prefix("groom-").is_some_and(|stamp| {
        !stamp.is_empty() && stamp.chars().all(|one| one.is_ascii_alphanumeric())
    });
    if valid {
        Ok(batch)
    } else {
        Err(Error::Refused(format!(
            "`{batch}` is not a grooming batch id: `groom-` and the stamp `groom prepare` answered"
        )))
    }
}
