//! The comment-answering pass: `comments gather`, `comments launches`, `comments check` and
//! `comments settle`, which answer people's comments on a host's follow-up tickets, each once,
//! by the batch that owns the ticket, on any destination the store reaches.
//!
//! **Its destination work is three store calls.** Gathering is one comments-since listing of
//! each destination ([`gather`]), narrowed to this host's items at a status whose comments are
//! answered; who owns each item, and what onefollowup last wrote to it, come from this host's
//! own records — each batch's receipts and its tickets in the drafts source. Every step after
//! reads the snapshot the gathering wrote and never the destination. Settle ([`settle`]) makes
//! one batched read of every quoted item, verifying each reply there, and sends every ticket
//! change the answering agents asked for in one `update-many`, each item's change one write
//! held to the version that read returned, and withheld into `pending.json` where the read
//! shows the item differs from what onefollowup last wrote. Under a relation with a rollup,
//! gathering also reads the related tickets of each item a selected comment sits on, in one
//! `task relations` and one `task show-many`.
//!
//! **The watermark moves only in settle**, to the listing's own watermark — the source's time
//! set back by its consistency window — and only for an unscoped snapshot whose every launched
//! batch's account was sound and whose every reply was verified. So a comment no verified
//! reply answers is listed again, and one a verified reply answers is left out as answered.

pub mod check;
pub mod gather;
pub mod launch;
pub mod settle;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use sha2::{Digest as _, Sha256};

use crate::config::{Config, Settings};
use crate::error::Error;
use crate::formats::Versioned as _;
use crate::formats::batch::BatchState;
use crate::formats::gathering::{Gathering, LastWritten};
use crate::formats::snapshot::{Launched, Snapshot};
use crate::store::Store;
use crate::verify::context::{Batch, store_directory};
use crate::verify::journal;
use crate::work::{self, Work};

/// The snapshot's record.
pub const SNAPSHOT_RECORD: &str = "snapshot.json";
/// The snapshot's gathering.
pub const GATHERING_RECORD: &str = "gathering.json";
/// The snapshot's launch, written once.
pub const LAUNCHED_RECORD: &str = "launched.json";
/// The directory holding what onefollowup last wrote of each quoted item.
pub const LAST_WRITTEN: &str = "last-written";
/// The directory holding each launched batch's tickets as they stood at the launch.
pub const TICKETS_AT_LAUNCH: &str = "tickets";

/// What every comment verb shares: the settings, the configuration and the working files.
pub struct Pass<'a> {
    /// The settings.
    pub settings: &'a Settings,
    /// The configuration.
    pub config: &'a Config,
    /// The working files.
    pub work: Work,
}

impl<'a> Pass<'a> {
    /// The pass's context.
    ///
    /// # Errors
    ///
    /// No store configuration.
    pub fn new(settings: &'a Settings) -> Result<Self, Error> {
        Ok(Self {
            settings,
            config: settings.config(),
            work: Work::beside(&store_directory(settings)?),
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
        .with_local(&self.config.drafts_source))
    }

    /// The batch context of `batch`.
    ///
    /// # Errors
    ///
    /// A batch id the configured pattern refuses.
    pub fn batch(&self, batch: &str) -> Result<Batch<'a>, Error> {
        Batch::new(self.settings, batch)
    }

    /// A path a verb was given, made absolute against the working directory.
    #[must_use]
    pub fn given(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.settings.working_directory().join(path)
        }
    }

    /// The metadata key the ticket record sits under.
    #[must_use]
    pub fn record_key(&self) -> String {
        format!("{}.follow-up", self.config.namespace)
    }

    /// Every destination source: the top-level one and each a kind names, each once.
    #[must_use]
    pub fn destinations(&self) -> Vec<String> {
        let mut found = vec![self.config.destination.clone()];
        for kind in &self.config.kinds {
            let destination = kind
                .destination
                .clone()
                .unwrap_or_else(|| self.config.destination.clone());
            if !found.contains(&destination) {
                found.push(destination);
            }
        }
        found
    }

    /// The store categories of the status roles whose comments are answered, each once.
    #[must_use]
    pub fn answered_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = Vec::new();
        for (_, mapping) in self.config.statuses.iter() {
            if mapping.answered && !categories.contains(&mapping.category) {
                categories.push(mapping.category.clone());
            }
        }
        categories
    }
}

/// One item a batch of this host filed, as the batch's own records tell it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    /// The batch.
    pub batch: String,
    /// The ticket's root cause.
    pub slug: String,
    /// The drafts-source item the ticket is stored as.
    pub stored_as: String,
    /// The latest version a receipt recorded for the item.
    pub version: Option<String>,
}

/// Every destination item a batch of this host filed, by the item: read from each batch's
/// receipt journal and state, never from the destination.
///
/// # Errors
///
/// A journal or state that cannot be read.
pub fn owners(pass: &Pass<'_>) -> Result<BTreeMap<String, Owned>, Error> {
    let mut batches: Vec<String> = match std::fs::read_dir(pass.work.receipts_directory()) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .filter_map(|path| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(Error::Failed(format!(
                "{}: could not be read: {error}",
                pass.work.receipts_directory().display()
            )));
        }
    };
    batches.sort();
    let mut owned = BTreeMap::new();
    for batch in batches {
        let journal = work::journal(&pass.work.receipts(&batch))?;
        let Some(state) = work::read::<BatchState>(&pass.work.state(&batch))? else {
            continue;
        };
        let latest = journal::latest(&journal, 0);
        for (stored_as, (card, version)) in journal::bound(&journal) {
            let Some((slug, _)) = state.tickets.iter().find(|(_, item)| **item == stored_as) else {
                continue;
            };
            owned.entry(card.clone()).or_insert_with(|| Owned {
                batch: batch.clone(),
                slug: slug.clone(),
                stored_as: stored_as.clone(),
                version: latest.get(&card).cloned().or(version),
            });
        }
    }
    Ok(owned)
}

/// A snapshot directory and what it holds.
pub struct Snapshotted {
    /// The directory.
    pub directory: PathBuf,
    /// Its name: the stamp of the instant it was gathered at.
    pub stamp: String,
    /// Its record.
    pub snapshot: Snapshot,
    /// Its gathering.
    pub gathering: Gathering,
}

impl Snapshotted {
    /// Read the snapshot at `directory`.
    ///
    /// # Errors
    ///
    /// No snapshot there, or one that does not read.
    pub fn read(directory: &Path) -> Result<Self, Error> {
        let snapshot = work::read::<Snapshot>(&directory.join(SNAPSHOT_RECORD))?.ok_or_else(|| {
            Error::Refused(format!(
                "{} holds no comment snapshot: name the directory `onefollowup comments gather` printed",
                directory.display()
            ))
        })?;
        let gathering = work::read::<Gathering>(&directory.join(GATHERING_RECORD))?
            .ok_or_else(|| {
                Error::Refused(format!(
                    "{} holds no {GATHERING_RECORD}: gather a fresh snapshot with `onefollowup comments gather`",
                    directory.display()
                ))
            })?;
        let stamp = directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self {
            directory: directory.to_path_buf(),
            stamp,
            snapshot,
            gathering,
        })
    }

    /// Its launch, when it was launched.
    ///
    /// # Errors
    ///
    /// A launch record that does not read.
    pub fn launched(&self) -> Result<Option<Launched>, Error> {
        Ok(work::read::<Launched>(
            &self.directory.join(LAUNCHED_RECORD),
        )?)
    }

    /// What onefollowup last wrote of `item`.
    #[must_use]
    pub fn last_written(&self, item: &str) -> Option<&LastWritten> {
        self.gathering
            .last_written
            .iter()
            .find(|written| written.item == item)
    }

    /// The batch owning `item`, as the snapshot recorded it.
    #[must_use]
    pub fn owner(&self, item: &str) -> Option<&str> {
        self.snapshot
            .items
            .iter()
            .find(|one| one.id == item)
            .and_then(|one| one.owner.as_deref())
    }

    /// Whether the snapshot was scoped to one batch or one item.
    #[must_use]
    pub fn scoped(&self) -> bool {
        self.snapshot.scope.is_some() || self.snapshot.issue.is_some()
    }

    /// Where `batch`'s ticket `slug` stood at the launch.
    #[must_use]
    pub fn ticket_at_launch(&self, batch: &str, slug: &str) -> PathBuf {
        self.directory
            .join(TICKETS_AT_LAUNCH)
            .join(batch)
            .join(format!("{slug}.md"))
    }
}

/// Every snapshot directory under `feedback/.snapshots/`, by name.
///
/// # Errors
///
/// A directory that cannot be read.
pub fn snapshot_directories(work: &Work) -> Result<Vec<PathBuf>, Error> {
    let root = work.snapshots();
    let mut found: Vec<PathBuf> = match std::fs::read_dir(&root) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join(SNAPSHOT_RECORD).is_file())
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            return Err(Error::Failed(format!(
                "{}: could not be read: {error}",
                root.display()
            )));
        }
    };
    found.sort();
    Ok(found)
}

/// `text` read as an instant.
#[must_use]
pub fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// `at` as a record spells an instant: whole seconds, `Z`.
#[must_use]
pub fn spelled(at: &DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `at` to the microsecond, as a snapshot records when it read and when it was launched, so
/// two moments within one second are still ordered.
#[must_use]
pub fn precise(at: &DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// A `launchable_for` duration — a whole number of `s`, `m`, `h` or `d` — as a span.
#[must_use]
pub fn span(text: &str) -> Option<chrono::TimeDelta> {
    let (count, unit) = text.split_at(text.len().checked_sub(1)?);
    let count: i64 = count.parse().ok()?;
    match unit {
        "s" => chrono::TimeDelta::try_seconds(count),
        "m" => chrono::TimeDelta::try_minutes(count),
        "h" => chrono::TimeDelta::try_hours(count),
        "d" => chrono::TimeDelta::try_days(count),
        _ => None,
    }
}

/// The digest a snapshot's selection records a comment's body under.
#[must_use]
pub fn body_sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `item`'s file name under the snapshot's `last-written/`.
#[must_use]
pub fn last_written_file(item: &str) -> String {
    let safe: String = item
        .chars()
        .map(|one| {
            if one.is_ascii_alphanumeric() || one == '-' || one == '_' || one == '.' {
                one
            } else {
                '-'
            }
        })
        .collect();
    format!("{LAST_WRITTEN}/{safe}.md")
}

/// The record a [`LastWritten`] holds, as the drafts source wrote it, rendered for a reader.
#[must_use]
pub fn last_written_text(written: &LastWritten) -> String {
    format!(
        "<!-- What onefollowup last wrote to {}: batch `{}`'s ticket `{}`, stored as {}, at version {}. This is not the destination's current text. -->\n---\ntitle: {}\nmetadata: {}\n---\n{}\n",
        written.item,
        written.batch,
        written.ticket,
        written.stored_as,
        written.version.as_deref().unwrap_or("none"),
        serde_json::to_string(&written.title).unwrap_or_default(),
        written.record,
        written.content.trim_end()
    )
}

/// Write `record` to `path` only when nothing is there yet.
///
/// # Errors
///
/// A record already there, or the filesystem's refusal.
pub fn write_once(path: &Path, text: &str) -> Result<bool, Error> {
    use std::io::Write as _;
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).map_err(|error| {
            Error::Failed(format!(
                "{}: could not be created: {error}",
                directory.display()
            ))
        })?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(text.as_bytes())
                .and_then(|()| file.flush())
                .map_err(|error| {
                    Error::Failed(format!("{}: could not be written: {error}", path.display()))
                })?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(Error::Failed(format!(
            "{}: could not be written: {error}",
            path.display()
        ))),
    }
}

/// `launched` rendered as its format spells it.
#[must_use]
pub fn launched_text(launched: &Launched) -> String {
    launched.render()
}
