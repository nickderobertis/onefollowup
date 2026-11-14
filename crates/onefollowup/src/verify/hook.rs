//! Running one of `hooks`: its argv with its JSON document on stdin, exactly as configured —
//! and, for a hook whose argv names [`CONTEXT_FILE`], a context file written for that one call.
//!
//! The context file is an engine's human-item phase context (`human-items raise --phase P
//! --context FILE`), whose reader refuses any member it does not know, so the file holds
//! exactly: `owner` (from `hooks.owner`), `dedup_key` (the same for the batch and the hook
//! every time), `tickets` (one `{id, url, priority}` per ticket the batch's receipts say it
//! created or updated), and `vars`, the hook's stdin document but for its `schema_version`,
//! each member as text — a string as itself, any other scalar as its JSON spelling, and a list
//! or mapping as compact JSON. `crates/onefollowup/schemas/hook-context.schema.json` declares
//! it.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use crate::config::{CONTEXT_FILE, ConfigError, HookOwner, Origin, variable};
use crate::error::Error;
use crate::formats::receipts::{CopyAction, Outcome};
use crate::store::Store;
use crate::verify::command::{self, Ran};
use crate::verify::context::Batch;
use crate::verify::journal;

/// Run the hook `name`, `argv`, with `document` on its stdin; a context file's tickets are read
/// through `store`, the verb's own, so the read is counted with its other operations — or,
/// with none given, through one opened for them.
///
/// # Errors
///
/// For a hook naming [`CONTEXT_FILE`]: a `hooks.owner` value naming an unset variable (a
/// configuration error, before the hook runs), the store's refusal of the tickets' read, or a
/// context file that cannot be written. For any hook: the command's own failure to run, as
/// [`command::run`] says it.
pub fn run(
    batch: &Batch<'_>,
    store: Option<&Store>,
    name: &str,
    argv: &[String],
    document: &Value,
) -> Result<Ran, Error> {
    let settings = batch.settings;
    let stdin = document.to_string();
    if !argv.iter().any(|one| one.contains(CONTEXT_FILE)) {
        return command::run(
            argv,
            &stdin,
            settings.working_directory(),
            settings.environment(),
        )
        .map_err(|error| could_not_run(name, argv, &error));
    }
    let context = context(batch, store, name, document)?;
    let path = context_path(batch, name);
    crate::work::write_text(
        &path,
        &(serde_json::to_string_pretty(&context).unwrap_or_default() + "\n"),
    )?;
    let given: Vec<String> = argv
        .iter()
        .map(|one| one.replace(CONTEXT_FILE, &path.display().to_string()))
        .collect();
    let ran = command::run(
        &given,
        &stdin,
        settings.working_directory(),
        settings.environment(),
    );
    crate::work::remove(&path)?;
    ran.map_err(|error| could_not_run(name, argv, &error))
}

/// Refuse a hook naming [`CONTEXT_FILE`] whose `hooks.owner` names an unset variable, before
/// the verb that runs it changes anything; a hook naming none is not checked.
///
/// # Errors
///
/// The configuration error naming the variable.
pub fn check_owner(batch: &Batch<'_>, argv: Option<&Vec<String>>) -> Result<(), Error> {
    match (argv, &batch.config.hooks.owner) {
        (Some(argv), Some(owner)) if argv.iter().any(|one| one.contains(CONTEXT_FILE)) => {
            resolve_owner(owner, batch.settings.environment()).map(|_| ())
        }
        _ => Ok(()),
    }
}

fn could_not_run(name: &str, argv: &[String], error: &str) -> Error {
    Error::Failed(format!(
        "the `hooks.{name}` command `{}` could not run: {error}",
        argv.join(" ")
    ))
}

/// Where the context file of the hook `name` for the batch is written: beside the batch's
/// other working files, and only while the hook runs.
fn context_path(batch: &Batch<'_>, name: &str) -> PathBuf {
    batch
        .work
        .tasks(&batch.batch)
        .join(format!("{name}.context.json"))
}

/// The dedup key of the hook `name` for the batch: the same every time the batch runs it.
#[must_use]
pub fn dedup_key(namespace: &str, batch: &str, name: &str) -> String {
    format!("{namespace}.follow-up/{batch}/{name}")
}

/// The context file of the hook `name`, over the hook's stdin `document`.
fn context(
    batch: &Batch<'_>,
    store: Option<&Store>,
    name: &str,
    document: &Value,
) -> Result<Value, Error> {
    let owner = batch.config.hooks.owner.as_ref().ok_or_else(|| {
        Error::Refused(format!(
            "the `hooks.{name}` command names `{CONTEXT_FILE}`, whose file names an owner; set `hooks.owner.session`"
        ))
    })?;
    let owner = resolve_owner(owner, batch.settings.environment())?;
    let vars: Map<String, Value> = document
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(key, _)| *key != "schema_version")
        .map(|(key, value)| (key.clone(), Value::String(text(value))))
        .collect();
    Ok(json!({
        "owner": owner,
        "dedup_key": dedup_key(&batch.config.namespace, &batch.batch, name),
        "tickets": tickets(batch, store)?,
        "vars": vars,
    }))
}

/// A stdin member as a template variable's text.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `hooks.owner` with each `${NAME}` read from the verb's `environment`.
fn resolve_owner(owner: &HookOwner, environment: &[(String, String)]) -> Result<Value, Error> {
    let resolve = |part: &str, value: &str| -> Result<String, Error> {
        let Some(name) = variable(value) else {
            return Ok(value.to_owned());
        };
        environment
            .iter()
            .find(|(held, _)| held == name)
            .map(|(_, value)| value.clone())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                Error::Config(ConfigError {
                    key: format!("hooks.owner.{part}"),
                    origin: Origin::Env(name.to_owned()),
                    message: format!(
                        "names the environment variable `{name}`, which is not set or is empty; set it, or state the value; the hook was not run"
                    ),
                })
            })
    };
    let optional = |part: &str, value: Option<&String>| -> Result<Value, Error> {
        value.map_or(Ok(Value::Null), |value| {
            resolve(part, value).map(Value::String)
        })
    };
    Ok(json!({
        "session": resolve("session", &owner.session)?,
        "run": optional("run", owner.run.as_ref())?,
        "plan": optional("plan", owner.plan.as_ref())?,
    }))
}

/// One `{id, url, priority}` per ticket the batch's receipts say it created or updated, in the
/// order the journal first names each, read from the store in one read.
fn tickets(batch: &Batch<'_>, store: Option<&Store>) -> Result<Vec<Value>, Error> {
    let journal = journal::read(batch)?;
    let mut cards: IndexMap<String, ()> = IndexMap::new();
    for receipt in &journal.receipts {
        if let (Outcome::Copy(copy), Some(card)) = (&receipt.outcome, &receipt.card)
            && matches!(copy.action, CopyAction::Created | CopyAction::Updated)
            && copy.error.is_none()
        {
            cards.insert(card.clone(), ());
        }
    }
    if cards.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = cards.into_keys().collect();
    let read = match store {
        Some(store) => store.read(&ids, false)?,
        None => batch.store()?.read(&ids, false)?,
    };
    Ok(ids
        .iter()
        .zip(read)
        .map(|(id, read)| {
            let item = read.item;
            let url = item
                .as_ref()
                .and_then(|item| item.url.clone())
                .filter(|url| !url.trim().is_empty())
                .or_else(|| {
                    item.as_ref()
                        .and_then(|item| item.path.as_deref())
                        .map(file_url)
                })
                .unwrap_or_else(|| id.clone());
            let priority = item.map_or_else(|| "none".to_owned(), |item| item.priority);
            json!({"id": id, "url": url, "priority": priority})
        })
        .collect())
}

/// A `file://` URL of `path`, for a ticket whose source keeps it in a file and states no URL.
fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}
