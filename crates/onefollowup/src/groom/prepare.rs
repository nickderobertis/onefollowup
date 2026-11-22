//! `groom prepare`: list a destination's open tickets, fetch each repository they name, select
//! the tickets to verify, and write the batch the verifying agents work from.
//!
//! **Three store operations**: the open-item listing (every page of the destination's tasks at
//! an open status category, bodies included), one `comments_since` read of every comment made or edited on
//! an open ticket since the window start, and one batched read of the selected tickets'
//! comments. `--dry-run` skips the last.
//!
//! **The comment window is derived on every pass, never stored**: it opens at the earliest
//! `verified_at` among the open tickets' records, less `groom.comment_overlap_seconds`, floored
//! at `now - groom.max_age_days - overlap`, since an older record selects its ticket anyway. So
//! a ticket whose record did not move holds the window back until it does, and no comment on an
//! open ticket is passed over unless a published record of that ticket was taken after it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use serde_json::{Value, json};

use super::{BATCH_RECORD, Groom, OPEN_CATEGORIES, SNAPSHOT_RECORD, TASK, VERDICTS, git};
use crate::api::{GroomPrepareArgs, GroomPrepared, GroomShard};
use crate::config::Settings;
use crate::error::Error;
use crate::formats::groom::{
    FetchedRepository, GroomBatch, GroomRecord, GroomSnapshot, HeldStatus, OverlapComment,
    Protection, SCHEMA_VERSION, SelectedTicket, Shard, SnapshotTicket, content_digest,
    is_repository, record_key, render,
};
use crate::store::{ListedComment, Narrowing, StoreComment, StoreItem};
use crate::verify::template::{self, Located};
use crate::work;

/// When a comment last changed, read from its RFC 3339 time.
fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn stamp(at: &DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

/// The pattern a grooming comment's marker line matches: the line `groom-verb`'s comment
/// template ends in.
fn grooming_marker(namespace: &str) -> Regex {
    Regex::new(&format!(
        r#"(?m)^<!-- {}:follow-up-comment\b[^\n]*\bkind="groom"[^\n]*-->\s*$"#,
        regex::escape(namespace)
    ))
    .unwrap_or_else(|_| unreachable!("an escaped namespace makes a valid pattern"))
}

/// One open ticket as prepare reads it.
struct Open<'a> {
    item: &'a StoreItem,
    record: Option<Result<GroomRecord, String>>,
    activity: Vec<&'a ListedComment>,
}

/// Why `open` is selected, empty when it is not.
fn reasons(
    git: git::Git<'_>,
    open: &Open<'_>,
    fetched: &BTreeMap<String, FetchedRepository>,
    verified_at: &DateTime<Utc>,
    overlap: Duration,
    max_age: Duration,
    full: bool,
) -> Result<Vec<String>, Error> {
    let mut reasons = Vec::new();
    if full {
        reasons.push("`--full` verifies every open ticket".to_owned());
    }
    let record = match &open.record {
        None => {
            reasons.push("it holds no groom record".to_owned());
            return Ok(reasons);
        }
        Some(Err(why)) => {
            reasons.push(format!("its groom record is not one this reads: {why}"));
            return Ok(reasons);
        }
        Some(Ok(record)) => record,
    };
    let Some(recorded) = instant(&record.verified_at) else {
        reasons.push(format!(
            "its record's `verified_at` `{}` is not an RFC 3339 time",
            record.verified_at
        ));
        return Ok(reasons);
    };
    if content_digest(&open.item.title, &open.item.content) != record.content_digest {
        reasons.push("its title or content changed since its record".to_owned());
    }
    if *verified_at - recorded > max_age {
        reasons.push(format!(
            "its record, of {}, is older than {} days",
            record.verified_at,
            max_age.num_days()
        ));
    }
    match fetched.get(&record.repository) {
        Some(repository) if open.item.repositories.contains(&record.repository) => {
            if repository.commit != record.commit {
                let clone = PathBuf::from(&repository.clone);
                match git::changed_paths(git, &clone, &record.commit, &repository.commit)? {
                    None => reasons.push(format!(
                        "`{}`'s `main` no longer holds its record's commit {}",
                        record.repository, record.commit
                    )),
                    Some(_) if record.evidence_paths.is_empty() => reasons.push(format!(
                        "`{}`'s `main` moved since {}, and its record names no evidence path",
                        record.repository, record.commit
                    )),
                    Some(changed) => {
                        for path in &record.evidence_paths {
                            if git::touches(&changed, path) {
                                reasons.push(format!(
                                    "`{}`'s `main` changed `{path}` since {}",
                                    record.repository, record.commit
                                ));
                            }
                        }
                    }
                }
            }
        }
        _ => reasons.push(format!(
            "it no longer names its record's repository `{}`",
            record.repository
        )),
    }
    let since = recorded - overlap;
    for comment in &open.activity {
        let Some(at) = instant(&comment.updated_at) else {
            continue;
        };
        if at > recorded {
            reasons.push(format!(
                "comment {} was made or edited at {}, after its record",
                comment.id, comment.updated_at
            ));
        } else if at > since {
            match record
                .overlap_comments
                .iter()
                .find(|held| held.id == comment.id)
            {
                None => reasons.push(format!(
                    "comment {}, of {} inside the overlap before its record, is not one its record verified",
                    comment.id, comment.updated_at
                )),
                Some(held) if instant(&held.edited_at) != Some(at) => reasons.push(format!(
                    "comment {} was edited at {}, and its record verified it as of {}",
                    comment.id, comment.updated_at, held.edited_at
                )),
                Some(_) => {}
            }
        }
    }
    Ok(reasons)
}

/// Run `groom prepare`.
///
/// # Errors
///
/// A refused shard size, the store's refusal, a repository that cannot be fetched, or a batch
/// that cannot be written.
pub fn run(settings: &Settings, args: GroomPrepareArgs) -> Result<GroomPrepared, Error> {
    let groom = Groom::new(settings)?;
    let config = groom.config;
    let shard_size = args.shard_size.unwrap_or(config.groom.shard_size);
    if shard_size == 0 {
        return Err(Error::Refused(
            "`--shard-size` is 0: a shard holds at least one ticket".to_owned(),
        ));
    }
    let overlap = Duration::seconds(
        i64::try_from(config.groom.comment_overlap_seconds).unwrap_or(i64::MAX / 2_000_000),
    );
    let max_age =
        Duration::days(i64::try_from(config.groom.max_age_days).unwrap_or(i64::MAX / 100_000_000));
    let destination = &config.destination;
    let key = record_key(&config.namespace);
    let store = groom.store()?;
    let statuses: Vec<String> = OPEN_CATEGORIES.iter().map(|&one| one.to_owned()).collect();

    let mut items = store.list(destination, &statuses)?;
    items.retain(|item| OPEN_CATEGORIES.contains(&item.status.category.as_str()));
    items.sort_by(|one, other| one.id.cmp(&other.id));
    items.dedup_by(|one, other| one.id == other.id);

    let records: Vec<Option<Result<GroomRecord, String>>> = items
        .iter()
        .map(|item| {
            item.metadata
                .get(&key)
                .map(|value| GroomRecord::from_value(value).map_err(|error| error.to_string()))
        })
        .collect();

    // The comment read begins: every record this batch publishes says it was verified now.
    let verified_at = Utc::now();
    let earliest = records
        .iter()
        .filter_map(|record| record.as_ref()?.as_ref().ok())
        .filter_map(|record| instant(&record.verified_at))
        .min()
        .map_or(verified_at, |earliest| earliest.min(verified_at));
    let start = (earliest - overlap).max(verified_at - max_age - overlap);
    let listing = store.comments_since(
        destination,
        &start,
        &Narrowing {
            statuses,
            ..Narrowing::default()
        },
    )?;
    let marker = grooming_marker(&config.namespace);
    let mut activity: BTreeMap<&str, Vec<&ListedComment>> = BTreeMap::new();
    for comment in &listing.comments {
        if !marker.is_match(&comment.body) {
            activity
                .entry(comment.item.as_str())
                .or_default()
                .push(comment);
        }
    }

    let named: BTreeSet<&str> = items
        .iter()
        .flat_map(|item| item.repositories.iter().map(String::as_str))
        // A repository is a path under the clones and part of a URL: only `host/owner/name`.
        .filter(|repository| is_repository(repository))
        .collect();
    // A dry run writes nothing: its clones are made in temporary storage, removed when the
    // verb returns, and the configured clones are neither created nor fetched.
    let scratch = if args.dry_run {
        Some(git::Scratch::new(settings.environment())?)
    } else {
        None
    };
    let clones = scratch
        .as_ref()
        .map_or_else(|| groom.clones(), |scratch| scratch.path().to_path_buf());
    let mut fetched: BTreeMap<String, FetchedRepository> = BTreeMap::new();
    for repository in named {
        let clone = git::clone_path(&clones, repository);
        let commit = if scratch.is_some() {
            git::fetch_temporary(groom.git(), repository, &clone)?
        } else {
            git::fetch(groom.git(), repository, &clone)?
        };
        fetched.insert(
            repository.to_owned(),
            FetchedRepository {
                repository: repository.to_owned(),
                clone: clone.display().to_string(),
                commit,
            },
        );
    }

    let mut selected: Vec<(&StoreItem, Vec<String>)> = Vec::new();
    for (item, record) in items.iter().zip(records) {
        let open = Open {
            item,
            record,
            activity: activity.get(item.id.as_str()).cloned().unwrap_or_default(),
        };
        let why = reasons(
            groom.git(),
            &open,
            &fetched,
            &verified_at,
            overlap,
            max_age,
            args.full,
        )?;
        if !why.is_empty() {
            selected.push((item, why));
        }
    }

    let batch = format!("groom-{}", verified_at.format("%Y%m%dT%H%M%S%3fZ"));
    let directory = groom.batch(&batch);
    let shards: Vec<Shard> = selected
        .chunks(shard_size)
        .enumerate()
        .map(|(index, chunk)| {
            let shard = format!("{batch}-{}", index + 1);
            Shard {
                task: directory.join(&shard).join(TASK).display().to_string(),
                verdicts: directory.join(&shard).join(VERDICTS).display().to_string(),
                shard,
                tickets: chunk.iter().map(|(item, _)| item.id.clone()).collect(),
            }
        })
        .collect();
    let protected = selected
        .iter()
        .filter(|(item, _)| groom.protection(&item.status.category) == Protection::Claimed)
        .count();

    if !args.dry_run && !shards.is_empty() {
        // Two passes in one millisecond would share a batch id; this guard keeps the second
        // from writing into the first's batch. Untested: provoking it needs two processes
        // started in the same millisecond, a flaky race to stage for one line that refuses.
        if directory.exists() {
            return Err(Error::Failed(format!(
                "{}: the batch {batch} already exists; next: run `groom prepare` again",
                directory.display()
            )));
        }
        let ids: Vec<String> = selected.iter().map(|(item, _)| item.id.clone()).collect();
        let read = store.read(&ids, true)?;
        let comments: BTreeMap<String, Vec<StoreComment>> = ids
            .iter()
            .cloned()
            .zip(read.into_iter().map(|one| one.comments.unwrap_or_default()))
            .collect();
        let written = Written {
            groom: &groom,
            batch: &batch,
            verified_at: &verified_at,
            overlap,
            items: &items,
            selected: &selected,
            activity: &activity,
            fetched: &fetched,
            comments: &comments,
            marker: &marker,
        };
        written.write(&shards)?;
    }

    Ok(GroomPrepared {
        schema_version: SCHEMA_VERSION,
        batch,
        destination: destination.clone(),
        selected: selected.len(),
        unchanged: items.len() - selected.len(),
        protected,
        shards: shards
            .into_iter()
            .map(|shard| GroomShard {
                tickets: shard.tickets.len(),
                shard: shard.shard,
                task: PathBuf::from(shard.task),
                verdicts: PathBuf::from(shard.verdicts),
            })
            .collect(),
        store_operations: store.operations(),
    })
}

/// What a batch is written from.
struct Written<'a> {
    groom: &'a Groom<'a>,
    batch: &'a str,
    verified_at: &'a DateTime<Utc>,
    overlap: Duration,
    items: &'a [StoreItem],
    selected: &'a [(&'a StoreItem, Vec<String>)],
    activity: &'a BTreeMap<&'a str, Vec<&'a ListedComment>>,
    fetched: &'a BTreeMap<String, FetchedRepository>,
    comments: &'a BTreeMap<String, Vec<StoreComment>>,
    marker: &'a Regex,
}

impl Written<'_> {
    fn write(&self, shards: &[Shard]) -> Result<(), Error> {
        let groom = self.groom;
        let directory = groom.batch(self.batch);
        let held = |item: &StoreItem| HeldStatus {
            category: item.status.category.clone(),
            name: item.status.name.clone(),
        };
        let snapshot = GroomSnapshot {
            schema_version: SCHEMA_VERSION,
            batch: self.batch.to_owned(),
            tickets: self
                .items
                .iter()
                .map(|item| SnapshotTicket {
                    ticket: item.id.clone(),
                    title: item.title.clone(),
                    status: held(item),
                    url: item.url.clone(),
                    repositories: item.repositories.clone(),
                    created_at: item.created_at.clone(),
                    content: item.content.clone(),
                })
                .collect(),
        };
        let since = *self.verified_at - self.overlap;
        let selected: Vec<SelectedTicket> = self
            .selected
            .iter()
            .map(|(item, reasons)| SelectedTicket {
                ticket: item.id.clone(),
                title: item.title.clone(),
                status: held(item),
                protection: groom.protection(&item.status.category),
                repositories: item.repositories.clone(),
                created_at: item.created_at.clone(),
                content_digest: content_digest(&item.title, &item.content),
                overlap_comments: self
                    .activity
                    .get(item.id.as_str())
                    .into_iter()
                    .flatten()
                    .filter(|comment| {
                        instant(&comment.updated_at)
                            .is_some_and(|at| at > since && at <= *self.verified_at)
                    })
                    .map(|comment| OverlapComment {
                        id: comment.id.clone(),
                        edited_at: comment.updated_at.clone(),
                    })
                    .collect(),
                reasons: reasons.clone(),
            })
            .collect();
        let record = GroomBatch {
            schema_version: SCHEMA_VERSION,
            batch: self.batch.to_owned(),
            destination: groom.config.destination.clone(),
            namespace: groom.config.namespace.clone(),
            verified_at: stamp(self.verified_at),
            repositories: self.fetched.values().cloned().collect(),
            open: self
                .items
                .iter()
                .map(|item| (item.id.clone(), groom.protection(&item.status.category)))
                .collect(),
            selected,
            shards: shards.to_vec(),
        };
        let persona = self.persona()?;
        let snapshot_path = directory.join(SNAPSHOT_RECORD);
        work::write_text(&snapshot_path, &render(&snapshot))?;
        for shard in shards {
            let task = self.task(shard, &record, &persona, &snapshot_path)?;
            work::write_text(&PathBuf::from(&shard.task), &task)?;
        }
        work::write_text(&directory.join(BATCH_RECORD), &render(&record))?;
        Ok(())
    }

    fn persona(&self) -> Result<Value, Error> {
        let located = Located::of(&self.groom.config.groom.persona, |path| {
            self.groom.settings.path("groom", path)
        });
        let text = template::text(&located)?;
        serde_saphyr::from_str::<Value>(&text).map_err(|error| {
            Error::Refused(format!(
                "the persona {}: is not YAML: {error}",
                self.groom.config.groom.persona
            ))
        })
    }

    fn task(
        &self,
        shard: &Shard,
        record: &GroomBatch,
        persona: &Value,
        snapshot: &std::path::Path,
    ) -> Result<String, Error> {
        let tickets: Vec<Value> = shard
            .tickets
            .iter()
            .filter_map(|id| {
                let selected = record.selected.iter().find(|one| &one.ticket == id)?;
                let (item, _) = self.selected.iter().find(|(item, _)| &item.id == id)?;
                let comments: Vec<&StoreComment> = self
                    .comments
                    .get(id)
                    .into_iter()
                    .flatten()
                    .filter(|comment| !self.marker.is_match(&comment.body))
                    .collect();
                Some(json!({
                    "ticket": id,
                    "title": item.title,
                    "url": item.url,
                    "status": selected.status,
                    "protection": selected.protection,
                    "repositories": item.repositories,
                    "reasons": selected.reasons,
                    "content": item.content,
                    "comments": comments,
                }))
            })
            .collect();
        let named: BTreeSet<&str> = self
            .selected
            .iter()
            .filter(|(item, _)| shard.tickets.contains(&item.id))
            .flat_map(|(item, _)| item.repositories.iter().map(String::as_str))
            .collect();
        let repositories: Vec<&FetchedRepository> = named
            .into_iter()
            .filter_map(|repository| self.fetched.get(repository))
            .collect();
        let located = Located::of(&self.groom.config.groom.task_template, |path| {
            self.groom.settings.path("groom", path)
        });
        Ok(template::render(
            &located,
            &json!({
                "batch": self.batch,
                "shard": shard.shard,
                "destination": self.groom.config.destination,
                "namespace": self.groom.config.namespace,
                "verdicts": shard.verdicts,
                "snapshot": snapshot,
                "persona": persona,
                "repositories": repositories,
                "tickets": tickets,
            }),
        )?)
    }
}
