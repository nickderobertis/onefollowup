//! `check`: hold the batch's answers, tickets, edges and staged writes to the rules before
//! anything is written, reading nothing of the destination (only, for a route that assigns,
//! the store's configuration); when they are sound, record what it held so `publish` sends
//! exactly that.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use sha2::{Digest as _, Sha256};

use crate::api::{BatchArgs, Checked, Problem, SCHEMA_VERSION};
use crate::config::{Duplicates, Requirement, Settings};
use crate::error::Error;
use crate::formats::batch::BatchState;
use crate::formats::dispositions::{CarriedDispositions, DispositionEntry, Dispositions};
use crate::formats::outbox::{Outbox, StagedWrite};
use crate::verify::context::Batch;
use crate::verify::relations::{Graph, Scope};
use crate::verify::tools::ticket_problems;
use crate::work;

/// A change an answer names: a URL, a commit, or a `#<number>` reference.
static CHANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:https?://\S+|\b[0-9a-f]{7,40}\b|#[0-9]+\b)").expect("valid"));

fn problem(subject: impl Into<String>, message: impl Into<String>) -> Problem {
    Problem {
        subject: subject.into(),
        message: message.into(),
    }
}

/// Each staged write's digest, in outbox order: what it stages, and for a copy the ticket file
/// it copies as it now stands.
///
/// # Errors
///
/// A ticket file that cannot be read.
pub fn digests(batch: &Batch<'_>, outbox: &Outbox) -> Result<Vec<String>, Error> {
    outbox
        .writes
        .iter()
        .map(|write| {
            let mut hash = Sha256::new();
            hash.update(serde_json::to_string(write).unwrap_or_default().as_bytes());
            if let StagedWrite::Copy { ticket, .. } = write
                && let Some(slug) = batch.ticket_slug(ticket)
            {
                let text = work::read_text(&batch.work.ticket(&batch.batch, &slug))?;
                hash.update(text.unwrap_or_default().as_bytes());
            }
            Ok(hash
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect())
        })
        .collect()
}

/// A staged write as a problem names it.
#[must_use]
pub fn named(index: usize, write: &StagedWrite) -> String {
    match write {
        StagedWrite::Copy { ticket, .. } => format!("outbox[{index}]: the copy of {ticket}"),
        StagedWrite::Comment { item, .. } => format!("outbox[{index}]: the comment on {item}"),
        StagedWrite::ReEstimate { item, .. } => {
            format!("outbox[{index}]: the re-estimate of {item}")
        }
        StagedWrite::Relate { from, to, .. } => {
            format!("outbox[{index}]: the edge {from} -> {to}")
        }
    }
}

/// Hold the batch to the rules.
///
/// # Errors
///
/// A batch not prepared, or a working file that cannot be read.
pub fn run(settings: &Settings, args: BatchArgs) -> Result<Checked, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let mut state = batch.state()?;
    let outbox = batch.outbox()?;
    let mut problems = Vec::new();
    answers(&batch, &state, &outbox, &mut problems)?;
    tickets(&batch, &state, &outbox, &mut problems)?;
    let bound = crate::verify::journal::bound(&crate::verify::journal::read(&batch)?);
    let graph = Graph::of(&batch, &state, &outbox, &bound, Scope::Staged)?;
    for (subject, message) in graph.problems(&batch, &state, &outbox, &bound) {
        problems.push(problem(subject, message));
    }
    assigning(&batch, &mut problems)?;
    let sound = problems.is_empty();
    state.checked = if sound {
        Some(digests(&batch, &outbox)?)
    } else {
        None
    };
    batch.save_state(&state)?;
    Ok(Checked {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        sound,
        problems,
    })
}

/// Every staged or published write of the batch.
fn all_writes<'a>(
    state: &'a BatchState,
    outbox: &'a Outbox,
) -> impl Iterator<Item = &'a StagedWrite> {
    state
        .publishes
        .iter()
        .flat_map(|publish| &publish.writes)
        .chain(&outbox.writes)
}

fn answers(
    batch: &Batch<'_>,
    state: &BatchState,
    outbox: &Outbox,
    problems: &mut Vec<Problem>,
) -> Result<(), Error> {
    let config = batch.config;
    let carried: Option<CarriedDispositions> = work::read(&batch.work.carried(&batch.batch))?;
    let written: Option<Dispositions> = work::read(&batch.work.dispositions(&batch.batch))?;
    let mut entries: Vec<DispositionEntry> = carried
        .map(|carried| carried.dispositions)
        .unwrap_or_default();
    for entry in written.iter().flat_map(|written| &written.dispositions) {
        if !entries.contains(entry) {
            entries.push(entry.clone());
        }
    }
    let mut by_input: BTreeMap<&str, Vec<&DispositionEntry>> = BTreeMap::new();
    for entry in &entries {
        if !state.inputs.iter().any(|input| input.id == entry.draft) {
            problems.push(problem(
                &entry.draft,
                "the answer names no input of this batch; answer only the batch's drafts and inputs",
            ));
            continue;
        }
        by_input.entry(&entry.draft).or_default().push(entry);
    }
    let staged_slugs: Vec<String> = all_writes(state, outbox)
        .filter_map(|write| match write {
            StagedWrite::Copy { ticket, .. } => batch.ticket_slug(ticket),
            _ => None,
        })
        .collect();
    let commented: Vec<&str> = all_writes(state, outbox)
        .filter_map(|write| match write {
            StagedWrite::Comment { root_cause, .. } => Some(root_cause.as_str()),
            _ => None,
        })
        .collect();
    for input in &state.inputs {
        let Some(given) = by_input.get(input.id.as_str()) else {
            problems.push(problem(
                &input.id,
                "the input has no disposition; give it exactly one",
            ));
            continue;
        };
        if given.len() > 1 {
            problems.push(problem(
                &input.id,
                format!(
                    "the input carries {} dispositions; give it exactly one",
                    given.len()
                ),
            ));
            continue;
        }
        let entry = given[0];
        let Some(kind_name) = &entry.kind else {
            problems.push(problem(
                &input.id,
                "the answer names no kind; give every input a configured kind, filed or not",
            ));
            continue;
        };
        let Some(kind) = batch.kind(kind_name) else {
            problems.push(problem(
                &input.id,
                format!("the kind `{kind_name}` is not a configured kind"),
            ));
            continue;
        };
        let Some(disposition) = config
            .dispositions
            .iter()
            .find(|one| one.name == entry.disposition)
        else {
            problems.push(problem(
                &input.id,
                format!(
                    "the disposition `{}` is not a configured disposition",
                    entry.disposition
                ),
            ));
            continue;
        };
        let allowed: Vec<String> = kind
            .dispositions
            .clone()
            .or_else(|| config.default_dispositions.clone())
            .unwrap_or_else(|| {
                config
                    .dispositions
                    .iter()
                    .map(|one| one.name.clone())
                    .collect()
            });
        if !allowed.contains(&entry.disposition) {
            problems.push(problem(
                &input.id,
                format!(
                    "the kind `{}` does not allow the disposition `{}`; it allows {}",
                    kind.name,
                    entry.disposition,
                    allowed
                        .iter()
                        .map(|one| format!("`{one}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        for requirement in &disposition.requires {
            match requirement {
                Requirement::Ticket => {
                    if entry.root_causes.is_empty() {
                        problems.push(problem(
                            &input.id,
                            format!(
                                "the disposition `{}` requires a ticket, and the answer names none",
                                entry.disposition
                            ),
                        ));
                    }
                    for root_cause in &entry.root_causes {
                        let held = staged_slugs.contains(root_cause)
                            || commented.contains(&root_cause.as_str())
                            || state
                                .merged
                                .iter()
                                .any(|merged| &merged.ticket == root_cause);
                        if !held {
                            problems.push(problem(
                                &input.id,
                                format!(
                                    "the answer names the ticket `{root_cause}`, which has neither a staged write nor an evidence comment in this batch"
                                ),
                            ));
                        }
                    }
                }
                Requirement::Reason => {
                    if entry.detail.trim().is_empty() {
                        problems.push(problem(
                            &input.id,
                            format!(
                                "the disposition `{}` requires a reason, and the answer's detail is empty",
                                entry.disposition
                            ),
                        ));
                    }
                }
                Requirement::Change => {
                    if !CHANGE.is_match(&entry.detail) {
                        problems.push(problem(
                            &input.id,
                            format!(
                                "the disposition `{}` requires the change that addressed it — a URL, a commit or a `#<number>` — and the answer's detail names none",
                                entry.disposition
                            ),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn tickets(
    batch: &Batch<'_>,
    state: &BatchState,
    outbox: &Outbox,
    problems: &mut Vec<Problem>,
) -> Result<(), Error> {
    let config = batch.config;
    let files = batch.tickets()?;
    for (index, write) in outbox.writes.iter().enumerate() {
        let StagedWrite::Copy { ticket, .. } = write else {
            continue;
        };
        let Some(slug) = batch.ticket_slug(ticket) else {
            problems.push(problem(
                named(index, write),
                "the staged copy names no ticket of this batch",
            ));
            continue;
        };
        match files.iter().find(|file| file.slug == slug) {
            None => problems.push(problem(
                named(index, write),
                format!("the ticket `{slug}` it copies is not under the batch's tickets"),
            )),
            Some(file) => {
                for found in ticket_problems(batch, Some(state), file) {
                    problems.push(problem(format!("ticket:{slug}"), found));
                }
            }
        }
    }
    for file in &files {
        let staged = outbox.writes.iter().any(|write| {
            matches!(write, StagedWrite::Copy { ticket, .. } if batch.ticket_slug(ticket).as_deref() == Some(file.slug.as_str()))
        });
        if staged {
            continue;
        }
        for (name, _) in file
            .record
            .iter()
            .flat_map(|record| record.fields.iter().flatten())
        {
            if !config.fields.contains_key(name) {
                problems.push(problem(
                    format!("ticket:{}", file.slug),
                    format!(
                        "the ticket records the field `{name}`, which `fields` does not declare"
                    ),
                ));
            }
        }
    }
    let mut seen: BTreeMap<(String, Vec<String>), String> = BTreeMap::new();
    for file in &files {
        let Some(kind) = file.kind(config) else {
            continue;
        };
        let Duplicates::Same(fields) = &kind.duplicates else {
            continue;
        };
        let values: Vec<String> = fields
            .iter()
            .map(|field| file.field(field).unwrap_or_default().trim().to_lowercase())
            .collect();
        if values.iter().any(String::is_empty) {
            continue;
        }
        if let Some(other) = seen.insert((kind.name.clone(), values), file.slug.clone()) {
            problems.push(problem(
                format!("ticket:{}", file.slug),
                format!(
                    "the tickets `{other}` and `{}` of the kind `{}` carry equal {}: they are one ticket — keep one and name it in both inputs' answers",
                    file.slug,
                    kind.name,
                    fields
                        .iter()
                        .map(|field| format!("`{field}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
    Ok(())
}

/// Every route that assigns a person, held to the store's configuration: each kind it may
/// match must go to a destination source, and come from a drafts source, that set
/// `config.assignees: true`, since an assignee travels from the one to the other.
fn assigning(batch: &Batch<'_>, problems: &mut Vec<Problem>) -> Result<(), Error> {
    let config = batch.config;
    let assigning: Vec<&crate::config::Route> = config
        .routes
        .iter()
        .filter(|route| route.set.assignee.is_some())
        .collect();
    if assigning.is_empty() {
        return Ok(());
    }
    let store = batch.store()?;
    for route in assigning {
        let person = route.set.assignee.clone().unwrap_or_default();
        let kinds: Vec<&crate::config::Kind> = config
            .kinds
            .iter()
            .filter(|kind| {
                route
                    .matches
                    .kind
                    .as_ref()
                    .is_none_or(|kinds| kinds.contains(&kind.name))
            })
            .collect();
        let mut reported: Vec<String> = Vec::new();
        for kind in kinds {
            for (source, role) in [
                (batch.destination_of(kind), "destination"),
                (config.drafts_source.clone(), "drafts source"),
            ] {
                if reported.contains(&source)
                    || store.source_setting(&source, "assignees")
                        == Some(serde_json::Value::Bool(true))
                {
                    continue;
                }
                problems.push(problem(
                    format!("route:{}", route.name),
                    format!(
                        "the route `{}` assigns `{person}` to tickets of the kind `{}`, and its {role} source `{source}` does not set `config.assignees: true`, so it holds no assignee: set `sources.{source}.config.assignees: true` in the store's configuration, or remove the route's `assignee`",
                        route.name, kind.name
                    ),
                ));
                reported.push(source);
            }
        }
    }
    Ok(())
}
