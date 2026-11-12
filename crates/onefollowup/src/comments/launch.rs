//! `comments launches`: claim a snapshot once and write one feedback file per batch it selected
//! comments for, from the snapshot alone — no destination read.
//!
//! Refused, with nothing written, for a snapshot already launched; for a dry run's snapshot
//! older than `snapshot.launchable_for`; and for a snapshot whose gathering preceded the launch
//! of another snapshot sharing any of its comments, in either order — each naming the fresh
//! dry run to take. So no comment is dispatched from two snapshots.

use chrono::Utc;
use serde_json::{Map, Value, json};

use crate::api::{CommentsLaunchesArgs, Launch, Launches, SCHEMA_VERSION};
use crate::comments::{
    LAUNCHED_RECORD, Pass, Snapshotted, instant, last_written_file, precise, snapshot_directories,
    span, write_once,
};
use crate::config::Settings;
use crate::error::Error;
use crate::formats::Versioned as _;
use crate::formats::markers::{
    CommentMarker, FeedbackBoundary, FeedbackSnapshot, IssueLine, QuotedAnchor, ReplyTo,
    reply_opening,
};
use crate::formats::snapshot::{Launch as LaunchKind, Launched};
use crate::formats::{Verdict, snapshot::Chosen};
use crate::verify::template;
use crate::work;

/// What a fresh dry run is, as a refusal names it.
const FRESH: &str =
    "take a fresh `onefollowup comments gather --dry-run` and launch the snapshot it prints";

/// Plan a snapshot's launches.
///
/// # Errors
///
/// A snapshot that does not read, or one refused as above; a template that cannot be
/// rendered; or a working file that cannot be written.
pub fn run(settings: &Settings, args: CommentsLaunchesArgs) -> Result<Launches, Error> {
    let pass = Pass::new(settings)?;
    let directory = pass.given(&args.snapshot);
    let snapshotted = Snapshotted::read(&directory)?;
    let now = Utc::now();
    if let Some(launched) = snapshotted.launched()? {
        return Err(Error::Refused(format!(
            "the snapshot {} was already launched at {}, and its comments are answered from that launch; {FRESH}",
            directory.display(),
            launched.claimed_at
        )));
    }
    let gathered_at = instant(&snapshotted.snapshot.queried_at).ok_or_else(|| {
        Error::Failed(format!(
            "{}: `queried_at` is not an instant: {}",
            directory.display(),
            snapshotted.snapshot.queried_at
        ))
    })?;
    if snapshotted.snapshot.dry_run {
        let window = span(&pass.config.snapshot.launchable_for).unwrap_or_default();
        if now - gathered_at > window {
            return Err(Error::Refused(format!(
                "the snapshot {} was gathered at {}, longer ago than `snapshot.launchable_for` ({}), so what it read may since have been answered; {FRESH}",
                directory.display(),
                snapshotted.snapshot.queried_at,
                pass.config.snapshot.launchable_for
            )));
        }
    }
    for other in snapshot_directories(&pass.work)? {
        if other == directory {
            continue;
        }
        let Ok(other) = Snapshotted::read(&other) else {
            continue;
        };
        let Some(launched) = other.launched()? else {
            continue;
        };
        let Some(shared) = shared(&snapshotted, &other) else {
            continue;
        };
        let claimed = instant(&launched.claimed_at);
        if claimed.is_some_and(|claimed| gathered_at <= claimed) {
            return Err(Error::Refused(format!(
                "the snapshot {} shares comment {} on {} with the snapshot {}, which was launched at {} after this one was gathered at {}, so launching this one would answer that comment twice; {FRESH}",
                directory.display(),
                shared.comment,
                shared.issue,
                other.directory.display(),
                launched.claimed_at,
                snapshotted.snapshot.queried_at
            )));
        }
    }

    // The claim, written once before anything else, so a second launch is refused.
    let claim = Launched {
        claimed_at: precise(&now),
        launch: if snapshotted.snapshot.dry_run {
            LaunchKind::From
        } else {
            LaunchKind::Fresh
        },
    };
    if !write_once(&directory.join(LAUNCHED_RECORD), &claim.render())? {
        return Err(Error::Refused(format!(
            "the snapshot {} was launched by another process just now; {FRESH}",
            directory.display()
        )));
    }

    let mut launches = Vec::new();
    for (batch_id, chosen) in &snapshotted.snapshot.selection {
        let batch = pass.batch(batch_id)?;
        // Every ticket of the batch as it stands at the launch, so `comments check` and settle
        // see which the answering agent changed.
        for ticket in batch.tickets()? {
            if let Some(text) = work::read_text(&ticket.path)? {
                work::write_text(&snapshotted.ticket_at_launch(batch_id, &ticket.slug), &text)?;
            }
        }
        let feedback = pass.work.feedback_file(batch_id, &snapshotted.stamp);
        let text = render(&pass, &batch, &snapshotted, chosen, &feedback)?;
        work::write_text(&feedback, &text)?;
        launches.push(Launch {
            batch: batch_id.clone(),
            feedback,
        });
    }
    Ok(Launches {
        schema_version: SCHEMA_VERSION,
        snapshot: directory,
        launches,
    })
}

/// A comment both snapshots selected, if any.
fn shared<'a>(one: &'a Snapshotted, other: &Snapshotted) -> Option<&'a Chosen> {
    one.snapshot.selection.values().flatten().find(|chosen| {
        other
            .snapshot
            .selection
            .values()
            .flatten()
            .any(|theirs| theirs.issue == chosen.issue && theirs.comment == chosen.comment)
    })
}

/// `text` verbatim inside a fence longer than any backtick run it holds.
fn fenced(text: &str) -> String {
    let longest = text
        .split(|one| one != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}text\n{}\n{fence}", text.trim_end())
}

/// The batch's feedback file: its boundary and snapshot anchors, then the task template
/// rendered in feedback mode.
fn render(
    pass: &Pass<'_>,
    batch: &crate::verify::context::Batch<'_>,
    snapshotted: &Snapshotted,
    chosen: &[Chosen],
    feedback: &std::path::Path,
) -> Result<String, Error> {
    let namespace = &pass.config.namespace;
    let mut comments = Vec::new();
    for one in chosen {
        let listed = snapshotted
            .gathering
            .listings
            .values()
            .flat_map(|listing| listing.comments.iter())
            .find(|listed| listed.item == one.issue && listed.id == one.comment)
            .ok_or_else(|| {
                Error::Failed(format!(
                    "{}: the selection quotes comment {} on {}, which the listing does not hold",
                    snapshotted.directory.display(),
                    one.comment,
                    one.issue
                ))
            })?;
        let written = snapshotted.last_written(&one.issue).ok_or_else(|| {
            Error::Failed(format!(
                "{}: the snapshot holds nothing onefollowup last wrote to {}",
                snapshotted.directory.display(),
                one.issue
            ))
        })?;
        let url = listed
            .url
            .clone()
            .unwrap_or_else(|| format!("{} (comment {})", one.issue, one.comment));
        let marker = |verdict| {
            CommentMarker {
                batch: batch.batch.clone(),
                root_cause: written.ticket.clone(),
                reply: Some(ReplyTo {
                    answers: one.comment.clone(),
                    verdict: Some(verdict),
                }),
            }
            .render(namespace)
        };
        comments.push(json!({
            "issue": one.issue,
            "comment": one.comment,
            "url": url,
            "author": listed.author.clone().unwrap_or_else(|| "an author the destination does not name".to_owned()),
            "version": one.last_changed,
            "body": fenced(&listed.body),
            "anchor": QuotedAnchor { issue: one.issue.clone(), comment: one.comment.clone() }.render(namespace),
            "issue_line": IssueLine {
                issue: one.issue.clone(),
                snapshot_id: last_written_file(&one.issue),
                version: written.version.clone(),
            }.render(),
            "last_written": snapshotted.directory.join(last_written_file(&one.issue)),
            "ticket": batch.work.ticket(&batch.batch, &written.ticket),
            "item_version": written.version,
            "reply_opening": reply_opening(&batch.batch, &url, listed.author.as_deref()),
            "marker_confirms": marker(Verdict::Confirms),
            "marker_does_not_confirm": marker(Verdict::DoesNotConfirm),
        }));
    }
    let stem = feedback
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let responses = feedback.with_file_name(format!("{stem}.responses.json"));
    let example = chosen.first().map_or_else(
        || json!({"comment": "<comment id>", "issue": "<issue>"}),
        |one| json!({"comment": one.comment, "issue": one.issue}),
    );
    let response_example = serde_json::to_string_pretty(&json!({
        "schema": 3,
        "run": batch.batch,
        "feedback": format!("{stem}.md"),
        "responses": [{
            "comment": example["comment"],
            "issue": example["issue"],
            "action": "<what you did about the comment, in your words>",
            "reply": "<the reply's id>",
            "reply_url": "<the reply's URL, or null>",
            "reply_fallback": null,
            "verdict": "confirms",
        }],
    }))
    .unwrap_or_default();
    let fields: Map<String, Value> = pass
        .config
        .fields
        .iter()
        .map(|(name, field)| (name.clone(), json!({"description": field.description})))
        .collect();
    let example_ticket = template::render(
        &batch.ticket_template(batch.kind(&pass.config.default_kind)),
        &json!({"rubric": pass.config.rubric, "fields": fields}),
    )?;
    let check = format!(
        "onefollowup comments check --feedback {}",
        feedback.display()
    );
    let task = template::render(
        &batch.task_template(),
        &json!({
            "mode": "feedback",
            "batch": batch.batch,
            "destination": pass.config.destination,
            "namespace": namespace,
            "statuses": pass.config.statuses,
            "rubric": pass.config.rubric,
            "estimate": pass.config.estimate,
            "fields": fields,
            "ticket_template": example_ticket,
            "tickets": batch.work.tickets(&batch.batch),
            "feedback": feedback,
            "responses": responses,
            "check": check,
            "snapshot": snapshotted.directory,
            "boundary": "none",
            "comments": comments,
            "response_example": response_example,
        }),
    )?;
    Ok(format!(
        "{}\n{}\n\n{}",
        FeedbackBoundary(None).render(namespace),
        FeedbackSnapshot(snapshotted.directory.display().to_string()).render(namespace),
        task
    ))
}
