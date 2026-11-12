//! `comments settle`: settle a launched snapshot's answers in one batched read and one
//! `update-many`.
//!
//! Each launched batch's account is held again ([`super::check::hold`]); an unsound one writes
//! nothing. The one read takes every item a sound account quotes, and every item
//! `pending.json` names, with their comments: each reply is verified there — on the issue,
//! under the batch's marker, answering its comment — and each one that is not is recorded in
//! `pending.json`, so the watermark holds and the next unscoped pass answers its comment again.
//!
//! Every ticket change the agents asked for — a changed ticket's content, record and
//! repositories, the re-estimate its comments and verified replies give, and the status of a
//! withdrawal of the batch's own proposal — is one held write in the one `update-many`, held to
//! the version the read returned. A write is withheld into `pending.json` instead where the read
//! shows the item differs from what onefollowup last wrote to it, so a person's edit stands.
//! An entry `pending.json` already holds is cleared when the read shows it landed, retried in the
//! same `update-many` when the item again holds what onefollowup last wrote, and otherwise kept,
//! needing attention.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::Utc;
use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::api::{CommentsSettleArgs, CommentsSettled, SCHEMA_VERSION, SettledLaunch};
use crate::comments::check::{self, Account};
use crate::comments::{Pass, Snapshotted, instant, spelled};
use crate::config::{PriorityModel, RelationStore, Rollup, Settings, StatusRole};
use crate::error::Error;
use crate::formats::Level;
use crate::formats::batch::Read;
use crate::formats::gathering::LastWritten;
use crate::formats::markers::Reply;
use crate::formats::outbox::Outbox;
use crate::formats::pending::{Pending, PendingEntry, PendingState, PendingWrite, WritePart};
use crate::formats::ticket::TicketRecord;
use crate::formats::watermark::Watermark;
use crate::store::{BatchAnswer, HeldUpdate, ReadItem, StoreItem};
use crate::verify::context::Batch;
use crate::verify::decide::{self, priority_of};
use crate::verify::ticket::{self, ESTIMATE_LINE, Estimated, level_word};
use crate::work;

/// The record keys other batches' and the store's own writes move, which a person's edit is
/// not read from: the estimate and the priority onefollowup last wrote, the edges and the
/// assignee.
const MOVED_BY_ONEFOLLOWUP: &[&str] = &[
    "schema",
    "priority_estimate",
    "board_estimate",
    "edges",
    "board_assignee",
];

/// An item as a person's edit is read from: its title, its content less the estimate line, and
/// its record less what onefollowup's own writes move.
fn normalized(title: &str, content: &str, record: &Value) -> (String, String, Value) {
    let estimate = format!("- {ESTIMATE_LINE}:");
    let starred = format!("* {ESTIMATE_LINE}:");
    let content: Vec<&str> = content
        .trim_end()
        .split('\n')
        .filter(|line| !line.starts_with(&estimate) && !line.starts_with(&starred))
        .collect();
    let mut record = record.clone();
    if let Value::Object(map) = &mut record {
        for key in MOVED_BY_ONEFOLLOWUP {
            map.shift_remove(*key);
        }
    }
    (
        title.trim().to_owned(),
        content.join("\n").trim_end().to_owned(),
        record,
    )
}

/// What differs between `item` and what onefollowup last wrote to it, if anything.
fn departure(pass: &Pass<'_>, item: &StoreItem, written: &LastWritten) -> Option<String> {
    let record = item
        .metadata
        .get(&pass.record_key())
        .cloned()
        .unwrap_or(Value::Null);
    let now = normalized(&item.title, &item.content, &record);
    let then = normalized(&written.title, &written.content, &written.record);
    let mut parts = Vec::new();
    if now.0 != then.0 {
        parts.push("its title");
    }
    if now.1 != then.1 {
        parts.push("its content");
    }
    if now.2 != then.2 {
        parts.push("its record");
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// One write settle sends, and what it is for.
struct Planned {
    batch: String,
    write: PendingWrite,
    update: HeldUpdate,
    intended: IndexMap<String, Value>,
    snapshot: PathBuf,
    snapshot_version: Option<String>,
    /// The drafts-source item to keep in step once it lands.
    stored_as: Option<String>,
}

/// What every part of one settle reads.
struct Settling<'a, 'b> {
    pass: &'a Pass<'b>,
    reads: BTreeMap<String, ReadItem>,
    now: String,
}

impl Settling<'_, '_> {
    fn item(&self, id: &str) -> Option<&StoreItem> {
        self.reads.get(id).and_then(|read| read.item.as_ref())
    }

    fn entry(
        &self,
        planned: &Planned,
        state: PendingState,
        seen: Option<String>,
        error: String,
    ) -> PendingEntry {
        PendingEntry {
            snapshot: planned.snapshot.display().to_string(),
            batch: planned.batch.clone(),
            item: planned.update.item.clone(),
            write: planned.write,
            state,
            snapshot_version: planned.snapshot_version.clone(),
            seen_version: seen,
            intended: planned.intended.clone(),
            parts: Vec::new(),
            error: Some(error),
            recorded_at: self.now.clone(),
        }
    }
}

/// Settle a snapshot's answers.
///
/// # Errors
///
/// A snapshot never launched, a `--launched` naming another snapshot's feedback, the store's
/// refusal of the read, or a working file that cannot be read or written.
#[allow(
    clippy::too_many_lines,
    reason = "one pass: hold the accounts, read once, verify, plan, write once, record"
)]
pub fn run(settings: &Settings, args: CommentsSettleArgs) -> Result<CommentsSettled, Error> {
    let pass = Pass::new(settings)?;
    let directory = pass.given(&args.snapshot);
    let snapshotted = Snapshotted::read(&directory)?;
    if snapshotted.launched()?.is_none() {
        return Err(Error::Refused(format!(
            "the snapshot {} was never launched: run `onefollowup comments launches --snapshot {}` first",
            directory.display(),
            directory.display()
        )));
    }
    let mut named: Vec<(String, PathBuf)> = Vec::new();
    if args.launched.is_empty() {
        for batch in snapshotted.snapshot.selection.keys() {
            named.push((
                batch.clone(),
                pass.work.feedback_file(batch, &snapshotted.stamp),
            ));
        }
    } else {
        for pair in &args.launched {
            let (batch, feedback) = pair.split_once('=').unwrap_or((pair, ""));
            named.push((batch.to_owned(), pass.given(std::path::Path::new(feedback))));
        }
    }
    let mut accounts: Vec<Account> = Vec::new();
    for (batch, feedback) in &named {
        let account = check::hold(&pass, feedback)?;
        if &account.batch != batch || account.snapshotted.directory != directory {
            return Err(Error::Refused(format!(
                "{} is batch `{}`'s feedback from the snapshot {}, not batch `{batch}`'s from {}: name each launched batch with the feedback this snapshot's launch wrote for it",
                feedback.display(),
                account.batch,
                account.snapshotted.directory.display(),
                directory.display()
            )));
        }
        accounts.push(account);
    }
    let held = work::read::<Pending>(&pass.work.pending())?;
    let had_pending = held.is_some();
    let held = held.map(|pending| pending.entries).unwrap_or_default();

    // The one read: every item a sound account quotes, and every item pending names.
    let mut ids: Vec<String> = Vec::new();
    for account in accounts.iter().filter(|account| account.sound()) {
        for one in &account.quoted {
            if !ids.contains(&one.issue) {
                ids.push(one.issue.clone());
            }
        }
    }
    for entry in &held {
        if !ids.contains(&entry.item) {
            ids.push(entry.item.clone());
        }
    }
    let store = pass.store()?;
    let reads: BTreeMap<String, ReadItem> = if ids.is_empty() {
        BTreeMap::new()
    } else {
        ids.iter().cloned().zip(store.read(&ids, true)?).collect()
    };
    let settling = Settling {
        pass: &pass,
        reads,
        now: spelled(&Utc::now()),
    };

    let mut pending: Vec<PendingEntry> = Vec::new();
    let mut planned: Vec<Planned> = Vec::new();
    let mut outstanding_total = 0;
    let mut results: Vec<SettledLaunch> = Vec::new();
    for account in &accounts {
        let mut launch = SettledLaunch {
            batch: account.batch.clone(),
            feedback: account.feedback.clone(),
            writes: 0,
            pending: 0,
            verified: 0,
            outstanding: 0,
            problems: account.problems.clone(),
        };
        if !account.sound() {
            results.push(launch);
            continue;
        }
        let batch = pass.batch(&account.batch)?;
        for response in account
            .responses
            .iter()
            .flat_map(|responses| responses.responses.iter())
        {
            if replied(
                &settling,
                &batch,
                &response.issue,
                &response.comment,
                Some(&response.reply),
            ) {
                launch.verified += 1;
                continue;
            }
            launch.outstanding += 1;
            launch.pending += 1;
            pending.push(PendingEntry {
                snapshot: directory.display().to_string(),
                batch: account.batch.clone(),
                item: response.issue.clone(),
                write: PendingWrite::Comment,
                state: PendingState::Unknown,
                snapshot_version: None,
                seen_version: settling.item(&response.issue).and_then(|item| item.version.clone()),
                intended: IndexMap::from([
                    ("answers".to_owned(), Value::from(response.comment.clone())),
                    ("reply".to_owned(), Value::from(response.reply.clone())),
                ]),
                parts: Vec::new(),
                error: Some(format!(
                    "no reply {} answering comment {} under batch `{}`'s marker is on {}: the comment is answered again by the next unscoped pass",
                    response.reply, response.comment, account.batch, response.issue
                )),
                recorded_at: settling.now.clone(),
            });
        }
        outstanding_total += launch.outstanding;
        let mut issues: Vec<&str> = Vec::new();
        for one in &account.quoted {
            if !issues.contains(&one.issue.as_str()) {
                issues.push(&one.issue);
            }
        }
        for issue in issues {
            match plan(&settling, &batch, account, issue) {
                Ok(Some(write)) => planned.push(write),
                Ok(None) => {}
                Err(entry) => {
                    launch.pending += 1;
                    pending.push(*entry);
                }
            }
        }
        results.push(launch);
    }

    // What pending.json already held: cleared once landed, retried where the item again holds
    // what onefollowup last wrote, and otherwise kept.
    for entry in held {
        if entry.write == PendingWrite::Comment {
            let answers = entry.intended.get("answers").and_then(Value::as_str);
            let landed = answers.is_some_and(|answers| {
                pass.batch(&entry.batch)
                    .is_ok_and(|batch| replied(&settling, &batch, &entry.item, answers, None))
            });
            let again = pending.iter().any(|one| {
                one.write == PendingWrite::Comment
                    && one.item == entry.item
                    && one.intended.get("answers") == entry.intended.get("answers")
            });
            if !landed && !again {
                pending.push(entry);
            }
            continue;
        }
        if planned.iter().any(|one| one.update.item == entry.item)
            || pending
                .iter()
                .any(|one| one.item == entry.item && one.write != PendingWrite::Comment)
        {
            continue;
        }
        match recover(&settling, entry) {
            Recovered::Landed => {}
            Recovered::Retry(write) => planned.push(*write),
            Recovered::Kept(entry) => pending.push(*entry),
        }
    }

    // The one `update-many` per destination.
    let mut by_source: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (at, write) in planned.iter().enumerate() {
        let source = write
            .update
            .item
            .split_once(':')
            .map(|(source, _)| source.to_owned())
            .unwrap_or_default();
        by_source.entry(source).or_default().push(at);
    }
    let mut landed: Vec<usize> = Vec::new();
    for (source, members) in &by_source {
        let updates: Vec<HeldUpdate> = members
            .iter()
            .map(|at| planned[*at].update.clone())
            .collect();
        let answers = store
            .update_many(source, &updates)
            .map_err(|error| error.to_string());
        for (position, at) in members.iter().enumerate() {
            let write = &planned[*at];
            let answer = answers.as_ref().map(|answers| answers.get(position));
            match unlanded(&write.update.item, answer) {
                None => landed.push(*at),
                Some((state, error)) => {
                    let seen = settling
                        .item(&write.update.item)
                        .and_then(|item| item.version.clone());
                    pending.push(settling.entry(write, state, seen, error));
                }
            }
        }
    }

    // What onefollowup last wrote, kept in step: each landed write's ticket in the drafts
    // source, so the next pass reads the item as it now stands.
    let mut drafts: Vec<HeldUpdate> = Vec::new();
    for at in &landed {
        let write = &planned[*at];
        if let Some(stored_as) = &write.stored_as {
            drafts.push(HeldUpdate {
                item: stored_as.clone(),
                title: write.update.title.clone(),
                body: write.update.body.clone(),
                metadata: write.update.metadata.clone(),
                repositories: write.update.repositories.clone(),
                ..HeldUpdate::default()
            });
        }
        if let Some(launch) = results
            .iter_mut()
            .find(|launch| launch.batch == write.batch)
        {
            launch.writes += 1;
        }
    }
    if !drafts.is_empty() {
        let answers = store.update_many(&pass.config.drafts_source, &drafts)?;
        if let Some(refused) = answers.iter().find(|answer| answer.outcome != "written") {
            let why = refused
                .reason
                .clone()
                .unwrap_or_else(|| refused.outcome.clone());
            return Err(Error::Failed(format!(
                "the destination took the writes, and the drafts source refused keeping {} in step ({why}): publish its batch again so the next pass reads what onefollowup last wrote",
                refused.item
            )));
        }
    }
    for (at, write) in planned.iter().enumerate() {
        if !landed.contains(&at)
            && let Some(launch) = results
                .iter_mut()
                .find(|launch| launch.batch == write.batch)
        {
            launch.pending += 1;
        }
    }

    // The watermark: moved only by an unscoped snapshot whose every selected batch settled a
    // sound account and whose every reply was verified.
    let every_batch = snapshotted
        .snapshot
        .selection
        .keys()
        .all(|batch| accounts.iter().any(|account| &account.batch == batch));
    let moves = !snapshotted.scoped()
        && every_batch
        && accounts.iter().all(Account::sound)
        && outstanding_total == 0;
    let mut watermark = None;
    if moves {
        for (board, listing) in &snapshotted.gathering.listings {
            let path = pass.work.watermark(board);
            let held = work::read::<Watermark>(&path)?;
            let newer = match (&held, instant(&listing.watermark)) {
                (Some(held), Some(next)) => instant(&held.since).is_none_or(|since| next > since),
                (None, Some(_)) => true,
                (_, None) => false,
            };
            if newer {
                work::write(
                    &path,
                    &Watermark {
                        board: board.clone(),
                        since: listing.watermark.clone(),
                        queried_at: snapshotted.snapshot.queried_at.clone(),
                    },
                )?;
            }
            watermark = Some(if newer {
                listing.watermark.clone()
            } else {
                held.map(|held| held.since).unwrap_or_default()
            });
        }
    }

    let count = pending.len();
    if had_pending || count > 0 {
        work::write(&pass.work.pending(), &Pending { entries: pending })?;
    }
    Ok(CommentsSettled {
        schema_version: SCHEMA_VERSION,
        snapshot: directory,
        settled: results,
        pending: count,
        needs_attention: count > 0 || accounts.iter().any(|account| !account.sound()),
        watermark,
        store_operations: store.operations(),
    })
}

/// Why a write of `item` did not land, from its answer, or `None` when it did.
fn unlanded(
    item: &str,
    answer: Result<Option<&BatchAnswer>, &String>,
) -> Option<(PendingState, String)> {
    let answer = match answer {
        Ok(Some(answer)) => answer,
        Ok(None) => {
            return Some((
                PendingState::Unknown,
                "the store's answer was lost".to_owned(),
            ));
        }
        Err(error) => return Some((PendingState::Unknown, error.clone())),
    };
    let moved = answer.current_version.as_deref().unwrap_or("another");
    match answer.outcome.as_str() {
        "written" => None,
        "version-moved" => Some((
            PendingState::Withheld,
            format!(
                "{item} moved to version {moved} after settle read it, so nothing was written over the change"
            ),
        )),
        "held" => Some((
            PendingState::Withheld,
            format!("{item} holds a priority a person set, so nothing of the write was sent"),
        )),
        _ => Some((
            PendingState::Failed,
            answer
                .reason
                .clone()
                .unwrap_or_else(|| answer.outcome.clone()),
        )),
    }
}

/// Whether the read shows a reply of `batch` on `issue` answering `comment` — the reply `id`
/// where one is named.
fn replied(
    settling: &Settling<'_, '_>,
    batch: &Batch<'_>,
    issue: &str,
    comment: &str,
    id: Option<&str>,
) -> bool {
    let namespace = &settling.pass.config.namespace;
    settling
        .reads
        .get(issue)
        .and_then(|read| read.comments.as_ref())
        .into_iter()
        .flatten()
        .filter(|one| id.is_none_or(|id| one.id == id))
        .any(|one| {
            // A source may hand a body back without the newline it was written with.
            let body = format!("{}\n", one.body.trim_end());
            Reply::parse(&body, namespace)
                .is_ok_and(|reply| reply.batch == batch.batch && reply.to.answers == comment)
        })
}

/// The write one quoted issue's answers ask for, `None` when they change nothing, or the
/// entry recording why it was not sent.
fn plan(
    settling: &Settling<'_, '_>,
    batch: &Batch<'_>,
    account: &Account,
    issue: &str,
) -> Result<Option<Planned>, Box<PendingEntry>> {
    let pass = settling.pass;
    let snapshotted = &account.snapshotted;
    let Some(written) = snapshotted.last_written(issue) else {
        return Ok(None);
    };
    let changed = account
        .changed
        .iter()
        .find(|ticket| ticket.slug == written.ticket);
    let refuse = |state: PendingState, write: PendingWrite, seen: Option<String>, error: String| {
        Box::new(PendingEntry {
            snapshot: snapshotted.directory.display().to_string(),
            batch: batch.batch.clone(),
            item: issue.to_owned(),
            write,
            state,
            snapshot_version: written.version.clone(),
            seen_version: seen,
            intended: IndexMap::new(),
            parts: Vec::new(),
            error: Some(error),
            recorded_at: settling.now.clone(),
        })
    };
    let Some(read) = settling.reads.get(issue) else {
        return Ok(None);
    };
    let Some(item) = read.item.clone() else {
        return Err(refuse(
            PendingState::Failed,
            PendingWrite::TicketCopy,
            None,
            format!(
                "{issue} could not be read ({}), so nothing was written to it",
                read.error
                    .clone()
                    .unwrap_or_else(|| "it is gone".to_owned())
            ),
        ));
    };
    let record: TicketRecord = match changed.and_then(|ticket| ticket.record.clone()) {
        Some(record) => record,
        None => match decide::item_record(
            batch,
            &Read {
                item: item.clone(),
                comments: None,
                found_by: Vec::new(),
            },
        ) {
            Some(record) => record,
            None => return Ok(None),
        },
    };
    let Some(kind) = batch.kind(record.kind.as_deref().unwrap_or(&batch.config.default_kind))
    else {
        return Ok(None);
    };
    let title = changed.map_or_else(|| item.title.clone(), |ticket| ticket.title.clone());
    let body = changed.map_or_else(|| item.content.clone(), |ticket| ticket.body.clone());
    let as_read = Read {
        item: item.clone(),
        comments: read.comments.clone(),
        found_by: Vec::new(),
    };
    let empty = Outbox {
        batch: batch.batch.clone(),
        writes: Vec::new(),
    };
    let count = decide::occurrences(batch, &as_read, &record, &empty);
    let estimated = match rolled(batch, snapshotted, issue, kind, &body, &record, count) {
        Ok(estimated) => estimated,
        Err(message) => {
            return Err(refuse(
                PendingState::Failed,
                PendingWrite::Priority,
                item.version.clone(),
                format!("{issue}'s estimate could not be made: {message}"),
            ));
        }
    };
    let body = estimated.line.as_ref().map_or(body.clone(), |line| {
        ticket::with_estimate_line(&body, line, &batch.config.rubric)
    });
    let mut record = record;
    record.priority_estimate = estimated.level;
    let held_now = priority_of(&item.priority);
    let stored = record
        .board_estimate
        .map_or(crate::config::Priority::None, |level| {
            priority_of(level_word(level))
        });
    let following = held_now == stored;
    if following {
        record.board_estimate = Some(estimated.level);
    }
    let withdraw = changed.is_some_and(crate::verify::ticket::TicketFile::withdrawn);
    let mut status = None;
    if withdraw {
        let category = &item.status.category;
        if batch.protected(category) || batch.role_of(category) == Some(StatusRole::Cancelled) {
            return Err(refuse(
                PendingState::Withheld,
                PendingWrite::Withdrawal,
                item.version.clone(),
                format!(
                    "{issue} is at `{}`, a status a person decided on, so the batch's withdrawal of it was not written",
                    item.status.name
                ),
            ));
        }
        status = Some(batch.category_of(StatusRole::Cancelled));
    }
    let repositories = changed
        .filter(|_| {
            written.record.get("repository").and_then(Value::as_str) != Some(&record.repository)
        })
        .map(|_| vec![record.repository.clone()]);
    let record_value = ticket::record_value(&record);
    let current_record = item
        .metadata
        .get(&pass.record_key())
        .cloned()
        .unwrap_or(Value::Null);
    let priority = (following && level_word(estimated.level) != item.priority)
        .then(|| level_word(estimated.level).to_owned());
    let content_moves = body.trim_end() != item.content.trim_end();
    let title_moves = title.trim() != item.title.trim();
    let record_moves = record_value != current_record;
    if !content_moves
        && !title_moves
        && !record_moves
        && priority.is_none()
        && status.is_none()
        && repositories.is_none()
    {
        return Ok(None);
    }
    let write = if status.is_some() {
        PendingWrite::Withdrawal
    } else if changed.is_some() {
        PendingWrite::TicketCopy
    } else {
        PendingWrite::Priority
    };
    let mut intended = IndexMap::new();
    if title_moves {
        intended.insert("title".to_owned(), Value::from(title.clone()));
    }
    if let Some(priority) = &priority {
        intended.insert("priority".to_owned(), Value::from(priority.clone()));
    }
    intended.insert("content".to_owned(), Value::from(body.clone()));
    intended.insert("record".to_owned(), record_value.clone());
    if let Some(repositories) = &repositories {
        intended.insert("repositories".to_owned(), json!(repositories));
    }
    if let Some(status) = &status {
        intended.insert("status".to_owned(), Value::from(status.clone()));
    }
    let planned = Planned {
        batch: batch.batch.clone(),
        write,
        update: HeldUpdate {
            item: issue.to_owned(),
            if_version: item.version.clone(),
            title: title_moves.then_some(title),
            body: Some(body),
            status,
            repositories,
            priority: priority.clone(),
            when_priority: if priority.is_some() {
                vec![item.priority.clone()]
            } else {
                Vec::new()
            },
            metadata: BTreeMap::from([(pass.record_key(), record_value)]),
            assignee: None,
            when_assignees: None,
        },
        intended,
        snapshot: snapshotted.directory.clone(),
        snapshot_version: written.version.clone(),
        stored_as: Some(written.stored_as.clone()),
    };
    if let Some(what) = departure(pass, &item, written) {
        let mut entry = settling.entry(
            &planned,
            PendingState::Withheld,
            item.version.clone(),
            format!(
                "{issue} differs from what onefollowup last wrote to it ({what}), so its write was withheld and the change stands; the next pass retries it once the item holds what onefollowup last wrote"
            ),
        );
        entry.parts = vec![WritePart {
            part: "issue".to_owned(),
            outcome: "withheld".to_owned(),
            error: None,
        }];
        return Err(Box::new(entry));
    }
    Ok(Some(planned))
}

/// The estimate of `issue` with its comments' occurrences, taking what the relations a
/// gathering read roll up into it: each `occurrences` relation's related tickets' occurrences
/// added to its count, and no lower than each `max` relation's highest related estimate.
#[allow(
    clippy::too_many_arguments,
    reason = "the snapshot's reads, and the facts the estimate is made of"
)]
fn rolled(
    batch: &Batch<'_>,
    snapshotted: &Snapshotted,
    issue: &str,
    kind: &crate::config::Kind,
    body: &str,
    record: &TicketRecord,
    count: u64,
) -> Result<Estimated, String> {
    let config = batch.config;
    let mut added = 0;
    let mut highest: Option<Level> = None;
    let held = snapshotted
        .gathering
        .relations
        .iter()
        .find(|held| held.item == issue);
    let empty = Outbox {
        batch: batch.batch.clone(),
        writes: Vec::new(),
    };
    if let Some(held) = held {
        for relation in &config.relations {
            if relation.rollup == Rollup::None
                || record.kind.as_deref().unwrap_or(&config.default_kind) != relation.to
            {
                continue;
            }
            let ends: Vec<&str> = match relation.store {
                RelationStore::Parent => held.children.iter().map(String::as_str).collect(),
                RelationStore::Link => held
                    .links
                    .iter()
                    .filter(|link| link.link_type == relation.name && !link.outgoing)
                    .map(|link| link.other.as_str())
                    .collect(),
            };
            for end in ends {
                let Some(read) = snapshotted
                    .gathering
                    .related
                    .iter()
                    .find(|read| read.item.id == end)
                else {
                    continue;
                };
                let Some(their) = decide::item_record(batch, read) else {
                    continue;
                };
                let occurrences = decide::occurrences(batch, read, &their, &empty);
                match relation.rollup {
                    Rollup::None => {}
                    Rollup::Occurrences => added += occurrences,
                    Rollup::Max => {
                        let own = batch
                            .kind(their.kind.as_deref().unwrap_or(&config.default_kind))
                            .and_then(|kind| {
                                ticket::estimate(
                                    config,
                                    kind,
                                    &read.item.content,
                                    their.frequency,
                                    occurrences,
                                )
                                .ok()
                            })
                            .map_or(their.priority_estimate, |estimated| estimated.level);
                        if highest.is_none_or(|level| rank(own) > rank(level)) {
                            highest = Some(own);
                        }
                    }
                }
            }
        }
    }
    let mut estimated = ticket::estimate(config, kind, body, record.frequency, count + added)?;
    if let Some(level) = highest
        && rank(level) > rank(estimated.level)
        && matches!(kind.priority, PriorityModel::Estimate)
    {
        estimated.line = estimated.line.map(|line| {
            let inner = line
                .split_once('(')
                .and_then(|(_, rest)| rest.strip_suffix(')'))
                .unwrap_or_default()
                .to_owned();
            format!(
                "- {ESTIMATE_LINE}: {} ({inner}; held at {}, its related tickets' highest)",
                level_word(level),
                level_word(level)
            )
        });
        estimated.level = level;
    }
    Ok(estimated)
}

fn rank(level: Level) -> u8 {
    match level {
        Level::Urgent => 3,
        Level::High => 2,
        Level::Medium => 1,
        Level::Low => 0,
    }
}

/// What became of an entry `pending.json` already held.
enum Recovered {
    /// The read shows it landed: cleared.
    Landed,
    /// The item again holds what onefollowup last wrote: sent again.
    Retry(Box<Planned>),
    /// Still not sendable: kept.
    Kept(Box<PendingEntry>),
}

fn recover(settling: &Settling<'_, '_>, mut entry: PendingEntry) -> Recovered {
    let pass = settling.pass;
    let Some(item) = settling.item(&entry.item) else {
        return Recovered::Kept(Box::new(entry));
    };
    let record = item
        .metadata
        .get(&pass.record_key())
        .cloned()
        .unwrap_or(Value::Null);
    let holds = |key: &str, now: &Value| entry.intended.get(key).is_none_or(|want| want == now);
    let content_holds = entry
        .intended
        .get("content")
        .and_then(Value::as_str)
        .is_none_or(|want| want.trim_end() == item.content.trim_end());
    if content_holds
        && holds("title", &Value::from(item.title.clone()))
        && holds("record", &record)
        && holds("priority", &Value::from(item.priority.clone()))
        && holds("status", &Value::from(item.status.category.clone()))
    {
        return Recovered::Landed;
    }
    let baseline = Snapshotted::read(std::path::Path::new(&entry.snapshot))
        .ok()
        .and_then(|snapshotted| snapshotted.last_written(&entry.item).cloned());
    let Some(baseline) = baseline else {
        return Recovered::Kept(Box::new(entry));
    };
    if let Some(what) = departure(pass, item, &baseline) {
        entry.seen_version.clone_from(&item.version);
        entry.error = Some(format!(
            "{} still differs from what onefollowup last wrote to it ({what}), so its write stays withheld; a person decides whether it stands",
            entry.item
        ));
        return Recovered::Kept(Box::new(entry));
    }
    let text = |key: &str| {
        entry
            .intended
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let update = HeldUpdate {
        item: entry.item.clone(),
        if_version: item.version.clone(),
        title: text("title"),
        body: text("content"),
        status: text("status"),
        repositories: entry.intended.get("repositories").and_then(|value| {
            value.as_array().map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
        }),
        priority: text("priority"),
        when_priority: if text("priority").is_some() {
            vec![item.priority.clone()]
        } else {
            Vec::new()
        },
        metadata: entry
            .intended
            .get("record")
            .map(|record| BTreeMap::from([(pass.record_key(), record.clone())]))
            .unwrap_or_default(),
        assignee: None,
        when_assignees: None,
    };
    Recovered::Retry(Box::new(Planned {
        batch: entry.batch.clone(),
        write: entry.write,
        update,
        intended: entry.intended.clone(),
        snapshot: PathBuf::from(&entry.snapshot),
        snapshot_version: entry.snapshot_version.clone(),
        stored_as: Some(baseline.stored_as),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(outcome: &str, current: Option<&str>, reason: Option<&str>) -> BatchAnswer {
        BatchAnswer {
            item: "followups:1".to_owned(),
            outcome: outcome.to_owned(),
            comment: None,
            version: None,
            current_version: current.map(str::to_owned),
            reason: reason.map(str::to_owned),
        }
    }

    #[test]
    fn each_answer_of_a_held_write_is_landed_withheld_failed_or_unknown() {
        let item = "followups:1";
        assert_eq!(
            unlanded(item, Ok(Some(&answered("written", None, None)))),
            None
        );
        let moved = unlanded(item, Ok(Some(&answered("version-moved", Some("v2"), None))));
        assert_eq!(
            moved.map(|(state, error)| (state, error.contains("moved to version v2"))),
            Some((PendingState::Withheld, true))
        );
        let held = unlanded(item, Ok(Some(&answered("held", None, None))));
        assert_eq!(held.map(|(state, _)| state), Some(PendingState::Withheld));
        let error = unlanded(item, Ok(Some(&answered("error", None, Some("refused")))));
        assert_eq!(error, Some((PendingState::Failed, "refused".to_owned())));
        let lost = unlanded(item, Ok(None));
        assert_eq!(lost.map(|(state, _)| state), Some(PendingState::Unknown));
        let refused = "the store refused the batch".to_owned();
        assert_eq!(
            unlanded(item, Err(&refused)),
            Some((PendingState::Unknown, refused.clone()))
        );
    }
}
