//! `comments check`: hold a feedback dispatch's response account to its snapshot — one
//! response per quoted comment, in the order quoted, each on the issue it was quoted on, under
//! the batch that owns that issue, with its reply's id — and hold the batch's tickets to
//! changing only the tickets of quoted issues. It reads the account, the snapshot and the
//! batch's ticket files, never the destination: whether each reply is there is settle's to
//! verify.

use std::path::{Path, PathBuf};

use crate::api::{CommentsCheckArgs, CommentsChecked, SCHEMA_VERSION};
use crate::comments::{Pass, Snapshotted};
use crate::config::Settings;
use crate::error::Error;
use crate::formats::markers::FeedbackSnapshot;
use crate::formats::responses::Responses;
use crate::formats::snapshot::Chosen;
use crate::verify::ticket::TicketFile;
use crate::verify::tools;
use crate::work;

/// One feedback dispatch's account, held to its snapshot.
pub struct Account {
    /// The batch that answers.
    pub batch: String,
    /// The feedback file.
    pub feedback: PathBuf,
    /// The snapshot it was launched from.
    pub snapshotted: Snapshotted,
    /// The comments it quotes, in order.
    pub quoted: Vec<Chosen>,
    /// The account, when it reads.
    pub responses: Option<Responses>,
    /// The batch's tickets the agent changed since the launch, each of a quoted issue.
    pub changed: Vec<TicketFile>,
    /// Each rule the account or the tickets break.
    pub problems: Vec<String>,
}

impl Account {
    /// Whether it breaks no rule.
    #[must_use]
    pub fn sound(&self) -> bool {
        self.problems.is_empty()
    }
}

/// The response account beside a feedback file.
#[must_use]
pub fn responses_path(feedback: &Path) -> PathBuf {
    let stem = feedback
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    feedback.with_file_name(format!("{stem}.responses.json"))
}

/// Hold the account of the feedback file at `feedback` to its snapshot.
///
/// # Errors
///
/// A feedback file that is not one `comments launches` wrote, or a snapshot that does not
/// read.
#[allow(
    clippy::too_many_lines,
    reason = "one account: its feedback, its responses in order, then the tickets it changed"
)]
pub fn hold(pass: &Pass<'_>, feedback: &Path) -> Result<Account, Error> {
    let namespace = &pass.config.namespace;
    let text = work::read_text(feedback)?.ok_or_else(|| {
        Error::Refused(format!(
            "there is no feedback file {}: name one `onefollowup comments launches` printed",
            feedback.display()
        ))
    })?;
    let snapshot = text
        .lines()
        .find_map(|line| FeedbackSnapshot::parse(line, namespace).ok())
        .ok_or_else(|| {
            Error::Refused(format!(
                "{} names no snapshot on a `{namespace}:follow-up-feedback snapshot=` line, so it is no feedback file `onefollowup comments launches` wrote",
                feedback.display()
            ))
        })?;
    let snapshotted = Snapshotted::read(Path::new(&snapshot.0))?;
    let batch_id = feedback
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let batch = pass.batch(&batch_id)?;
    let quoted = snapshotted
        .snapshot
        .selection
        .get(&batch_id)
        .cloned()
        .ok_or_else(|| {
            Error::Refused(format!(
                "the snapshot {} quotes no comment for batch `{batch_id}`, so {} is none of its feedback",
                snapshotted.directory.display(),
                feedback.display()
            ))
        })?;
    let mut problems = Vec::new();
    let name = feedback
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let path = responses_path(feedback);
    let responses = match work::read::<Responses>(&path) {
        Ok(Some(responses)) => Some(responses),
        Ok(None) => {
            problems.push(format!(
                "there is no response account at {}: write one response per quoted comment, in the order quoted",
                path.display()
            ));
            None
        }
        Err(error) => {
            problems.push(format!("the response account does not read: {error}"));
            None
        }
    };
    if let Some(responses) = &responses {
        if responses.batch != batch_id {
            problems.push(format!(
                "the account names batch `{}`, but {name} is batch `{batch_id}`'s feedback: every reply is posted under the batch that owns the comment's issue",
                responses.batch
            ));
        }
        if responses.feedback != name {
            problems.push(format!(
                "the account names the feedback `{}`, and it answers `{name}`",
                responses.feedback
            ));
        }
        let mut next = 0;
        for response in &responses.responses {
            let at = quoted
                .iter()
                .position(|one| one.comment == response.comment && one.issue == response.issue);
            match at {
                None => {
                    let owner = snapshotted.owner(&response.issue);
                    match owner {
                        Some(owner) if owner != batch_id => problems.push(format!(
                            "the response for comment {} answers on {}, which batch `{owner}` owns, not `{batch_id}`: a reply is posted only by the batch owning the comment's issue",
                            response.comment, response.issue
                        )),
                        _ => problems.push(format!(
                            "the response for comment {} on {} answers a comment this feedback did not quote; remove it",
                            response.comment, response.issue
                        )),
                    }
                }
                Some(at) if at < next => problems.push(format!(
                    "comment {} on {} is answered twice or out of the order it is quoted in",
                    response.comment, response.issue
                )),
                Some(at) => {
                    for skipped in &quoted[next..at] {
                        problems.push(format!(
                            "quoted comment {} on {} has no response before comment {}: answer every quoted comment, in the order quoted",
                            skipped.comment, skipped.issue, response.comment
                        ));
                    }
                    next = at + 1;
                }
            }
            if response.reply.trim().is_empty() {
                problems.push(format!(
                    "the response for comment {} names no reply id: record the id the reply's write answered",
                    response.comment
                ));
            }
        }
        for missing in quoted.iter().skip(next) {
            if !responses.responses.iter().any(|response| {
                response.comment == missing.comment && response.issue == missing.issue
            }) {
                problems.push(format!(
                    "quoted comment {} on {} has no response: answer it and record it",
                    missing.comment, missing.issue
                ));
            }
        }
    }

    // The tickets: only a quoted issue's may change, and each that changed holds to the rules.
    let quoted_tickets: Vec<String> = quoted
        .iter()
        .filter_map(|one| snapshotted.last_written(&one.issue))
        .map(|written| written.ticket.clone())
        .collect();
    let mut changed = Vec::new();
    for ticket in batch.tickets()? {
        let at_launch = snapshotted.ticket_at_launch(&batch_id, &ticket.slug);
        let before = work::read_text(&at_launch)?;
        let now = work::read_text(&ticket.path)?;
        if before == now {
            continue;
        }
        if before.is_none() {
            problems.push(format!(
                "ticket `{}` was written after the launch, and no comment this feedback quotes sits on it: remove {}",
                ticket.slug,
                ticket.path.display()
            ));
            continue;
        }
        if !quoted_tickets.contains(&ticket.slug) {
            problems.push(format!(
                "ticket `{}` changed, but no comment this feedback quotes sits on its item: restore it as it stood at the launch ({})",
                ticket.slug,
                at_launch.display()
            ));
            continue;
        }
        for problem in tools::ticket_problems(&batch, None, &ticket) {
            problems.push(format!("ticket `{}`: {problem}", ticket.slug));
        }
        changed.push(ticket);
    }
    Ok(Account {
        batch: batch_id,
        feedback: feedback.to_path_buf(),
        snapshotted,
        quoted,
        responses,
        changed,
        problems,
    })
}

/// Check a feedback dispatch's response account.
///
/// # Errors
///
/// A feedback file `comments launches` did not write, or a snapshot that does not read.
pub fn run(settings: &Settings, args: CommentsCheckArgs) -> Result<CommentsChecked, Error> {
    let pass = Pass::new(settings)?;
    let feedback = pass.given(&args.feedback);
    let account = hold(&pass, &feedback)?;
    Ok(CommentsChecked {
        schema_version: SCHEMA_VERSION,
        feedback,
        sound: account.sound(),
        problems: account.problems,
    })
}
