//! `publish`: send the outbox the latest passing `check` held, exactly as it held it, in a
//! handful of batched store calls — one create-only copy of the batch's new tickets and one
//! version-held copy of its bound ones per destination, one `update-many` of its
//! re-estimates and one `comment add-many` of its evidence comments — and receipt every
//! outcome.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{BatchArgs, PublishOutcome, Published, SCHEMA_VERSION, StagedOp};
use crate::config::{Priority, Settings, StatusRole};
use crate::error::Error;
use crate::formats::batch::{BatchState, PublishRecord};
use crate::formats::outbox::{Outbox, StagedWrite};
use crate::formats::receipts::{
    CommentOutcome, Condition, CopyAction, CopyOutcome, Outcome, ReEstimateOutcome,
    ReEstimateWrite, Receipt, ReceiptError, ReceiptResult, ReceiptStatus, WriteKind,
};
use crate::formats::{HeldPriority, Level};
use crate::store::{
    CommentPost, Copied, CopyBatch, HeldUpdate, NewTask, Store, TaskChange, WriteState,
};
use crate::verify::check::{self, named};
use crate::verify::command;
use crate::verify::context::Batch;
use crate::verify::decide::{self, Decision, priority_of};
use crate::verify::journal;
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
    let mut receipts: Vec<(usize, Receipt)> = Vec::new();
    let mut outcomes: Vec<PublishOutcome> = Vec::new();
    let mut planned: Vec<Planned> = Vec::new();
    let mut digests = IndexMap::new();
    let drafts = batch.config.drafts_source.clone();

    // Every copy, decided from the ticket, its kind, its route and prepare's read.
    for (index, write) in outbox.writes.iter().enumerate() {
        let StagedWrite::Copy { ticket: key, .. } = write else {
            continue;
        };
        let slug = batch.ticket_slug(key).unwrap_or_default();
        let file = batch.ticket(&slug)?;
        let mut decision = match decide::decide(&batch, &state, &bound, &latest, &file) {
            Ok(decision) => decision,
            Err(message) => {
                receipts.push((
                    index,
                    journal::refused_copy(&batch.batch, key, None, "invalid", &message),
                ));
                outcomes.push(outcome(
                    index,
                    StagedOp::Copy,
                    None,
                    "invalid",
                    Some(message),
                ));
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
            match overridden {
                Err(message) => {
                    receipts.push((
                        index,
                        journal::refused_copy(
                            &batch.batch,
                            &ticket_id,
                            decision.card.clone(),
                            "route-command-failed",
                            &message,
                        ),
                    ));
                    outcomes.push(outcome(
                        index,
                        StagedOp::Copy,
                        decision.card.clone(),
                        "route-command-failed",
                        Some(message),
                    ));
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
                    let _assignee = given.assignee;
                }
            }
        }
        if decision.withdrawal {
            match (&decision.card, &decision.read) {
                (None, _) => {
                    let message =
                        "the ticket was never copied, so there is no item to withdraw".to_owned();
                    receipts.push((
                        index,
                        journal::refused_copy(
                            &batch.batch,
                            &ticket_id,
                            None,
                            "nothing-to-withdraw",
                            &message,
                        ),
                    ));
                    outcomes.push(outcome(
                        index,
                        StagedOp::Copy,
                        None,
                        "nothing-to-withdraw",
                        Some(message),
                    ));
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
                    receipts.push((index, receipt));
                    outcomes.push(outcome(
                        index,
                        StagedOp::Copy,
                        Some(card.clone()),
                        "protected",
                        Some(message),
                    ));
                    continue;
                }
                _ => {}
            }
        }
        let record = ticket::record_value(&decision.record);
        let digest = ticket::digest(&file.title, &decision.body, &record);
        digests.insert(index.to_string(), digest.clone());
        // A withdrawal changes the item's status, which the digest does not hold, so it is
        // never an unchanged re-copy.
        if decision.card.is_some()
            && !decision.withdrawal
            && let Some(item) = &decision.item
            && let Some((line, Some(landed))) = journal::last_landed(&journal, item)
            && landed == digest
        {
            receipts.push((
                index,
                Receipt {
                    batch: batch.batch.clone(),
                    ticket: Some(item.clone()),
                    card: decision.card.clone(),
                    outcome: Outcome::Copy(CopyOutcome {
                        action: CopyAction::SkippedUnchanged,
                        result: None,
                        status_condition: None,
                        priority_condition: None,
                        error: None,
                        held: None,
                        digest: Some(digest),
                        unchanged_since: Some(line),
                    }),
                },
            ));
            outcomes.push(outcome(
                index,
                StagedOp::Copy,
                decision.card.clone(),
                "skipped-unchanged",
                None,
            ));
            continue;
        }
        planned.push(Planned {
            index,
            title: file.title.clone(),
            record,
            digest,
            item: ticket_id,
            decision,
        });
    }

    // What this publish sends is recorded, and the outbox emptied, before anything is sent.
    state.publishes.push(PublishRecord {
        journal_from: journal.receipts.len(),
        writes: outbox.writes.clone(),
        digests,
        settled: false,
    });
    state.checked = None;
    batch.save_state(&state)?;
    batch.save_outbox(&Outbox {
        batch: batch.batch.clone(),
        writes: Vec::new(),
    })?;

    // Each ticket is written to the drafts source as it is copied, under the batch's project.
    if planned
        .iter()
        .any(|plan| !state.tickets.contains_key(&plan.decision.slug))
    {
        store.ensure_project(&drafts, &state.project, &state.project, &BTreeMap::new())?;
    }
    for plan in &mut planned {
        let decision = &plan.decision;
        let metadata = BTreeMap::from([(batch.record_key(), plan.record.clone())]);
        let status = batch.category_of(decision.status_role);
        let priority = priority_word(decision.priority).to_owned();
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
            })?,
        };
        state
            .tickets
            .insert(decision.slug.clone(), written.id.clone());
        plan.item = written.id;
    }
    batch.save_state(&state)?;

    // The copies: per destination, new tickets in one create-only copy, bound ones in one
    // copy held to their versions, withdrawals in one held copy that writes their status.
    let mut groups: BTreeMap<(String, u8), Vec<&Planned>> = BTreeMap::new();
    for plan in &planned {
        let group = match (&plan.decision.card, plan.decision.withdrawal) {
            (None, _) => 0,
            (Some(_), false) => 1,
            (Some(_), true) => 2,
        };
        groups
            .entry((plan.decision.destination.clone(), group))
            .or_default()
            .push(plan);
    }
    for ((destination, group), plans) in &groups {
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
                    let (receipt, published) = copy_receipt(&batch, plan, answer);
                    receipts.push((plan.index, receipt));
                    outcomes.push(published);
                }
            }
            Err(error) => {
                for plan in plans {
                    let message = error.to_string();
                    receipts.push((
                        plan.index,
                        journal::refused_copy(
                            &batch.batch,
                            &plan.item,
                            plan.decision.card.clone(),
                            "store-refused",
                            &message,
                        ),
                    ));
                    outcomes.push(outcome(
                        plan.index,
                        StagedOp::Copy,
                        plan.decision.card.clone(),
                        "store-refused",
                        Some(message),
                    ));
                }
            }
        }
    }

    re_estimates(
        &batch,
        &state,
        &outbox,
        &latest,
        &store,
        &mut receipts,
        &mut outcomes,
    );
    comments(&batch, &outbox, &store, &mut receipts, &mut outcomes);

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

#[allow(
    clippy::too_many_arguments,
    reason = "the publish's shared reads and the two lists it appends to"
)]
fn re_estimates(
    batch: &Batch<'_>,
    state: &BatchState,
    outbox: &Outbox,
    latest: &BTreeMap<String, String>,
    store: &Store,
    receipts: &mut Vec<(usize, Receipt)>,
    outcomes: &mut Vec<PublishOutcome>,
) {
    let mut by_source: BTreeMap<String, Vec<(usize, HeldUpdate, ReEstimateOutcome)>> =
        BTreeMap::new();
    for (index, write) in outbox.writes.iter().enumerate() {
        let StagedWrite::ReEstimate { item, .. } = write else {
            continue;
        };
        let (estimated, count, mut record, read) =
            match decide::re_estimate(batch, state, outbox, item) {
                Ok(found) => found,
                Err(message) => {
                    receipts.push((
                        index,
                        re_estimate_refused(batch, item, None, "invalid", &message),
                    ));
                    outcomes.push(outcome(
                        index,
                        StagedOp::ReEstimate,
                        Some(item.clone()),
                        "invalid",
                        Some(message),
                    ));
                    continue;
                }
            };
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
                .get(item)
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
        let source = item
            .split_once(':')
            .map(|(source, _)| source.to_owned())
            .unwrap_or_default();
        by_source
            .entry(source)
            .or_default()
            .push((index, update, summary));
    }
    for (source, entries) in by_source {
        let updates: Vec<HeldUpdate> = entries
            .iter()
            .map(|(_, update, _)| update.clone())
            .collect();
        let answered = store.update_many(&source, &updates);
        for (at, (index, update, mut summary)) in entries.into_iter().enumerate() {
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
                            update.item,
                            answer.current_version.clone().unwrap_or_default()
                        )),
                        "held" => Some(format!(
                            "{} no longer holds the priority it was read at, so nothing was written",
                            update.item
                        )),
                        _ => Some(answer.reason.clone().unwrap_or_default()),
                    },
                ),
            };
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
            receipts.push((
                index,
                Receipt {
                    batch: batch.batch.clone(),
                    ticket: None,
                    card: Some(update.item.clone()),
                    outcome: Outcome::ReEstimate(summary),
                },
            ));
            outcomes.push(outcome(
                index,
                StagedOp::ReEstimate,
                Some(update.item.clone()),
                &word,
                error,
            ));
        }
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

fn comments(
    batch: &Batch<'_>,
    outbox: &Outbox,
    store: &Store,
    receipts: &mut Vec<(usize, Receipt)>,
    outcomes: &mut Vec<PublishOutcome>,
) {
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
            receipts.push((index, receipt));
            outcomes.push(outcome(
                index,
                StagedOp::Comment,
                Some(post.item),
                &word,
                error,
            ));
        }
    }
}
