//! What every batch verb shares: the batch, its configuration, its working files and the
//! store, opened only by a verb that reads or writes it.

use std::path::{Path, PathBuf};

use crate::config::{Config, Kind, Settings, StatusRole};
use crate::error::Error;
use crate::formats::batch::BatchState;
use crate::formats::outbox::Outbox;
use crate::store::{self, Store};
use crate::verify::template::Located;
use crate::verify::ticket::{self, TicketFile};
use crate::work::{self, Work};

/// One batch's verb context.
pub struct Batch<'a> {
    /// The settings.
    pub settings: &'a Settings,
    /// The configuration.
    pub config: &'a Config,
    /// The batch id.
    pub batch: String,
    /// The working files.
    pub work: Work,
}

/// Whether `id` is a batch id under `pattern`, matched in full.
fn is_batch(pattern: &str, id: &str) -> bool {
    regex::Regex::new(&format!("^(?:{pattern})$")).is_ok_and(|pattern| pattern.is_match(id))
}

/// The directory holding the store's configuration, as every verb resolves it.
///
/// # Errors
///
/// No store configuration given or found.
pub fn store_directory(settings: &Settings) -> Result<PathBuf, Error> {
    let named = settings
        .config()
        .store
        .as_deref()
        .map(|path| settings.path("store", path));
    Ok(store::directory(
        named.as_deref(),
        settings.working_directory(),
    )?)
}

impl<'a> Batch<'a> {
    /// The context of `batch`.
    ///
    /// # Errors
    ///
    /// A batch id the configured pattern refuses, or no store configuration.
    pub fn new(settings: &'a Settings, batch: &str) -> Result<Self, Error> {
        let config = settings.config();
        if !is_batch(&config.batch_id_pattern, batch) {
            return Err(Error::Refused(format!(
                "`{batch}` is not a batch id: `batch_id_pattern` is `{}`, matched in full",
                config.batch_id_pattern
            )));
        }
        Ok(Self {
            settings,
            config,
            batch: batch.to_owned(),
            work: Work::beside(&store_directory(settings)?),
        })
    }

    /// Refuse `id` unless it is a batch id.
    ///
    /// # Errors
    ///
    /// An id the configured pattern refuses.
    pub fn check_id(&self, id: &str) -> Result<(), Error> {
        if is_batch(&self.config.batch_id_pattern, id) {
            Ok(())
        } else {
            Err(Error::Refused(format!(
                "`{id}` is not a batch id: `batch_id_pattern` is `{}`, matched in full",
                self.config.batch_id_pattern
            )))
        }
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

    /// The batch's state, `None` before its first `prepare`.
    ///
    /// # Errors
    ///
    /// A state file that cannot be read.
    pub fn read_state(&self) -> Result<Option<BatchState>, Error> {
        Ok(work::read(&self.work.state(&self.batch))?)
    }

    /// The batch's state.
    ///
    /// # Errors
    ///
    /// A batch not yet prepared.
    pub fn state(&self) -> Result<BatchState, Error> {
        self.read_state()?.ok_or_else(|| {
            Error::Refused(format!(
                "batch `{}` has not been prepared: run `onefollowup prepare --batch {}` first",
                self.batch, self.batch
            ))
        })
    }

    /// Write the batch's state.
    ///
    /// # Errors
    ///
    /// The filesystem's refusal.
    pub fn save_state(&self, state: &BatchState) -> Result<(), Error> {
        Ok(work::write(&self.work.state(&self.batch), state)?)
    }

    /// The batch's outbox, empty when nothing is staged.
    ///
    /// # Errors
    ///
    /// An outbox that cannot be read.
    pub fn outbox(&self) -> Result<Outbox, Error> {
        Ok(
            work::read(&self.work.outbox(&self.batch))?.unwrap_or_else(|| Outbox {
                batch: self.batch.clone(),
                writes: Vec::new(),
            }),
        )
    }

    /// Write the batch's outbox.
    ///
    /// # Errors
    ///
    /// The filesystem's refusal.
    pub fn save_outbox(&self, outbox: &Outbox) -> Result<(), Error> {
        Ok(work::write(&self.work.outbox(&self.batch), outbox)?)
    }

    /// The configured kind `name`.
    #[must_use]
    pub fn kind(&self, name: &str) -> Option<&'a Kind> {
        self.config.kinds.iter().find(|kind| kind.name == name)
    }

    /// The source a kind's tickets go to.
    #[must_use]
    pub fn destination_of(&self, kind: &Kind) -> String {
        kind.destination
            .clone()
            .unwrap_or_else(|| self.config.destination.clone())
    }

    /// Every destination source: the top-level one and each a kind names, each once.
    #[must_use]
    pub fn destinations(&self) -> Vec<String> {
        let mut found = vec![self.config.destination.clone()];
        for kind in &self.config.kinds {
            let destination = self.destination_of(kind);
            if !found.contains(&destination) {
                found.push(destination);
            }
        }
        found
    }

    /// Whether `item` is a qualified id of a destination source.
    #[must_use]
    pub fn is_destination_item(&self, item: &str) -> bool {
        item.split_once(':').is_some_and(|(source, native)| {
            !native.is_empty() && self.destinations().iter().any(|one| one == source)
        })
    }

    /// A kind's ticket template.
    #[must_use]
    pub fn ticket_template(&self, kind: Option<&Kind>) -> Located {
        let configured = kind
            .and_then(|kind| kind.ticket_template.clone())
            .unwrap_or_else(|| crate::api::BUILTIN_TICKET_TEMPLATE.to_owned());
        Located::of(&configured, |path| self.settings.path("kinds", path))
    }

    /// The verifying agent's task template.
    #[must_use]
    pub fn task_template(&self) -> Located {
        Located::of(&self.config.task_template, |path| {
            self.settings.path("task_template", path)
        })
    }

    /// The batch's key for the ticket `slug`, as the outbox names it.
    #[must_use]
    pub fn ticket_key(&self, slug: &str) -> String {
        format!(
            "{}:{}/tickets/{slug}",
            self.config.drafts_source, self.batch
        )
    }

    /// The slug of the ticket `key` names, when it names one of this batch's.
    #[must_use]
    pub fn ticket_slug(&self, key: &str) -> Option<String> {
        key.strip_prefix(&format!(
            "{}:{}/tickets/",
            self.config.drafts_source, self.batch
        ))
        .map(str::to_owned)
    }

    /// Read the ticket `name` — its root cause, or its file.
    ///
    /// # Errors
    ///
    /// No such ticket file.
    pub fn ticket(&self, name: &str) -> Result<TicketFile, Error> {
        let given = Path::new(name);
        let path = if name.ends_with(".md") || given.components().count() > 1 {
            if given.is_absolute() {
                given.to_path_buf()
            } else {
                self.settings.working_directory().join(given)
            }
        } else {
            self.work.ticket(&self.batch, name)
        };
        let slug = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = work::read_text(&path)?.ok_or_else(|| {
            Error::Refused(format!(
                "there is no ticket `{name}`: write it as {}",
                self.work
                    .ticket(&self.batch, if slug.is_empty() { name } else { &slug })
                    .display()
            ))
        })?;
        Ok(ticket::parse(&slug, &path, &text, &self.config.namespace))
    }

    /// Every ticket file under the batch's tickets directory, by slug.
    ///
    /// # Errors
    ///
    /// A directory that cannot be read.
    pub fn tickets(&self) -> Result<Vec<TicketFile>, Error> {
        let directory = self.work.tickets(&self.batch);
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(Error::Failed(format!(
                    "{}: could not be read: {error}",
                    directory.display()
                )));
            }
        };
        let mut slugs: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
            .filter_map(|path| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })
            .collect();
        slugs.sort();
        slugs.iter().map(|slug| self.ticket(slug)).collect()
    }

    /// The status role a store category is held at: the first role mapping to it.
    #[must_use]
    pub fn role_of(&self, category: &str) -> Option<StatusRole> {
        self.config
            .statuses
            .iter()
            .into_iter()
            .find(|(_, mapping)| mapping.category == category)
            .map(|(role, _)| role)
    }

    /// Whether an item at `category` is protected from a withdrawal.
    #[must_use]
    pub fn protected(&self, category: &str) -> bool {
        self.config
            .statuses
            .iter()
            .into_iter()
            .any(|(_, mapping)| mapping.category == category && mapping.protected)
    }

    /// The store category a status role is written as.
    #[must_use]
    pub fn category_of(&self, role: StatusRole) -> String {
        self.config
            .statuses
            .iter()
            .into_iter()
            .find(|(one, _)| *one == role)
            .map(|(_, mapping)| mapping.category.clone())
            .unwrap_or_default()
    }

    /// The metadata key the ticket record sits under.
    #[must_use]
    pub fn record_key(&self) -> String {
        format!("{}.follow-up", self.config.namespace)
    }

    /// The metadata key a draft's record sits under.
    #[must_use]
    pub fn draft_key(&self) -> String {
        format!("{}.follow-up-draft", self.config.namespace)
    }
}
