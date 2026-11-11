//! The verifying agent's tools, `ticket validate|search|board-status|copy|comment|re-estimate`.
//! None writes to a destination: `copy`, `comment` and `re-estimate` stage into the batch's
//! outbox, and `search` is the one that reads it.

use std::io::Read as _;

use crate::api::{
    BoardComment, BoardStatus, ItemStatus, SCHEMA_VERSION, SearchHit, SearchResults, Staged,
    TicketBoardStatusArgs, TicketCommentArgs, TicketCopyArgs, TicketReEstimateArgs,
    TicketSearchArgs, TicketValidateArgs, TicketValidated,
};
use crate::config::{Duplicates, Settings};
use crate::error::Error;
use crate::formats::HeldPriority;
use crate::formats::batch::{BatchState, Merged, Read};
use crate::formats::markers::EvidenceComment;
use crate::formats::outbox::{Outbox, StagedWrite};
use crate::store::{Search, SearchIn};
use crate::verify::context::Batch;
use crate::verify::decide::{self, read_of};
use crate::verify::journal;
use crate::verify::ticket::{self, Context, SLUG, TicketFile};

/// Every rule `ticket` breaks, held to the batch's input set when it was prepared.
#[must_use]
pub fn ticket_problems(
    batch: &Batch<'_>,
    state: Option<&BatchState>,
    ticket: &TicketFile,
) -> Vec<String> {
    let inputs: Option<Vec<String>> =
        state.map(|state| state.inputs.iter().map(|input| input.id.clone()).collect());
    let template = batch.ticket_template(ticket.kind(batch.config));
    ticket::problems(
        ticket,
        batch.config,
        &Context {
            batch: &batch.batch,
            inputs: inputs.as_deref(),
            template: &template,
        },
    )
}

/// `ticket validate`.
///
/// # Errors
///
/// A refused batch id, or no such ticket.
pub fn validate(settings: &Settings, args: TicketValidateArgs) -> Result<TicketValidated, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let state = batch.read_state()?;
    let ticket = batch.ticket(&args.ticket)?;
    let problems = ticket_problems(&batch, state.as_ref(), &ticket);
    Ok(TicketValidated {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        ticket: ticket.slug,
        sound: problems.is_empty(),
        problems,
    })
}

fn hit(read: &crate::store::StoreItem) -> SearchHit {
    SearchHit {
        item: read.id.clone(),
        title: read.title.clone(),
        status: Some(ItemStatus {
            category: read.status.category.clone(),
            name: read.status.name.clone(),
        }),
        url: read.url.clone(),
    }
}

/// `ticket search`: one `task find` of every question asked.
///
/// # Errors
///
/// A batch not prepared, an unnarrowed question, or the store's refusal.
pub fn search(settings: &Settings, args: TicketSearchArgs) -> Result<SearchResults, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let mut state = batch.state()?;
    let questions: Vec<String> = std::iter::once(&args.query)
        .chain(&args.queries)
        .map(|query| query.trim().to_owned())
        .collect();
    if let Some(empty) = questions.iter().position(String::is_empty) {
        return Err(Error::Refused(format!(
            "question {} is narrowed by nothing; ask the destination for words an item would carry",
            empty + 1
        )));
    }
    let destination = batch
        .ticket(&args.ticket)
        .ok()
        .and_then(|ticket| {
            ticket
                .kind(batch.config)
                .map(|kind| batch.destination_of(kind))
        })
        .unwrap_or_else(|| batch.config.destination.clone());
    let store = batch.store()?;
    let searches: Vec<Search> = questions
        .iter()
        .enumerate()
        .map(|(at, question)| Search {
            key: format!("search:{}:{at}", args.ticket),
            text: Some(question.clone()),
            within: SearchIn::Both,
            ..Search::default()
        })
        .collect();
    let found = store.find(&destination, &searches)?;
    let mut hits: Vec<SearchHit> = Vec::new();
    for answer in &found {
        for item in &answer.items {
            if !hits.iter().any(|one| one.item == item.id) {
                hits.push(hit(item));
            }
            match state.reads.iter_mut().find(|read| read.item.id == item.id) {
                Some(read) => {
                    if !read.found_by.contains(&answer.key) {
                        read.found_by.push(answer.key.clone());
                    }
                }
                None => state.reads.push(Read {
                    item: item.clone(),
                    comments: None,
                    found_by: vec![answer.key.clone()],
                }),
            }
        }
    }
    batch.save_state(&state)?;
    Ok(SearchResults {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        ticket: args.ticket,
        query: args.query,
        hits,
        store_operations: store.operations(),
    })
}

fn held_priority(word: &str) -> HeldPriority {
    serde_json::from_value(serde_json::Value::from(word)).unwrap_or(HeldPriority::None)
}

/// `ticket board-status`: an item as prepare read it, or the status and priority a ticket
/// would be copied with.
///
/// # Errors
///
/// A batch not prepared, or an item it did not read, or a ticket it cannot decide.
pub fn board_status(
    settings: &Settings,
    args: TicketBoardStatusArgs,
) -> Result<BoardStatus, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let state = batch.state()?;
    if batch.is_destination_item(&args.item) {
        let read = read_of(&state, &args.item).ok_or_else(|| {
            Error::Refused(format!(
                "prepare did not read `{}`; search for it, or prepare the batch again",
                args.item
            ))
        })?;
        return Ok(item_status(&batch, read));
    }
    let ticket = batch.ticket(&args.item)?;
    let journal = journal::read(&batch)?;
    let decision = decide::decide(
        &batch,
        &state,
        &journal::bound(&journal),
        &journal::latest(&journal, state.read_at_receipt),
        &ticket,
    )
    .map_err(Error::Refused)?;
    let (title, version, url, comments) = match &decision.read {
        Some(read) => {
            let status = item_status(&batch, read);
            (status.title, status.version, status.url, status.comments)
        }
        None => (ticket.title.clone(), None, None, Vec::new()),
    };
    Ok(BoardStatus {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch.clone(),
        item: decision
            .card
            .clone()
            .unwrap_or_else(|| batch.ticket_key(&ticket.slug)),
        title,
        status: Some(ItemStatus {
            category: batch.category_of(decision.status_role),
            name: decision.status_role.as_str().to_owned(),
        }),
        priority: held_priority(ticket::priority_word(decision.priority)),
        version,
        url,
        comments,
        status_role: Some(decision.status_role),
        priority_estimate: Some(decision.estimate.level),
        estimate_line: decision.estimate.line.clone(),
    })
}

fn item_status(batch: &Batch<'_>, read: &Read) -> BoardStatus {
    let item = &read.item;
    BoardStatus {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch.clone(),
        item: item.id.clone(),
        title: item.title.clone(),
        status: Some(ItemStatus {
            category: item.status.category.clone(),
            name: item.status.name.clone(),
        }),
        priority: held_priority(&item.priority),
        version: item.version.clone(),
        url: item.url.clone(),
        comments: read
            .comments
            .iter()
            .flatten()
            .map(|comment| BoardComment {
                id: comment.id.clone(),
                author: comment.author.clone(),
                url: comment.url.clone(),
                last_changed: comment.last_changed.clone().unwrap_or_default(),
                body: comment.body.clone(),
            })
            .collect(),
        status_role: batch.role_of(&item.status.category),
        priority_estimate: decide::item_record(batch, read).map(|record| record.priority_estimate),
        estimate_line: None,
    }
}

/// Stage `write`, replacing the staged write `same` names, and answer where it sits.
fn stage(outbox: &mut Outbox, write: StagedWrite, same: impl Fn(&StagedWrite) -> bool) -> usize {
    match outbox.writes.iter().position(same) {
        Some(at) => {
            outbox.writes[at] = write;
            at
        }
        None => {
            outbox.writes.push(write);
            outbox.writes.len() - 1
        }
    }
}

fn casefolded(value: &str) -> String {
    value.trim().to_lowercase()
}

fn is_open(category: &str) -> bool {
    !matches!(category, "done" | "cancelled")
}

/// `ticket copy`: stage a ticket's copy, refusing one `ticket validate` refuses, and
/// treating a new ticket of a `same` kind whose listed values an open item or a staged
/// ticket of its kind already carries as that ticket.
///
/// # Errors
///
/// A batch not prepared, no such ticket, or a ticket the rules refuse — receipted, with
/// nothing staged.
pub fn copy(settings: &Settings, args: TicketCopyArgs) -> Result<Staged, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let mut state = batch.state()?;
    let ticket = batch.ticket(&args.ticket)?;
    let key = batch.ticket_key(&ticket.slug);
    let problems = ticket_problems(&batch, Some(&state), &ticket);
    if !problems.is_empty() {
        let message = problems.join("; ");
        journal::append(
            &batch,
            &[journal::refused_copy(
                &batch.batch,
                &key,
                None,
                "invalid",
                &message,
            )],
        )?;
        return Err(Error::Refused(format!(
            "the ticket `{}` is not sound, so nothing was staged: {message}",
            ticket.slug
        )));
    }
    let kind = ticket
        .kind(batch.config)
        .ok_or_else(|| Error::Refused("the ticket's kind is not configured".to_owned()))?;
    let destination = batch.destination_of(kind);
    let mut outbox = batch.outbox()?;
    let journal = journal::read(&batch)?;
    let bound = journal::bound(&journal);
    let is_new = state
        .tickets
        .get(&ticket.slug)
        .is_none_or(|item| !bound.contains_key(item));
    if let Duplicates::Same(fields) = &kind.duplicates
        && is_new
        && !ticket.withdrawn()
    {
        let values: Vec<String> = fields
            .iter()
            .map(|field| casefolded(ticket.field(field).unwrap_or_default()))
            .collect();
        if values.iter().all(|value| !value.is_empty()) {
            let store_operations = ask_values(&batch, &mut state, fields, &ticket, &destination)?;
            let carries = |field_value: &dyn Fn(&str) -> Option<String>| {
                fields.iter().zip(&values).all(|(field, value)| {
                    field_value(field).is_some_and(|held| casefolded(&held) == *value)
                })
            };
            let matched = state.reads.iter().find(|read| {
                read.item.id.starts_with(&format!("{destination}:"))
                    && is_open(&read.item.status.category)
                    && decide::item_record(&batch, read).is_some_and(|record| {
                        record.kind.as_deref().unwrap_or(&batch.config.default_kind) == kind.name
                            && carries(&|field| {
                                record
                                    .fields
                                    .as_ref()
                                    .and_then(|held| held.get(field).cloned())
                            })
                    })
            });
            if let Some(read) = matched.cloned() {
                let root_cause = decide::item_record(&batch, &read)
                    .map_or_else(|| ticket.slug.clone(), |record| record.root_cause);
                let evidence = ticket
                    .section(ticket::EVIDENCE)
                    .filter(|evidence| !evidence.is_empty())
                    .ok_or_else(|| {
                        Error::Refused(format!(
                            "the ticket `{}` is `{}` already filed, and carries no `## {}` to stage there",
                            ticket.slug, read.item.id, ticket::EVIDENCE
                        ))
                    })?;
                let body = EvidenceComment {
                    batch: batch.batch.clone(),
                    root_cause: root_cause.clone(),
                    evidence,
                }
                .render(&batch.config.namespace);
                let item = read.item.id.clone();
                let write = StagedWrite::Comment {
                    item: item.clone(),
                    root_cause: root_cause.clone(),
                    body,
                };
                let index = stage(
                    &mut outbox,
                    write.clone(),
                    |one| matches!(one, StagedWrite::Comment { item: i, root_cause: r, .. } if *i == item && *r == root_cause),
                );
                outbox
                    .writes
                    .retain(|one| !matches!(one, StagedWrite::Copy { ticket: t, .. } if *t == key));
                let index = outbox
                    .writes
                    .iter()
                    .position(|one| *one == write)
                    .unwrap_or(index);
                state.merged.retain(|merged| merged.ticket != ticket.slug);
                state.merged.push(Merged {
                    ticket: ticket.slug.clone(),
                    into: item.clone(),
                });
                batch.save_outbox(&outbox)?;
                batch.save_state(&state)?;
                return Ok(Staged {
                    schema_version: SCHEMA_VERSION,
                    batch: batch.batch,
                    index,
                    write,
                    matched: Some(item),
                    store_operations,
                });
            }
            for other in batch.tickets()? {
                if other.slug == ticket.slug || other.kind_name(batch.config) != kind.name {
                    continue;
                }
                let other_key = batch.ticket_key(&other.slug);
                let staged = outbox.writes.iter().position(
                    |one| matches!(one, StagedWrite::Copy { ticket: t, .. } if *t == other_key),
                );
                if let Some(index) = staged
                    && carries(&|field| other.field(field).map(str::to_owned))
                {
                    state.merged.retain(|merged| merged.ticket != ticket.slug);
                    state.merged.push(Merged {
                        ticket: ticket.slug.clone(),
                        into: other.slug.clone(),
                    });
                    batch.save_state(&state)?;
                    return Ok(Staged {
                        schema_version: SCHEMA_VERSION,
                        batch: batch.batch,
                        index,
                        write: outbox.writes[index].clone(),
                        matched: Some(other_key),
                        store_operations,
                    });
                }
            }
        }
    }
    let write = StagedWrite::Copy {
        ticket: key.clone(),
        destination,
    };
    let index = stage(
        &mut outbox,
        write.clone(),
        |one| matches!(one, StagedWrite::Copy { ticket: t, .. } if *t == key),
    );
    state.merged.retain(|merged| merged.ticket != ticket.slug);
    batch.save_outbox(&outbox)?;
    batch.save_state(&state)?;
    Ok(Staged {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        index,
        write,
        matched: None,
        store_operations: Vec::new(),
    })
}

/// Ask the destination about a `same` ticket's listed values prepare's search did not cover,
/// in one search, adding what it finds to the batch's reads.
fn ask_values(
    batch: &Batch<'_>,
    state: &mut BatchState,
    fields: &[String],
    ticket: &TicketFile,
    destination: &str,
) -> Result<Vec<crate::store::StoreOperation>, Error> {
    let mut searches = Vec::new();
    for field in fields {
        let value = ticket.field(field).unwrap_or_default().trim().to_owned();
        let asked = format!("{field}={value}");
        if state.searched_values.contains(&asked) {
            continue;
        }
        let at = state.searched_values.len();
        searches.push(Search {
            key: format!("value:{at}"),
            metadata: vec![(
                format!("{}/fields/{field}", batch.record_key()),
                value.clone(),
            )],
            ..Search::default()
        });
        searches.push(Search {
            key: format!("value-text:{at}"),
            text: Some(value),
            within: SearchIn::Content,
            ..Search::default()
        });
        state.searched_values.push(asked);
    }
    if searches.is_empty() {
        return Ok(Vec::new());
    }
    let store = batch.store()?;
    for found in store.find(destination, &searches)? {
        for item in found.items {
            if !state.reads.iter().any(|read| read.item.id == item.id) {
                state.reads.push(Read {
                    item,
                    comments: None,
                    found_by: vec![found.key.clone()],
                });
            }
        }
    }
    Ok(store.operations())
}

/// `ticket comment`: stage this batch's evidence comment on another batch's item.
///
/// # Errors
///
/// A batch not prepared, an item it did not read or that is this batch's own, a root cause
/// that is not a slug, or empty evidence.
pub fn comment(settings: &Settings, args: TicketCommentArgs) -> Result<Staged, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let state = batch.state()?;
    if !SLUG.is_match(&args.root_cause) {
        return Err(Error::Refused(format!(
            "`{}` is not a root-cause slug",
            args.root_cause
        )));
    }
    let read = read_of(&state, &args.item).ok_or_else(|| {
        Error::Refused(format!(
            "`{}` is not an item this batch read: search for it, or prepare the batch again",
            args.item
        ))
    })?;
    if decide::item_record(&batch, read)
        .is_some_and(|record| record.created_by_batch == batch.batch)
    {
        return Err(Error::Refused(format!(
            "`{}` is this batch's own item: stage its ticket again with `ticket copy` instead",
            args.item
        )));
    }
    let evidence = match args.evidence {
        Some(evidence) => evidence,
        None => {
            let mut evidence = String::new();
            std::io::stdin()
                .read_to_string(&mut evidence)
                .map_err(|error| Error::Failed(format!("stdin could not be read: {error}")))?;
            evidence
        }
    };
    if evidence.trim().is_empty() {
        return Err(Error::Refused(
            "the evidence, read from stdin, is empty".to_owned(),
        ));
    }
    let body = EvidenceComment {
        batch: batch.batch.clone(),
        root_cause: args.root_cause.clone(),
        evidence,
    }
    .render(&batch.config.namespace);
    let write = StagedWrite::Comment {
        item: args.item.clone(),
        root_cause: args.root_cause.clone(),
        body,
    };
    let mut outbox = batch.outbox()?;
    let index = stage(
        &mut outbox,
        write.clone(),
        |one| matches!(one, StagedWrite::Comment { item, root_cause, .. } if *item == args.item && *root_cause == args.root_cause),
    );
    batch.save_outbox(&outbox)?;
    Ok(Staged {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        index,
        write,
        matched: None,
        store_operations: Vec::new(),
    })
}

/// `ticket re-estimate`: stage the estimate an item's recounted occurrences give.
///
/// # Errors
///
/// A batch not prepared, or an item it cannot estimate.
pub fn re_estimate(settings: &Settings, args: TicketReEstimateArgs) -> Result<Staged, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let state = batch.state()?;
    let mut outbox = batch.outbox()?;
    let (estimated, _, _, _) =
        decide::re_estimate(&batch, &state, &outbox, &args.item).map_err(Error::Refused)?;
    let write = StagedWrite::ReEstimate {
        item: args.item.clone(),
        priority_estimate: estimated.level,
    };
    let index = stage(
        &mut outbox,
        write.clone(),
        |one| matches!(one, StagedWrite::ReEstimate { item, .. } if *item == args.item),
    );
    batch.save_outbox(&outbox)?;
    Ok(Staged {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        index,
        write,
        matched: None,
        store_operations: Vec::new(),
    })
}
