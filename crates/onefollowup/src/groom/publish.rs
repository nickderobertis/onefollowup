//! `groom publish`: send what the batch's passing check planned, receipt every outcome, read
//! back every ticket it changed, and report.
//!
//! **Three store operations of writes and reads, then one read-back.** One batched status read
//! of the tickets to close or fold, made once [`Store::admit_batch`] admitted the batch, so no
//! governor wait falls between that read and the closes; one `update-many` carrying every close
//! the read still admits, every duplicate close and every verified ticket's `groom` record; one
//! `comment add-many` built from that write's receipts — each closed ticket's evidence comment,
//! then each survivor's consolidation comment naming only the duplicates shown closed. Then one
//! batched read of every ticket changed.
//!
//! **Protection is decided by the status read**, by onefollowup's status roles: a ticket still
//! at a non-claimable open status (the proposal or deferred role's, or one no role maps) is
//! closed or folded; one still Todo is closed only on a definitely-outdated verdict and never
//! folded; any other — Todo short of that, Queued, In Progress, In Review or no longer open — is
//! left for a person, written nothing, so its older record selects it again next pass. Each
//! close is held to the version that read found, so on a source that holds a write to its
//! version a change made after the read leaves the ticket for a person too.
//!
//! **A re-run reads only `plan.json` and `receipts.json`**, which hold every record and comment
//! it writes from: it sends only what no receipt confirms, re-reading the status of every close
//! no receipt confirms (in the same order) and the comments of every ticket owed one, so a
//! comment that landed with its answer lost is not posted twice. A close whose answer was lost is
//! confirmed by that read finding the ticket at its planned close holding this batch's record.
//! Every close is sent before any comment about it, so no ticket is ever open with a comment
//! saying it was closed.
//!
//! **The read-back** confirms each closed ticket's status category, duplicate link, comment and
//! record, each recorded ticket's record, and each survivor's comment. A ticket's issue closing,
//! beyond its card, rests on the store: onetaskgraph's terminal status write closes the issue
//! and its duplicate close reads `cancelled`. A read reports one status per ticket, so the
//! read-back cannot see an issue left open under a terminal card.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{BATCH_RECORD, Groom, OPEN_CATEGORIES, PLAN_RECORD, SNAPSHOT_RECORD, batch_id, git};
use crate::api::{GroomPublishArgs, GroomPublished};
use crate::config::{Settings, StatusRole};
use crate::error::Error;
use crate::formats::groom::{
    Confidence, GroomBatch, GroomReceipts, GroomRecord, GroomReport, GroomSnapshot, GroomVerdict,
    HeldStatus, LeftForPerson, Mismatch, PlanEntry, PlanRecord, PostedComment, Protection,
    PublishAction, PublishOutcome, ReportedTicket, SCHEMA_VERSION, SnapshotTicket, SurvivorReceipt,
    TicketReceipt, is_commit, parse, record_key, render,
};
use crate::formats::markers::{CommentMarker, GroomMark};
use crate::store::{CommentPost, HeldUpdate, ReadItem, Store, StoreItem};
use crate::verify::template::{self, Located};
use crate::work;

/// A batch's receipts.
pub const RECEIPTS_RECORD: &str = "receipts.json";
/// A batch's report, for a person.
pub const REPORT: &str = "report.md";
/// A batch's report, as a record.
pub const REPORT_RECORD: &str = "report.json";

/// The files a published batch keeps: what a re-run reads, and its reports.
const KEPT: [&str; 4] = [PLAN_RECORD, RECEIPTS_RECORD, REPORT, REPORT_RECORD];

fn held(item: &StoreItem) -> HeldStatus {
    HeldStatus {
        category: item.status.category.clone(),
        name: item.status.name.clone(),
    }
}

/// The receipts a first publish starts from: every ticket of the plan, with the record its
/// write carries and whether its verdict is definitely outdated, read from the batch prepare
/// wrote while it is still there.
fn first_receipts(
    groom: &Groom<'_>,
    directory: &Path,
    plan: &PlanRecord,
) -> Result<GroomReceipts, Error> {
    let read = |name: &str| {
        let path = directory.join(name);
        std::fs::read_to_string(&path).map_err(|error| {
            Error::Failed(format!(
                "{}: could not be read: {error}; next: prepare and check the batch again",
                path.display()
            ))
        })
    };
    let batch: GroomBatch = parse(&read(BATCH_RECORD)?, "grooming batch")?;
    let snapshot: GroomSnapshot = parse(&read(SNAPSHOT_RECORD)?, "grooming snapshot")?;
    let listed: BTreeMap<&str, &SnapshotTicket> = snapshot
        .tickets
        .iter()
        .map(|ticket| (ticket.ticket.as_str(), ticket))
        .collect();
    // Check plans only tickets of the snapshot; one it does not hold is refused here.
    let describe = |ticket: &str| -> Result<(String, Option<String>, HeldStatus), Error> {
        let one = listed.get(ticket).ok_or_else(|| {
            Error::Failed(format!(
                "{ticket} is planned and not in {SNAPSHOT_RECORD}; next: prepare and check the batch again"
            ))
        })?;
        Ok((one.title.clone(), one.url.clone(), one.status.clone()))
    };
    let git = groom.git();
    let outdated = |entry: &PlanEntry| -> Result<bool, Error> {
        let fetched = batch.repositories.iter().find(|one| {
            Some(&one.repository) == entry.repository.as_ref()
                && Some(&one.commit) == entry.commit.as_ref()
        });
        match (fetched, &entry.deciding_commit) {
            // Only a full commit id reaches git, so a plan's value is never read as an option.
            (Some(fetched), Some(commit))
                if entry.verdict.closes()
                    && entry.confidence == Confidence::High
                    && is_commit(commit) =>
            {
                git::is_ancestor(git, Path::new(&fetched.clone), commit, &fetched.commit)
            }
            _ => Ok(false),
        }
    };
    let record = |entry: &PlanEntry| -> Option<GroomRecord> {
        let selected = batch
            .selected
            .iter()
            .find(|one| one.ticket == entry.ticket)?;
        Some(GroomRecord {
            schema_version: SCHEMA_VERSION,
            batch: batch.batch.clone(),
            verified_at: batch.verified_at.clone(),
            verdict: entry.verdict,
            repository: entry.repository.clone()?,
            commit: entry.commit.clone()?,
            evidence_paths: entry.evidence_paths.clone(),
            content_digest: selected.content_digest.clone(),
            overlap_comments: selected.overlap_comments.clone(),
        })
    };
    let mut tickets = Vec::new();
    let planned = plan
        .plan
        .fixed
        .iter()
        .chain(&plan.plan.obsolete)
        .map(|entry| (PublishAction::Close, entry))
        .chain(
            plan.plan
                .duplicates
                .iter()
                .map(|entry| (PublishAction::Fold, entry)),
        )
        .chain(plan.plan.kept.iter().map(|entry| {
            let action = if entry.left_for_person.is_some() {
                PublishAction::Keep
            } else {
                PublishAction::Record
            };
            (action, entry)
        }));
    for (action, entry) in planned {
        let (title, url, listed) = describe(&entry.ticket)?;
        let record = match action {
            PublishAction::Keep => None,
            _ => record(entry),
        };
        // A kept ticket whose record names no repository has nothing to write.
        let action = if action == PublishAction::Record && record.is_none() {
            PublishAction::Keep
        } else {
            action
        };
        let (outcome, reason) = match (action, &entry.left_for_person) {
            (PublishAction::Keep, Some(why)) => (PublishOutcome::LeftForPerson, Some(why.clone())),
            (PublishAction::Keep, None) => (
                PublishOutcome::Kept,
                Some("its plan entry names no repository and commit to record".to_owned()),
            ),
            _ => (PublishOutcome::Pending, None),
        };
        tickets.push(TicketReceipt {
            ticket: entry.ticket.clone(),
            title,
            url,
            listed,
            action,
            definitely_outdated: outdated(entry)?,
            entry: entry.clone(),
            record,
            outcome,
            found: None,
            reason,
            comment: None,
        });
    }
    let survivors = plan
        .plan
        .survivors
        .iter()
        .map(|survivor| {
            let (title, url, listed) = describe(&survivor.ticket)?;
            Ok(SurvivorReceipt {
                ticket: survivor.ticket.clone(),
                title,
                url,
                listed,
                members: survivor.members.clone(),
                chosen_by: survivor.chosen_by,
                named: Vec::new(),
                comment: None,
            })
        })
        .collect::<Result<_, Error>>()?;
    Ok(GroomReceipts {
        schema_version: SCHEMA_VERSION,
        batch: plan.batch.clone(),
        destination: batch.destination,
        namespace: batch.namespace,
        tickets,
        survivors,
    })
}

/// What one publish run works from.
struct Publishing<'a> {
    groom: &'a Groom<'a>,
    store: &'a Store,
    receipts: GroomReceipts,
    path: PathBuf,
}

/// Whether `ticket` is a close or fold no receipt confirms.
fn unconfirmed(ticket: &TicketReceipt) -> bool {
    matches!(ticket.action, PublishAction::Close | PublishAction::Fold)
        && matches!(
            ticket.outcome,
            PublishOutcome::Pending | PublishOutcome::Failed
        )
}

/// Whether `ticket` is owed its evidence comment.
fn owed_evidence(ticket: &TicketReceipt) -> bool {
    ticket.outcome == PublishOutcome::Closed && ticket.comment.is_none()
}

/// The grooming mark and batch of `body`'s marker line, when it is a grooming comment's.
fn grooming_mark(body: &str, namespace: &str) -> Option<(String, GroomMark)> {
    let last = body
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let marker = CommentMarker::parse(last, namespace).ok()?;
    Some((marker.batch, marker.groom?))
}

impl Publishing<'_> {
    fn save(&self) -> Result<(), Error> {
        work::write_text(&self.path, &render(&self.receipts))?;
        Ok(())
    }

    /// The category a ticket's close writes: the `done` role's for `fixed`, the `cancelled`
    /// role's for `obsolete`, and `cancelled` for a duplicate, the one a duplicate close takes.
    fn target(&self, ticket: &TicketReceipt) -> String {
        let statuses = &self.groom.config.statuses;
        match (ticket.action, ticket.entry.verdict) {
            (PublishAction::Fold, _) => "cancelled".to_owned(),
            (_, GroomVerdict::Fixed) => statuses.done.category.clone(),
            _ => statuses.cancelled.category.clone(),
        }
    }

    /// Whether the status read `found` still admits `ticket`'s close, and why not when it
    /// does not.
    fn admits(&self, ticket: &TicketReceipt, found: &StoreItem) -> Result<(), String> {
        let key = record_key(&self.receipts.namespace);
        let category = found.status.category.as_str();
        // A close whose answer was lost: the ticket stands at its planned close holding this
        // batch's record, which only this batch's write puts there.
        let ours = ticket
            .record
            .as_ref()
            .is_some_and(|record| found.metadata.get(&key) == Some(&record.to_value()));
        if ours && category == self.target(ticket) {
            return Ok(());
        }
        if !OPEN_CATEGORIES.contains(&category) {
            return Err(format!(
                "it is no longer open: publish's status read found it {}",
                found.status.name
            ));
        }
        match (self.groom.protection(category), ticket.action) {
            (Protection::Unprotected, _) => Ok(()),
            (Protection::Todo, PublishAction::Close) if ticket.definitely_outdated => Ok(()),
            (Protection::Todo, PublishAction::Close) => Err(format!(
                "publish's status read found it {}, Todo, which grooming closes only on a high-confidence verdict whose deciding commit is verified on the default branch",
                found.status.name
            )),
            (Protection::Todo, _) => Err(format!(
                "publish's status read found it {}, Todo, which grooming never folds as a duplicate",
                found.status.name
            )),
            (Protection::Claimed, _) => Err(format!(
                "publish's status read found it {}, claimed (Queued, In Progress or In Review), which grooming never closes or folds",
                found.status.name
            )),
        }
    }

    /// The survivors whose consolidation comment is owed: none yet posted, every duplicate
    /// folded into it settled — closed or left for a person — and at least one closed.
    fn owed_survivors(&self) -> Vec<usize> {
        self.receipts
            .survivors
            .iter()
            .enumerate()
            .filter(|(_, survivor)| survivor.comment.is_none())
            .filter(|(_, survivor)| {
                let folded: Vec<&TicketReceipt> = self.folded_into(&survivor.ticket);
                !folded.is_empty()
                    && folded.iter().all(|one| !unconfirmed(one))
                    && folded
                        .iter()
                        .any(|one| one.outcome == PublishOutcome::Closed)
            })
            .map(|(at, _)| at)
            .collect()
    }

    fn folded_into(&self, survivor: &str) -> Vec<&TicketReceipt> {
        self.receipts
            .tickets
            .iter()
            .filter(|one| {
                one.action == PublishAction::Fold
                    && one.entry.duplicate_of.as_deref() == Some(survivor)
            })
            .collect()
    }

    /// Read the status of every close no receipt confirms and, on a re-run, the comments of
    /// every ticket owed one, then send every write still owed, then every comment owed.
    fn send(&mut self, rerun: bool) -> Result<(), Error> {
        let destination = self.receipts.destination.clone();
        let to_read: Vec<String> = self
            .receipts
            .tickets
            .iter()
            .filter(|one| unconfirmed(one) || (rerun && owed_evidence(one)))
            .map(|one| one.ticket.clone())
            .chain(
                self.owed_survivors()
                    .into_iter()
                    .filter(|_| rerun)
                    .map(|at| self.receipts.survivors[at].ticket.clone()),
            )
            .collect();
        let records: usize = self
            .receipts
            .tickets
            .iter()
            .filter(|one| {
                one.action == PublishAction::Record && one.outcome != PublishOutcome::Recorded
            })
            .count();
        let closes = self
            .receipts
            .tickets
            .iter()
            .filter(|one| unconfirmed(one))
            .count();
        let owed = self
            .receipts
            .tickets
            .iter()
            .filter(|one| owed_evidence(one))
            .count();
        let survivors = self
            .receipts
            .survivors
            .iter()
            .filter(|one| one.comment.is_none())
            .count();
        // A batch whose every write and comment is receipted sends nothing.
        if closes == 0 && records == 0 && owed == 0 && self.owed_survivors().is_empty() {
            return Ok(());
        }
        let comments = closes + owed + survivors;
        self.store
            .admit_batch(&destination, to_read.len(), closes + records, comments)?;
        let read: BTreeMap<String, ReadItem> = if to_read.is_empty() {
            BTreeMap::new()
        } else {
            to_read
                .iter()
                .cloned()
                .zip(self.store.read(&to_read, rerun)?)
                .collect()
        };

        // A grooming comment of this batch already on a ticket owed one landed with its answer
        // lost: it is receipted, never posted again.
        let namespace = self.receipts.namespace.clone();
        let batch = self.receipts.batch.clone();
        let posted = |id: &str| -> Option<(GroomMark, PostedComment)> {
            read.get(id)?.comments.as_ref()?.iter().find_map(|comment| {
                match grooming_mark(&comment.body, &namespace) {
                    Some((of, mark)) if of == batch => Some((
                        mark,
                        PostedComment {
                            id: comment.id.clone(),
                            body: comment.body.clone(),
                        },
                    )),
                    _ => None,
                }
            })
        };
        for survivor in &mut self.receipts.survivors {
            if survivor.comment.is_none()
                && let Some((GroomMark::Survivor, comment)) = posted(&survivor.ticket)
            {
                survivor.comment = Some(comment);
            }
        }

        let key = record_key(&namespace);
        let mut updates: Vec<HeldUpdate> = Vec::new();
        let mut sent: Vec<usize> = Vec::new();
        for at in 0..self.receipts.tickets.len() {
            let ticket = &self.receipts.tickets[at];
            let metadata = ticket
                .record
                .as_ref()
                .map(|record| BTreeMap::from([(key.clone(), record.to_value())]))
                .unwrap_or_default();
            if ticket.action == PublishAction::Record && ticket.outcome != PublishOutcome::Recorded
            {
                updates.push(HeldUpdate {
                    item: ticket.ticket.clone(),
                    metadata,
                    ..HeldUpdate::default()
                });
                sent.push(at);
                continue;
            }
            if !unconfirmed(ticket) {
                continue;
            }
            let found = read.get(&ticket.ticket).and_then(|one| one.item.as_ref());
            let decided = match found {
                None => Err(format!(
                    "publish's status read found no such ticket{}",
                    read.get(&ticket.ticket)
                        .and_then(|one| one.error.as_deref())
                        .map(|why| format!(": {why}"))
                        .unwrap_or_default()
                )),
                Some(item) => self.admits(ticket, item),
            };
            let target = self.target(ticket);
            let ticket = &mut self.receipts.tickets[at];
            ticket.found = found.map(held);
            match decided {
                Err(why) => {
                    ticket.outcome = PublishOutcome::LeftForPerson;
                    ticket.reason = Some(why);
                }
                Ok(()) => {
                    let fold = ticket.action == PublishAction::Fold;
                    updates.push(HeldUpdate {
                        item: ticket.ticket.clone(),
                        if_version: found.and_then(|item| item.version.clone()),
                        // A duplicate close names no status: it writes `cancelled` itself.
                        status: (!fold).then_some(target),
                        duplicate_of: fold.then(|| ticket.entry.duplicate_of.clone()).flatten(),
                        metadata,
                        ..HeldUpdate::default()
                    });
                    sent.push(at);
                }
            }
        }
        if !updates.is_empty() {
            let answers = self.store.update_many(&destination, &updates)?;
            for (at, answer) in sent.iter().zip(answers) {
                let ticket = &mut self.receipts.tickets[*at];
                let closes = ticket.action != PublishAction::Record;
                match answer.outcome.as_str() {
                    "written" | "held" => {
                        ticket.outcome = if closes {
                            PublishOutcome::Closed
                        } else {
                            PublishOutcome::Recorded
                        };
                        ticket.reason = None;
                    }
                    "version-moved" if closes => {
                        ticket.outcome = PublishOutcome::LeftForPerson;
                        ticket.reason = Some(
                            "it changed after publish's status read, so its close was not written"
                                .to_owned(),
                        );
                    }
                    _ => {
                        ticket.outcome = PublishOutcome::Failed;
                        ticket.reason = Some(answer.reason.unwrap_or(answer.outcome));
                    }
                }
            }
        }
        for at in 0..self.receipts.tickets.len() {
            let ticket = &self.receipts.tickets[at];
            if owed_evidence(ticket)
                && let Some((_, comment)) = posted(&ticket.ticket)
            {
                self.receipts.tickets[at].comment = Some(comment);
            }
        }
        self.save()?;

        let mut posts: Vec<CommentPost> = Vec::new();
        let mut owners: Vec<(bool, usize)> = Vec::new();
        for at in 0..self.receipts.tickets.len() {
            if owed_evidence(&self.receipts.tickets[at]) {
                posts.push(CommentPost {
                    item: self.receipts.tickets[at].ticket.clone(),
                    body: self.evidence(&self.receipts.tickets[at])?,
                });
                owners.push((false, at));
            }
        }
        for at in self.owed_survivors() {
            let named: Vec<String> = self
                .folded_into(&self.receipts.survivors[at].ticket)
                .iter()
                .filter(|one| one.outcome == PublishOutcome::Closed)
                .map(|one| one.ticket.clone())
                .collect();
            posts.push(CommentPost {
                item: self.receipts.survivors[at].ticket.clone(),
                body: self.consolidation(&self.receipts.survivors[at], &named)?,
            });
            self.receipts.survivors[at].named = named;
            owners.push((true, at));
        }
        if posts.is_empty() {
            return Ok(());
        }
        let answers = self.store.add_comments(&destination, &posts)?;
        for ((survivor, at), (answer, post)) in
            owners.into_iter().zip(answers.into_iter().zip(posts))
        {
            let Some(comment) = answer.comment.filter(|_| answer.outcome == "written") else {
                continue;
            };
            let posted = Some(PostedComment {
                id: comment.id,
                body: post.body,
            });
            if survivor {
                self.receipts.survivors[at].comment = posted;
            } else {
                self.receipts.tickets[at].comment = posted;
            }
        }
        self.save()
    }

    /// Render every comment this run could post, before any store operation: a template that
    /// cannot render, or writes a marker itself, is refused before a ticket is closed.
    fn preflight(&self) -> Result<(), Error> {
        for ticket in &self.receipts.tickets {
            if unconfirmed(ticket) || owed_evidence(ticket) {
                self.evidence(ticket)?;
            }
        }
        for survivor in &self.receipts.survivors {
            if survivor.comment.is_none() {
                let folded: Vec<String> = self
                    .folded_into(&survivor.ticket)
                    .iter()
                    .map(|one| one.ticket.clone())
                    .collect();
                self.consolidation(survivor, &folded)?;
            }
        }
        Ok(())
    }

    fn located(&self) -> Located {
        Located::of(&self.groom.config.groom.comment_template, |path| {
            self.groom.settings.path("groom", path)
        })
    }

    /// A comment's whole body: the template rendered with `comment`, a blank line, and the
    /// marker as its last line.
    fn body(&self, comment: &Value, mark: GroomMark) -> Result<String, Error> {
        let namespace = &self.receipts.namespace;
        let rendered = template::render(&self.located(), &json!({ "comment": comment }))?;
        let visible = rendered.trim_start_matches(['\n', '\r']).trim_end();
        let reserved = format!("<!-- {namespace}:follow-up-comment");
        if visible
            .lines()
            .any(|line| line.trim_start().starts_with(&reserved))
        {
            return Err(Error::Refused(format!(
                "`groom.comment_template` ({}) writes a comment marker itself; onefollowup appends the marker line",
                self.groom.config.groom.comment_template
            )));
        }
        let marker = CommentMarker {
            batch: self.receipts.batch.clone(),
            root_cause: String::new(),
            reply: None,
            groom: Some(mark),
        };
        Ok(format!("{visible}\n\n{}\n", marker.render(namespace)))
    }

    fn evidence(&self, ticket: &TicketReceipt) -> Result<String, Error> {
        let entry = &ticket.entry;
        let (mark, verdict) = match (ticket.action, entry.verdict) {
            (PublishAction::Fold, _) => (GroomMark::Duplicate, "duplicate"),
            (_, GroomVerdict::Fixed) => (GroomMark::Fixed, "fixed"),
            _ => (GroomMark::Obsolete, "obsolete"),
        };
        let survivor = entry.duplicate_of.as_ref().map(|survivor| {
            let held = self
                .receipts
                .survivors
                .iter()
                .find(|one| &one.ticket == survivor);
            json!({
                "ticket": survivor,
                "title": held.map_or(survivor.as_str(), |one| one.title.as_str()),
                "url": held.and_then(|one| one.url.clone()),
            })
        });
        self.body(
            &json!({
                "batch": self.receipts.batch,
                "verdict": verdict,
                "ticket": ticket.ticket,
                "title": ticket.title,
                "url": ticket.url,
                "confidence": entry.confidence.as_str(),
                "evidence": entry.evidence,
                "deciding_commit": entry.deciding_commit,
                "repository": entry.repository,
                "commit": entry.commit,
                "evidence_paths": entry.evidence_paths,
                "root_cause": entry.root_cause,
                "survivor": survivor,
            }),
            mark,
        )
    }

    fn consolidation(&self, survivor: &SurvivorReceipt, named: &[String]) -> Result<String, Error> {
        let closed: Vec<&TicketReceipt> = named
            .iter()
            .filter_map(|id| self.receipts.tickets.iter().find(|one| &one.ticket == id))
            .collect();
        let mut evidence: Vec<&str> = Vec::new();
        for one in &closed {
            let text = one.entry.evidence.trim();
            if !evidence.contains(&text) {
                evidence.push(text);
            }
        }
        let role = self
            .groom
            .config
            .statuses
            .iter()
            .into_iter()
            .find(|(_, mapping)| mapping.category == survivor.listed.category)
            .map(|(role, _)| role);
        let claimed_as = match role {
            Some(StatusRole::Accepted) => Some("todo"),
            Some(StatusRole::Queued) => Some("queued"),
            Some(StatusRole::InProgress) => Some("in-progress"),
            Some(StatusRole::InReview) => Some("in-review"),
            _ => None,
        };
        self.body(
            &json!({
                "batch": self.receipts.batch,
                "verdict": "survivor",
                "ticket": survivor.ticket,
                "title": survivor.title,
                "url": survivor.url,
                "status": survivor.listed.name,
                "claimed_as": claimed_as,
                "duplicates": closed.iter().map(|one| json!({
                    "ticket": one.ticket,
                    "title": one.title,
                    "url": one.url,
                    "root_cause": one.entry.root_cause,
                    "evidence_paths": one.entry.evidence_paths,
                })).collect::<Vec<_>>(),
                "evidence": evidence,
            }),
            GroomMark::Survivor,
        )
    }

    /// Read back every ticket this batch changed, in one read, and hold each to its receipts.
    fn read_back(&self) -> Result<(usize, Vec<Mismatch>), Error> {
        let key = record_key(&self.receipts.namespace);
        let mut expected: BTreeMap<&str, Vec<Expect<'_>>> = BTreeMap::new();
        for ticket in &self.receipts.tickets {
            let mut expect = Vec::new();
            match ticket.outcome {
                PublishOutcome::Closed => {
                    expect.push(Expect::Status(self.target(ticket)));
                    if let Some(survivor) = ticket
                        .entry
                        .duplicate_of
                        .as_deref()
                        .filter(|_| ticket.action == PublishAction::Fold)
                    {
                        expect.push(Expect::DuplicateOf(survivor));
                    }
                }
                PublishOutcome::Recorded => {}
                _ => continue,
            }
            if let Some(record) = &ticket.record {
                expect.push(Expect::Record(record.to_value()));
            }
            if let Some(comment) = &ticket.comment {
                expect.push(Expect::Comment(comment));
            }
            expected.entry(&ticket.ticket).or_default().extend(expect);
        }
        for survivor in &self.receipts.survivors {
            if let Some(comment) = &survivor.comment {
                expected
                    .entry(&survivor.ticket)
                    .or_default()
                    .push(Expect::Comment(comment));
            }
        }
        if expected.is_empty() {
            return Ok((0, Vec::new()));
        }
        let ids: Vec<String> = expected.keys().map(|id| (*id).to_owned()).collect();
        let read = self.store.read(&ids, true)?;
        let mut confirmed = 0;
        let mut mismatched = Vec::new();
        for (id, one) in ids.iter().zip(read) {
            let before = mismatched.len();
            let mut differs = |expected: String, found: String| {
                mismatched.push(Mismatch {
                    ticket: id.clone(),
                    expected,
                    found,
                });
            };
            let Some(item) = one.item else {
                differs(
                    "the ticket".to_owned(),
                    one.error.unwrap_or_else(|| "no such ticket".to_owned()),
                );
                continue;
            };
            for expect in &expected[id.as_str()] {
                match expect {
                    Expect::Status(category) if &item.status.category != category => differs(
                        format!("status category `{category}`"),
                        format!(
                            "status category `{}` ({})",
                            item.status.category, item.status.name
                        ),
                    ),
                    Expect::DuplicateOf(survivor) => {
                        let link = item.duplicate_of();
                        if link.as_deref() != Some(*survivor) {
                            differs(
                                format!("a duplicate link to {survivor}"),
                                link.map_or_else(
                                    || "no duplicate link".to_owned(),
                                    |other| format!("a duplicate link to {other}"),
                                ),
                            );
                        }
                    }
                    Expect::Record(record) => {
                        let found = item.metadata.get(&key);
                        if found != Some(record) {
                            differs(
                                format!("`{key}` {record}"),
                                found.map_or_else(
                                    || format!("no `{key}`"),
                                    |value| format!("`{key}` {value}"),
                                ),
                            );
                        }
                    }
                    Expect::Comment(comment) => {
                        let held = one
                            .comments
                            .as_ref()
                            .and_then(|all| all.iter().find(|held| held.id == comment.id));
                        match held {
                            Some(held) if held.body.trim() == comment.body.trim() => {}
                            Some(held) => differs(
                                format!(
                                    "comment {} as posted: {:?}",
                                    comment.id,
                                    comment.body.trim()
                                ),
                                format!("comment {} reading {:?}", comment.id, held.body.trim()),
                            ),
                            None => differs(
                                format!("comment {}", comment.id),
                                "no such comment".to_owned(),
                            ),
                        }
                    }
                    Expect::Status(_) => {}
                }
            }
            if mismatched.len() == before {
                confirmed += 1;
            }
        }
        Ok((confirmed, mismatched))
    }

    fn report(&self, confirmed: usize, mismatched: Vec<Mismatch>) -> GroomReport {
        let survivor_of = |ticket: &TicketReceipt| {
            ticket.entry.duplicate_of.clone().or_else(|| {
                self.receipts
                    .survivors
                    .iter()
                    .find(|one| one.ticket != ticket.ticket && one.members.contains(&ticket.ticket))
                    .map(|one| one.ticket.clone())
            })
        };
        let left_for_person = self
            .receipts
            .tickets
            .iter()
            .filter(|one| one.outcome == PublishOutcome::LeftForPerson)
            .filter(|one| one.entry.verdict.closes() || survivor_of(one).is_some())
            .map(|one| LeftForPerson {
                ticket: one.ticket.clone(),
                title: one.title.clone(),
                url: one.url.clone(),
                status: one.found.clone().unwrap_or_else(|| one.listed.clone()),
                verdict: one.entry.verdict,
                confidence: one.entry.confidence,
                deciding_commit: one.entry.deciding_commit.clone(),
                evidence: one.entry.evidence.clone(),
                survivor: survivor_of(one),
                reason: one.reason.clone().unwrap_or_default(),
            })
            .collect();
        GroomReport {
            schema_version: SCHEMA_VERSION,
            batch: self.receipts.batch.clone(),
            destination: self.receipts.destination.clone(),
            confirmed,
            mismatched,
            outcomes: self
                .receipts
                .tickets
                .iter()
                .map(|one| ReportedTicket {
                    ticket: one.ticket.clone(),
                    title: one.title.clone(),
                    verdict: one.entry.verdict,
                    action: one.action,
                    outcome: one.outcome,
                    duplicate_of: one.entry.duplicate_of.clone(),
                    comment: one.comment.as_ref().map(|comment| comment.id.clone()),
                    reason: one.reason.clone(),
                })
                .collect(),
            survivors: self.receipts.survivors.clone(),
            left_for_person,
        }
    }
}

/// What the read-back holds one ticket to.
enum Expect<'a> {
    Status(String),
    DuplicateOf(&'a str),
    Record(Value),
    Comment(&'a PostedComment),
}

/// `report.md`: what a person reads of the publish.
fn markdown(report: &GroomReport) -> String {
    let count = |outcome: PublishOutcome, action: Option<PublishAction>| {
        report
            .outcomes
            .iter()
            .filter(|one| {
                one.outcome == outcome && action.is_none_or(|action| one.action == action)
            })
            .count()
    };
    let mut text = format!(
        "# Grooming batch `{}`\n\nDestination `{}`: {} closed, {} closed as duplicates, {} recorded, {} left for a person, {} failed. The read-back confirmed {} ticket(s) and found {} mismatch(es).\n",
        report.batch,
        report.destination,
        count(PublishOutcome::Closed, Some(PublishAction::Close)),
        count(PublishOutcome::Closed, Some(PublishAction::Fold)),
        count(PublishOutcome::Recorded, None),
        report.left_for_person.len(),
        count(PublishOutcome::Failed, None),
        report.confirmed,
        report.mismatched.len(),
    );
    if !report.mismatched.is_empty() {
        text.push_str("\n## Mismatches\n\n| Ticket | Expected | Found |\n|---|---|---|\n");
        for one in &report.mismatched {
            text.push_str(&format!(
                "| `{}` | {} | {} |\n",
                one.ticket,
                cell(&one.expected),
                cell(&one.found)
            ));
        }
    }
    if !report.left_for_person.is_empty() {
        text.push_str("\n## Left for a person\n\nEach ticket below has a closing or duplicate verdict this pass did not act on; a person decides it.\n");
        for one in &report.left_for_person {
            text.push_str(&format!(
                "\n### `{}` — {}\n\n- Status: {}\n- Verdict: {} at {} confidence\n- Deciding commit: {}\n- Survivor: {}\n- Why left: {}\n- Evidence: {}\n",
                one.ticket,
                one.url.as_ref().map_or_else(|| one.title.clone(), |url| format!("[{}]({url})", one.title)),
                one.status.name,
                one.verdict.as_str(),
                one.confidence.as_str(),
                one.deciding_commit.as_deref().unwrap_or("none"),
                one.survivor.as_deref().map_or_else(|| "none".to_owned(), |one| format!("`{one}`")),
                one.reason,
                one.evidence.trim().replace('\n', " "),
            ));
        }
    }
    let failed: Vec<&ReportedTicket> = report
        .outcomes
        .iter()
        .filter(|one| one.outcome == PublishOutcome::Failed)
        .collect();
    if !failed.is_empty() {
        text.push_str("\n## Failed\n\nRun `groom publish` again to send these.\n\n");
        for one in failed {
            text.push_str(&format!(
                "- `{}`: {}\n",
                one.ticket,
                one.reason.as_deref().unwrap_or("refused")
            ));
        }
    }
    let closed: Vec<&ReportedTicket> = report
        .outcomes
        .iter()
        .filter(|one| one.outcome == PublishOutcome::Closed)
        .collect();
    if !closed.is_empty() {
        text.push_str("\n## Closed\n\n");
        for one in closed {
            text.push_str(&format!(
                "- `{}` {}: {}\n",
                one.ticket,
                one.duplicate_of.as_deref().map_or_else(
                    || one.verdict.as_str().to_owned(),
                    |survivor| format!("duplicate of `{survivor}`")
                ),
                one.title
            ));
        }
    }
    text
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// Remove every working file of the batch but what a re-run reads and its reports.
fn retire(directory: &Path) -> Result<(), Error> {
    let unreadable = |error: std::io::Error| {
        Error::Failed(format!(
            "{}: could not be read: {error}",
            directory.display()
        ))
    };
    for entry in std::fs::read_dir(directory).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let path = entry.path();
        if KEPT.iter().any(|kept| entry.file_name() == *kept) {
            continue;
        }
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.map_err(|error| {
            Error::Failed(format!("{}: could not be removed: {error}", path.display()))
        })?;
    }
    Ok(())
}

/// Run `groom publish`.
///
/// # Errors
///
/// A batch id that is not one, a batch no passing check planned, receipts of another batch, a
/// file that cannot be read or written, the store's refusal of a whole call — or, at exit 1 with
/// its answer printed, a write the destination refused or a ticket read back otherwise than its
/// receipts say.
pub fn run(settings: &Settings, args: GroomPublishArgs) -> Result<GroomPublished, Error> {
    let groom = Groom::new(settings)?;
    let id = batch_id(&args.batch)?;
    let directory = groom.batch(id);
    let plan_path = directory.join(PLAN_RECORD);
    let text = std::fs::read_to_string(&plan_path).map_err(|_| {
        Error::Refused(format!(
            "{}: no passing `groom check` planned batch {id}; next: run `onefollowup groom check --batch {id}` until it passes",
            plan_path.display()
        ))
    })?;
    let plan: PlanRecord = parse(&text, "grooming plan")?;
    if plan.batch != id {
        return Err(Error::Refused(format!(
            "{} is batch {}'s plan, not {id}'s; next: run `onefollowup groom check --batch {id}`",
            plan_path.display(),
            plan.batch
        )));
    }
    let path = directory.join(RECEIPTS_RECORD);
    let (receipts, rerun) = match work::read_text(&path)? {
        Some(text) => {
            let receipts: GroomReceipts = parse(&text, "grooming receipts")?;
            if receipts.batch != id {
                return Err(Error::Refused(format!(
                    "{} holds batch {}'s receipts, not {id}'s",
                    path.display(),
                    receipts.batch
                )));
            }
            (receipts, true)
        }
        None => (first_receipts(&groom, &directory, &plan)?, false),
    };
    let store = groom.store()?;
    let mut publishing = Publishing {
        groom: &groom,
        store: &store,
        receipts,
        path,
    };
    publishing.preflight()?;
    if !rerun {
        publishing.save()?;
    }
    let sent = publishing.send(rerun);
    // Whatever the writes came to, what landed is read back and reported.
    let (confirmed, mismatched) = publishing.read_back()?;
    sent?;
    let report = publishing.report(confirmed, mismatched.clone());
    let report_json = directory.join(REPORT_RECORD);
    let report_md = directory.join(REPORT);
    work::write_text(&report_json, &render(&report))?;
    work::write_text(&report_md, &markdown(&report))?;
    retire(&directory)?;
    let failed: BTreeSet<&str> = report
        .outcomes
        .iter()
        .filter(|one| one.outcome == PublishOutcome::Failed)
        .map(|one| one.ticket.as_str())
        .collect();
    let published = GroomPublished {
        batch: id.to_owned(),
        confirmed,
        mismatched,
        report: report_md,
        report_json,
        store_operations: store.operations(),
        governed: crate::api::Governed::of(&store),
    };
    if failed.is_empty() && published.mismatched.is_empty() {
        return Ok(published);
    }
    Err(Error::Unpublished {
        message: format!(
            "the grooming batch {id} is not fully published: {} write(s) refused{}, {} mismatch(es) read back; next: {}",
            failed.len(),
            if failed.is_empty() {
                String::new()
            } else {
                format!(" ({})", failed.into_iter().collect::<Vec<_>>().join(", "))
            },
            published.mismatched.len(),
            if published.mismatched.is_empty() {
                format!(
                    "run `onefollowup groom publish --batch {id}` again, which sends only those"
                )
            } else {
                format!(
                    "read {} for each, and decide it",
                    published.report.display()
                )
            }
        ),
        answer: Box::new(published),
    })
}
