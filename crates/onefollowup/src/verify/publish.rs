//! `publish`: send the outbox the latest passing `check` held, exactly as it held it, in a
//! handful of batched store calls — one create-only copy of the batch's new tickets and one
//! version-held copy of its bound ones per destination, one `update-many` of its
//! re-estimates and one `comment add-many` of its evidence comments — and receipt every
//! outcome.
//!
//! Under a configuration with relations, a ticket whose edges point to another new ticket is
//! copied after it, so the record it lands with names that ticket's item in its `edges`; the
//! batch's edges are then written in one `task parent set-many` and one `task link add-many`,
//! each edge receipted. An existing item the batch relates, or whose estimate its related
//! tickets now move, is re-estimated in the same `update-many`, which also carries each
//! existing item's assignee, held to what onefollowup last wrote there.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{BatchArgs, PublishOutcome, Published, SCHEMA_VERSION, StagedOp};
use crate::config::{Priority, RelationStore, Route, Settings, StatusRole};
use crate::error::Error;
use crate::formats::batch::{BatchState, PublishRecord};
use crate::formats::outbox::{Outbox, StagedWrite};
use crate::formats::receipts::{
    AssignAction, AssignOutcome, CommentOutcome, Condition, CopyAction, CopyOutcome, EdgeAction,
    Outcome, ReEstimateOutcome, ReEstimateWrite, Receipt, ReceiptError, ReceiptResult,
    ReceiptStatus, RelateOutcome, WriteKind,
};
use crate::formats::ticket::{Edge as RecordEdge, TicketRecord};
use crate::formats::{HeldPriority, Level};
use crate::store::{
    CommentPost, Copied, CopyBatch, EdgeWrite, HeldUpdate, NewTask, Store, TaskChange, WriteState,
};
use crate::verify::check::{self, named};
use crate::verify::command;
use crate::verify::context::Batch;
use crate::verify::decide::{self, Bound, Decision, priority_of};
use crate::verify::journal;
use crate::verify::relations::{self, End, Graph, Scope};
use crate::verify::routes::{self, Facts};
use crate::verify::ticket::{self, priority_word};

/// What a route's command may print to override its `set`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Override {
    #[serde(default)]
    status_role: Option<StatusRole>,
    #[serde(default)]
    priority: Option<Priority>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    assignee: Option<String>,
}

/// One ticket's copy, ready to send.
struct Planned {
    index: usize,
    decision: Decision,
    title: String,
    record: Value,
    digest: String,
    item: String,
    /// When it is copied: after every new ticket its edges point to.
    level: usize,
    /// Whether an earlier landed copy of the same content makes it write nothing.
    unchanged: bool,
}

/// What every part of one publish reads.
struct Publishing<'a, 'b> {
    batch: &'a Batch<'b>,
    state: &'a BatchState,
    outbox: &'a Outbox,
    latest: &'a BTreeMap<String, String>,
    bound: &'a Bound,
    graph: &'a Graph,
    store: &'a Store,
    /// Each ticket's card this publish's copies landed, by its root cause.
    cards: &'a BTreeMap<String, String>,
}

/// An existing item's assignee to write, held to what onefollowup last wrote there.
struct Assignment {
    index: usize,
    item: String,
    ticket: Option<String>,
    person: String,
    recorded: Option<String>,
    record: TicketRecord,
}

/// The receipts and outcomes a publish gathers, each in its write's place.
#[derive(Default)]
struct Answers {
    receipts: Vec<(usize, Receipt)>,
    outcomes: Vec<PublishOutcome>,
}

impl Answers {
    fn push(&mut self, index: usize, receipt: Receipt, published: PublishOutcome) {
        self.receipts.push((index, receipt));
        self.outcomes.push(published);
    }
}

/// Whether the store's configuration has the source `name` hold assignees.
fn holds_assignees(store: &Store, name: &str) -> bool {
    store.source_setting(name, "assignees") == Some(Value::Bool(true))
}

fn held(word: &str) -> HeldPriority {
    serde_json::from_value(Value::from(word)).unwrap_or(HeldPriority::None)
}

fn result_of(state: &WriteState) -> ReceiptResult {
    ReceiptResult {
        priority: state.priority.as_deref().map(held),
        status: state.status.as_ref().map(|status| ReceiptStatus {
            category: status.category.clone(),
            name: status.name.clone(),
        }),
        version: state.version.clone(),
    }
}

fn outcome(
    index: usize,
    op: StagedOp,
    item: Option<String>,
    word: &str,
    error: Option<String>,
) -> PublishOutcome {
    PublishOutcome {
        index,
        op,
        item,
        outcome: word.to_owned(),
        error,
    }
}

fn level_word(level: Level) -> &'static str {
    ticket::level_word(level)
}

/// Send the batch's checked outbox.
///
/// # Errors
///
/// A batch not prepared; an outbox the latest passing `check` did not hold exactly as it
/// stands, refused with nothing written; or a journal that could not record every answer,
/// after every write was sent.
#[allow(
    clippy::too_many_lines,
    reason = "one pass over the outbox: plan, persist, send in batched calls, receipt"
)]
pub fn run(settings: &Settings, args: BatchArgs) -> Result<Published, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let mut state = batch.state()?;
    let outbox = batch.outbox()?;
    let receipts_path = batch.work.receipts(&batch.batch);
    if state.checked.is_none() {
        return Err(Error::Refused(format!(
            "no `check` of batch `{}` has passed since it was last prepared or published, so nothing was written: run `onefollowup check --batch {}` until it passes",
            batch.batch, batch.batch
        )));
    }
    if outbox.writes.is_empty() {
        return Ok(Published {
            schema_version: SCHEMA_VERSION,
            batch: batch.batch,
            receipts: receipts_path,
            outcomes: Vec::new(),
            store_operations: Vec::new(),
        });
    }
    let current = check::digests(&batch, &outbox)?;
    let checked = state.checked.clone().unwrap_or_default();
    let unchecked: Vec<String> = outbox
        .writes
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            checked.get(*index) != current.get(*index) || checked.len() != current.len()
        })
        .map(|(index, write)| named(index, write))
        .collect();
    if !unchecked.is_empty() {
        return Err(Error::Refused(format!(
            "publish sends only what the latest passing `check` held, and {} was staged or changed since; nothing was written: run `onefollowup check --batch {}` until it passes",
            unchecked.join(", "),
            batch.batch
        )));
    }
    let journal = journal::read(&batch)?;
    let bound = journal::bound(&journal);
    let latest = journal::latest(&journal, state.read_at_receipt);
    let store = batch.store()?;
    let graph = Graph::of(&batch, &state, &outbox, &bound, Scope::Staged)?;
    let mut answers = Answers::default();
    let mut planned: Vec<Planned> = Vec::new();
    let drafts = batch.config.drafts_source.clone();

    // Every copy, decided from the ticket, its kind, its route and prepare's read.
    for (index, write) in outbox.writes.iter().enumerate() {
        let StagedWrite::Copy { ticket: key, .. } = write else {
            continue;
        };
        let slug = batch.ticket_slug(key).unwrap_or_default();
        let file = batch.ticket(&slug)?;
        let mut decision =
            match decide::decide(&batch, &state, &bound, &latest, &graph, &outbox, &file) {
                Ok(decision) => decision,
                Err(message) => {
                    answers.push(
                        index,
                        journal::refused_copy(&batch.batch, key, None, "invalid", &message),
                        outcome(index, StagedOp::Copy, None, "invalid", Some(message)),
                    );
                    continue;
                }
            };
        let ticket_id = decision.item.clone().unwrap_or_else(|| key.clone());
        if let Some((_, route)) = &decision.route
            && let Some(argv) = &route.command
        {
            let stdin = json!({
                "schema_version": 1,
                "ticket": ticket::record_value(&decision.record),
                "kind": decision.kind.name,
                "severity": decision.severity,
                "repository": decision.record.repository,
                "paths": decision.paths,
                "route": route.name,
            })
            .to_string();
            let ran = command::run(
                argv,
                &stdin,
                settings.working_directory(),
                settings.environment(),
            );
            let overridden: Result<Override, String> = match ran {
                Err(error) => Err(error),
                Ok(ran) if ran.exit != 0 => Err(format!(
                    "the route `{}` command exited {}: {}",
                    route.name,
                    ran.exit,
                    ran.stderr.trim()
                )),
                Ok(ran) if ran.stdout.trim().is_empty() => Ok(Override::default()),
                Ok(ran) => serde_json::from_str(&ran.stdout).map_err(|error| {
                    format!(
                        "the route `{}` command printed what is not an override of `status_role`, `priority`, `labels` and `assignee`: {error}",
                        route.name
                    )
                }),
            };
            let overridden = overridden.and_then(|given| {
                let unheld = [decision.destination.as_str(), drafts.as_str()]
                    .into_iter()
                    .find(|source| !holds_assignees(&store, source));
                match (&given.assignee, unheld) {
                    (Some(person), Some(source)) => Err(format!(
                        "the route `{}` command assigned `{person}`, and the source `{source}` does not set `config.assignees: true`, so it holds no assignee",
                        route.name
                    )),
                    _ => Ok(given),
                }
            });
            match overridden {
                Err(message) => {
                    answers.push(
                        index,
                        journal::refused_copy(
                            &batch.batch,
                            &ticket_id,
                            decision.card.clone(),
                            "route-command-failed",
                            &message,
                        ),
                        outcome(
                            index,
                            StagedOp::Copy,
                            decision.card.clone(),
                            "route-command-failed",
                            Some(message),
                        ),
                    );
                    continue;
                }
                Ok(given) => {
                    if decision.card.is_none()
                        && let Some(role) = given.status_role
                    {
                        decision.status_role = role;
                    }
                    if !decision.priority_kept
                        && let Some(priority) = given.priority
                    {
                        decision.priority = priority;
                        decision.record.board_estimate = match priority {
                            Priority::None => None,
                            other => Some(ticket::level_of(other)),
                        };
                    }
                    if let Some(labels) = given.labels {
                        decision.labels = labels;
                    }
                    if let Some(person) = given.assignee {
                        if decision.card.is_none() {
                            decision.record.board_assignee = Some(person.clone());
                        }
                        decision.assignee = Some(person);
                    }
                }
            }
        }
        if decision.withdrawal {
            match (&decision.card, &decision.read) {
                (None, _) => {
                    let message =
                        "the ticket was never copied, so there is no item to withdraw".to_owned();
                    answers.push(
                        index,
                        journal::refused_copy(
                            &batch.batch,
                            &ticket_id,
                            None,
                            "nothing-to-withdraw",
                            &message,
                        ),
                        outcome(
                            index,
                            StagedOp::Copy,
                            None,
                            "nothing-to-withdraw",
                            Some(message),
                        ),
                    );
                    continue;
                }
                (Some(card), Some(read)) if batch.protected(&read.item.status.category) => {
                    let message = format!(
                        "{card} is at `{}`, a protected status a person decided on, so no batch withdraws it",
                        read.item.status.name
                    );
                    let mut receipt = journal::refused_copy(
                        &batch.batch,
                        &ticket_id,
                        Some(card.clone()),
                        "protected",
                        &message,
                    );
                    if let Outcome::Copy(copy) = &mut receipt.outcome {
                        copy.held = Some(ReceiptResult {
                            priority: Some(held(&read.item.priority)),
                            status: Some(ReceiptStatus {
                                category: read.item.status.category.clone(),
                                name: read.item.status.name.clone(),
                            }),
                            version: read.item.version.clone(),
                        });
                    }
                    answers.push(
                        index,
                        receipt,
                        outcome(
                            index,
                            StagedOp::Copy,
                            Some(card.clone()),
                            "protected",
                            Some(message),
                        ),
                    );
                    continue;
                }
                _ => {}
            }
        }
        planned.push(Planned {
            index,
            title: file.title.clone(),
            record: Value::Null,
            digest: String::new(),
            item: ticket_id,
            decision,
            level: 0,
            unchanged: false,
        });
    }
    levels(&state, &bound, &mut planned);

    // Level by level: each ticket's record, with the items its edges point to; what this
    // publish sends recorded, and the outbox emptied, before anything is sent; each ticket
    // written to the drafts source under the batch's project; then the level's copies — per
    // destination, new tickets in one create-only copy, bound ones in one copy held to their
    // versions, withdrawals in one held copy that writes their status.
    let mut cards: BTreeMap<String, String> = BTreeMap::new();
    let mut digests = IndexMap::new();
    let mut project_written = false;
    let top = planned.iter().map(|plan| plan.level).max().unwrap_or(0);
    for level in 0..=top {
        for plan in planned.iter_mut().filter(|plan| plan.level == level) {
            record_edges(&batch, &state, &bound, &cards, plan);
            plan.record = ticket::record_value(&plan.decision.record);
            plan.digest = ticket::digest(&plan.title, &plan.decision.body, &plan.record);
            digests.insert(plan.index.to_string(), plan.digest.clone());
            // A withdrawal changes the item's status, which the digest does not hold, so it is
            // never an unchanged re-copy.
            if plan.decision.card.is_some()
                && !plan.decision.withdrawal
                && let Some(item) = &plan.decision.item
                && let Some((line, Some(landed))) = journal::last_landed(&journal, item)
                && landed == plan.digest
            {
                plan.unchanged = true;
                answers.push(
                    plan.index,
                    Receipt {
                        batch: batch.batch.clone(),
                        ticket: Some(item.clone()),
                        card: plan.decision.card.clone(),
                        outcome: Outcome::Copy(CopyOutcome {
                            action: CopyAction::SkippedUnchanged,
                            result: None,
                            status_condition: None,
                            priority_condition: None,
                            error: None,
                            held: None,
                            digest: Some(plan.digest.clone()),
                            unchanged_since: Some(line),
                        }),
                    },
                    outcome(
                        plan.index,
                        StagedOp::Copy,
                        plan.decision.card.clone(),
                        "skipped-unchanged",
                        None,
                    ),
                );
            }
        }
        if level == 0 {
            state.publishes.push(PublishRecord {
                journal_from: journal.receipts.len(),
                writes: outbox.writes.clone(),
                digests: digests.clone(),
                settled: false,
            });
            state.checked = None;
            batch.save_state(&state)?;
            batch.save_outbox(&Outbox {
                batch: batch.batch.clone(),
                writes: Vec::new(),
            })?;
        } else if let Some(publish) = state.publishes.last_mut() {
            publish.digests = digests.clone();
            batch.save_state(&state)?;
        }
        let sending: Vec<usize> = (0..planned.len())
            .filter(|at| planned[*at].level == level && !planned[*at].unchanged)
            .collect();
        if !project_written
            && sending
                .iter()
                .any(|at| !state.tickets.contains_key(&planned[*at].decision.slug))
        {
            store.ensure_project(&drafts, &state.project, &state.project, &BTreeMap::new())?;
            project_written = true;
        }
        for &at in &sending {
            let plan = &mut planned[at];
            let decision = &plan.decision;
            let metadata = BTreeMap::from([(batch.record_key(), plan.record.clone())]);
            let status = batch.category_of(decision.status_role);
            let priority = priority_word(decision.priority).to_owned();
            // Only a ticket not yet on its card carries an assignee: the copy that creates the
            // card writes it there.
            let assignee = decision
                .card
                .is_none()
                .then(|| decision.assignee.clone())
                .flatten();
            let written = match state.tickets.get(&decision.slug) {
                Some(item) => store.update_task(
                    item,
                    &TaskChange {
                        title: Some(plan.title.clone()),
                        body: Some(decision.body.clone()),
                        status: Some(status),
                        priority: Some(priority),
                        repositories: Some(vec![decision.record.repository.clone()]),
                        metadata,
                        assignee,
                    },
                )?,
                None => store.create_task(&NewTask {
                    source: drafts.clone(),
                    project: state.project.clone(),
                    title: plan.title.clone(),
                    body: decision.body.clone(),
                    status,
                    priority,
                    labels: decision.labels.clone(),
                    repositories: vec![decision.record.repository.clone()],
                    metadata,
                    assignee,
                })?,
            };
            state
                .tickets
                .insert(decision.slug.clone(), written.id.clone());
            plan.item = written.id;
        }
        batch.save_state(&state)?;

        let mut groups: BTreeMap<(String, u8), Vec<usize>> = BTreeMap::new();
        for &at in &sending {
            let plan = &planned[at];
            let group = match (&plan.decision.card, plan.decision.withdrawal) {
                (None, _) => 0,
                (Some(_), false) => 1,
                (Some(_), true) => 2,
            };
            groups
                .entry((plan.decision.destination.clone(), group))
                .or_default()
                .push(at);
        }
        for ((destination, group), members) in &groups {
            let plans: Vec<&Planned> = members.iter().map(|at| &planned[*at]).collect();
            let request = CopyBatch {
                destination: destination.clone(),
                items: plans.iter().map(|plan| plan.item.clone()).collect(),
                create: *group == 0,
                preserve_status: *group != 2,
                versions: plans
                    .iter()
                    .filter_map(|plan| {
                        plan.decision
                            .hold
                            .clone()
                            .map(|hold| (plan.item.clone(), hold))
                    })
                    .collect(),
            };
            match store.copy(&request) {
                Ok(copied) => {
                    for plan in plans {
                        let answer = copied.iter().find(|one| one.source == plan.item);
                        if let Some(answer) = answer
                            && answer.error.is_none()
                            && let Some(card) = &answer.item
                            && matches!(answer.action.as_str(), "created" | "updated" | "unchanged")
                        {
                            cards.insert(plan.decision.slug.clone(), card.clone());
                        }
                        let (receipt, published) = copy_receipt(&batch, plan, answer);
                        answers.push(plan.index, receipt, published);
                    }
                }
                Err(error) => {
                    for plan in plans {
                        let message = error.to_string();
                        answers.push(
                            plan.index,
                            journal::refused_copy(
                                &batch.batch,
                                &plan.item,
                                plan.decision.card.clone(),
                                "store-refused",
                                &message,
                            ),
                            outcome(
                                plan.index,
                                StagedOp::Copy,
                                plan.decision.card.clone(),
                                "store-refused",
                                Some(message),
                            ),
                        );
                    }
                }
            }
        }
    }

    let publishing = Publishing {
        batch: &batch,
        state: &state,
        outbox: &outbox,
        latest: &latest,
        bound: &bound,
        graph: &graph,
        store: &store,
        cards: &cards,
    };
    // A bound ticket re-copied keeps its card's assignee; one its route now names another
    // person for is assigned in the `update-many`, held to what onefollowup last wrote.
    let assignments: Vec<Assignment> = planned
        .iter()
        .filter(|plan| !plan.unchanged)
        .filter_map(|plan| {
            let card = plan.decision.card.clone()?;
            let person = plan.decision.assignee.clone()?;
            let recorded = plan.decision.record.board_assignee.clone();
            (recorded.as_deref() != Some(person.as_str())).then(|| Assignment {
                index: plan.index,
                item: card,
                ticket: Some(plan.item.clone()),
                person,
                recorded,
                record: plan.decision.record.clone(),
            })
        })
        .collect();
    updates(&publishing, assignments, &mut answers);
    comments(&batch, &outbox, &store, &mut answers);
    edges(&publishing, &mut answers);

    let Answers {
        mut receipts,
        mut outcomes,
    } = answers;
    outcomes.sort_by_key(|one| one.index);
    receipts.sort_by_key(|(index, _)| *index);
    let receipts: Vec<Receipt> = receipts.into_iter().map(|(_, receipt)| receipt).collect();
    let sent = outcomes.len();
    if let Err(error) = journal::append(&batch, &receipts) {
        return Err(Error::Failed(format!(
            "publish sent the batch's {sent} writes and could not record their {} receipts: {error}; next: make the journal writable again and run `onefollowup settle --batch {}`, which reads back each write whose answer was not recorded",
            receipts.len(),
            batch.batch
        )));
    }
    Ok(Published {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        receipts: receipts_path,
        outcomes,
        store_operations: store.operations(),
    })
}

/// Give each planned copy its level: one past the deepest new ticket of this publish its
/// edges point to, so that ticket's card exists when its own record is written. A cycle is
/// cut at the number of tickets.
fn levels(state: &BatchState, bound: &Bound, planned: &mut [Planned]) {
    let new: BTreeMap<String, usize> = planned
        .iter()
        .enumerate()
        .filter(|(_, plan)| plan.decision.card.is_none())
        .map(|(at, plan)| (plan.decision.slug.clone(), at))
        .collect();
    let cap = planned.len();
    for _ in 0..cap {
        let mut moved = false;
        for at in 0..planned.len() {
            let level = planned[at]
                .decision
                .edges
                .iter()
                .filter_map(|(_, to)| match to {
                    End::Ticket(slug) if relations::card(state, bound, slug).is_none() => {
                        new.get(slug).copied()
                    }
                    _ => None,
                })
                .filter(|other| *other != at)
                .map(|other| (planned[other].level + 1).min(cap))
                .max()
                .unwrap_or(0);
            if level != planned[at].level {
                planned[at].level = level;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

/// The item an end stands for now: an item, or a ticket's card — landed by this publish, or
/// by an earlier one.
fn item_of(
    state: &BatchState,
    bound: &Bound,
    cards: &BTreeMap<String, String>,
    end: &End,
) -> Option<String> {
    match end {
        End::Item(item) => Some(item.clone()),
        End::Ticket(slug) => cards
            .get(slug)
            .cloned()
            .or_else(|| relations::card(state, bound, slug)),
    }
}

/// Record in a ticket's record each edge from it whose `to` has an item, beside the edges its
/// card already records.
fn record_edges(
    batch: &Batch<'_>,
    state: &BatchState,
    bound: &Bound,
    cards: &BTreeMap<String, String>,
    plan: &mut Planned,
) {
    let mut edges: Vec<RecordEdge> = plan
        .decision
        .read
        .as_ref()
        .and_then(|read| decide::item_record(batch, read))
        .and_then(|record| record.edges)
        .unwrap_or_default();
    for (relation, to) in &plan.decision.edges {
        if let Some(to) = item_of(state, bound, cards, to) {
            let edge = RecordEdge {
                relation: relation.clone(),
                to,
            };
            if !edges.contains(&edge) {
                edges.push(edge);
            }
        }
    }
    plan.decision.record.edges = (!edges.is_empty()).then_some(edges);
}

/// The first route an existing item matches, as its record and content state its facts.
fn item_route<'a>(
    publishing: &Publishing<'_, 'a>,
    item: &str,
    record: &TicketRecord,
    content: &str,
) -> Option<&'a Route> {
    let batch = publishing.batch;
    let kind = record
        .kind
        .clone()
        .unwrap_or_else(|| batch.config.default_kind.clone());
    let severity = ticket::severity(content, &batch.config.rubric);
    let counts = publishing.graph.counts(
        batch,
        publishing.state,
        publishing.bound,
        &End::Item(item.to_owned()),
    );
    routes::first(
        &batch.config.routes,
        &Facts {
            kind: &kind,
            severity: severity.as_deref(),
            repository: &record.repository,
            paths: &[],
            relations: &counts,
        },
    )
    .map(|(_, route)| route)
}

fn copy_receipt(
    batch: &Batch<'_>,
    plan: &Planned,
    answer: Option<&Copied>,
) -> (Receipt, PublishOutcome) {
    let decision = &plan.decision;
    let priority_condition = decision.read.as_ref().map(|read| Condition {
        held: Some(read.item.priority.clone()),
        met: !decision.priority_kept,
    });
    let Some(answer) = answer else {
        let message = "the copy answered nothing for this ticket".to_owned();
        return (
            journal::refused_copy(
                &batch.batch,
                &plan.item,
                decision.card.clone(),
                "unanswered",
                &message,
            ),
            outcome(
                plan.index,
                StagedOp::Copy,
                decision.card.clone(),
                "unanswered",
                Some(message),
            ),
        );
    };
    let action = match answer.action.as_str() {
        "created" => CopyAction::Created,
        "updated" => CopyAction::Updated,
        "unchanged" => CopyAction::Unchanged,
        "skipped" => CopyAction::Skipped,
        "orphaned" => CopyAction::Orphaned,
        _ => CopyAction::Failed,
    };
    let card = answer.item.clone().or_else(|| decision.card.clone());
    let error = answer.error.as_ref().map(|(kind, message)| ReceiptError {
        kind: kind.clone(),
        message: message.clone(),
    });
    let word = error
        .as_ref()
        .map_or_else(|| answer.action.clone(), |error| error.kind.clone());
    let receipt = Receipt {
        batch: batch.batch.clone(),
        ticket: Some(plan.item.clone()),
        card: card.clone(),
        outcome: Outcome::Copy(CopyOutcome {
            action,
            result: answer.result.as_ref().map(result_of),
            status_condition: None,
            priority_condition,
            held: answer.held.as_ref().map(result_of),
            digest: Some(plan.digest.clone()),
            unchanged_since: None,
            error,
        }),
    };
    let message = answer.error.as_ref().map(|(_, message)| message.clone());
    (
        receipt,
        outcome(plan.index, StagedOp::Copy, card, &word, message),
    )
}

/// The existing items the batch's edges reach without a staged re-estimate: each `from`
/// item to record a new edge in, and each `to` item a rollup feeds, with the edges to record.
fn implied(publishing: &Publishing<'_, '_>) -> IndexMap<String, Vec<RecordEdge>> {
    let mut implied: IndexMap<String, Vec<RecordEdge>> = IndexMap::new();
    let Publishing {
        batch,
        state,
        bound,
        cards,
        graph,
        ..
    } = publishing;
    let planned_card = |end: &End| match end {
        End::Ticket(slug) => {
            let key = batch.ticket_key(slug);
            let staged =
                publishing.outbox.writes.iter().any(
                    |write| matches!(write, StagedWrite::Copy { ticket, .. } if *ticket == key),
                );
            (!staged)
                .then(|| relations::card(state, bound, slug))
                .flatten()
        }
        End::Item(item) => Some(item.clone()),
    };
    for edge in &graph.edges {
        let Some(relation) = graph.relation(batch, &edge.relation) else {
            continue;
        };
        if let Some(from) = planned_card(&edge.from)
            && let Some(to) = item_of(state, bound, cards, &edge.to)
        {
            let recorded = decide::read_of(state, &from)
                .and_then(|read| decide::item_record(batch, read))
                .and_then(|record| record.edges)
                .unwrap_or_default();
            let wanted = RecordEdge {
                relation: relation.name.clone(),
                to,
            };
            if !recorded.contains(&wanted) {
                let edges = implied.entry(from).or_default();
                if !edges.contains(&wanted) {
                    edges.push(wanted);
                }
            }
        }
        if relation.rollup != crate::config::Rollup::None
            && let Some(to) = planned_card(&edge.to)
        {
            implied.entry(to).or_default();
        }
    }
    implied
}

/// One write of an item's `update-many`, and what its answer is receipted as.
enum Entry {
    ReEstimate(usize, HeldUpdate, ReEstimateOutcome),
    Assign(usize, HeldUpdate, Box<Assignment>),
}

/// One `update-many` per source: each staged re-estimate, each item the batch's edges reach,
/// and each existing item's assignee, after them and held to what onefollowup last wrote.
#[allow(
    clippy::too_many_lines,
    reason = "one pass: decide each item's update, send per source, receipt each answer"
)]
fn updates(publishing: &Publishing<'_, '_>, assignments: Vec<Assignment>, answers: &mut Answers) {
    let Publishing {
        batch,
        state,
        outbox,
        latest,
        bound,
        graph,
        store,
        ..
    } = publishing;
    let mut targets: Vec<(usize, String, Vec<RecordEdge>, bool)> = outbox
        .writes
        .iter()
        .enumerate()
        .filter_map(|(index, write)| match write {
            StagedWrite::ReEstimate { item, .. } => Some((index, item.clone(), Vec::new(), true)),
            _ => None,
        })
        .collect();
    let mut next = outbox.writes.len();
    for (item, edges) in implied(publishing) {
        match targets.iter_mut().find(|(_, one, _, _)| *one == item) {
            Some(target) => target.2 = edges,
            None => {
                targets.push((next, item, edges, false));
                next += 1;
            }
        }
    }
    let mut by_source: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
    let source_of = |item: &str| {
        item.split_once(':')
            .map(|(source, _)| source.to_owned())
            .unwrap_or_default()
    };
    for (index, item, edges, staged) in targets {
        let (estimated, count, mut record, read) =
            match decide::re_estimate(batch, state, outbox, bound, graph, &item) {
                Ok(found) => found,
                Err(message) => {
                    answers.push(
                        index,
                        re_estimate_refused(batch, &item, None, "invalid", &message),
                        outcome(
                            index,
                            StagedOp::ReEstimate,
                            Some(item.clone()),
                            "invalid",
                            Some(message),
                        ),
                    );
                    continue;
                }
            };
        if !staged && edges.is_empty() && estimated.level == record.priority_estimate {
            continue;
        }
        if !edges.is_empty() {
            let mut held_edges = record.edges.take().unwrap_or_default();
            held_edges.extend(edges);
            record.edges = Some(held_edges);
        }
        let held_now = priority_of(&read.item.priority);
        let stored = record
            .board_estimate
            .map_or(Priority::None, |level| match level {
                Level::Urgent => Priority::Urgent,
                Level::High => Priority::High,
                Level::Medium => Priority::Medium,
                Level::Low => Priority::Low,
            });
        let following = held_now == stored;
        record.priority_estimate = estimated.level;
        if following {
            record.board_estimate = Some(estimated.level);
        }
        let body = estimated.line.as_ref().map_or_else(
            || read.item.content.clone(),
            |line| ticket::with_estimate_line(&read.item.content, line, &batch.config.rubric),
        );
        let update = HeldUpdate {
            item: item.clone(),
            if_version: latest
                .get(&item)
                .cloned()
                .or_else(|| read.item.version.clone()),
            body: Some(body),
            priority: following.then(|| level_word(estimated.level).to_owned()),
            when_priority: if following {
                vec![read.item.priority.clone()]
            } else {
                Vec::new()
            },
            metadata: BTreeMap::from([(batch.record_key(), ticket::record_value(&record))]),
            assignee: None,
            when_assignees: None,
        };
        let summary = ReEstimateOutcome {
            estimate_line: estimated.line.clone().unwrap_or_default(),
            frequency: record.frequency,
            occurrences: count,
            priority: if following {
                held(level_word(estimated.level))
            } else {
                held(&read.item.priority)
            },
            priority_estimate: estimated.level,
            root_cause: Some(record.root_cause.clone()),
            severity_with_workaround: ticket::with_workaround(
                &read.item.content,
                &batch.config.rubric,
            )
            .unwrap_or_default(),
            title: Some(read.item.title.clone()),
            writes: Vec::new(),
            error: None,
        };
        let person = item_route(publishing, &item, &record, &read.item.content)
            .and_then(|route| route.set.assignee.clone());
        let assignment = person
            .filter(|person| record.board_assignee.as_deref() != Some(person.as_str()))
            .map(|person| Assignment {
                index,
                item: item.clone(),
                ticket: None,
                person,
                recorded: record.board_assignee.clone(),
                record: record.clone(),
            });
        let entries = by_source.entry(source_of(&item)).or_default();
        entries.push(Entry::ReEstimate(index, update, summary));
        if let Some(assignment) = assignment {
            entries.push(assign_entry(batch, assignment));
        }
    }
    for assignment in assignments {
        by_source
            .entry(source_of(&assignment.item))
            .or_default()
            .push(assign_entry(batch, assignment));
    }
    for (source, mut entries) in by_source {
        // Each assignee is written after the item's other writes, which hold to its version.
        entries.sort_by_key(|entry| matches!(entry, Entry::Assign(..)));
        let updates: Vec<HeldUpdate> = entries
            .iter()
            .map(|entry| match entry {
                Entry::ReEstimate(_, update, _) | Entry::Assign(_, update, _) => update.clone(),
            })
            .collect();
        let answered = store.update_many(&source, &updates);
        for (at, entry) in entries.into_iter().enumerate() {
            let answer = answered.as_ref().ok().and_then(|answers| answers.get(at));
            let (word, error) = match (&answered, answer) {
                (Err(error), _) => ("store-refused".to_owned(), Some(error.to_string())),
                (Ok(_), None) => (
                    "unanswered".to_owned(),
                    Some("the update answered nothing".to_owned()),
                ),
                (Ok(_), Some(answer)) => (
                    answer.outcome.clone(),
                    match answer.outcome.as_str() {
                        "written" => None,
                        "version-moved" => Some(format!(
                            "{} moved to version {} since it was read, so nothing was written over the change",
                            answer.item,
                            answer.current_version.clone().unwrap_or_default()
                        )),
                        "held" => Some(match &entry {
                            Entry::Assign(..) => format!(
                                "{} is no longer assigned to exactly whom onefollowup last assigned, so a person's assignment stands and nothing was written",
                                answer.item
                            ),
                            Entry::ReEstimate(..) => format!(
                                "{} no longer holds the priority it was read at, so nothing was written",
                                answer.item
                            ),
                        }),
                        _ => Some(answer.reason.clone().unwrap_or_default()),
                    },
                ),
            };
            match entry {
                Entry::ReEstimate(index, update, mut summary) => {
                    match &error {
                        None => {
                            let version = answer.and_then(|answer| answer.version.clone());
                            let result = ReceiptResult {
                                priority: Some(summary.priority),
                                status: None,
                                version,
                            };
                            let mut parts = Vec::new();
                            if update.priority.is_some() {
                                parts.push(WriteKind::Priority);
                            }
                            parts.push(WriteKind::Content);
                            parts.push(WriteKind::Metadata);
                            summary.writes = parts
                                .into_iter()
                                .map(|write| ReEstimateWrite {
                                    result: Some(result.clone()),
                                    write,
                                })
                                .collect();
                        }
                        Some(message) => {
                            summary.error = Some(ReceiptError {
                                kind: word.clone(),
                                message: message.clone(),
                            });
                        }
                    }
                    answers.push(
                        index,
                        Receipt {
                            batch: batch.batch.clone(),
                            ticket: None,
                            card: Some(update.item.clone()),
                            outcome: Outcome::ReEstimate(summary),
                        },
                        outcome(
                            index,
                            StagedOp::ReEstimate,
                            Some(update.item.clone()),
                            &word,
                            error,
                        ),
                    );
                }
                Entry::Assign(index, _, assignment) => {
                    let (action, word) = match word.as_str() {
                        "written" => (AssignAction::Written, "written".to_owned()),
                        "held" => (AssignAction::Held, "assignee-held".to_owned()),
                        _ => (AssignAction::Failed, word),
                    };
                    answers.push(
                        index,
                        Receipt {
                            batch: batch.batch.clone(),
                            ticket: assignment.ticket.clone(),
                            card: Some(assignment.item.clone()),
                            outcome: Outcome::Assign(AssignOutcome {
                                assignee: assignment.person.clone(),
                                when: assignment.recorded.clone().into_iter().collect(),
                                action,
                                error: error.as_ref().map(|message| ReceiptError {
                                    kind: word.clone(),
                                    message: message.clone(),
                                }),
                            }),
                        },
                        outcome(
                            index,
                            StagedOp::Assign,
                            Some(assignment.item.clone()),
                            &word,
                            error,
                        ),
                    );
                }
            }
        }
    }
}

/// An existing item's assignee as its `update-many` writes it: the one person, held to the
/// people onefollowup last wrote there — none, when it never assigned it — and its record
/// naming that person once it lands.
fn assign_entry(batch: &Batch<'_>, assignment: Assignment) -> Entry {
    let mut record = assignment.record.clone();
    record.board_assignee = Some(assignment.person.clone());
    Entry::Assign(
        assignment.index,
        HeldUpdate {
            item: assignment.item.clone(),
            if_version: None,
            body: None,
            priority: None,
            when_priority: Vec::new(),
            metadata: BTreeMap::from([(batch.record_key(), ticket::record_value(&record))]),
            assignee: Some(assignment.person.clone()),
            when_assignees: Some(assignment.recorded.clone().into_iter().collect()),
        },
        Box::new(assignment),
    )
}

/// The batch's edges, derived and asserted, each written once: every parent in one
/// `task parent set-many`, every link in one `task link add-many`, and each receipted — one
/// whose end has no item, its ticket's copy not having landed, receipted without a write.
#[allow(
    clippy::too_many_lines,
    reason = "one pass: resolve each edge, send per kind of store, receipt each answer"
)]
fn edges(publishing: &Publishing<'_, '_>, answers: &mut Answers) {
    let Publishing {
        batch,
        state,
        outbox,
        bound,
        graph,
        store,
        cards,
        ..
    } = publishing;
    struct Sending {
        index: usize,
        relation: String,
        store: RelationStore,
        from: String,
        to: String,
        derived: bool,
        write: Option<EdgeWrite>,
    }
    let mut sending: Vec<Sending> = Vec::new();
    let mut seen: Vec<(String, String, String)> = Vec::new();
    for edge in &graph.edges {
        let Some(relation) = graph.relation(batch, &edge.relation) else {
            continue;
        };
        let index = edge.asserted.unwrap_or_else(|| match &edge.from {
            End::Ticket(slug) => {
                let key = batch.ticket_key(slug);
                outbox
                    .writes
                    .iter()
                    .position(
                        |write| matches!(write, StagedWrite::Copy { ticket, .. } if *ticket == key),
                    )
                    .unwrap_or(outbox.writes.len())
            }
            End::Item(_) => outbox.writes.len(),
        });
        let from = item_of(state, bound, cards, &edge.from);
        let to = item_of(state, bound, cards, &edge.to);
        let write = match (from, to) {
            (Some(from), Some(to)) => {
                let identity = (relation.name.clone(), from.clone(), to.clone());
                if seen.contains(&identity) {
                    continue;
                }
                seen.push(identity);
                Some(EdgeWrite {
                    from,
                    to,
                    link_type: relation.name.clone(),
                })
            }
            _ => None,
        };
        sending.push(Sending {
            index,
            relation: relation.name.clone(),
            store: relation.store,
            from: relations::spelled(batch, &edge.from),
            to: relations::spelled(batch, &edge.to),
            derived: edge.asserted.is_none(),
            write,
        });
    }
    let parents: Vec<EdgeWrite> = sending
        .iter()
        .filter(|one| one.store == RelationStore::Parent)
        .filter_map(|one| one.write.clone())
        .collect();
    let links: Vec<EdgeWrite> = sending
        .iter()
        .filter(|one| one.store == RelationStore::Link)
        .filter_map(|one| one.write.clone())
        .collect();
    let parents_answered = (!parents.is_empty()).then(|| store.set_parents(&parents));
    let links_answered = (!links.is_empty()).then(|| store.add_links(&links));
    let (mut parent_at, mut link_at) = (0, 0);
    for one in sending {
        let (card, to_card, action, error) = match &one.write {
            None => (
                None,
                None,
                EdgeAction::Failed,
                Some(ReceiptError {
                    kind: "endpoint-not-written".to_owned(),
                    message: format!(
                        "the edge {} -> {} has an end whose ticket's copy did not land, so there is no item to join; it is written when that copy lands",
                        one.from, one.to
                    ),
                }),
            ),
            Some(write) => {
                let (answered, at) = match one.store {
                    RelationStore::Parent => (&parents_answered, &mut parent_at),
                    RelationStore::Link => (&links_answered, &mut link_at),
                };
                let answer = match answered {
                    Some(Ok(answers)) => Ok(answers.get(*at).cloned()),
                    Some(Err(error)) => Err(error.to_string()),
                    None => Ok(None),
                };
                *at += 1;
                let (action, error) = match answer {
                    Err(message) => (
                        EdgeAction::Failed,
                        Some(ReceiptError {
                            kind: "store-refused".to_owned(),
                            message,
                        }),
                    ),
                    Ok(None) => (
                        EdgeAction::Failed,
                        Some(ReceiptError {
                            kind: "unanswered".to_owned(),
                            message: "the edge batch answered nothing for this edge".to_owned(),
                        }),
                    ),
                    Ok(Some(answer)) => match answer.outcome.as_str() {
                        "written" => (EdgeAction::Written, None),
                        "exists" | "unchanged" => (EdgeAction::Exists, None),
                        other => (
                            EdgeAction::Failed,
                            Some(ReceiptError {
                                kind: other.to_owned(),
                                message: answer.reason.clone().unwrap_or_else(|| other.to_owned()),
                            }),
                        ),
                    },
                };
                (
                    Some(write.from.clone()),
                    Some(write.to.clone()),
                    action,
                    error,
                )
            }
        };
        let word = match (&action, &error) {
            (_, Some(error)) => error.kind.clone(),
            (EdgeAction::Exists, None) => "exists".to_owned(),
            _ => "written".to_owned(),
        };
        let message = error.as_ref().map(|error| error.message.clone());
        answers.push(
            one.index,
            Receipt {
                batch: batch.batch.clone(),
                ticket: None,
                card: card.clone(),
                outcome: Outcome::Relate(RelateOutcome {
                    relation: one.relation,
                    store: match one.store {
                        RelationStore::Parent => "parent",
                        RelationStore::Link => "link",
                    }
                    .to_owned(),
                    from: one.from,
                    to: one.to,
                    to_card,
                    derived: one.derived,
                    action,
                    error,
                }),
            },
            outcome(one.index, StagedOp::Relate, card, &word, message),
        );
    }
}

fn re_estimate_refused(
    batch: &Batch<'_>,
    item: &str,
    title: Option<String>,
    kind: &str,
    message: &str,
) -> Receipt {
    Receipt {
        batch: batch.batch.clone(),
        ticket: None,
        card: Some(item.to_owned()),
        outcome: Outcome::ReEstimate(ReEstimateOutcome {
            estimate_line: String::new(),
            frequency: None,
            occurrences: 0,
            priority: HeldPriority::None,
            priority_estimate: Level::Low,
            root_cause: None,
            severity_with_workaround: String::new(),
            title,
            writes: Vec::new(),
            error: Some(ReceiptError {
                kind: kind.to_owned(),
                message: message.to_owned(),
            }),
        }),
    }
}

fn comments(batch: &Batch<'_>, outbox: &Outbox, store: &Store, answers: &mut Answers) {
    let mut by_source: BTreeMap<String, Vec<(usize, CommentPost, String)>> = BTreeMap::new();
    for (index, write) in outbox.writes.iter().enumerate() {
        let StagedWrite::Comment {
            item,
            root_cause,
            body,
        } = write
        else {
            continue;
        };
        let source = item
            .split_once(':')
            .map(|(source, _)| source.to_owned())
            .unwrap_or_default();
        by_source.entry(source).or_default().push((
            index,
            CommentPost {
                item: item.clone(),
                body: body.clone(),
            },
            root_cause.clone(),
        ));
    }
    for (source, entries) in by_source {
        let posts: Vec<CommentPost> = entries.iter().map(|(_, post, _)| post.clone()).collect();
        let answered = store.add_comments(&source, &posts);
        for (at, (index, post, root_cause)) in entries.into_iter().enumerate() {
            let answer = answered.as_ref().ok().and_then(|answers| answers.get(at));
            let written = answer.and_then(|answer| answer.comment.as_ref());
            let (word, error) = match (&answered, answer) {
                (Err(error), _) => ("store-refused".to_owned(), Some(error.to_string())),
                (Ok(_), None) => (
                    "unanswered".to_owned(),
                    Some("the comment batch answered nothing".to_owned()),
                ),
                (Ok(_), Some(answer)) if answer.outcome == "written" && written.is_some() => {
                    ("written".to_owned(), None)
                }
                (Ok(_), Some(answer)) => (
                    answer.outcome.clone(),
                    Some(
                        answer
                            .reason
                            .clone()
                            .unwrap_or_else(|| answer.outcome.clone()),
                    ),
                ),
            };
            let receipt = Receipt {
                batch: batch.batch.clone(),
                ticket: None,
                card: Some(post.item.clone()),
                outcome: Outcome::Comment(CommentOutcome {
                    id: written.map(|comment| comment.id.clone()),
                    item: Some(post.item.clone()),
                    root_cause: Some(root_cause),
                    url: written.map(|comment| comment.url.clone()),
                    error: error.as_ref().map(|message| ReceiptError {
                        kind: word.clone(),
                        message: message.clone(),
                    }),
                }),
            };
            answers.push(
                index,
                receipt,
                outcome(index, StagedOp::Comment, Some(post.item), &word, error),
            );
        }
    }
}
