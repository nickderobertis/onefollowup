//! `comments gather`: list each destination's comments since its watermark in one
//! comments-since listing, select each comment to answer for the batch owning the item it sits
//! on, and write the snapshot every later step reads.
//!
//! A comment is selected for its owning batch, or left out for exactly one reason, tried in
//! this order: no batch of this host filed the item; its owning batch is no batch id; the
//! comment is marked as any batch's own; a bot wrote it; a reply of any batch answers it as it
//! now stands. A reply answers the version of the comment its gathering quoted, where this
//! host's accounts say which; otherwise it answers when it is no older than the comment's last
//! change. So a comment edited after its reply was quoted is answered again.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde_json::Value;

use crate::api::{CommentsGatherArgs, Gathered, SCHEMA_VERSION};
use crate::comments::{
    self, GATHERING_RECORD, LAST_WRITTEN, Owned, Pass, SNAPSHOT_RECORD, Snapshotted, instant,
    last_written_file, last_written_text, precise, spelled,
};
use crate::config::{RelationStore, Rollup, Settings};
use crate::error::Error;
use crate::formats::Versioned as _;
use crate::formats::batch::Read;
use crate::formats::gathering::{Gathering, LastWritten};
use crate::formats::markers::CommentMarker;
use crate::formats::responses::Responses;
use crate::formats::snapshot::{
    Boundary, Chosen, LeftOut, LeftOutcome, Snapshot, SnapshotItem, Start,
};
use crate::formats::watermark::Watermark;
use crate::store::{Author, ItemRelations, ListedComment, Narrowing, STORE_LINE};
use crate::work;

/// The format a snapshot directory's name is the gathering's instant in.
const STAMP: &str = "%Y%m%dT%H%M%SZ";

/// Gather people's comments to answer into a snapshot.
///
/// # Errors
///
/// A scope naming an item no batch of this host filed, or a batch that filed none; the
/// store's refusal of the listing; or a working file that cannot be read or written.
pub fn run(settings: &Settings, args: CommentsGatherArgs) -> Result<Gathered, Error> {
    let pass = Pass::new(settings)?;
    let owners = comments::owners(&pass)?;
    let destinations = pass.destinations();
    let queried_at = Utc::now();

    // What each destination is asked: every comment since its watermark, or — scoped — every
    // comment of the items the scope names.
    let mut scoped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if let Some(item) = &args.item {
        let owned = owners.get(item).ok_or_else(|| {
            Error::Refused(format!(
                "no batch of this host filed `{item}`, so none answers its comments: name an item a batch's receipts record, and nothing was gathered"
            ))
        })?;
        pass.batch(&owned.batch)?;
        let source = item
            .split_once(':')
            .map(|(source, _)| source)
            .unwrap_or_default();
        scoped
            .entry(source.to_owned())
            .or_default()
            .push(item.clone());
    }
    if let Some(batch) = &args.batch {
        pass.batch(batch)?;
        for (item, owned) in &owners {
            if &owned.batch == batch {
                let source = item
                    .split_once(':')
                    .map(|(source, _)| source)
                    .unwrap_or_default();
                scoped
                    .entry(source.to_owned())
                    .or_default()
                    .push(item.clone());
            }
        }
        if scoped.is_empty() {
            return Err(Error::Refused(format!(
                "batch `{batch}` has filed no ticket on this host, so it has no comments to answer; nothing was gathered"
            )));
        }
    }
    let scope = args.item.is_some() || args.batch.is_some();
    let boards: Vec<String> = if scope {
        scoped.keys().cloned().collect()
    } else {
        destinations
    };

    let store = pass.store()?;
    let epoch = DateTime::<Utc>::UNIX_EPOCH;
    let mut starts = IndexMap::new();
    let mut listings = IndexMap::new();
    for board in &boards {
        let (since, origin) = if scope {
            (
                epoch,
                "a scoped gathering reads every comment of its items".to_owned(),
            )
        } else {
            match work::read::<Watermark>(&pass.work.watermark(board))? {
                Some(watermark) => match instant(&watermark.since) {
                    Some(since) => (since, "the destination's watermark".to_owned()),
                    None => {
                        return Err(Error::Failed(format!(
                            "{}: `since` is not an instant: {}",
                            pass.work.watermark(board).display(),
                            watermark.since
                        )));
                    }
                },
                None => (epoch, "no watermark yet: every comment".to_owned()),
            }
        };
        let narrowing = Narrowing {
            metadata: vec![(
                format!("{}/host", pass.record_key()),
                pass.config.host_label.clone(),
            )],
            statuses: pass.answered_categories(),
            items: scoped.get(board).cloned().unwrap_or_default(),
        };
        let listing = store.comments_since(board, &since, &narrowing)?;
        starts.insert(
            board.clone(),
            Start {
                since: spelled(&since),
                origin,
            },
        );
        listings.insert(board.clone(), listing);
    }

    let comments: Vec<&ListedComment> = listings
        .values()
        .flat_map(|listing| listing.comments.iter())
        .collect();
    let quoted = quoted_versions(&pass)?;
    let selection = select(&pass, &owners, &comments, &quoted);

    // What onefollowup last wrote to each item a selected comment sits on: the batch's own
    // copy of its ticket in the drafts source, read in one local read.
    let mut selected_items: Vec<String> = Vec::new();
    for chosen in selection.chosen.values().flatten() {
        if !selected_items.contains(&chosen.issue) {
            selected_items.push(chosen.issue.clone());
        }
    }
    let stored: Vec<String> = selected_items
        .iter()
        .filter_map(|item| owners.get(item).map(|owned| owned.stored_as.clone()))
        .collect();
    let drafts_read = if stored.is_empty() {
        Vec::new()
    } else {
        store.read(&stored, false)?
    };
    let mut last_written = Vec::new();
    for item in &selected_items {
        let Some(owned) = owners.get(item) else {
            continue;
        };
        let read = drafts_read
            .iter()
            .filter_map(|read| read.item.as_ref())
            .find(|read| read.id == owned.stored_as);
        let Some(read) = read else {
            return Err(Error::Failed(format!(
                "batch `{}`'s ticket `{}` is no longer stored as {} in the drafts source, so what onefollowup last wrote to {item} is unknown; publish the batch again",
                owned.batch, owned.slug, owned.stored_as
            )));
        };
        last_written.push(LastWritten {
            item: item.clone(),
            batch: owned.batch.clone(),
            ticket: owned.slug.clone(),
            stored_as: owned.stored_as.clone(),
            version: owned.version.clone(),
            title: read.title.clone(),
            content: read.content.clone(),
            record: read
                .metadata
                .get(&pass.record_key())
                .cloned()
                .unwrap_or(Value::Null),
        });
    }

    // Under a relation with a rollup, the related tickets of each item a selected comment
    // sits on: one `task relations` and one read of their other ends, with their comments.
    let rollups = pass
        .config
        .relations
        .iter()
        .any(|relation| relation.rollup != Rollup::None);
    let (relations, related) = if rollups && !selected_items.is_empty() {
        let relations = store.relations(&selected_items)?;
        let others = related_items(&pass, &relations);
        let related = if others.is_empty() {
            Vec::new()
        } else {
            store
                .read(&others, true)?
                .into_iter()
                .filter_map(|read| {
                    read.item.map(|item| Read {
                        item,
                        comments: read.comments,
                        found_by: Vec::new(),
                    })
                })
                .collect()
        };
        (relations, related)
    } else {
        (Vec::new(), Vec::new())
    };

    let mut items: Vec<SnapshotItem> = Vec::new();
    for comment in &comments {
        if items.iter().any(|item| item.id == comment.item) {
            continue;
        }
        let owned = owners.get(&comment.item);
        let written = last_written
            .iter()
            .find(|written| written.item == comment.item);
        items.push(SnapshotItem {
            id: comment.item.clone(),
            snapshot_id: written
                .map(|written| last_written_file(&written.item))
                .unwrap_or_default(),
            version: owned.and_then(|owned| owned.version.clone()),
            title: written
                .map(|written| written.title.clone())
                .unwrap_or_default(),
            owner: owned.map(|owned| owned.batch.clone()),
            host: Some(pass.config.host_label.clone()),
            url: comment
                .url
                .as_deref()
                .map(|url| url.split('#').next().unwrap_or(url).to_owned()),
        });
    }
    let boundaries: IndexMap<String, Boundary> = selection
        .chosen
        .keys()
        .map(|batch| {
            (
                batch.clone(),
                Boundary {
                    boundary: None,
                    computed: false,
                },
            )
        })
        .collect();
    let snapshot = Snapshot {
        queried_at: precise(&queried_at),
        board: boards.first().cloned().unwrap_or_default(),
        boards: boards.clone(),
        named_board: false,
        scope: args.batch.clone(),
        issue: args.item.clone(),
        starts,
        store: STORE_LINE.to_owned(),
        dry_run: args.dry_run,
        items,
        boundaries,
        selection: selection.chosen.clone(),
        left: selection.left.clone(),
    };
    let gathering = Gathering {
        listings,
        last_written,
        relations,
        related,
    };
    let directory = write_snapshot(&pass, &queried_at, &snapshot, &gathering)?;

    let selected: IndexMap<String, usize> = selection
        .chosen
        .iter()
        .map(|(batch, chosen)| (batch.clone(), chosen.len()))
        .collect();
    let launch_command = args.dry_run.then(|| {
        format!(
            "onefollowup comments launches --snapshot {}",
            directory.display()
        )
    });
    Ok(Gathered {
        schema_version: SCHEMA_VERSION,
        snapshot: Some(directory),
        dry_run: args.dry_run,
        items: snapshot.items.len(),
        selected,
        left: snapshot.left.len(),
        launch_command,
        store_operations: store.operations(),
    })
}

/// The comments selected for each batch, and those left out.
struct Selection {
    chosen: IndexMap<String, Vec<Chosen>>,
    left: Vec<LeftOut>,
}

/// A reply the listing holds: its id, when it last changed, and the last change of the comment
/// its feedback quoted, where this host's accounts say.
type ReplySeen = (String, Option<DateTime<Utc>>, Option<DateTime<Utc>>);

/// The last non-empty line of `body`, read as a batch's comment marker.
fn marker(body: &str, namespace: &str) -> Option<CommentMarker> {
    let last = body
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    CommentMarker::parse(last, namespace).ok()
}

fn select(
    pass: &Pass<'_>,
    owners: &BTreeMap<String, Owned>,
    comments: &[&ListedComment],
    quoted: &BTreeMap<(String, String), DateTime<Utc>>,
) -> Selection {
    let namespace = &pass.config.namespace;
    // Each reply, on the issue holding the comment it answers.
    let mut replies: BTreeMap<(String, String), Vec<ReplySeen>> = BTreeMap::new();
    for comment in comments {
        if let Some(marker) = marker(&comment.body, namespace)
            && let Some(reply) = marker.reply
        {
            replies
                .entry((comment.item.clone(), reply.answers))
                .or_default()
                .push((
                    comment.id.clone(),
                    instant(&comment.version),
                    quoted
                        .get(&(comment.item.clone(), comment.id.clone()))
                        .copied(),
                ));
        }
    }
    let mut chosen: IndexMap<String, Vec<Chosen>> = IndexMap::new();
    let mut left = Vec::new();
    for comment in comments {
        let leave = |reason: String| LeftOut {
            issue: comment.item.clone(),
            comment: Some(comment.id.clone()),
            reason,
            outcome: LeftOutcome::Comment,
        };
        let Some(owned) = owners.get(&comment.item) else {
            left.push(leave(
                "no batch of this host filed the item, so none answers it".to_owned(),
            ));
            continue;
        };
        if pass.batch(&owned.batch).is_err() {
            left.push(leave(format!(
                "owning batch {} is not a batch id `batch_id_pattern` admits",
                owned.batch
            )));
            continue;
        }
        if let Some(marker) = marker(&comment.body, namespace) {
            left.push(leave(format!("marked by batch {}", marker.batch)));
            continue;
        }
        if comment.author_kind == Author::Bot {
            left.push(leave(format!(
                "bot author {}",
                comment.author.as_deref().unwrap_or("not named")
            )));
            continue;
        }
        let changed = instant(&comment.version);
        let answering = replies
            .get(&(comment.item.clone(), comment.id.clone()))
            .into_iter()
            .flatten()
            .filter(|(_, written, quoted)| match (quoted, written, changed) {
                (Some(quoted), _, Some(changed)) => *quoted >= changed,
                (None, Some(written), Some(changed)) => *written >= changed,
                _ => false,
            })
            .map(|(id, _, _)| id.clone())
            .next_back();
        if let Some(reply) = answering {
            left.push(leave(format!("answered by reply {reply}")));
            continue;
        }
        chosen.entry(owned.batch.clone()).or_default().push(Chosen {
            issue: comment.item.clone(),
            comment: comment.id.clone(),
            last_changed: comment.version.clone(),
            body_sha256: comments::body_sha256(&comment.body),
        });
    }
    Selection { chosen, left }
}

/// The comment version each reply this host's accounts record answered: by the issue and the
/// reply, the last change of the comment its feedback quoted.
fn quoted_versions(pass: &Pass<'_>) -> Result<BTreeMap<(String, String), DateTime<Utc>>, Error> {
    let mut quoted = BTreeMap::new();
    for directory in comments::snapshot_directories(&pass.work)? {
        let Ok(snapshotted) = Snapshotted::read(&directory) else {
            continue;
        };
        for (batch, chosen) in &snapshotted.snapshot.selection {
            let path = pass
                .work
                .feedback()
                .join(batch)
                .join(format!("{}.responses.json", snapshotted.stamp));
            let Ok(Some(responses)) = work::read::<Responses>(&path) else {
                continue;
            };
            for response in &responses.responses {
                if let Some(one) = chosen
                    .iter()
                    .find(|one| one.issue == response.issue && one.comment == response.comment)
                    && let Some(at) = instant(&one.last_changed)
                {
                    quoted.insert((response.issue.clone(), response.reply.clone()), at);
                }
            }
        }
    }
    Ok(quoted)
}

/// Every item at the other end of an edge of a relation with a rollup, from the `to` ticket's
/// side: a child of a parent relation, or the `from` of an incoming link of the relation.
fn related_items(pass: &Pass<'_>, relations: &[ItemRelations]) -> Vec<String> {
    let mut found = Vec::new();
    for held in relations {
        for relation in &pass.config.relations {
            if relation.rollup == Rollup::None {
                continue;
            }
            let ends: Vec<String> = match relation.store {
                RelationStore::Parent => held.children.clone(),
                RelationStore::Link => held
                    .links
                    .iter()
                    .filter(|link| link.link_type == relation.name && !link.outgoing)
                    .map(|link| link.other.clone())
                    .collect(),
            };
            for end in ends {
                if !found.contains(&end) {
                    found.push(end);
                }
            }
        }
    }
    found
}

/// Write the snapshot under `feedback/.snapshots/<stamp>[-<n>]/`, staged and renamed into place
/// whole, and answer its directory.
fn write_snapshot(
    pass: &Pass<'_>,
    queried_at: &DateTime<Utc>,
    snapshot: &Snapshot,
    gathering: &Gathering,
) -> Result<std::path::PathBuf, Error> {
    let root = pass.work.snapshots();
    let stamp = queried_at.format(STAMP).to_string();
    let mut attempt = 0;
    let directory = loop {
        let name = if attempt == 0 {
            stamp.clone()
        } else {
            format!("{stamp}-{attempt}")
        };
        let candidate = root.join(&name);
        if !candidate.exists() {
            break candidate;
        }
        attempt += 1;
    };
    let staged = root.join(format!(
        ".staging-{}",
        directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    if staged.exists() {
        std::fs::remove_dir_all(&staged).map_err(|error| {
            Error::Failed(format!(
                "{}: could not be cleared: {error}",
                staged.display()
            ))
        })?;
    }
    work::write_text(&staged.join(SNAPSHOT_RECORD), &snapshot.render())?;
    work::write_text(&staged.join(GATHERING_RECORD), &gathering.render())?;
    std::fs::create_dir_all(staged.join(LAST_WRITTEN)).map_err(|error| {
        Error::Failed(format!(
            "{}: could not be created: {error}",
            staged.display()
        ))
    })?;
    for written in &gathering.last_written {
        work::write_text(
            &staged.join(last_written_file(&written.item)),
            &last_written_text(written),
        )?;
    }
    std::fs::rename(&staged, &directory).map_err(|error| {
        Error::Failed(format!(
            "{}: the snapshot could not be put in place: {error}",
            directory.display()
        ))
    })?;
    Ok(directory)
}
