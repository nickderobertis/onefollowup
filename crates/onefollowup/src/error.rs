//! What a verb refuses with, and the exit code the CLI maps it to.

use crate::config::ConfigError;
use crate::formats::FormatError;
use crate::store::StoreError;

/// Exit code: the verb succeeded.
pub const EXIT_OK: u8 = 0;
/// Exit code: the verb failed while running.
pub const EXIT_FAILED: u8 = 1;
/// Exit code: the verb refused — a usage error, a refused configuration, or a verb this
/// release does not implement.
pub const EXIT_REFUSED: u8 = 2;

/// Why a verb did not succeed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The verb is part of the v1 surface and is not implemented in this release.
    #[error("{verb}: not implemented in this release")]
    NotImplemented {
        /// The verb, as the command line spells it.
        verb: &'static str,
    },
    /// The configuration was refused.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A record was refused.
    #[error(transparent)]
    Format(#[from] FormatError),
    /// The store could not be used.
    #[error(transparent)]
    Store(#[from] StoreError),
}

impl Error {
    /// The exit code the CLI ends with.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotImplemented { .. } | Self::Config(_) => EXIT_REFUSED,
            Self::Format(_) | Self::Store(_) => EXIT_FAILED,
        }
    }
}
