//! `settle`: close out a published batch. A write whose answer named the item's resulting
//! version is confirmed by that answer; only the writes no answer confirmed are read back,
//! in one batched read; any that did not certainly land, refused ones included, are recorded
//! in `pending.json`; then the `batch_settled` hook runs and the working files are retired.

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::api::{BatchArgs, HookRun, SCHEMA_VERSION, Settled};
use crate::config::Settings;
use crate::error::Error;
use crate::formats::Versioned;
use crate::formats::markers::EvidenceComment;
use crate::formats::outbox::StagedWrite;
use crate::formats::pending::{Pending, PendingEntry, PendingState, PendingWrite};
use crate::formats::receipts::{Outcome, Receipt};
use crate::formats::ticket::TicketRecord;
use crate::store::Search;
use crate::verify::command;
use crate::verify::context::Batch;
use crate::verify::journal;
use crate::verify::ticket;
use crate::work;

/// One sent write, and what its receipt says of it.
struct Sent {
    write: StagedWrite,
    receipt: Option<Receipt>,
    digest: Option<String>,
    ticket_item: Option<String>,
}

enum Standing {
    /// Its answer confirmed it landed.
    Confirmed,
    /// It was refused or withheld: it did not land.
    Refused {
        kind: String,
        message: String,
        card: Option<String>,
    },
    /// Nothing confirmed it: it is read back.
    Unconfirmed,
    /// It wrote nothing on purpose (skipped as unchanged).
    Nothing,
}

fn standing(sent: &Sent) -> Standing {
    let Some(receipt) = &sent.receipt else {
        return Standing::Unconfirmed;
    };
    match &receipt.outcome {
        Outcome::Copy(copy) => match (&copy.error, copy.action) {
            (Some(error), _) => Standing::Refused {
                kind: error.kind.clone(),
                message: error.message.clone(),
                card: receipt.card.clone(),
            },
            (None, crate::formats::receipts::CopyAction::SkippedUnchanged) => Standing::Nothing,
            (None, _)
                if copy
                    .result
                    .as_ref()
                    .is_some_and(|result| result.version.is_some()) =>
            {
                Standing::Confirmed
            }
            (None, _) => Standing::Unconfirmed,
        },
        Outcome::Comment(comment) => match &comment.error {
            Some(error) => Standing::Refused {
                kind: error.kind.clone(),
                message: error.message.clone(),
                card: receipt.card.clone(),
            },
            None if comment.id.is_some() => Standing::Confirmed,
            None => Standing::Unconfirmed,
        },
        Outcome::ReEstimate(estimate) => match &estimate.error {
            Some(error) => Standing::Refused {
                kind: error.kind.clone(),
                message: error.message.clone(),
                card: receipt.card.clone(),
            },
            None if estimate.writes.iter().any(|write| {
                write
                    .result
                    .as_ref()
                    .is_some_and(|result| result.version.is_some())
            }) =>
            {
                Standing::Confirmed
            }
            None => Standing::Unconfirmed,
        },
    }
}

/// Whether `receipt` answers `write`.
fn answers(
    batch: &Batch<'_>,
    write: &StagedWrite,
    receipt: &Receipt,
    ticket_item: Option<&str>,
) -> bool {
    match (write, &receipt.outcome) {
        (StagedWrite::Copy { ticket, .. }, Outcome::Copy(_)) => {
            receipt.ticket.as_deref() == Some(ticket.as_str())
                || (ticket_item.is_some() && receipt.ticket.as_deref() == ticket_item)
                || batch
                    .ticket_slug(ticket)
                    .is_some_and(|_| receipt.ticket.as_deref() == Some(ticket.as_str()))
        }
        (
            StagedWrite::Comment {
                item, root_cause, ..
            },
            Outcome::Comment(comment),
        ) => {
            receipt.card.as_deref() == Some(item.as_str())
                && comment.root_cause.as_deref() == Some(root_cause.as_str())
        }
        (StagedWrite::ReEstimate { item, .. }, Outcome::ReEstimate(_)) => {
            receipt.card.as_deref() == Some(item.as_str())
        }
        _ => false,
    }
}

fn pending_kind(write: &StagedWrite, withdrawal: bool) -> PendingWrite {
    match write {
        StagedWrite::Copy { .. } if withdrawal => PendingWrite::Withdrawal,
        StagedWrite::Copy { .. } | StagedWrite::Relate { .. } => PendingWrite::TicketCopy,
        StagedWrite::Comment { .. } => PendingWrite::Comment,
        StagedWrite::ReEstimate { .. } => PendingWrite::Priority,
    }
}

fn intended(write: &StagedWrite) -> IndexMap<String, Value> {
    match write {
        StagedWrite::Copy {
            ticket,
            destination,
        } => IndexMap::from([
            ("ticket".to_owned(), Value::from(ticket.as_str())),
            ("destination".to_owned(), Value::from(destination.as_str())),
        ]),
        StagedWrite::Comment {
            root_cause, body, ..
        } => IndexMap::from([
            ("root_cause".to_owned(), Value::from(root_cause.as_str())),
            ("body".to_owned(), Value::from(body.as_str())),
        ]),
        StagedWrite::ReEstimate {
            priority_estimate, ..
        } => IndexMap::from([(
            "priority_estimate".to_owned(),
            serde_json::to_value(priority_estimate).unwrap_or(Value::Null),
        )]),
        StagedWrite::Relate { relation, from, to } => IndexMap::from([
            ("relation".to_owned(), Value::from(relation.as_str())),
            ("from".to_owned(), Value::from(from.as_str())),
            ("to".to_owned(), Value::from(to.as_str())),
        ]),
    }
}

fn write_item(write: &StagedWrite) -> Option<String> {
    match write {
        StagedWrite::Comment { item, .. } | StagedWrite::ReEstimate { item, .. } => {
            Some(item.clone())
        }
        StagedWrite::Copy { .. } | StagedWrite::Relate { .. } => None,
    }
}

/// Close out the batch.
///
/// # Errors
///
/// A batch not prepared, the store's refusal of the read-back, or a working file that cannot
/// be written.
#[allow(
    clippy::too_many_lines,
    reason = "one pass: match receipts, read back what nothing confirmed, record, hook, retire"
)]
pub fn run(settings: &Settings, args: BatchArgs) -> Result<Settled, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let mut state = batch.state()?;
    let journal = journal::read(&batch)?;
    let mut sent: Vec<Sent> = Vec::new();
    for publish in state.publishes.iter().filter(|publish| !publish.settled) {
        let after: Vec<&Receipt> = journal.receipts.iter().skip(publish.journal_from).collect();
        let mut used = vec![false; after.len()];
        for (index, write) in publish.writes.iter().enumerate() {
            let ticket_item = match write {
                StagedWrite::Copy { ticket, .. } => batch
                    .ticket_slug(ticket)
                    .and_then(|slug| state.tickets.get(&slug).cloned()),
                _ => None,
            };
            let found = after.iter().enumerate().position(|(at, receipt)| {
                !used[at] && answers(&batch, write, receipt, ticket_item.as_deref())
            });
            if let Some(at) = found {
                used[at] = true;
            }
            sent.push(Sent {
                write: write.clone(),
                receipt: found.map(|at| after[at].clone()),
                digest: publish.digests.get(&index.to_string()).cloned(),
                ticket_item,
            });
        }
    }

    // Read back, in one read, only what no answer confirmed.
    let store_needed = sent
        .iter()
        .any(|one| matches!(standing(one), Standing::Unconfirmed));
    let store = if store_needed {
        Some(batch.store()?)
    } else {
        None
    };
    let mut confirmed_by_read = vec![false; sent.len()];
    let mut read_back = 0;
    if let Some(store) = &store {
        let mut ids: Vec<String> = Vec::new();
        let mut origins: Vec<(usize, String, String)> = Vec::new();
        for (at, one) in sent.iter().enumerate() {
            if !matches!(standing(one), Standing::Unconfirmed) {
                continue;
            }
            read_back += 1;
            let card = one
                .receipt
                .as_ref()
                .and_then(|receipt| receipt.card.clone());
            match (&one.write, card.or_else(|| write_item(&one.write))) {
                (_, Some(item)) => {
                    if !ids.contains(&item) {
                        ids.push(item);
                    }
                }
                (StagedWrite::Copy { destination, .. }, None) => {
                    if let Some(item) = &one.ticket_item {
                        origins.push((at, destination.clone(), item.clone()));
                    }
                }
                _ => {}
            }
        }
        let read = if ids.is_empty() {
            Vec::new()
        } else {
            store.read(&ids, true)?
        };
        let mut found_cards: Vec<(usize, crate::store::StoreItem)> = Vec::new();
        let mut by_destination: IndexMap<String, Vec<(usize, String)>> = IndexMap::new();
        for (at, destination, item) in origins {
            by_destination
                .entry(destination)
                .or_default()
                .push((at, item));
        }
        for (destination, asked) in by_destination {
            let searches: Vec<Search> = asked
                .iter()
                .map(|(at, item)| Search {
                    key: format!("sent:{at}"),
                    origin: Some(item.clone()),
                    ..Search::default()
                })
                .collect();
            for found in store.find(&destination, &searches)? {
                let at: usize = found
                    .key
                    .trim_start_matches("sent:")
                    .parse()
                    .unwrap_or(usize::MAX);
                if let Some(item) = found.items.into_iter().next() {
                    found_cards.push((at, item));
                }
            }
        }
        for (at, one) in sent.iter().enumerate() {
            if !matches!(standing(one), Standing::Unconfirmed) {
                continue;
            }
            let card = one
                .receipt
                .as_ref()
                .and_then(|receipt| receipt.card.clone())
                .or_else(|| write_item(&one.write));
            let detail = card.as_ref().and_then(|card| {
                ids.iter()
                    .position(|id| id == card)
                    .and_then(|position| read.get(position))
            });
            let landed = match &one.write {
                StagedWrite::Copy { .. } => {
                    let item = detail.and_then(|detail| detail.item.clone()).or_else(|| {
                        found_cards
                            .iter()
                            .find(|(found, _)| *found == at)
                            .map(|(_, item)| item.clone())
                    });
                    item.is_some_and(|item| {
                        let record = item
                            .metadata
                            .get(&batch.record_key())
                            .cloned()
                            .unwrap_or(Value::Null);
                        one.digest.as_deref()
                            == Some(ticket::digest(&item.title, &item.content, &record).as_str())
                    })
                }
                StagedWrite::Comment {
                    root_cause, body, ..
                } => detail.is_some_and(|detail| {
                    detail.comments.iter().flatten().any(|comment| {
                        EvidenceComment::parse(&comment.body, &batch.config.namespace).is_ok_and(
                            |held| held.batch == batch.batch && &held.root_cause == root_cause,
                        ) && comment.body.trim() == body.trim()
                    })
                }),
                StagedWrite::ReEstimate {
                    priority_estimate, ..
                } => detail
                    .and_then(|detail| detail.item.as_ref())
                    .and_then(|item| item.metadata.get(&batch.record_key()))
                    .and_then(|value| TicketRecord::parse(&value.to_string()).ok())
                    .is_some_and(|record| record.priority_estimate == *priority_estimate),
                StagedWrite::Relate { .. } => false,
            };
            confirmed_by_read[at] = landed;
        }
    }

    // What did not certainly land is pending.
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let journal_path = batch.work.receipts(&batch.batch).display().to_string();
    let mut entries: Vec<PendingEntry> = Vec::new();
    let mut confirmed = 0;
    for (at, one) in sent.iter().enumerate() {
        let withdrawal = matches!(&one.write, StagedWrite::Copy { ticket, .. }
            if batch.ticket_slug(ticket).and_then(|slug| batch.ticket(&slug).ok()).is_some_and(|file| file.withdrawn()));
        let (state_of, error, card) = match standing(one) {
            Standing::Confirmed | Standing::Nothing => continue,
            Standing::Unconfirmed if confirmed_by_read[at] => {
                confirmed += 1;
                continue;
            }
            Standing::Unconfirmed => (
                PendingState::Unknown,
                Some("the store's answer was not recorded, and reading the item back did not find the write".to_owned()),
                one.receipt.as_ref().and_then(|receipt| receipt.card.clone()).or_else(|| write_item(&one.write)),
            ),
            Standing::Refused { kind, message, card } => (
                if kind == "route-command-failed" || kind == "protected" {
                    PendingState::Withheld
                } else {
                    PendingState::Failed
                },
                Some(format!("{kind}: {message}")),
                card.or_else(|| write_item(&one.write)),
            ),
        };
        entries.push(PendingEntry {
            snapshot: journal_path.clone(),
            batch: batch.batch.clone(),
            item: card.or_else(|| one.ticket_item.clone()).unwrap_or_default(),
            write: pending_kind(&one.write, withdrawal),
            state: state_of,
            snapshot_version: None,
            seen_version: None,
            intended: intended(&one.write),
            parts: Vec::new(),
            error,
            recorded_at: now.clone(),
        });
    }
    let pending_count = entries.len();
    if !entries.is_empty() {
        let path = batch.work.pending();
        let mut pending: Pending = work::read(&path)?.unwrap_or(Pending {
            entries: Vec::new(),
        });
        pending.entries.extend(entries);
        work::write(&path, &pending)?;
    }
    for publish in &mut state.publishes {
        publish.settled = true;
    }
    batch.save_state(&state)?;

    let needs_attention = pending_count > 0;
    let hook = match &batch.config.hooks.batch_settled {
        None => None,
        Some(argv) => {
            let summary = json!({
                "schema_version": 1,
                "batch": batch.batch,
                "writes": sent.len(),
                "read_back": read_back,
                "confirmed": confirmed,
                "pending": pending_count,
                "needs_attention": needs_attention,
                "receipts": batch.work.receipts(&batch.batch),
            })
            .to_string();
            let ran = command::run(
                argv,
                &summary,
                settings.working_directory(),
                settings.environment(),
            )
            .map_err(|error| {
                Error::Failed(format!(
                    "the `hooks.batch_settled` command `{}` could not run: {error}",
                    argv.join(" ")
                ))
            })?;
            Some(HookRun {
                argv: argv.clone(),
                exit: ran.exit,
            })
        }
    };

    let mut retired = Vec::new();
    for path in [
        batch.work.outbox(&batch.batch),
        batch.work.task(&batch.batch),
        batch.work.existing_tickets(&batch.batch),
    ] {
        if work::remove(&path)? {
            retired.push(path);
        }
    }
    Ok(Settled {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        read_back,
        confirmed,
        hook,
        retired,
        pending: pending_count,
        needs_attention,
        store_operations: store.map(|store| store.operations()).unwrap_or_default(),
    })
}
