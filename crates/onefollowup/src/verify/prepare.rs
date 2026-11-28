//! `prepare`: snapshot the batch's input set — its drafts and the `batch_preparing` hook's
//! inputs — read the destination once for the whole batch (one search per source searched,
//! each input's duplicate queries going only to the source a ticket about it would land in by
//! the store's routes, one comments read, and under a configuration with relations one
//! `task relations` read of the tickets found), and write the verifying agent's task, with
//! each input's duplicate candidates, and answers skeleton. Each phase prints its progress on
//! standard error, for people (`Store::phase`).

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::api::{DuplicateQuery, DuplicateSearch, PrepareArgs, Prepared, SCHEMA_VERSION};
use crate::config::{DecidedBy, Duplicates, PriorityModel, Settings};
use crate::error::Error;
use crate::formats::Versioned;
use crate::formats::batch::{BatchState, Input, Read};
use crate::formats::dispositions::{CarriedDispositions, DispositionEntry, Dispositions};
use crate::formats::draft::DraftRecord;
use crate::formats::existing_tickets::ExistingTickets;
use crate::store::{Search, SearchIn, Store};
use crate::verify::candidates;
use crate::verify::context::Batch;
use crate::verify::hook;
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
fn hook_inputs(batch: &Batch<'_>, store: &Store) -> Result<Vec<Input>, Error> {
    let Some(argv) = &batch.config.hooks.batch_preparing else {
        return Ok(Vec::new());
    };
    let named = format!("the `hooks.batch_preparing` command `{}`", argv.join(" "));
    let document = json!({"schema_version": 1, "batch": batch.batch});
    let ran =
        hook::run(batch, Some(store), "batch_preparing", argv, &document).map_err(|error| {
            match error {
                Error::Failed(message) => {
                    Error::Failed(format!("{message}; nothing was snapshotted"))
                }
                other => other,
            }
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
            author: None,
            transcript: None,
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
            author: record.as_ref().map(|record| record.author.source()),
            transcript: record.as_ref().and_then(|record| record.transcript.clone()),
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

/// Which pass of the batch an input set belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The batch's first: it had no state.
    First,
    /// The open pass its state holds: no publish yet, or one not settled.
    Continuing,
    /// The pass after a settled one, carrying that pass's inputs.
    AfterSettled,
}

/// The batch's input set: the step `prepare` and `snapshot` share, so the two cannot
/// disagree about what a batch holds. A continuing pass keeps its input set and runs no
/// hook; any other runs the `batch_preparing` hook and one drafts search for the batch and
/// its aliases, adding what is new by id. It writes nothing.
///
/// # Errors
///
/// A refused alias, a failing hook (nothing snapshotted), or the store's refusal.
pub fn input_set(
    batch: &Batch<'_>,
    store: &Store,
    with_aliases: &[String],
) -> Result<(BatchState, Pass), Error> {
    for alias in with_aliases {
        batch.check_id(alias)?;
    }
    let existing = batch.read_state()?;
    let pass = match &existing {
        None => Pass::First,
        Some(state)
            if state.publishes.is_empty()
                || state.publishes.iter().any(|publish| !publish.settled) =>
        {
            Pass::Continuing
        }
        Some(_) => Pass::AfterSettled,
    };
    let hooked = if pass == Pass::Continuing {
        Vec::new()
    } else {
        hook_inputs(batch, store)?
    };
    let state = match existing {
        Some(state) if pass == Pass::Continuing => state,
        earlier => {
            // Each alias once, and never the batch itself: the drafts search asks one
            // question per id, which the store refuses to see twice.
            let mut aliases = earlier
                .as_ref()
                .map(|state| state.aliases.clone())
                .unwrap_or_default();
            for alias in with_aliases {
                if !aliases.contains(alias) && *alias != batch.batch {
                    aliases.push(alias.clone());
                }
            }
            let (drafted, project) = drafts(batch, store, &aliases)?;
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
    Ok((state, pass))
}

/// Snapshot the batch and write the verifying agent's task.
///
/// # Errors
///
/// A refused batch id, a failing hook (nothing snapshotted and nothing read), or the store's
/// refusal.
pub fn run(settings: &Settings, args: PrepareArgs) -> Result<Prepared, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let store = batch.store()?;
    let phase = store.phase(
        "prepare",
        "snapshot read",
        &format!("batch {}", batch.batch),
    );
    let (mut state, _) = input_set(&batch, &store, &args.aliases)?;
    phase.finish();

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

    // Each input's duplicate queries: its title, and under a kind whose duplicates the agent
    // judges, its distinctive terms and file names, all riding the one search below.
    let judged = batch
        .config
        .kinds
        .iter()
        .any(|kind| kind.duplicates == Duplicates::Agent);
    let plans: Vec<(&Input, candidates::Plan)> = scoped
        .iter()
        .map(|input| {
            let mut plan = candidates::plan(input);
            if !judged {
                plan.queries
                    .retain(|query| query.kind == candidates::QueryKind::Title);
            }
            (*input, plan)
        })
        .collect();

    // The destination, read once: one search per source searched, one comments read. Each
    // input's duplicate queries go only to the source a ticket about it would land in from
    // each destination, by the store's routes; the tickets' own searches to their destination.
    let tickets = batch.tickets()?;
    let bound = crate::verify::journal::bound(&crate::verify::journal::read(&batch)?);
    let receipted: Vec<&String> = bound.keys().collect();
    let mut searched_values = Vec::new();
    let mut reads: Vec<Read> = Vec::new();
    // How many items the store answered each search with, before its limit.
    let mut answered: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    // Each source searched: its searches, and the inputs whose queries they carry.
    let mut routed: IndexMap<String, (Vec<Search>, Vec<String>)> = IndexMap::new();
    let placed: Vec<(&str, Option<&str>)> = plans
        .iter()
        .map(|(input, _)| (input.id.as_str(), input.repository.as_deref()))
        .collect();
    for destination in batch.destinations() {
        let lands = store.routed(&destination, &placed)?;
        for ((input, plan), lands) in plans.iter().zip(lands) {
            let (searches, inputs) = routed.entry(lands).or_default();
            if !inputs.contains(&input.id) {
                searches.extend(candidates::searches(plan));
                inputs.push(input.id.clone());
            }
        }
        let mut searches: Vec<Search> = Vec::new();
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
        routed.entry(destination).or_default().0.extend(searches);
    }
    for (source, (searches, inputs)) in &routed {
        if searches.is_empty() {
            continue;
        }
        let phase = store.phase(
            "prepare",
            &format!("duplicate find on {source}"),
            &format!(
                "{} drafts ({}), {} searches",
                inputs.len(),
                inputs.join(", "),
                searches.len()
            ),
        );
        let found = store.find(source, searches)?;
        phase.finish();
        for found in found {
            *answered.entry(found.key.clone()).or_default() += found.total;
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
    // Read with their comments — which a re-estimate counts occurrences from — every read an
    // answer can relate: each input's rendered candidates and whatever a search other than a
    // duplicate query found. The find already answered every hit whole, and candidates are a
    // pure function of the reads, so a hit no input renders keeps the find's answer alone, as a
    // `ticket search` hit does; the reads, and so the candidates `check` recomputes, are the same.
    let commented: Option<std::collections::BTreeSet<String>> = judged.then(|| {
        let mut commented: std::collections::BTreeSet<String> = reads
            .iter()
            .filter(|read| {
                read.found_by
                    .iter()
                    .any(|key| !candidates::is_duplicate_query(key))
            })
            .map(|read| read.item.id.clone())
            .collect();
        for (input, plan) in &plans {
            for candidate in candidates::ranked(&batch, input, plan, &reads).rendered {
                commented.insert(candidate.item);
            }
        }
        commented
    });
    let mut ids: Vec<String> = reads
        .iter()
        .map(|read| read.item.id.clone())
        .filter(|id| {
            commented
                .as_ref()
                .is_none_or(|commented| commented.contains(id))
        })
        .collect();
    for end in named {
        if !ids.contains(&end) {
            ids.push(end);
        }
    }
    if !ids.is_empty() {
        let phase = store.phase(
            "prepare",
            "kept-hit detail read",
            &format!("{} items ({})", ids.len(), ids.join(", ")),
        );
        let details = store.read(&ids, true)?;
        phase.finish();
        for (id, detail) in ids.iter().zip(details) {
            let Some(item) = detail.item else {
                continue;
            };
            match reads.iter_mut().find(|read| read.item.id == *id) {
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
    let relating = !batch.config.relations.is_empty() && !existing_tickets.is_empty();
    let phase = relating.then(|| {
        store.phase(
            "prepare",
            "relation reads",
            &format!(
                "{} tickets ({})",
                existing_tickets.len(),
                existing_tickets.join(", ")
            ),
        )
    });
    state.relations = if relating {
        store.relations(&existing_tickets)?
    } else {
        Vec::new()
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
    if let Some(phase) = phase {
        phase.finish();
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
    let hits = candidates::hits(&state.reads);
    let duplicate_searches: Vec<DuplicateSearch> = if judged {
        plans
            .iter()
            .map(|(input, plan)| {
                let ranked = candidates::ranked(&batch, input, plan, &state.reads);
                DuplicateSearch {
                    input: input.id.clone(),
                    stoplisted: plan.stoplisted.clone(),
                    queries: plan
                        .queries
                        .iter()
                        .map(|query| {
                            let found = answered.get(&query.key).copied().unwrap_or_default();
                            DuplicateQuery {
                                text: query.text.clone(),
                                kind: query.kind,
                                hits: found,
                                retained: hits.get(query.key.as_str()).copied().unwrap_or_default(),
                                capped: found > candidates::HIT_CAP,
                            }
                        })
                        .collect(),
                    matching: ranked.matching,
                    candidates: ranked.rendered,
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let rendered: Map<String, Value> = duplicate_searches
        .iter()
        .map(|search| (search.input.clone(), json!(search.candidates)))
        .collect();
    let task = render_task(&batch, &drafts, &inputs, &rendered)?;
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
        duplicate_searches,
        store_operations: store.operations(),
        governed: crate::api::Governed::of(&store),
    })
}

/// The verifying agent's task, rendered from the configured task template.
fn render_task(
    batch: &Batch<'_>,
    drafts: &[Value],
    inputs: &[Value],
    candidates: &Map<String, Value>,
) -> Result<String, Error> {
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
    let default_kind = batch
        .kind(&config.default_kind)
        .ok_or_else(|| Error::Refused("the default kind is not configured".to_owned()))?;
    let example = template::render_value(
        &batch.ticket_template(Some(default_kind)),
        crate::verify::render::Common {
            config,
            kind: default_kind,
            batch: &batch.batch,
            destination: &batch.destination_of(default_kind),
        }
        .example(),
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
            "candidates": candidates,
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
