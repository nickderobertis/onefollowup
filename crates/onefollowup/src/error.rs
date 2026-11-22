//! What a verb refuses with, and the exit code the CLI maps it to.

use crate::config::ConfigError;
use crate::formats::FormatError;
use crate::store::StoreError;
use crate::verify::template::TemplateError;
use crate::work::WorkError;

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
    /// A working file could not be read or written.
    #[error(transparent)]
    Work(#[from] WorkError),
    /// A template could not be rendered.
    #[error(transparent)]
    Template(#[from] TemplateError),
    /// The verb refused: what it was asked breaks a rule, and nothing was written.
    #[error("{0}")]
    Refused(String),
    /// The verb failed while running.
    #[error("{0}")]
    Failed(String),
    /// `comments answer` launched every batch and at least one command failed, so nothing was
    /// settled; the CLI prints `answer` on stdout as the verb's document.
    #[error("{message}")]
    Unsettled {
        /// What failed, naming every failed batch.
        message: String,
        /// The pass's answer: every launch's exit status, and no settle.
        answer: Box<crate::api::CommentsAnswered>,
    },
    /// `groom check` found a problem standing, so no plan was written; the CLI prints
    /// `answer` on stdout as the verb's document and exits refused.
    #[error("{message}")]
    Unpassed {
        /// Every problem, by rule and ticket.
        message: String,
        /// The check's answer: its problems, and the plan as far as it stands.
        answer: Box<crate::api::GroomChecked>,
    },
    /// `groom publish` sent what it could, and a write the destination refused or a ticket read
    /// back otherwise than its receipts say stands; the CLI prints `answer` on stdout as the
    /// verb's document and exits failed.
    #[error("{message}")]
    Unpublished {
        /// What stands: the refused writes and the mismatches.
        message: String,
        /// The publish's answer: what the read-back confirmed and found.
        answer: Box<crate::api::GroomPublished>,
    },
}

impl Error {
    /// The document a failure still prints on stdout, when it has one.
    #[must_use]
    pub fn answer(&self) -> Option<&crate::api::CommentsAnswered> {
        match self {
            Self::Unsettled { answer, .. } => Some(answer),
            _ => None,
        }
    }

    /// The document a failure still prints on stdout, as the command line prints it, when it
    /// has one.
    #[must_use]
    pub fn document(&self) -> Option<String> {
        match self {
            Self::Unsettled { answer, .. } => Some(crate::cli::print(answer)),
            Self::Unpassed { answer, .. } => Some(crate::cli::print(answer)),
            Self::Unpublished { answer, .. } => Some(crate::cli::print(answer)),
            _ => None,
        }
    }

    /// The exit code the CLI ends with.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::NotImplemented { .. }
            | Self::Config(_)
            | Self::Refused(_)
            | Self::Unpassed { .. } => EXIT_REFUSED,
            Self::Format(_)
            | Self::Store(_)
            | Self::Work(_)
            | Self::Template(_)
            | Self::Failed(_)
            | Self::Unsettled { .. }
            | Self::Unpublished { .. } => EXIT_FAILED,
        }
    }
}
