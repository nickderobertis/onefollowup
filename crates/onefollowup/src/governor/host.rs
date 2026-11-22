//! The governor's host-level state: one file per backend kind in `governor.state_dir`, shared
//! by every onefollowup process on the host and written by nothing but the governor.
//!
//! Each read-modify-write holds an exclusive lock on the backend's `<backend>.lock` beside it,
//! so two processes never interleave one; the file itself is replaced through a staging file
//! and a rename, so a reader never sees half of one. A process holds `processes/<id>.lock` for
//! its whole life: a lock anyone else can take is a process that is gone, whose cost in flight
//! is dropped from the state. A missing, unreadable or other-schema state is no history, never
//! a refusal, and a state that cannot be written leaves the process governing on its own.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The host state's one schema: a file of another is read as no history and overwritten.
pub const SCHEMA_VERSION: u32 = 1;

/// Where this process's liveness locks live, under the state directory.
const PROCESSES: &str = "processes";

/// One governor's handle on the host state directory.
#[derive(Debug)]
pub struct Host {
    directory: PathBuf,
    process: String,
    /// Held, locked, for the governor's life: what tells other processes this one is alive.
    _alive: File,
}

#[derive(Serialize, Deserialize)]
struct Document<T> {
    schema_version: u32,
    #[serde(flatten)]
    state: T,
}

/// A name every process on the host tells apart: the process id, when it started, and a count
/// for each governor one process builds.
#[must_use]
pub fn process_name() -> String {
    static BUILT: AtomicU64 = AtomicU64::new(0);
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!(
        "{}-{started}-{}",
        std::process::id(),
        BUILT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A backend kind as a file name: anything but ASCII letters, digits, `-` and `_` replaced.
fn file_stem(backend: &str) -> String {
    backend
        .chars()
        .map(|one| {
            if one.is_ascii_alphanumeric() || one == '-' || one == '_' {
                one
            } else {
                '_'
            }
        })
        .collect()
}

impl Host {
    /// Open `directory` for the process `process`, creating it, and mark the process alive.
    ///
    /// # Errors
    ///
    /// The directory or the liveness lock cannot be made: the caller governs on its own.
    pub fn open(directory: &Path, process: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(directory.join(PROCESSES))?;
        let alive = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.join(PROCESSES).join(format!("{process}.lock")))?;
        alive.lock()?;
        Ok(Self {
            directory: directory.to_path_buf(),
            process: process.to_owned(),
            _alive: alive,
        })
    }

    /// The directory this handle shares.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Whether the process `name` still holds its liveness lock. A process whose lock file is
    /// gone, or whose lock anyone can take, is over.
    #[must_use]
    pub fn alive(&self, name: &str) -> bool {
        if name == self.process {
            return true;
        }
        let path = self.directory.join(PROCESSES).join(format!("{name}.lock"));
        let Ok(file) = OpenOptions::new().write(true).open(&path) else {
            return false;
        };
        match file.try_lock() {
            Ok(()) => {
                drop(file);
                let _ = std::fs::remove_file(&path);
                false
            }
            Err(_) => true,
        }
    }

    /// Lock every one of `backends`, in name order so two processes never wait on each other,
    /// until the answer is dropped.
    ///
    /// # Errors
    ///
    /// A lock file that cannot be made or locked.
    pub fn lock(&self, backends: &[String]) -> std::io::Result<Locked<'_>> {
        let mut names: Vec<&String> = backends.iter().collect();
        names.sort_unstable();
        names.dedup();
        let mut locks = Vec::with_capacity(names.len());
        for name in names {
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(self.directory.join(format!("{}.lock", file_stem(name))))?;
            lock.lock()?;
            locks.push(lock);
        }
        Ok(Locked {
            host: self,
            _locks: locks,
        })
    }

    fn path(&self, backend: &str) -> PathBuf {
        self.directory.join(format!("{}.json", file_stem(backend)))
    }
}

/// The state of some backend kinds, locked against every other process until dropped.
pub struct Locked<'a> {
    host: &'a Host,
    _locks: Vec<File>,
}

impl Locked<'_> {
    /// `backend`'s state as stored; `None` when there is none, or it cannot be read, or it is
    /// of another schema.
    #[must_use]
    pub fn read<T: DeserializeOwned>(&self, backend: &str) -> Option<T> {
        let text = std::fs::read(self.host.path(backend)).ok()?;
        let document: Document<T> = serde_json::from_slice(&text).ok()?;
        (document.schema_version == SCHEMA_VERSION).then_some(document.state)
    }

    /// Replace `backend`'s state with `state`.
    ///
    /// # Errors
    ///
    /// The state cannot be staged or moved into place.
    pub fn write<T: Serialize>(&self, backend: &str, state: &T) -> std::io::Result<()> {
        let document = Document {
            schema_version: SCHEMA_VERSION,
            state,
        };
        let text = serde_json::to_vec(&document).map_err(std::io::Error::other)?;
        let staging = self.host.directory.join(format!(
            ".{}.{}.staging",
            file_stem(backend),
            self.host.process
        ));
        std::fs::write(&staging, text)?;
        std::fs::rename(&staging, self.host.path(backend))
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(
            self.directory
                .join(PROCESSES)
                .join(format!("{}.lock", self.process)),
        );
    }
}
