//! Where a batch's working files live, and how each is read and written.
//!
//! **The work root is `.onefollowup/` beside the store's `onetaskgraph.yaml`**, whatever the
//! working directory a verb runs in: every verb resolves the store's directory the same way
//! ([`crate::store::directory`]), so one batch's files are found from any subdirectory. A
//! host gitignores `.onefollowup/`.
//!
//! ```text
//! .onefollowup/
//!   state/<batch>.json                  the batch state (formats::batch)
//!   dispositions/<batch>.json           the verifying agent's answers
//!   dispositions/<batch>.carried.json   answers a re-prepare carries
//!   outbox/<batch>.json                 the staged writes
//!   receipts/<batch>.jsonl              one receipt per write outcome
//!   tasks/<batch>/task.md               the verifying agent's task
//!   tasks/<batch>/existing-tickets.json the destination's tickets prepare found
//!   tasks/<batch>/tickets/<slug>.md     the batch's tickets, written by the agent
//!   feedback/pending.json               writes that did not certainly land
//!   feedback/.watermark-<source>.json   from when the next gathering lists a destination
//!   feedback/.snapshots/<stamp>/        one comment gathering: snapshot.json, gathering.json,
//!                                       launched.json, last-written/ and tickets/
//!   feedback/<batch>/<stamp>.md         a batch's feedback task from one snapshot
//!   feedback/<batch>/<stamp>.responses.json   the answering agent's account of it
//! ```

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::formats::Versioned;
use crate::formats::receipts::ReceiptJournal;

/// The directory under the store's that holds every batch's working files.
pub const WORK_DIRECTORY: &str = ".onefollowup";

/// A working file that could not be read or written.
#[derive(Debug, thiserror::Error)]
#[error("{path}: {message}")]
pub struct WorkError {
    /// The file.
    pub path: PathBuf,
    /// What went wrong, and what to do.
    pub message: String,
}

/// The work root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Work {
    root: PathBuf,
}

impl Work {
    /// The work root beside the store whose configuration is in `store_directory`.
    #[must_use]
    pub fn beside(store_directory: &Path) -> Self {
        Self {
            root: store_directory.join(WORK_DIRECTORY),
        }
    }

    /// `state/<batch>.json`.
    #[must_use]
    pub fn state(&self, batch: &str) -> PathBuf {
        self.root.join("state").join(format!("{batch}.json"))
    }

    /// `dispositions/<batch>.json`.
    #[must_use]
    pub fn dispositions(&self, batch: &str) -> PathBuf {
        self.root.join("dispositions").join(format!("{batch}.json"))
    }

    /// `dispositions/<batch>.carried.json`.
    #[must_use]
    pub fn carried(&self, batch: &str) -> PathBuf {
        self.root
            .join("dispositions")
            .join(format!("{batch}.carried.json"))
    }

    /// `outbox/<batch>.json`.
    #[must_use]
    pub fn outbox(&self, batch: &str) -> PathBuf {
        self.root.join("outbox").join(format!("{batch}.json"))
    }

    /// `receipts/<batch>.jsonl`.
    #[must_use]
    pub fn receipts(&self, batch: &str) -> PathBuf {
        self.root.join("receipts").join(format!("{batch}.jsonl"))
    }

    /// `tasks/<batch>/`.
    #[must_use]
    pub fn tasks(&self, batch: &str) -> PathBuf {
        self.root.join("tasks").join(batch)
    }

    /// `tasks/<batch>/task.md`.
    #[must_use]
    pub fn task(&self, batch: &str) -> PathBuf {
        self.tasks(batch).join("task.md")
    }

    /// `tasks/<batch>/existing-tickets.json`.
    #[must_use]
    pub fn existing_tickets(&self, batch: &str) -> PathBuf {
        self.tasks(batch).join("existing-tickets.json")
    }

    /// `tasks/<batch>/tickets/`.
    #[must_use]
    pub fn tickets(&self, batch: &str) -> PathBuf {
        self.tasks(batch).join("tickets")
    }

    /// `tasks/<batch>/tickets/<slug>.md`.
    #[must_use]
    pub fn ticket(&self, batch: &str, slug: &str) -> PathBuf {
        self.tickets(batch).join(format!("{slug}.md"))
    }

    /// `feedback/pending.json`.
    #[must_use]
    pub fn pending(&self) -> PathBuf {
        self.feedback().join("pending.json")
    }

    /// `feedback/`.
    #[must_use]
    pub fn feedback(&self) -> PathBuf {
        self.root.join("feedback")
    }

    /// `feedback/.watermark-<source>.json`.
    #[must_use]
    pub fn watermark(&self, source: &str) -> PathBuf {
        self.feedback().join(format!(".watermark-{source}.json"))
    }

    /// `feedback/.snapshots/`.
    #[must_use]
    pub fn snapshots(&self) -> PathBuf {
        self.feedback().join(".snapshots")
    }

    /// `feedback/<batch>/<stamp>.md`.
    #[must_use]
    pub fn feedback_file(&self, batch: &str, stamp: &str) -> PathBuf {
        self.feedback().join(batch).join(format!("{stamp}.md"))
    }

    /// `receipts/`.
    #[must_use]
    pub fn receipts_directory(&self) -> PathBuf {
        self.root.join("receipts")
    }
}

fn failed(path: &Path, message: impl std::fmt::Display) -> WorkError {
    WorkError {
        path: path.to_path_buf(),
        message: message.to_string(),
    }
}

/// The text of `path`, or `None` when there is no such file.
///
/// # Errors
///
/// A file that is there and cannot be read.
pub fn read_text(path: &Path) -> Result<Option<String>, WorkError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failed(path, format!("could not be read: {error}"))),
    }
}

/// Write `text` to `path` whole, through a staging file and a rename, creating its directory.
///
/// # Errors
///
/// The filesystem's refusal.
pub fn write_text(path: &Path, text: &str) -> Result<(), WorkError> {
    let directory = path
        .parent()
        .ok_or_else(|| failed(path, "names no directory"))?;
    std::fs::create_dir_all(directory)
        .map_err(|error| failed(directory, format!("could not be created: {error}")))?;
    let staging = directory.join(format!(
        ".{}.staging",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    std::fs::write(&staging, text)
        .map_err(|error| failed(&staging, format!("could not be written: {error}")))?;
    std::fs::rename(&staging, path)
        .map_err(|error| failed(path, format!("could not be written: {error}")))
}

/// Read the record of format `T` at `path`, or `None` when there is none.
///
/// # Errors
///
/// A file that cannot be read, or that is not of the format.
pub fn read<T: Versioned>(path: &Path) -> Result<Option<T>, WorkError> {
    read_text(path)?
        .map(|text| T::parse(&text).map_err(|error| failed(path, error)))
        .transpose()
}

/// Write `record` to `path`.
///
/// # Errors
///
/// The filesystem's refusal.
pub fn write<T: Versioned>(path: &Path, record: &T) -> Result<(), WorkError> {
    write_text(path, &record.render())
}

/// The batch's receipt journal, empty when it has none yet.
///
/// # Errors
///
/// A journal that cannot be read or is not of the format.
pub fn journal(path: &Path) -> Result<ReceiptJournal, WorkError> {
    match read_text(path)? {
        Some(text) => ReceiptJournal::parse(&text).map_err(|error| failed(path, error)),
        None => Ok(ReceiptJournal::default()),
    }
}

/// Append `lines` to the journal at `path`, each already rendered, never rewriting a line.
///
/// # Errors
///
/// The filesystem's refusal: a journal that cannot be opened for appending.
pub fn append(path: &Path, lines: &[String]) -> Result<(), WorkError> {
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|error| failed(directory, format!("could not be created: {error}")))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| failed(path, format!("could not be opened to append: {error}")))?;
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    file.write_all(text.as_bytes())
        .and_then(|()| file.flush())
        .map_err(|error| failed(path, format!("could not be appended to: {error}")))
}

/// Remove `path` when it is there.
///
/// # Errors
///
/// A file that is there and cannot be removed.
pub fn remove(path: &Path) -> Result<bool, WorkError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(failed(path, format!("could not be removed: {error}"))),
    }
}
