//! The Jinja templates the loop renders: the verifying agent's task, each kind's ticket shape,
//! destination body and projections, and the comments a batch writes. A template is a
//! `builtin:<name>` shipped in the binary, or a path; a template includes others by name,
//! resolved beside it (a shipped one's beside the shipped files).
//!
//! Every template may call `collapsed(summary, content)`, which writes one expandable block in
//! the authored form onetaskgraph's README states (its "Expandable blocks"): `<details>` alone,
//! `<summary>…</summary>`, a blank line, the content, a blank line, `</details>`. The store
//! writes that form as each destination's own.

use std::path::{Path, PathBuf};

use minijinja::{Environment, UndefinedBehavior, Value as Jinja};
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
        "task-template/feedback.md.j2",
        include_str!("../../defaults/task-template/feedback.md.j2"),
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
    (
        "comment-evidence.md.j2",
        include_str!("../../defaults/comment-evidence.md.j2"),
    ),
    (
        "comment-reply.md.j2",
        include_str!("../../defaults/comment-reply.md.j2"),
    ),
];

/// One expandable block in the authored form: `<details>` alone, `<summary>…</summary>`, a blank
/// line, `content` trimmed, a blank line, `</details>`.
#[must_use]
pub fn collapsed(summary: &str, content: &str) -> String {
    format!(
        "<details>\n<summary>{}</summary>\n\n{}\n\n</details>",
        summary.trim(),
        content.trim()
    )
}

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

/// Whether the template at `located` can be read: a shipped name something ships, or a file.
#[must_use]
pub fn exists(located: &Located) -> bool {
    match located {
        Located::Shipped(name) => shipped(name).is_some(),
        Located::File(path) => path.is_file(),
    }
}

/// Render the template at `located` with `context`.
///
/// # Errors
///
/// A template that cannot be read, parsed or rendered, or that names an undefined variable.
pub fn render<C: Serialize>(located: &Located, context: &C) -> Result<String, TemplateError> {
    render_value(located, Jinja::from_serialize(context))
}

/// Render the template text `source`, named `name` in an error, with `context`: a projection,
/// which is a template string rather than a file.
///
/// # Errors
///
/// A template that cannot be parsed or rendered.
pub fn render_text(name: &str, source: &str, context: Jinja) -> Result<String, TemplateError> {
    let mut environment = environment();
    environment.set_loader(|name| {
        Ok(shipped(name.strip_prefix(BUILTIN).unwrap_or(name)).map(str::to_owned))
    });
    let failed = |error: minijinja::Error| TemplateError {
        template: name.to_owned(),
        message: message(&error),
    };
    environment
        .add_template_owned("(projection)", source.to_owned())
        .map_err(failed)?;
    environment
        .get_template("(projection)")
        .and_then(|template| template.render(context))
        .map_err(failed)
}

/// What went wrong, and where: the error and each error it was caused by, without the debug
/// dump of every variable the template referenced, which for a ticket is the whole ticket.
fn message(error: &minijinja::Error) -> String {
    let mut message = error.to_string();
    let mut cause = std::error::Error::source(error);
    while let Some(inner) = cause {
        message.push_str(&format!(": {inner}"));
        cause = inner.source();
    }
    message
}

fn environment() -> Environment<'static> {
    let mut environment = Environment::new();
    environment.set_undefined_behavior(UndefinedBehavior::Chainable);
    environment.set_keep_trailing_newline(true);
    environment.add_function("collapsed", |summary: String, content: String| {
        collapsed(&summary, &content)
    });
    environment
}

/// Render the template at `located` with a context already made a template value — one that
/// may hold objects, as a ticket's `section(name)` is.
///
/// # Errors
///
/// A template that cannot be read, parsed or rendered, or that names an undefined variable.
pub fn render_value(located: &Located, context: Jinja) -> Result<String, TemplateError> {
    let root = text(located)?;
    let directory: Option<PathBuf> = match located {
        Located::File(path) => path.parent().map(Path::to_path_buf),
        Located::Shipped(_) => None,
    };
    let mut environment = environment();
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
        message: message(&error),
    };
    environment
        .add_template_owned("(root)", root)
        .map_err(failed)?;
    environment
        .get_template("(root)")
        .and_then(|template| template.render(context))
        .map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::{Located, collapsed, exists, render_text};

    #[test]
    fn a_projection_renders_with_collapsed_and_may_include_a_shipped_template() {
        let rendered = render_text(
            "projection",
            "{{ collapsed(' Why ', '\n because \n') }}|{% include 'builtin:comment-reply.md.j2' %}",
            minijinja::Value::from_serialize(serde_json::json!({
                "comment": {"batch": "b", "url": "u", "author": null, "response": "r"}
            })),
        )
        .expect("it renders");
        assert!(
            rendered.starts_with(&format!("{}|", collapsed("Why", "because"))),
            "{rendered}"
        );
        assert!(rendered.contains("Reply from follow-up run `b` to the comment: u"));
        let refused = render_text(
            "p",
            "{% include 'nothing.j2' %}",
            minijinja::Value::UNDEFINED,
        )
        .expect_err("nothing is named so");
        assert_eq!(refused.template, "p");
        assert!(!exists(&Located::Shipped("nothing.j2".to_owned())));
        assert!(exists(&Located::Shipped("comment-reply.md.j2".to_owned())));
    }
}
