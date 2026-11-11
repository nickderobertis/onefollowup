//! The Jinja templates the loop renders: the verifying agent's task and each kind's ticket
//! shape. A template is a `builtin:<name>` shipped in the binary, or a path; a template
//! includes others by name, resolved beside it (a shipped one's beside the shipped files).

use std::path::{Path, PathBuf};

use minijinja::{Environment, UndefinedBehavior};
use serde::Serialize;

/// The prefix naming a shipped file.
pub const BUILTIN: &str = "builtin:";

/// Every shipped template, by name.
const SHIPPED: &[(&str, &str)] = &[
    (
        "task-template.md.j2",
        include_str!("../../defaults/task-template.md.j2"),
    ),
    (
        "task-template/ownership.md.j2",
        include_str!("../../defaults/task-template/ownership.md.j2"),
    ),
    (
        "ticket-template.md.j2",
        include_str!("../../defaults/ticket-template.md.j2"),
    ),
    ("persona.yaml", include_str!("../../defaults/persona.yaml")),
];

/// A template that could not be read or rendered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the template {template}: {message}")]
pub struct TemplateError {
    /// The template, as configured.
    pub template: String,
    /// What went wrong.
    pub message: String,
}

/// Where a configured template is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// A shipped file, by name.
    Shipped(String),
    /// A file on disk.
    File(PathBuf),
}

impl Located {
    /// `configured` — `builtin:<name>`, or a path already made absolute by `absolute`.
    #[must_use]
    pub fn of(configured: &str, absolute: impl FnOnce(&str) -> PathBuf) -> Self {
        configured.strip_prefix(BUILTIN).map_or_else(
            || Self::File(absolute(configured)),
            |name| Self::Shipped(name.to_owned()),
        )
    }

    fn name(&self) -> String {
        match self {
            Self::Shipped(name) => format!("{BUILTIN}{name}"),
            Self::File(path) => path.display().to_string(),
        }
    }
}

fn shipped(name: &str) -> Option<&'static str> {
    SHIPPED
        .iter()
        .find(|(shipped, _)| *shipped == name)
        .map(|(_, text)| *text)
}

/// The text of a shipped file or a file on disk.
///
/// # Errors
///
/// A shipped name nothing ships, or a file that cannot be read.
pub fn text(located: &Located) -> Result<String, TemplateError> {
    match located {
        Located::Shipped(name) => shipped(name)
            .map(str::to_owned)
            .ok_or_else(|| TemplateError {
                template: located.name(),
                message: format!(
                    "no shipped file is named `{name}`; the shipped ones are {}",
                    SHIPPED
                        .iter()
                        .map(|(name, _)| format!("`{BUILTIN}{name}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
        Located::File(path) => std::fs::read_to_string(path).map_err(|error| TemplateError {
            template: located.name(),
            message: format!("could not be read: {error}"),
        }),
    }
}

/// Render the template at `located` with `context`.
///
/// # Errors
///
/// A template that cannot be read, parsed or rendered, or that names an undefined variable.
pub fn render<C: Serialize>(located: &Located, context: &C) -> Result<String, TemplateError> {
    let root = text(located)?;
    let directory: Option<PathBuf> = match located {
        Located::File(path) => path.parent().map(Path::to_path_buf),
        Located::Shipped(_) => None,
    };
    let mut environment = Environment::new();
    environment.set_undefined_behavior(UndefinedBehavior::Chainable);
    environment.set_keep_trailing_newline(true);
    environment.set_loader(move |name| {
        if let Some(name) = name.strip_prefix(BUILTIN) {
            return Ok(shipped(name).map(str::to_owned));
        }
        match &directory {
            None => Ok(shipped(name).map(str::to_owned)),
            Some(directory) => match std::fs::read_to_string(directory.join(name)) {
                Ok(text) => Ok(Some(text)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("{name} could not be read: {error}"),
                )),
            },
        }
    });
    let failed = |error: minijinja::Error| TemplateError {
        template: located.name(),
        message: format!("{error:#}"),
    };
    environment
        .add_template_owned("(root)", root)
        .map_err(failed)?;
    environment
        .get_template("(root)")
        .and_then(|template| template.render(context))
        .map_err(failed)
}
