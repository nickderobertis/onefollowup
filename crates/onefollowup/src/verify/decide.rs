//! What a ticket's copy would write, and what an item's re-estimate would: decided from the
//! ticket, its kind, the first matching route and prepare's read of the destination — never
//! from a fresh read.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::config::{Kind, Priority, Route, StatusRole};
use crate::formats::batch::{BatchState, Read};
use crate::formats::outbox::{Outbox, StagedWrite};
use crate::formats::ticket::TicketRecord;
use crate::formats::{Level, Versioned};
use crate::verify::context::Batch;
use crate::verify::relations::{End, Graph};
use crate::verify::routes::{self, Facts};
use crate::verify::ticket::{self, Estimated, TicketFile};

/// A ticket's copy, decided.
#[derive(Debug, Clone)]
pub struct Decision {
    /// The ticket's root cause.
    pub slug: String,
    /// Its kind.
    pub kind: Kind,
    /// The source it is copied to.
    pub destination: String,
    /// The drafts-source item it is stored as, once it is.
    pub item: Option<String>,
    /// The card it is bound to, once a copy of it landed.
    pub card: Option<String>,
    /// The version the card's write is held to.
    pub hold: Option<String>,
    /// Prepare's read of the card.
    pub read: Option<Read>,
    /// Its occurrences.
    pub occurrences: u64,
    /// Its estimate.
    pub estimate: Estimated,
    /// The first route it matches, and its position.
    pub route: Option<(usize, Route)>,
    /// The status role it is copied at.
    pub status_role: StatusRole,
    /// The priority it is copied with.
    pub priority: Priority,
    /// Whether that is a person's priority the copy keeps.
    pub priority_kept: bool,
    /// The labels it carries.
    pub labels: Vec<String>,
    /// Whether it is withdrawn.
    pub withdrawal: bool,
    /// The rubric severity it records.
    pub severity: Option<String>,
    /// The paths its drafts name.
    pub paths: Vec<String>,
    /// The record it is copied with.
    pub record: TicketRecord,
    /// The body it is copied with.
    pub body: String,
    /// The batch's edges from it: each relation and its `to` end.
    pub edges: Vec<(String, End)>,
    /// The one person its route assigns it to.
    pub assignee: Option<String>,
}

/// A priority word read as a configured priority.
#[must_use]
pub fn priority_of(word: &str) -> Priority {
    serde_json::from_value(Value::from(word)).unwrap_or(Priority::None)
}

fn as_priority(level: Level) -> Priority {
    match level {
        Level::Urgent => Priority::Urgent,
        Level::High => Priority::High,
        Level::Medium => Priority::Medium,
        Level::Low => Priority::Low,
    }
}

fn as_level(priority: Priority) -> Option<Level> {
    match priority {
        Priority::Urgent => Some(Level::Urgent),
        Priority::High => Some(Level::High),
        Priority::Medium => Some(Level::Medium),
        Priority::Low => Some(Level::Low),
        Priority::None => None,
    }
}

/// The record an item read carries, when it carries one this reads.
#[must_use]
pub fn item_record(batch: &Batch<'_>, read: &Read) -> Option<TicketRecord> {
    read.item
        .metadata
        .get(&batch.record_key())
        .and_then(|value| TicketRecord::parse(&value.to_string()).ok())
}

/// How many of `read`'s comments count as occurrences of its record's root cause, plus the
/// item's own evidence, plus each evidence comment this batch staged on it.
#[must_use]
pub fn occurrences(batch: &Batch<'_>, read: &Read, record: &TicketRecord, outbox: &Outbox) -> u64 {
    let namespace = &batch.config.namespace;
    let counted = read
        .comments
        .iter()
        .flatten()
        .filter(|comment| {
            ticket::counts_as_occurrence(
                &comment.body,
                namespace,
                &record.created_by_batch,
                &record.root_cause,
            )
        })
        .count();
    let staged = outbox
        .writes
        .iter()
        .filter(|write| match write {
            StagedWrite::Comment { item, body, .. } => {
                item == &read.item.id
                    && ticket::counts_as_occurrence(
                        body,
                        namespace,
                        &record.created_by_batch,
                        &record.root_cause,
                    )
            }
            _ => false,
        })
        .count();
    1 + counted as u64 + staged as u64
}

/// The read of `item` prepare made, or a search added.
#[must_use]
pub fn read_of<'a>(state: &'a BatchState, item: &str) -> Option<&'a Read> {
    state.reads.iter().find(|read| read.item.id == item)
}

/// The card each ticket is bound to, by its drafts-source item, with the version its latest
/// landed copy recorded.
pub type Bound = BTreeMap<String, (String, Option<String>)>;

/// Decide `ticket`'s copy, its estimate taking what its relations roll up and its route
/// matched on its edges too.
///
/// # Errors
///
/// A ticket of an unconfigured kind, or one whose facts estimate nothing.
#[allow(
    clippy::too_many_lines,
    reason = "one decision: the card, the estimate, the route, then what each sets"
)]
pub fn decide(
    batch: &Batch<'_>,
    state: &BatchState,
    bound: &Bound,
    latest: &BTreeMap<String, String>,
    graph: &Graph,
    outbox: &Outbox,
    ticket: &TicketFile,
) -> Result<Decision, String> {
    let config = batch.config;
    let kind = ticket
        .kind(config)
        .ok_or_else(|| format!("the kind `{}` is not configured", ticket.kind_name(config)))?
        .clone();
    let mut record = ticket
        .record
        .clone()
        .ok_or_else(|| "the ticket's record does not read".to_owned())?;
    let destination = batch.destination_of(&kind);
    let item = state.tickets.get(&ticket.slug).cloned();
    let card_and_version = item.as_ref().and_then(|item| bound.get(item)).cloned();
    let card = card_and_version.as_ref().map(|(card, _)| card.clone());
    let read = card.as_ref().and_then(|card| read_of(state, card)).cloned();
    let hold = card.as_ref().and_then(|card| {
        latest
            .get(card)
            .cloned()
            .or_else(|| read.as_ref().and_then(|read| read.item.version.clone()))
            .or_else(|| {
                card_and_version
                    .as_ref()
                    .and_then(|(_, version)| version.clone())
            })
    });
    let empty = Outbox {
        batch: batch.batch.clone(),
        writes: Vec::new(),
    };
    let occurrences = read
        .as_ref()
        .map_or(1, |read| occurrences(batch, read, &record, &empty));
    let end = End::Ticket(ticket.slug.clone());
    let (estimate, occurrences) = if graph.edges.is_empty() && graph.held.is_empty() {
        (
            ticket::estimate(config, &kind, &ticket.body, record.frequency, occurrences)?,
            occurrences,
        )
    } else {
        graph.rolled(
            batch,
            state,
            outbox,
            bound,
            &end,
            &kind,
            &ticket.body,
            record.frequency,
            occurrences,
        )?
    };
    let counts = graph.counts(batch, state, bound, &end);
    let severity = ticket::severity(&ticket.body, &config.rubric);
    let mut paths: Vec<String> = Vec::new();
    for draft in &record.drafts {
        if let Some(input) = state.inputs.iter().find(|input| &input.id == draft) {
            for path in &input.paths {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
        }
    }
    let route = routes::first(
        &config.routes,
        &Facts {
            kind: &kind.name,
            severity: severity.as_deref(),
            repository: &record.repository,
            paths: &paths,
            relations: &counts,
        },
    )
    .map(|(at, route)| (at, route.clone()));
    let withdrawal = ticket.withdrawn();
    let set = route
        .as_ref()
        .map(|(_, route)| route.set.clone())
        .unwrap_or_default();
    let status_role = if withdrawal {
        StatusRole::Cancelled
    } else if let Some(read) = &read {
        batch
            .role_of(&read.item.status.category)
            .unwrap_or(kind.initial_status)
    } else {
        set.status_role.unwrap_or(kind.initial_status)
    };
    let decided = set.priority.unwrap_or_else(|| as_priority(estimate.level));
    let (priority, priority_kept) = match &read {
        Some(read) => {
            let held = priority_of(&read.item.priority);
            let stored = item_record(batch, read)
                .and_then(|record| record.board_estimate)
                .map_or(Priority::None, as_priority);
            if held == stored {
                (decided, false)
            } else {
                (held, true)
            }
        }
        None => (decided, false),
    };
    record.priority_estimate = estimate.level;
    record.board_estimate = as_level(priority);
    if record.kind.as_deref() == Some(config.default_kind.as_str()) {
        record.kind = None;
    }
    if record
        .fields
        .as_ref()
        .is_some_and(indexmap::IndexMap::is_empty)
    {
        record.fields = None;
    }
    let body = match &estimate.line {
        Some(line) => ticket::with_estimate_line(&ticket.body, line, &config.rubric),
        None => ticket.body.clone(),
    };
    // A new ticket is copied with the person its route assigns it; a bound one keeps what its
    // item records, the batch's `update-many` writing any other.
    let assignee = set.assignee.clone();
    record.board_assignee = match &read {
        None => assignee.clone(),
        Some(read) => item_record(batch, read).and_then(|held| held.board_assignee),
    };
    let edges = graph
        .edges
        .iter()
        .filter(|edge| edge.from == end)
        .map(|edge| (edge.relation.clone(), edge.to.clone()))
        .collect();
    Ok(Decision {
        slug: ticket.slug.clone(),
        kind,
        destination,
        item,
        card,
        hold,
        read,
        occurrences,
        estimate,
        route,
        status_role,
        priority,
        priority_kept,
        labels: set.labels.unwrap_or_default(),
        withdrawal,
        severity,
        paths,
        record,
        body,
        edges,
        assignee,
    })
}

/// The estimate an item's re-estimate writes, from prepare's read of it, this batch's
/// staged comments on it, and what its relations roll up into it.
///
/// # Errors
///
/// An item prepare did not read with its comments, or that carries no record this reads.
pub fn re_estimate(
    batch: &Batch<'_>,
    state: &BatchState,
    outbox: &Outbox,
    bound: &Bound,
    graph: &Graph,
    item: &str,
) -> Result<(Estimated, u64, TicketRecord, Read), String> {
    let read = read_of(state, item)
        .ok_or_else(|| {
            format!("prepare did not read `{item}`; search for it, or prepare the batch again")
        })?
        .clone();
    if read.comments.is_none() {
        return Err(format!(
            "prepare did not read `{item}`'s comments, which its occurrences are counted from; prepare the batch again"
        ));
    }
    let record = item_record(batch, &read).ok_or_else(|| {
        format!(
            "`{item}` carries no `{}` record this reads, so it has nothing to estimate from",
            batch.record_key()
        )
    })?;
    let kind_name = record
        .kind
        .clone()
        .unwrap_or_else(|| batch.config.default_kind.clone());
    let kind = batch.kind(&kind_name).ok_or_else(|| {
        format!("`{item}` records the kind `{kind_name}`, which is not configured")
    })?;
    let count = occurrences(batch, &read, &record, outbox);
    let (estimated, count) = if graph.edges.is_empty() && graph.held.is_empty() {
        (
            ticket::estimate(
                batch.config,
                kind,
                &read.item.content,
                record.frequency,
                count,
            )?,
            count,
        )
    } else {
        graph.rolled(
            batch,
            state,
            outbox,
            bound,
            &End::Item(item.to_owned()),
            kind,
            &read.item.content,
            record.frequency,
            count,
        )?
    };
    Ok((estimated, count, record, read))
}
