//! The batch's edges, held to the configured relations.
//!
//! An edge is a relation's name over two ends — a ticket of this batch, by its root cause, or a
//! destination item the batch read — pointing from the more specific ticket to the more
//! general. A relation `decided_by: {match}` has its edges **derived**: the `to` ticket is the
//! one of the relation's `to` kind whose values of the relation's fields equal the `from`
//! ticket's, compared as `same` compares (trimmed, case-folded, an empty value matching
//! nothing), so no such edge is ever asserted or read from prose. A relation `decided_by:
//! agent` has its edges **asserted**, each a `relate` the agent staged.
//!
//! Every verb that reads edges builds one [`Graph`]: the batch's own edges (derived and
//! asserted), the edges the store already holds between the items the batch read (prepare's one
//! `task relations` read and the records' `edges`), and each derived edge whose target does
//! not exist. An edge's identity is its relation and its two ends, a ticket already copied
//! standing for the item it was copied to.

use std::collections::BTreeSet;

use indexmap::IndexMap;

use crate::config::{Cardinality, DecidedBy, Kind, Relation, RelationStore, Rollup};
use crate::formats::Level;
use crate::formats::batch::{BatchState, Read};
use crate::formats::outbox::{Outbox, StagedWrite};
use crate::formats::ticket::TicketRecord;
use crate::verify::context::Batch;
use crate::verify::decide::{self, Bound};
use crate::verify::ticket::{self, ESTIMATE_LINE, Estimated, TicketFile};

/// One end of an edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum End {
    /// A ticket of this batch, by its root cause.
    Ticket(String),
    /// A destination item, qualified.
    Item(String),
}

/// One edge of a relation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// The relation's name.
    pub relation: String,
    /// The more specific end.
    pub from: End,
    /// The more general end.
    pub to: End,
    /// The outbox position of the `relate` that asserted it; `None` for a derived edge.
    pub asserted: Option<usize>,
}

/// A derived edge whose target does not exist: the ticket to write and stage.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Absent {
    /// The `from` ticket, by its root cause.
    pub ticket: String,
    /// The relation.
    pub relation: String,
    /// The kind of the ticket that does not exist.
    pub kind: String,
    /// The values of the relation's fields that ticket carries.
    pub fields: IndexMap<String, String>,
}

/// A link one end of which lost its record of it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MissingLink {
    /// The end that still holds the link.
    pub item: String,
    /// The link's type: the relation's name.
    pub relation: String,
    /// The end that lost it.
    pub other: String,
}

/// An edge the store already holds, between two items.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Held {
    /// The relation's name.
    pub relation: String,
    /// The `from` item.
    pub from: String,
    /// The `to` item.
    pub to: String,
}

/// A value as `same` compares it.
#[must_use]
pub fn folded(value: &str) -> String {
    value.trim().to_lowercase()
}

fn is_open(category: &str) -> bool {
    !matches!(category, "done" | "cancelled")
}

fn rank(level: Level) -> u8 {
    match level {
        Level::Urgent => 3,
        Level::High => 2,
        Level::Medium => 1,
        Level::Low => 0,
    }
}

/// Every edge a verb reads.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    /// The batch's own edges: each derived one, then each asserted one that reads.
    pub edges: Vec<Edge>,
    /// Each derived edge whose target does not exist.
    pub absent: Vec<Absent>,
    /// Each edge the store holds between items the batch read.
    pub held: Vec<Held>,
    /// The batch's ticket files.
    pub files: Vec<TicketFile>,
    /// Each staged `relate` that does not read as an edge, with why.
    pub unread: Vec<(usize, String)>,
    /// Each of those whose ends read, which still counts against its relation's cardinality.
    pub refused: Vec<Edge>,
}

/// Which tickets a graph derives edges from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every ticket file the agent wrote: what `prepare` and `ticket validate` report on.
    Written,
    /// Only the tickets whose copy is staged or was published: what `check` and `publish`
    /// hold and send.
    Staged,
}

impl Graph {
    /// The graph of `batch` as `state` and `outbox` hold it; empty under a configuration with
    /// no relations, which reads nothing for it.
    ///
    /// # Errors
    ///
    /// A ticket directory that cannot be read.
    pub fn of(
        batch: &Batch<'_>,
        state: &BatchState,
        outbox: &Outbox,
        bound: &Bound,
        scope: Scope,
    ) -> Result<Self, crate::error::Error> {
        if batch.config.relations.is_empty() {
            return Ok(Self::default());
        }
        let files = batch.tickets()?;
        let mut graph = Self {
            files,
            ..Self::default()
        };
        let in_play: Vec<&TicketFile> = graph
            .files
            .iter()
            .filter(|file| !file.withdrawn())
            .filter(|file| match scope {
                Scope::Written => true,
                Scope::Staged => staged(batch, state, outbox, bound, &file.slug),
            })
            .collect();
        let mut edges = Vec::new();
        let mut absent = Vec::new();
        for relation in &batch.config.relations {
            let DecidedBy::Match(fields) = &relation.decided_by else {
                continue;
            };
            for file in in_play
                .iter()
                .filter(|file| file.kind_name(batch.config) == relation.from)
            {
                match target(batch, state, &in_play, relation, fields, file) {
                    Some(to) => edges.push(Edge {
                        relation: relation.name.clone(),
                        from: End::Ticket(file.slug.clone()),
                        to,
                        asserted: None,
                    }),
                    None => absent.push(Absent {
                        ticket: file.slug.clone(),
                        relation: relation.name.clone(),
                        kind: relation.to.clone(),
                        fields: fields
                            .iter()
                            .map(|field| {
                                (
                                    field.clone(),
                                    file.field(field).unwrap_or_default().trim().to_owned(),
                                )
                            })
                            .collect(),
                    }),
                }
            }
        }
        let mut unread = Vec::new();
        for (index, write) in outbox.writes.iter().enumerate() {
            let StagedWrite::Relate { relation, from, to } = write else {
                continue;
            };
            match graph.asserted(batch, state, relation, from, to) {
                Ok((from, to)) => edges.push(Edge {
                    relation: relation.clone(),
                    from,
                    to,
                    asserted: Some(index),
                }),
                Err(message) => {
                    // An edge refused only for being asserted still counts against its
                    // relation's cardinality.
                    if let Some(configured) = graph.relation(batch, relation)
                        && let Ok((from, to)) = graph.ends(batch, state, configured, from, to)
                    {
                        graph.refused.push(Edge {
                            relation: relation.clone(),
                            from,
                            to,
                            asserted: Some(index),
                        });
                    }
                    unread.push((index, message));
                }
            }
        }
        graph.edges = edges;
        graph.absent = absent;
        graph.unread = unread;
        graph.held = held(batch, state);
        Ok(graph)
    }

    /// The ticket file `slug`.
    #[must_use]
    pub fn file(&self, slug: &str) -> Option<&TicketFile> {
        self.files.iter().find(|file| file.slug == slug)
    }

    /// Read an edge as asserted: its relation configured and not derived, its ends resolved
    /// and of the relation's kinds.
    ///
    /// # Errors
    ///
    /// What keeps it from being an edge, and what to do.
    pub fn asserted(
        &self,
        batch: &Batch<'_>,
        state: &BatchState,
        relation: &str,
        from: &str,
        to: &str,
    ) -> Result<(End, End), String> {
        let config = batch.config;
        let Some(configured) = config.relations.iter().find(|one| one.name == relation) else {
            return Err(format!(
                "`{relation}` is not a configured relation; the relations are {}",
                config
                    .relations
                    .iter()
                    .map(|one| format!("`{}`", one.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        if let DecidedBy::Match(fields) = &configured.decided_by {
            return Err(format!(
                "the relation `{relation}` is decided by matching {}: its edges are derived from the tickets' fields, never asserted",
                fields
                    .iter()
                    .map(|field| format!("`{field}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        self.ends(batch, state, configured, from, to)
    }

    /// An edge's two ends, resolved and held to the relation's kinds.
    fn ends(
        &self,
        batch: &Batch<'_>,
        state: &BatchState,
        configured: &Relation,
        from: &str,
        to: &str,
    ) -> Result<(End, End), String> {
        let relation = &configured.name;
        let from = end_of(batch, state, &self.files, from)?;
        let to = end_of(batch, state, &self.files, to)?;
        for (end, wanted, side) in [
            (&from, &configured.from, "from"),
            (&to, &configured.to, "to"),
        ] {
            match kind_of(batch, state, &self.files, end) {
                Some(kind) if kind == *wanted => {}
                Some(kind) => {
                    return Err(format!(
                        "the relation `{relation}` goes from a `{}` to a `{}`, and its `{side}` end {} is a `{kind}`",
                        configured.from,
                        configured.to,
                        named(batch, end)
                    ));
                }
                None => {
                    return Err(format!(
                        "the `{side}` end {} carries no record of its kind",
                        named(batch, end)
                    ));
                }
            }
        }
        Ok((from, to))
    }

    /// Every edge, the batch's and the store's, each once by its identity.
    #[must_use]
    pub fn all(&self, batch: &Batch<'_>, state: &BatchState, bound: &Bound) -> Vec<Edge> {
        let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
        let mut all = Vec::new();
        for edge in &self.edges {
            let identity = (
                edge.relation.clone(),
                identity(batch, state, bound, &edge.from),
                identity(batch, state, bound, &edge.to),
            );
            if seen.insert(identity) {
                all.push(edge.clone());
            }
        }
        for held in &self.held {
            let identity = (held.relation.clone(), held.from.clone(), held.to.clone());
            if seen.insert(identity) {
                all.push(Edge {
                    relation: held.relation.clone(),
                    from: End::Item(held.from.clone()),
                    to: End::Item(held.to.clone()),
                    asserted: None,
                });
            }
        }
        all
    }

    /// How many edges of each relation `end` is either end of, staged ones counted.
    #[must_use]
    pub fn counts(
        &self,
        batch: &Batch<'_>,
        state: &BatchState,
        bound: &Bound,
        end: &End,
    ) -> IndexMap<String, u32> {
        let at = identity(batch, state, bound, end);
        let mut counts = IndexMap::new();
        for edge in self.all(batch, state, bound) {
            if identity(batch, state, bound, &edge.from) == at
                || identity(batch, state, bound, &edge.to) == at
            {
                *counts.entry(edge.relation.clone()).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Each problem of the batch's edges, by the outbox position of the write it names (a
    /// derived edge by its `from` ticket's copy), held to cardinality, to the ends being
    /// staged or read, and to a parent never crossing sources.
    #[must_use]
    pub fn problems(
        &self,
        batch: &Batch<'_>,
        state: &BatchState,
        outbox: &Outbox,
        bound: &Bound,
    ) -> Vec<(String, String)> {
        let mut problems: Vec<(String, String)> = self
            .unread
            .iter()
            .map(|(index, message)| (written(outbox, *index), message.clone()))
            .collect();
        for absent in &self.absent {
            problems.push((
                format!("ticket:{}", absent.ticket),
                format!(
                    "the `{}` ticket its `{}` edge derives, the one whose {}, neither exists nor is staged: write that ticket under the `{}` kind's template and stage it",
                    absent.kind,
                    absent.relation,
                    absent
                        .fields
                        .iter()
                        .map(|(field, value)| format!("`{field}` is `{value}`"))
                        .collect::<Vec<_>>()
                        .join(" and "),
                    absent.kind
                ),
            ));
        }
        for edge in &self.edges {
            let subject = match edge.asserted {
                Some(index) => written(outbox, index),
                None => match &edge.from {
                    End::Ticket(slug) => format!("ticket:{slug}"),
                    End::Item(item) => item.clone(),
                },
            };
            for end in [&edge.from, &edge.to] {
                if let End::Item(item) = end
                    && decide::read_of(state, item).is_some()
                    && !state.relations.iter().any(|read| read.item == *item)
                {
                    problems.push((
                        subject.clone(),
                        format!(
                            "prepare has not read the edges `{item}` already holds, which its relations' cardinality, rollups and routes count: run `onefollowup prepare --batch {}` again, which reads them",
                            batch.batch
                        ),
                    ));
                }
                let in_play = match end {
                    End::Ticket(slug) => staged(batch, state, outbox, bound, slug),
                    End::Item(item) => decide::read_of(state, item).is_some(),
                };
                if !in_play {
                    problems.push((
                        subject.clone(),
                        format!(
                            "the edge's end {} has neither a staged copy nor a read item: stage its ticket, or relate an item the batch read",
                            named(batch, end)
                        ),
                    ));
                }
            }
            let Some(relation) = self.relation(batch, &edge.relation) else {
                continue;
            };
            if relation.store == RelationStore::Parent {
                let from = source_of(batch, self, &edge.from);
                let to = source_of(batch, self, &edge.to);
                if let (Some(from), Some(to)) = (from, to)
                    && from != to
                {
                    problems.push((
                        subject.clone(),
                        format!(
                            "the relation `{}` is held as the store's parent, which joins two items of one source, and {} is on `{from}` while {} is on `{to}`; hold it as a `link`, or give both kinds one destination",
                            relation.name,
                            named(batch, &edge.from),
                            named(batch, &edge.to)
                        ),
                    ));
                }
            }
        }
        let mut all = self.all(batch, state, bound);
        all.extend(self.refused.iter().cloned());
        for relation in &batch.config.relations {
            if relation.cardinality != Cardinality::ManyToOne {
                continue;
            }
            let mut ends: IndexMap<String, Vec<&Edge>> = IndexMap::new();
            for edge in all.iter().filter(|edge| edge.relation == relation.name) {
                ends.entry(identity(batch, state, bound, &edge.from))
                    .or_default()
                    .push(edge);
            }
            for edges in ends.values().filter(|edges| edges.len() > 1) {
                let first = edges[0];
                let subject = edges
                    .iter()
                    .find_map(|edge| edge.asserted)
                    .map_or_else(|| named(batch, &first.from), |index| written(outbox, index));
                problems.push((
                    subject,
                    format!(
                        "the relation `{}` is many-to-one, and {} is the `from` end of {} of its edges, to {}: keep one",
                        relation.name,
                        named(batch, &first.from),
                        edges.len(),
                        edges
                            .iter()
                            .map(|edge| named(batch, &edge.to))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
        problems
    }

    /// The configured relation `name`.
    #[must_use]
    pub fn relation<'a>(&self, batch: &Batch<'a>, name: &str) -> Option<&'a Relation> {
        batch.config.relations.iter().find(|one| one.name == name)
    }

    /// `to`'s estimate with what its relations roll up into it over its direct edges, staged
    /// ones counted: each `occurrences` relation's related tickets' occurrences added to the
    /// count it raises on, and no lower than each `max` relation's related tickets' highest
    /// own estimate. A related ticket's own estimate takes nothing of its own related tickets,
    /// so a rollup is one hop.
    ///
    /// # Errors
    ///
    /// An estimated kind's body stating no severity with the workaround.
    #[allow(
        clippy::too_many_arguments,
        reason = "the batch's shared reads, and the facts the estimate is made of"
    )]
    pub fn rolled(
        &self,
        batch: &Batch<'_>,
        state: &BatchState,
        outbox: &Outbox,
        bound: &Bound,
        to: &End,
        kind: &Kind,
        body: &str,
        frequency: Option<crate::formats::Frequency>,
        occurrences: u64,
    ) -> Result<(Estimated, u64), String> {
        let at = identity(batch, state, bound, to);
        let mut added = 0;
        let mut highest: Option<Level> = None;
        for edge in self.all(batch, state, bound) {
            if identity(batch, state, bound, &edge.to) != at {
                continue;
            }
            let Some(relation) = self.relation(batch, &edge.relation) else {
                continue;
            };
            match relation.rollup {
                Rollup::None => {}
                Rollup::Occurrences => {
                    added += self.occurrences(batch, state, outbox, bound, &edge.from);
                }
                Rollup::Max => {
                    if let Some(level) = self.own(batch, state, outbox, bound, &edge.from)
                        && highest.is_none_or(|held| rank(level) > rank(held))
                    {
                        highest = Some(level);
                    }
                }
            }
        }
        let count = occurrences + added;
        let mut estimated = ticket::estimate(batch.config, kind, body, frequency, count)?;
        if let Some(level) = highest
            && rank(level) > rank(estimated.level)
        {
            estimated.line = estimated.line.map(|line| {
                let inner = line
                    .split_once('(')
                    .and_then(|(_, rest)| rest.strip_suffix(')'))
                    .unwrap_or_default()
                    .to_owned();
                format!(
                    "- {ESTIMATE_LINE}: {} ({inner}; held at {}, its related tickets' highest)",
                    ticket::level_word(level),
                    ticket::level_word(level)
                )
            });
            estimated.level = level;
        }
        Ok((estimated, count))
    }

    fn occurrences(
        &self,
        batch: &Batch<'_>,
        state: &BatchState,
        outbox: &Outbox,
        bound: &Bound,
        end: &End,
    ) -> u64 {
        match read_at(batch, state, bound, end) {
            Some((read, record)) if read.comments.is_some() => {
                decide::occurrences(batch, read, &record, outbox)
            }
            _ => 1,
        }
    }

    /// An end's own estimate, taking nothing of its related tickets.
    fn own(
        &self,
        batch: &Batch<'_>,
        state: &BatchState,
        outbox: &Outbox,
        bound: &Bound,
        end: &End,
    ) -> Option<Level> {
        let occurrences = self.occurrences(batch, state, outbox, bound, end);
        if let End::Ticket(slug) = end
            && let Some(file) = self.file(slug)
        {
            let kind = file.kind(batch.config)?;
            let frequency = file.record.as_ref().and_then(|record| record.frequency);
            return ticket::estimate(batch.config, kind, &file.body, frequency, occurrences)
                .ok()
                .map(|estimated| estimated.level);
        }
        let (read, record) = read_at(batch, state, bound, end)?;
        let kind = batch.kind(record.kind.as_deref().unwrap_or(&batch.config.default_kind))?;
        Some(
            ticket::estimate(
                batch.config,
                kind,
                &read.item.content,
                record.frequency,
                occurrences,
            )
            .map_or(record.priority_estimate, |estimated| estimated.level),
        )
    }
}

/// The read of an end, with its record: an item's, or a copied ticket's card.
fn read_at<'s>(
    batch: &Batch<'_>,
    state: &'s BatchState,
    bound: &Bound,
    end: &End,
) -> Option<(&'s Read, TicketRecord)> {
    let item = match end {
        End::Item(item) => item.clone(),
        End::Ticket(slug) => card(state, bound, slug)?,
    };
    let read = decide::read_of(state, &item)?;
    Some((read, decide::item_record(batch, read)?))
}

/// The card a ticket of the batch was copied to, once a copy of it landed.
#[must_use]
pub fn card(state: &BatchState, bound: &Bound, slug: &str) -> Option<String> {
    let item = state.tickets.get(slug)?;
    bound.get(item).map(|(card, _)| card.clone())
}

/// What an end is, wherever the batch names it: a copied ticket is its card.
#[must_use]
pub fn identity(batch: &Batch<'_>, state: &BatchState, bound: &Bound, end: &End) -> String {
    match end {
        End::Item(item) => item.clone(),
        End::Ticket(slug) => card(state, bound, slug).unwrap_or_else(|| batch.ticket_key(slug)),
    }
}

/// An end as a problem or the outbox names it.
#[must_use]
pub fn named(batch: &Batch<'_>, end: &End) -> String {
    match end {
        End::Ticket(slug) => format!("`{}`", batch.ticket_key(slug)),
        End::Item(item) => format!("`{item}`"),
    }
}

/// An end as the outbox spells it.
#[must_use]
pub fn spelled(batch: &Batch<'_>, end: &End) -> String {
    match end {
        End::Ticket(slug) => batch.ticket_key(slug),
        End::Item(item) => item.clone(),
    }
}

fn written(outbox: &Outbox, index: usize) -> String {
    outbox.writes.get(index).map_or_else(
        || format!("outbox[{index}]"),
        |write| crate::verify::check::named(index, write),
    )
}

/// Whether the ticket `slug` has a copy staged, or published by an earlier pass of the batch.
fn staged(
    batch: &Batch<'_>,
    state: &BatchState,
    outbox: &Outbox,
    bound: &Bound,
    slug: &str,
) -> bool {
    let key = batch.ticket_key(slug);
    outbox
        .writes
        .iter()
        .any(|write| matches!(write, StagedWrite::Copy { ticket, .. } if *ticket == key))
        || card(state, bound, slug).is_some()
}

/// The end `text` names: a ticket of the batch by its key, its root cause or its file — a
/// ticket `ticket copy` found to be an existing one standing for that one — or an item the
/// batch read.
///
/// # Errors
///
/// A name that is neither.
pub fn end_of(
    batch: &Batch<'_>,
    state: &BatchState,
    files: &[TicketFile],
    text: &str,
) -> Result<End, String> {
    let slug = batch.ticket_slug(text).or_else(|| {
        if batch.is_destination_item(text) {
            None
        } else {
            batch.ticket(text).ok().map(|file| file.slug)
        }
    });
    let Some(slug) = slug else {
        if decide::read_of(state, text).is_some() {
            return Ok(End::Item(text.to_owned()));
        }
        return Err(format!(
            "`{text}` is neither a ticket of this batch nor an item it read: write the ticket, or search for the item"
        ));
    };
    if let Some(merged) = state.merged.iter().find(|merged| merged.ticket == slug) {
        return Ok(if batch.is_destination_item(&merged.into) {
            End::Item(merged.into.clone())
        } else {
            End::Ticket(merged.into.clone())
        });
    }
    if files.iter().any(|file| file.slug == slug) {
        Ok(End::Ticket(slug))
    } else {
        Err(format!(
            "there is no ticket `{slug}`: write it as {}",
            batch.work.ticket(&batch.batch, &slug).display()
        ))
    }
}

/// The kind an end records.
#[must_use]
pub fn kind_of(
    batch: &Batch<'_>,
    state: &BatchState,
    files: &[TicketFile],
    end: &End,
) -> Option<String> {
    match end {
        End::Ticket(slug) => files
            .iter()
            .find(|file| file.slug == *slug)
            .map(|file| file.kind_name(batch.config).to_owned()),
        End::Item(item) => {
            let read = decide::read_of(state, item)?;
            let record = decide::item_record(batch, read)?;
            Some(
                record
                    .kind
                    .unwrap_or_else(|| batch.config.default_kind.clone()),
            )
        }
    }
}

/// The source an end is, or will be, on.
fn source_of(batch: &Batch<'_>, graph: &Graph, end: &End) -> Option<String> {
    match end {
        End::Item(item) => item.split_once(':').map(|(source, _)| source.to_owned()),
        End::Ticket(slug) => graph
            .file(slug)
            .and_then(|file| file.kind(batch.config))
            .map(|kind| batch.destination_of(kind)),
    }
}

/// The derived target of `file` under `relation`: the ticket in play, or else the open item
/// read, of the relation's `to` kind carrying equal values of its fields.
fn target(
    batch: &Batch<'_>,
    state: &BatchState,
    in_play: &[&TicketFile],
    relation: &Relation,
    fields: &[String],
    file: &TicketFile,
) -> Option<End> {
    let values: Vec<String> = fields
        .iter()
        .map(|field| folded(file.field(field).unwrap_or_default()))
        .collect();
    if values.iter().any(String::is_empty) {
        return None;
    }
    let carries = |held: &dyn Fn(&str) -> Option<String>| {
        fields
            .iter()
            .zip(&values)
            .all(|(field, value)| held(field).is_some_and(|one| folded(&one) == *value))
    };
    if let Some(found) = in_play.iter().find(|other| {
        other.slug != file.slug
            && other.kind_name(batch.config) == relation.to
            && carries(&|field| other.field(field).map(str::to_owned))
    }) {
        return Some(End::Ticket(found.slug.clone()));
    }
    let destination = batch.destination_of(batch.kind(&relation.to)?);
    state
        .reads
        .iter()
        .find(|read| {
            read.item.id.starts_with(&format!("{destination}:"))
                && is_open(&read.item.status.category)
                && decide::item_record(batch, read).is_some_and(|record| {
                    record.kind.as_deref().unwrap_or(&batch.config.default_kind) == relation.to
                        && carries(&|field| {
                            record
                                .fields
                                .as_ref()
                                .and_then(|held| held.get(field).cloned())
                        })
                })
        })
        .map(|read| End::Item(read.item.id.clone()))
}

/// Every edge the store holds between items the batch read: their links of a configured
/// relation's name, their parents, and the edges their records name.
fn held(batch: &Batch<'_>, state: &BatchState) -> Vec<Held> {
    let config = batch.config;
    let kind = |item: &str| -> Option<String> {
        decide::read_of(state, item)
            .and_then(|read| decide::item_record(batch, read))
            .map(|record| record.kind.unwrap_or_else(|| config.default_kind.clone()))
    };
    let mut held = BTreeSet::new();
    for item in &state.relations {
        for link in &item.links {
            if config
                .relations
                .iter()
                .any(|one| one.name == link.link_type)
            {
                let (from, to) = if link.outgoing {
                    (item.item.clone(), link.other.clone())
                } else {
                    (link.other.clone(), item.item.clone())
                };
                held.insert(Held {
                    relation: link.link_type.clone(),
                    from,
                    to,
                });
            }
        }
        if let Some(parent) = &item.parent {
            let from = kind(&item.item);
            let to = kind(parent);
            if let Some(relation) = config.relations.iter().find(|one| {
                one.store == RelationStore::Parent
                    && from.as_deref() == Some(one.from.as_str())
                    && to.as_deref().is_none_or(|to| to == one.to)
            }) {
                held.insert(Held {
                    relation: relation.name.clone(),
                    from: item.item.clone(),
                    to: parent.clone(),
                });
            }
        }
    }
    for read in &state.reads {
        let Some(record) = decide::item_record(batch, read) else {
            continue;
        };
        for edge in record.edges.iter().flatten() {
            if config.relations.iter().any(|one| one.name == edge.relation) {
                held.insert(Held {
                    relation: edge.relation.clone(),
                    from: read.item.id.clone(),
                    to: edge.to.clone(),
                });
            }
        }
    }
    held.into_iter().collect()
}

/// Each link prepare's relations read found one end of to have lost.
#[must_use]
pub fn missing_links(state: &BatchState) -> Vec<MissingLink> {
    state
        .relations
        .iter()
        .flat_map(|item| {
            item.links
                .iter()
                .filter(|link| link.other_end == "missing")
                .map(|link| MissingLink {
                    item: item.item.clone(),
                    relation: link.link_type.clone(),
                    other: link.other.clone(),
                })
        })
        .collect()
}
