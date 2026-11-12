//! What the comment-answering journeys share: tickets filed through the verify loop, people's
//! comments written into a destination's local-md items as a person writing through the store
//! would leave them, and the answering agent played by scripted steps that read the feedback
//! file it is given, post each reply to the item and write its account.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::world::{Ticket, World, answer};

/// The destination every comment journey answers on unless it names another.
pub const DESTINATION: &str = "followups";

/// The time now, as a comment's timestamps spell it.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// File one ticket per slug for `batch` through the verify loop, and answer each one's card.
pub fn file(world: &mut World, batch: &str, slugs: &[&str]) -> Vec<String> {
    let mut drafts = Vec::new();
    for slug in slugs {
        drafts.push(world.draft(
            batch,
            &format!("the {slug} breaks saving"),
            &["src/layout.rs"],
            "Seen while saving a layout.\n",
        ));
    }
    world.ok("file", &["prepare", "--batch", batch], "");
    let mut answers = Vec::new();
    for (slug, draft) in slugs.iter().zip(&drafts) {
        world.ticket(
            batch,
            &Ticket::new(slug, &format!("the {slug} breaks saving"), &[draft]),
        );
        world.ok("file", &["ticket", "copy", "--batch", batch, slug], "");
        answers.push(answer(draft, "root-cause", "filed", &[slug], "Verified."));
    }
    world.answer(batch, &answers);
    let checked = world.ok("file", &["check", "--batch", batch], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("file", &["publish", "--batch", batch], "");
    world.ok("file", &["settle", "--batch", batch], "");
    slugs.iter().map(|slug| card(world, batch, slug)).collect()
}

/// The card `batch`'s ticket `slug` was copied to.
pub fn card(world: &World, batch: &str, slug: &str) -> String {
    world
        .receipts(batch)
        .iter()
        .rev()
        .find(|receipt| {
            receipt["command"] == "copy"
                && receipt["ticket"]
                    .as_str()
                    .is_some_and(|ticket| ticket.contains(&format!("-{slug}-")))
                && receipt["card"].is_string()
        })
        .and_then(|receipt| receipt["card"].as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("batch {batch} copied {slug}: {:?}", world.receipts(batch)))
}

/// The file of the qualified item `item`.
pub fn item_path(world: &World, item: &str) -> PathBuf {
    let (source, native) = item.split_once(':').expect("a qualified item");
    world.root.join(format!("{source}-root/tasks/{native}.md"))
}

/// The text of the qualified item `item`.
pub fn item_text(world: &World, item: &str) -> String {
    std::fs::read_to_string(item_path(world, item)).expect("the item is there")
}

/// Add a comment to `item`, as the store writes one to a local-md item: `author` writing
/// `body` at `at`, with the id `id`.
pub fn comment(world: &World, item: &str, id: &str, author: &str, at: &str, body: &str) {
    let path = item_path(world, item);
    let mut text = std::fs::read_to_string(&path).expect("the item is there");
    if !text.contains("\n## Comments\n") {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("\n## Comments\n");
    }
    text.push_str(&format!(
        "\n<!-- onetaskgraph:comment id=\"{id}\" author=\"{author}\" created_at=\"{at}\" updated_at=\"{at}\" -->\n### {author} — {at}\n\n{body}\n\n<!-- /onetaskgraph:comment -->\n"
    ));
    std::fs::write(&path, text).expect("the comment is written");
}

/// Remove the comment `id` from `item`, as a person deleting it would.
pub fn remove_comment(world: &World, item: &str, id: &str) {
    let path = item_path(world, item);
    let text = std::fs::read_to_string(&path).expect("the item is there");
    let open = format!("\n<!-- onetaskgraph:comment id=\"{id}\"");
    let at = text.find(&open).expect("the comment is there");
    let close = "<!-- /onetaskgraph:comment -->\n";
    let end = text[at..].find(close).expect("the comment closes") + at + close.len();
    std::fs::write(&path, format!("{}{}", &text[..at], &text[end..])).expect("removed");
}

/// How many replies on `item` answer the comment `id`.
pub fn replies_to(world: &World, item: &str, id: &str) -> usize {
    item_text(world, item)
        .matches(&format!("kind=\"reply\" answers=\"{id}\""))
        .count()
}

/// One comment a feedback file quotes, as the answering agent reads it there.
#[derive(Debug, Clone)]
pub struct Quoted {
    /// The issue it sits on.
    pub issue: String,
    /// Its id.
    pub comment: String,
    /// The ticket file it names.
    pub ticket: PathBuf,
    /// What onefollowup last wrote of the issue, as it names it.
    pub last_written: PathBuf,
    /// The reply's opening line.
    pub opening: String,
    /// The marker of a reply that confirms.
    pub confirms: String,
    /// The marker of a reply that does not.
    pub does_not_confirm: String,
}

fn ticked(line: &str, label: &str) -> Option<String> {
    line.strip_prefix(&format!("- {label}: `"))
        .and_then(|rest| rest.strip_suffix('`'))
        .map(str::to_owned)
}

/// Every comment the feedback file at `feedback` quotes, in order.
pub fn quoted(feedback: &Path) -> Vec<Quoted> {
    let text = std::fs::read_to_string(feedback).expect("the feedback file");
    let mut found: Vec<Quoted> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("<!-- orchestrator:follow-up-quoted issue=\"") {
            let (issue, rest) = rest.split_once('"').expect("an issue");
            let comment = rest
                .strip_prefix(" comment=\"")
                .and_then(|rest| rest.split_once('"'))
                .map(|(comment, _)| comment.to_owned())
                .expect("a comment");
            found.push(Quoted {
                issue: issue.to_owned(),
                comment,
                ticket: PathBuf::new(),
                last_written: PathBuf::new(),
                opening: String::new(),
                confirms: String::new(),
                does_not_confirm: String::new(),
            });
            continue;
        }
        let Some(last) = found.last_mut() else {
            continue;
        };
        if let Some(value) = ticked(line, "Its ticket") {
            last.ticket = PathBuf::from(value);
        } else if let Some(value) = ticked(line, "What onefollowup last wrote of it") {
            last.last_written = PathBuf::from(value);
        } else if let Some(value) = ticked(line, "Reply opening") {
            last.opening = value;
        } else if let Some(value) = ticked(line, "Marker if it confirms") {
            last.confirms = value;
        } else if let Some(value) = ticked(line, "Marker if it does not") {
            last.does_not_confirm = value;
        }
    }
    found
}

/// Post the reply to `quoted` the feedback file shapes, carrying `verdict`, and answer its id.
pub fn post_reply(world: &World, quoted: &Quoted, verdict: &str, text: &str) -> String {
    let marker = if verdict == "confirms" {
        &quoted.confirms
    } else {
        &quoted.does_not_confirm
    };
    let id = format!("{}-reply", quoted.comment);
    comment(
        world,
        &quoted.issue,
        &id,
        "follow-up-agent",
        &now(),
        &format!("{}\n\n{text}\n\n{marker}", quoted.opening),
    );
    id
}

/// One response of an account.
pub fn response(quoted: &Quoted, reply: &str, verdict: &str, action: &str) -> Value {
    json!({
        "comment": quoted.comment,
        "issue": quoted.issue,
        "action": action,
        "reply": reply,
        "reply_url": null,
        "reply_fallback": null,
        "verdict": verdict,
    })
}

/// Write `batch`'s account of the feedback file at `feedback`.
pub fn account(feedback: &Path, batch: &str, responses: &[Value]) {
    let stem = feedback
        .file_stem()
        .expect("a stem")
        .to_string_lossy()
        .into_owned();
    std::fs::write(
        feedback.with_file_name(format!("{stem}.responses.json")),
        serde_json::to_string_pretty(&json!({
            "schema": 3,
            "run": batch,
            "feedback": format!("{stem}.md"),
            "responses": responses,
        }))
        .expect("JSON")
            + "\n",
    )
    .expect("the account is written");
}

/// Answer every comment the feedback file quotes with a reply carrying `verdict_of` its
/// comment, and account for each — the whole of a sound dispatch that changes no ticket.
pub fn answer_all(
    world: &World,
    batch: &str,
    feedback: &Path,
    verdict_of: &dyn Fn(&Quoted) -> &'static str,
) -> Vec<Quoted> {
    let quoted = quoted(feedback);
    let mut responses = Vec::new();
    for one in &quoted {
        let verdict = verdict_of(one);
        let reply = post_reply(
            world,
            one,
            verdict,
            "Thank you: the ticket stands as it is written.",
        );
        responses.push(response(
            one,
            &reply,
            verdict,
            "Read the ticket against the comment.",
        ));
    }
    account(feedback, batch, &responses);
    quoted
}

/// Each launch a `comments launches` printed, as `(batch, feedback file)`.
pub fn launches(printed: &Value) -> Vec<(String, PathBuf)> {
    printed["launches"]
        .as_array()
        .expect("launches")
        .iter()
        .map(|launch| {
            (
                launch["batch"].as_str().expect("a batch").to_owned(),
                PathBuf::from(launch["feedback"].as_str().expect("a feedback file")),
            )
        })
        .collect()
}

/// The destination's watermark, when one was written.
pub fn watermark(world: &World, destination: &str) -> Option<Value> {
    std::fs::read_to_string(
        world
            .work()
            .join(format!("feedback/.watermark-{destination}.json")),
    )
    .ok()
    .map(|text| serde_json::from_str(&text).expect("JSON"))
}

/// The pending writes, when there are any.
pub fn pending(world: &World) -> Vec<Value> {
    std::fs::read_to_string(world.work().join("feedback/pending.json"))
        .ok()
        .map(|text| {
            serde_json::from_str::<Value>(&text).expect("JSON")["entries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

/// Replace `from` with `to` in a ticket file, as the answering agent editing it would.
pub fn edit_ticket(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).expect("the ticket");
    assert!(text.contains(from), "{} holds {from:?}", path.display());
    std::fs::write(path, text.replacen(from, to, 1)).expect("edited");
}

/// Withdraw a ticket file, as the answering agent does: `status: cancelled`.
pub fn withdraw_ticket(path: &Path) {
    let text = std::fs::read_to_string(path).expect("the ticket");
    let text = text.replacen("---\ntitle:", "---\nstatus: cancelled\ntitle:", 1);
    std::fs::write(path, text).expect("withdrawn");
}

/// The destination operations of the phases kept since the last reset, as `op` names.
pub fn destination_ops(world: &World) -> Vec<String> {
    world
        .destination_operations()
        .into_iter()
        .map(|(_, op, _, _)| op)
        .collect()
}
