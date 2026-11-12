//! `prepare`: snapshot the batch's input set — its drafts and the `batch_preparing` hook's
//! inputs — read the destination once for the whole batch (one search, one comments read, and
//! under a configuration with relations one `task relations` read of the tickets found), and
//! write the verifying agent's task and answers skeleton.

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::api::{PrepareArgs, Prepared, SCHEMA_VERSION};
use crate::config::{DecidedBy, Duplicates, PriorityModel, Settings};
use crate::error::Error;
use crate::formats::Versioned;
use crate::formats::batch::{BatchState, Input, Read};
use crate::formats::dispositions::{CarriedDispositions, DispositionEntry, Dispositions};
use crate::formats::draft::DraftRecord;
use crate::formats::existing_tickets::ExistingTickets;
use crate::store::{Search, SearchIn, Store};
use crate::verify::command;
use crate::verify::context::Batch;
use crate::verify::relations::{self, Graph, Scope};
use crate::verify::template;
use crate::work;

/// The document the `batch_preparing` hook prints.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookInputs {
    schema_version: u32,
    inputs: Vec<HookInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookInput {
    id: String,
    source: String,
    summary: String,
    detail: String,
}

/// Run the `batch_preparing` hook and read the inputs it prints.
fn hook_inputs(batch: &Batch<'_>) -> Result<Vec<Input>, Error> {
    let Some(argv) = &batch.config.hooks.batch_preparing else {
        return Ok(Vec::new());
    };
    let named = format!("the `hooks.batch_preparing` command `{}`", argv.join(" "));
    let stdin = json!({"schema_version": 1, "batch": batch.batch}).to_string();
    let ran = command::run(
        argv,
        &stdin,
        batch.settings.working_directory(),
        batch.settings.environment(),
    )
    .map_err(|error| {
        Error::Failed(format!(
            "{named} could not run: {error}; nothing was snapshotted"
        ))
    })?;
    if ran.exit != 0 {
        return Err(Error::Failed(format!(
            "{named} exited {}: {}; nothing was snapshotted",
            ran.exit,
            ran.stderr.trim()
        )));
    }
    let refused = |why: String| {
        Error::Failed(format!(
            "{named} printed {why}; it prints {{\"schema_version\": 1, \"inputs\": [{{\"id\", \"source\", \"summary\", \"detail\"}}]}}; nothing was snapshotted"
        ))
    };
    let document: HookInputs = serde_json::from_str(&ran.stdout)
        .map_err(|error| refused(format!("a document that is not the inputs shape ({error})")))?;
    if document.schema_version != 1 {
        return Err(refused(format!(
            "schema_version {}",
            document.schema_version
        )));
    }
    let mut inputs: Vec<Input> = Vec::new();
    for input in document.inputs {
        if input.id.trim().is_empty()
            || input.id.chars().any(char::is_whitespace)
            || input.source.trim().is_empty()
            || input.summary.trim().is_empty()
        {
            return Err(refused(format!(
                "the input `{}` with an empty or spaced id, or an empty source or summary",
                input.id
            )));
        }
        if inputs.iter().any(|one| one.id == input.id) {
            return Err(refused(format!("the id `{}` twice", input.id)));
        }
        inputs.push(Input {
            id: input.id,
            source: input.source,
            title: input.summary,
            repository: None,
            paths: Vec::new(),
            body: input.detail,
        });
    }
    Ok(inputs)
}

/// The batch's drafts, read from the drafts source.
fn drafts(
    batch: &Batch<'_>,
    store: &Store,
    aliases: &[String],
) -> Result<(Vec<Input>, Option<String>), Error> {
    let key = format!("{}/run", batch.draft_key());
    let searches: Vec<Search> = std::iter::once(&batch.batch)
        .chain(aliases)
        .map(|id| Search {
            key: format!("batch:{id}"),
            metadata: vec![(key.clone(), id.clone())],
            ..Search::default()
        })
        .collect();
    let found = store.find(&batch.config.drafts_source, &searches)?;
    let mut inputs: Vec<Input> = Vec::new();
    let mut project = None;
    for item in found.into_iter().flat_map(|one| one.items) {
        if inputs.iter().any(|one| one.id == item.id) {
            continue;
        }
        let record = item
            .metadata
            .get(&batch.draft_key())
            .map(|value| DraftRecord::parse(&value.to_string()))
            .transpose()
            .map_err(|error| Error::Failed(format!("the draft {}: {error}", item.id)))?;
        project = project.or(item.project.clone());
        inputs.push(Input {
            id: item.id.clone(),
            source: "draft".to_owned(),
            title: item.title.clone(),
            repository: record.as_ref().map(|record| record.repository.clone()),
            paths: record.map(|record| record.paths).unwrap_or_default(),
            body: item.content.clone(),
        });
    }
    inputs.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((inputs, project))
}

/// Every answer the batch already holds: the carried ones, then the answers file's.
fn answers(batch: &Batch<'_>) -> Result<(Vec<DispositionEntry>, Option<Dispositions>), Error> {
    let carried: Option<CarriedDispositions> = work::read(&batch.work.carried(&batch.batch))?;
    let written: Option<Dispositions> = work::read(&batch.work.dispositions(&batch.batch))?;
    let mut entries = carried
        .map(|carried| carried.dispositions)
        .unwrap_or_default();
    for entry in written.iter().flat_map(|written| &written.dispositions) {
        if !entries.contains(entry) {
            entries.push(entry.clone());
        }
    }
    Ok((entries, written))
}

/// Snapshot the batch and write the verifying agent's task.
///
/// # Errors
///
/// A refused batch id, a failing hook (nothing snapshotted and nothing read), or the store's
/// refusal.
pub fn run(settings: &Settings, args: PrepareArgs) -> Result<Prepared, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    for alias in &args.aliases {
        batch.check_id(alias)?;
    }
    let existing = batch.read_state()?;
    // A re-prepare within a pass keeps that pass's input set and runs no hook; a prepare after
    // a settled pass starts the next one, adding the drafts and inputs there are now.
    let continuing = existing.as_ref().is_some_and(|state| {
        state.publishes.is_empty() || state.publishes.iter().any(|publish| !publish.settled)
    });
    let hooked = if continuing {
        Vec::new()
    } else {
        hook_inputs(&batch)?
    };
    let store = batch.store()?;
    let mut state = match existing {
        Some(state) if continuing => state,
        earlier => {
            // Each alias once, and never the batch itself: the drafts search asks one
            // question per id, which the store refuses to see twice.
            let mut aliases = earlier
                .as_ref()
                .map(|state| state.aliases.clone())
                .unwrap_or_default();
            for alias in &args.aliases {
                if !aliases.contains(alias) && *alias != batch.batch {
                    aliases.push(alias.clone());
                }
            }
            let (drafted, project) = drafts(&batch, &store, &aliases)?;
            let mut inputs: Vec<Input> = earlier
                .as_ref()
                .map(|state| state.inputs.clone())
                .unwrap_or_default();
            for input in drafted.into_iter().chain(hooked) {
                if !inputs.iter().any(|one| one.id == input.id) {
                    inputs.push(input);
                }
            }
            match earlier {
                Some(mut state) => {
                    state.aliases = aliases;
                    state.inputs = inputs;
                    state.merged = Vec::new();
                    state
                }
                None => BatchState {
                    batch: batch.batch.clone(),
                    aliases,
                    project: project.unwrap_or_else(|| batch.batch.clone()),
                    inputs,
                    reads: Vec::new(),
                    read_at_receipt: 0,
                    searched_values: Vec::new(),
                    tickets: IndexMap::new(),
                    merged: Vec::new(),
                    checked: None,
                    publishes: Vec::new(),
                    relations: Vec::new(),
                },
            }
        }
    };

    // The answers: an earlier pass's are carried forward as they stand.
    let (answered, written) = answers(&batch)?;
    if written.is_some() && !answered.is_empty() {
        work::write(
            &batch.work.carried(&batch.batch),
            &CarriedDispositions {
                batch: batch.batch.clone(),
                dispositions: answered.clone(),
            },
        )?;
    }
    let ids: Vec<String> = {
        let mut ids: Vec<String> = state.inputs.iter().map(|input| input.id.clone()).collect();
        ids.sort();
        ids
    };
    match written {
        Some(mut written) => {
            written.drafts = ids.clone();
            work::write(&batch.work.dispositions(&batch.batch), &written)?;
        }
        None => work::write(
            &batch.work.dispositions(&batch.batch),
            &Dispositions {
                batch: batch.batch.clone(),
                drafts: ids.clone(),
                dispositions: Vec::new(),
            },
        )?,
    }
    let scoped: Vec<&Input> = state
        .inputs
        .iter()
        .filter(|input| !args.unanswered || !answered.iter().any(|entry| entry.draft == input.id))
        .collect();

    // The destination, read once: one search per destination, one comments read.
    let tickets = batch.tickets()?;
    let bound = crate::verify::journal::bound(&crate::verify::journal::read(&batch)?);
    let receipted: Vec<&String> = bound.keys().collect();
    let mut searched_values = Vec::new();
    let mut reads: Vec<Read> = Vec::new();
    for destination in batch.destinations() {
        let mut searches: Vec<Search> = Vec::new();
        for (at, input) in scoped.iter().enumerate() {
            if !input.title.trim().is_empty() {
                searches.push(Search {
                    key: format!("input:{at}"),
                    text: Some(input.title.trim().to_owned()),
                    within: SearchIn::Both,
                    ..Search::default()
                });
            }
        }
        for ticket in &tickets {
            let Some(kind) = ticket.kind(batch.config) else {
                continue;
            };
            if batch.destination_of(kind) != destination {
                continue;
            }
            let item = state.tickets.get(&ticket.slug);
            let confirmed = item.is_some_and(|item| receipted.contains(&item));
            if args.unanswered && confirmed {
                continue;
            }
            if let Duplicates::Same(fields) = &kind.duplicates {
                for field in fields {
                    let Some(value) = ticket.field(field).map(str::trim).filter(|v| !v.is_empty())
                    else {
                        continue;
                    };
                    let asked = crate::verify::tools::asked(&destination, field, value);
                    if searched_values.contains(&asked) {
                        continue;
                    }
                    let at = searched_values.len();
                    searches.push(Search {
                        key: format!("value:{at}"),
                        metadata: vec![(
                            format!("{}/fields/{field}", batch.record_key()),
                            value.to_owned(),
                        )],
                        ..Search::default()
                    });
                    searches.push(Search {
                        key: format!("value-text:{at}"),
                        text: Some(value.to_owned()),
                        within: SearchIn::Content,
                        ..Search::default()
                    });
                    searched_values.push(asked);
                }
            }
            if let Some(item) = item
                && let Some((card, _)) = bound.get(item)
                && card.starts_with(&format!("{destination}:"))
            {
                searches.push(Search {
                    key: format!("bound:{}", ticket.slug),
                    origin: Some(item.clone()),
                    ..Search::default()
                });
            }
        }
        // Each match-decided relation's target is looked for on its `to` kind's destination.
        for ticket in &tickets {
            for relation in &batch.config.relations {
                let DecidedBy::Match(fields) = &relation.decided_by else {
                    continue;
                };
                let to = batch.kind(&relation.to);
                if ticket.kind_name(batch.config) != relation.from
                    || to.is_none_or(|to| batch.destination_of(to) != destination)
                {
                    continue;
                }
                for field in fields {
                    let Some(value) = ticket.field(field).map(str::trim).filter(|v| !v.is_empty())
                    else {
                        continue;
                    };
                    let asked = crate::verify::tools::asked(&destination, field, value);
                    if searched_values.contains(&asked) {
                        continue;
                    }
                    let at = searched_values.len();
                    searches.push(Search {
                        key: format!("value:{at}"),
                        metadata: vec![(
                            format!("{}/fields/{field}", batch.record_key()),
                            value.to_owned(),
                        )],
                        ..Search::default()
                    });
                    searches.push(Search {
                        key: format!("value-text:{at}"),
                        text: Some(value.to_owned()),
                        within: SearchIn::Content,
                        ..Search::default()
                    });
                    searched_values.push(asked);
                }
            }
        }
        if searches.is_empty() {
            continue;
        }
        for found in store.find(&destination, &searches)? {
            for item in found.items {
                match reads.iter_mut().find(|read| read.item.id == item.id) {
                    Some(read) => read.found_by.push(found.key.clone()),
                    None => reads.push(Read {
                        item,
                        comments: None,
                        found_by: vec![found.key.clone()],
                    }),
                }
            }
        }
    }
    // Every item a staged edge names is read too, in the same read, so its relations are.
    let named: Vec<String> = batch
        .outbox()?
        .writes
        .iter()
        .flat_map(|write| match write {
            crate::formats::outbox::StagedWrite::Relate { from, to, .. } => {
                vec![from.clone(), to.clone()]
            }
            _ => Vec::new(),
        })
        .filter(|end| batch.is_destination_item(end))
        .collect();
    let mut ids: Vec<String> = reads.iter().map(|read| read.item.id.clone()).collect();
    for end in named {
        if !ids.contains(&end) {
            ids.push(end);
        }
    }
    if !ids.is_empty() {
        let details = store.read(&ids, true)?;
        for (at, detail) in details.into_iter().enumerate() {
            let Some(item) = detail.item else {
                continue;
            };
            match reads.get_mut(at) {
                Some(read) => {
                    read.item = item;
                    read.comments = detail.comments;
                }
                None => reads.push(Read {
                    item,
                    comments: detail.comments,
                    found_by: vec!["edge".to_owned()],
                }),
            }
        }
    }
    let record_key = batch.record_key();
    let mut existing_tickets: Vec<String> = reads
        .iter()
        .filter(|read| read.item.metadata.contains_key(&record_key))
        .map(|read| read.item.id.clone())
        .collect();
    existing_tickets.sort();
    work::write(
        &batch.work.existing_tickets(&batch.batch),
        &ExistingTickets {
            tickets: existing_tickets.clone(),
        },
    )?;
    // Under relations, the edges the found tickets hold, in one read; then, in one more, each
    // ticket at their other ends, whose facts a rollup takes.
    state.relations = if batch.config.relations.is_empty() || existing_tickets.is_empty() {
        Vec::new()
    } else {
        store.relations(&existing_tickets)?
    };
    let mut related: Vec<String> = Vec::new();
    for item in &state.relations {
        let ends = item
            .parent
            .iter()
            .chain(&item.children)
            .chain(item.links.iter().map(|link| &link.other));
        for end in ends {
            if !reads.iter().any(|read| read.item.id == *end) && !related.contains(end) {
                related.push(end.clone());
            }
        }
    }
    if !related.is_empty() {
        for detail in store.read(&related, true)? {
            if let Some(item) = detail.item {
                reads.push(Read {
                    item,
                    comments: detail.comments,
                    found_by: vec!["related".to_owned()],
                });
            }
        }
    }
    state.reads = reads;
    state.read_at_receipt = crate::verify::journal::read(&batch)?.receipts.len();
    state.searched_values = searched_values;
    state.checked = None;
    batch.save_state(&state)?;
    let journal = crate::verify::journal::read(&batch)?;
    let graph = Graph::of(
        &batch,
        &state,
        &batch.outbox()?,
        &crate::verify::journal::bound(&journal),
        Scope::Written,
    )?;
    let missing_links = relations::missing_links(&state);

    // The verifying agent's task.
    let drafts: Vec<Value> = scoped
        .iter()
        .filter(|input| input.source == "draft")
        .map(|input| {
            json!({"id": input.id, "title": input.title, "repository": input.repository,
                   "paths": input.paths, "body": input.body})
        })
        .collect();
    let inputs: Vec<Value> = scoped
        .iter()
        .filter(|input| input.source != "draft")
        .map(|input| {
            json!({"id": input.id, "source": input.source, "summary": input.title,
                   "detail": input.body})
        })
        .collect();
    let task = render_task(&batch, &drafts, &inputs)?;
    let task_path = batch.work.task(&batch.batch);
    work::write_text(&task_path, &task)?;
    Ok(Prepared {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch.clone(),
        drafts: drafts.len(),
        inputs: inputs.len(),
        answers: batch.work.dispositions(&batch.batch),
        task_template: task_path,
        absent_targets: graph.absent,
        missing_links,
        store_operations: store.operations(),
    })
}

/// The verifying agent's task, rendered from the configured task template.
fn render_task(batch: &Batch<'_>, drafts: &[Value], inputs: &[Value]) -> Result<String, Error> {
    let config = batch.config;
    let kinds: Vec<Value> = crate::api::kinds(batch.settings, crate::api::NoArgs {})?
        .kinds
        .into_iter()
        .map(|kind| {
            json!({
                "name": kind.name,
                "description": kind.description,
                "duplicates": match &kind.duplicates {
                    Duplicates::Agent => "agent".to_owned(),
                    Duplicates::Same(fields) => format!("same: [{}]", fields.join(", ")),
                },
                "priority": {"model": match &kind.priority {
                    PriorityModel::Estimate => "estimate".to_owned(),
                    PriorityModel::Fixed(priority) =>
                        format!("fixed: {}", crate::verify::ticket::priority_word(*priority)),
                }},
                "initial_status": kind.initial_status,
                "destination": kind.destination,
                "dispositions": kind.dispositions,
            })
        })
        .collect();
    let fields: Map<String, Value> = config
        .fields
        .iter()
        .map(|(name, field)| (name.clone(), json!({"description": field.description})))
        .collect();
    let default_kind = batch.kind(&config.default_kind);
    let example = template::render(
        &batch.ticket_template(default_kind),
        &json!({"rubric": config.rubric, "fields": fields}),
    )?;
    let relations: Vec<Value> = config
        .relations
        .iter()
        .map(|relation| {
            let description = match &relation.decided_by {
                DecidedBy::Match(fields) => format!(
                    "a `{}` to the `{}` carrying its {}: derived from those fields, never staged by hand",
                    relation.from,
                    relation.to,
                    fields
                        .iter()
                        .map(|field| format!("`{field}`"))
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
                DecidedBy::Agent => format!(
                    "a `{}` to a `{}`, as you judge it: stage each edge with `onefollowup ticket relate --relation {}`",
                    relation.from, relation.to, relation.name
                ),
            };
            json!({"name": relation.name, "description": description})
        })
        .collect();
    Ok(template::render(
        &batch.task_template(),
        &json!({
            "batch": batch.batch,
            "answers": batch.work.dispositions(&batch.batch),
            "tickets": batch.work.tickets(&batch.batch),
            "drafts": drafts,
            "inputs": inputs,
            "destination": config.destination,
            "namespace": config.namespace,
            "kinds": kinds,
            "fields": fields,
            "relations": relations,
            "dispositions": config.dispositions,
            "statuses": config.statuses,
            "rubric": config.rubric,
            "estimate": config.estimate,
            "ticket_template": example,
        }),
    )?)
}
