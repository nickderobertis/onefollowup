//! `comments answer`: the comment pass in one verb — gather exactly as `comments gather`, plan
//! the launches exactly as `comments launches`, run `comments.launch_command` once per launch,
//! and settle exactly as `comments settle`, but only when every batch's command exited 0.
//!
//! Each step is the verb it names, so its state is the snapshot those verbs keep in this
//! host's working files and nothing is held between them in the host. A batch whose command
//! fails does not stop the others; the pass then settles nothing, so the watermark holds and a
//! re-run gathers afresh, as a re-run of the three verbs does.

use serde_json::json;

use crate::api::{
    AnsweredLaunch, AnsweredPass, CommentsAnswerArgs, CommentsAnswered, CommentsGatherArgs,
    CommentsLaunchesArgs, CommentsSettleArgs, Launch, SCHEMA_VERSION,
};
use crate::config::{LAUNCH_PLACEHOLDERS, Settings};
use crate::error::Error;
use crate::verify::command;

/// Answer people's comments in one pass.
///
/// # Errors
///
/// No `comments.launch_command`, refused before anything is gathered; what gather, launches
/// or settle refuses with; or, with nothing settled, a batch whose command did not exit 0.
pub fn run(settings: &Settings, args: CommentsAnswerArgs) -> Result<CommentsAnswered, Error> {
    let Some(argv) = settings.config().comments.launch_command.clone() else {
        return Err(Error::Refused(format!(
            "`comments.launch_command` is not configured: set it to the argv that answers one batch's feedback ({} are replaced in it); nothing was gathered",
            LAUNCH_PLACEHOLDERS.map(|one| format!("`{one}`")).join(", ")
        )));
    };
    let gathered = crate::api::comments_gather(
        settings,
        CommentsGatherArgs {
            batch: None,
            item: None,
            dry_run: args.dry_run,
        },
    )?;
    if args.dry_run {
        return Ok(CommentsAnswered::DryRun(gathered));
    }
    let snapshot = gathered.snapshot.clone().ok_or_else(|| {
        Error::Failed("the gathering named no snapshot, so nothing was launched".to_owned())
    })?;
    let planned = crate::api::comments_launches(
        settings,
        CommentsLaunchesArgs {
            snapshot: snapshot.clone(),
        },
    )?;

    let mut launches = Vec::new();
    let mut failed = Vec::new();
    for launch in &planned.launches {
        match launched(settings, &argv, launch, &planned.snapshot) {
            Ok(ran) if ran.exit == 0 => launches.push(AnsweredLaunch {
                batch: launch.batch.clone(),
                feedback: launch.feedback.clone(),
                exit: ran.exit,
            }),
            Ok(ran) => {
                let said = ran
                    .stderr
                    .lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(|line| format!(": {}", line.trim()))
                    .unwrap_or_default();
                failed.push(format!("`{}` (exit {}{said})", launch.batch, ran.exit));
            }
            Err(error) => failed.push(format!("`{}` ({error})", launch.batch)),
        }
    }
    if !failed.is_empty() {
        return Err(Error::Failed(format!(
            "`comments.launch_command` failed for batch {}; nothing was settled from the snapshot {}, so the watermark holds: run `onefollowup comments answer` again to gather afresh",
            failed.join(", batch "),
            planned.snapshot.display()
        )));
    }

    let mut settled = crate::api::comments_settle(
        settings,
        CommentsSettleArgs {
            snapshot,
            launched: Vec::new(),
        },
    )?;
    let mut store_operations = gathered.store_operations;
    store_operations.append(&mut settled.store_operations);
    Ok(CommentsAnswered::Settled(AnsweredPass {
        schema_version: SCHEMA_VERSION,
        snapshot: planned.snapshot,
        launches,
        settled,
        store_operations,
    }))
}

/// Run `argv` for one launch, its placeholders replaced and the launch on its stdin: what it
/// did, or why it could not run.
fn launched(
    settings: &Settings,
    argv: &[String],
    launch: &Launch,
    snapshot: &std::path::Path,
) -> Result<command::Ran, String> {
    let batch = launch.batch.clone();
    let feedback = launch.feedback.display().to_string();
    let snapshot = snapshot.display().to_string();
    let values = [batch.as_str(), feedback.as_str(), snapshot.as_str()];
    let given: Vec<String> = argv
        .iter()
        .map(|one| {
            LAUNCH_PLACEHOLDERS
                .iter()
                .zip(values)
                .fold(one.clone(), |held, (placeholder, value)| {
                    held.replace(placeholder, value)
                })
        })
        .collect();
    let document = json!({
        "schema_version": SCHEMA_VERSION,
        "batch": batch,
        "feedback": feedback,
        "snapshot": snapshot,
    });
    command::run(
        &given,
        &document.to_string(),
        settings.working_directory(),
        settings.environment(),
    )
}
