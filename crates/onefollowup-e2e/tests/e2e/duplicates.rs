//! Duplicate handling and its safeguards, decided by onefollowup itself rather than by the
//! scripted agent's choices: `duplicates: agent` matching nothing by field, `duplicates:
//! {same: [...]}` matching an open item or a staged ticket by its listed values, and every
//! refusal of `ticket copy` and `check` writing nothing to the destination.

use serde_json::{Value, json};

use crate::journey::{comments, record};
use crate::world::{Ticket, World, answer, filed_item};

const BATCH: &str = "nightly-2026-11-03";

/// A host whose `security` kind finds duplicates by `error_signature`.
const SAME: &str = "host_label: build-host-1
fields:
  error_signature:
    description: The error's signature, as the log prints it.
kinds:
  - name: root-cause
    description: One root cause.
  - name: security
    description: A security finding.
    duplicates: {same: [error_signature]}
";

fn security(slug: &str, title: &str, drafts: &[&str], signature: &str) -> Ticket {
    let mut ticket = Ticket::new(slug, title, drafts);
    ticket.kind = Some("security".to_owned());
    ticket.fields = vec![("error_signature".to_owned(), signature.to_owned())];
    ticket
}

#[test]
fn under_duplicates_agent_one_ticket_names_both_drafts_and_equal_root_causes_stay_two_tickets() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    world.seed("followups", 30);
    let first = world.draft(
        BATCH,
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Lost layout.\n",
    );
    let second = world.draft(
        BATCH,
        "the layout resets after a resize",
        &["src/window.rs"],
        "Layout reset.\n",
    );
    let third = world.draft(
        BATCH,
        "the palette resets after a resize",
        &["src/palette.rs"],
        "Palette reset.\n",
    );
    let fourth = world.draft(
        BATCH,
        "the toolbar resets after a resize",
        &["src/toolbar.rs"],
        "Toolbar reset.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");

    // Two drafts the agent judges one root cause: one ticket naming both.
    world.ticket(
        BATCH,
        &Ticket::new(
            "layout-save-reads-size-twice",
            "a layout save reads one window size",
            &[&first, &second],
        ),
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "copy",
            "--batch",
            BATCH,
            "layout-save-reads-size-twice",
        ],
        "",
    );
    // Two tickets whose root cause reads the same, word for word: nothing matches them by field.
    let mut palette = Ticket::new(
        "palette-resets-on-resize",
        "the palette resets after a resize",
        &[&third],
    );
    let mut toolbar = Ticket::new(
        "toolbar-resets-on-resize",
        "the toolbar resets after a resize",
        &[&fourth],
    );
    palette.cause = "A resize resets the user's saved arrangement.".to_owned();
    toolbar.cause = palette.cause.clone();
    for ticket in [&palette, &toolbar] {
        world.ticket(BATCH, ticket);
        let staged = world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
        assert_eq!(staged["write"]["op"], "copy", "{staged}");
        assert_eq!(
            staged.get("matched"),
            None,
            "nothing is matched by field: {staged}"
        );
    }
    world.answer(
        BATCH,
        &[
            answer(
                &first,
                "root-cause",
                "filed",
                &["layout-save-reads-size-twice"],
                "Verified.",
            ),
            answer(
                &second,
                "root-cause",
                "filed",
                &["layout-save-reads-size-twice"],
                "The same root cause.",
            ),
            answer(
                &third,
                "root-cause",
                "filed",
                &["palette-resets-on-resize"],
                "Verified.",
            ),
            answer(
                &fourth,
                "root-cause",
                "filed",
                &["toolbar-resets-on-resize"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let filed = world.filed("followups");
    assert_eq!(
        filed.len(),
        3,
        "one ticket for the two drafts, and one for each equal root cause"
    );
    let joined = filed
        .iter()
        .map(|(_, text)| record(text))
        .find(|held| held["root_cause"] == "layout-save-reads-size-twice")
        .expect("the joined ticket");
    assert_eq!(joined["drafts"], json!([first, second]));
}

#[test]
fn under_same_an_open_item_with_the_values_takes_the_evidence_and_an_empty_value_never_merges() {
    let mut world = World::new(&["followups"], SAME);
    world.seed("followups", 30);
    world.item(
        "followups",
        "token-leak",
        &filed_item(
            "widgets: tokens leak into the request log",
            "nightly-2026-10-01",
            "tokens-leak-into-logs",
            &json!({"kind": "security", "fields": {"error_signature": "E-1042"}}),
            "medium",
        ),
        "## Root cause\n\nTokens leak.\n\n## Fields\n\n- error_signature: E-1042\n\n## Evidence\n\nSeen once.\n",
    );
    let leaked = world.draft(
        BATCH,
        "a session token is printed",
        &["src/log.rs"],
        "Token in the log.\n",
    );
    let blank = world.draft(
        BATCH,
        "a stack trace prints a secret",
        &["src/trace.rs"],
        "Secret in a trace.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");

    // The scripted agent files each draft as a new ticket; onefollowup matches the first.
    world.ticket(
        BATCH,
        &security(
            "session-token-printed",
            "a session token is printed",
            &[&leaked],
            "  e-1042 ",
        ),
    );
    let staged = world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "session-token-printed"],
        "",
    );
    assert_eq!(
        staged["matched"], "followups:token-leak",
        "the answer names the item: {staged}"
    );
    assert_eq!(
        staged["write"]["op"], "comment",
        "evidence, not a new card: {staged}"
    );
    assert_eq!(staged["write"]["item"], "followups:token-leak");
    assert_eq!(staged["write"]["root_cause"], "tokens-leak-into-logs");
    assert!(
        staged["write"]["body"]
            .as_str()
            .is_some_and(|body| body.contains("the run's log shows a session token is printed")),
        "the ticket's evidence: {staged}"
    );
    world.ticket(
        BATCH,
        &security(
            "stack-trace-prints-secret",
            "a stack trace prints a secret",
            &[&blank],
            "",
        ),
    );
    let staged = world.ok(
        "agent",
        &[
            "ticket",
            "copy",
            "--batch",
            BATCH,
            "stack-trace-prints-secret",
        ],
        "",
    );
    assert_eq!(
        staged["write"]["op"], "copy",
        "an empty value never merges: {staged}"
    );
    world.answer(
        BATCH,
        &[
            answer(
                &leaked,
                "security",
                "filed",
                &["session-token-printed"],
                "Evidence on the open item.",
            ),
            answer(
                &blank,
                "security",
                "filed",
                &["stack-trace-prints-secret"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let filed = world.filed("followups");
    assert_eq!(
        filed.len(),
        2,
        "the matched ticket made no card: {filed:#?}"
    );
    let leak = world.read_item("followups", "token-leak");
    assert!(
        comments(&leak).contains("root_cause=\"tokens-leak-into-logs\""),
        "{leak}"
    );
}

#[test]
fn copy_refuses_an_unsound_ticket_and_check_refuses_an_unsound_batch_writing_nothing() {
    let mut world = World::new(&["followups"], SAME);
    world.seed("followups", 30);
    let one = world.draft(
        BATCH,
        "a session token is printed",
        &["src/log.rs"],
        "Token.\n",
    );
    let two = world.draft(
        BATCH,
        "a cookie is printed",
        &["src/cookie.rs"],
        "Cookie.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let before = world.items("followups");

    // `ticket copy` refuses what `ticket validate` refuses, staging nothing and receipting it.
    let mut missing = Ticket::new("missing-impact", "a session token is printed", &[&one]);
    missing.without = vec!["Impact".to_owned()];
    let mut broken = Ticket::new("broken-record", "a session token is printed", &[&one]);
    broken.extra = vec![("schema".to_owned(), json!(13))];
    let mut undeclared = Ticket::new("undeclared-field", "a session token is printed", &[&one]);
    undeclared.fields = vec![("client".to_owned(), "acme".to_owned())];
    for (ticket, says) in [
        (&missing, "missing the `## Impact` section"),
        (&broken, "the ticket record is schema 13"),
        (
            &undeclared,
            "the field `client`, which `fields` does not declare",
        ),
    ] {
        world.ticket(BATCH, ticket);
        let validated = world.ok(
            "agent",
            &["ticket", "validate", "--batch", BATCH, &ticket.slug],
            "",
        );
        assert_eq!(validated["sound"], false, "{validated}");
        let refused = world.fails(2, &["ticket", "copy", "--batch", BATCH, &ticket.slug], "");
        assert!(refused.contains(says), "{refused}");
    }
    let outbox = world.work().join(format!("outbox/{BATCH}.json"));
    assert!(!outbox.exists(), "nothing was staged");
    let receipts = world.receipts(BATCH);
    assert_eq!(receipts.len(), 3);
    for (receipt, slug) in
        receipts
            .iter()
            .zip(["missing-impact", "broken-record", "undeclared-field"])
    {
        assert_eq!(receipt["command"], "copy");
        assert_eq!(receipt["ticket"], format!("drafts:{BATCH}/tickets/{slug}"));
        assert_eq!(receipt["card"], Value::Null);
        assert_eq!(receipt["outcome"]["action"], "failed");
        assert_eq!(receipt["outcome"]["error"]["kind"], "invalid");
    }
    for slug in ["missing-impact", "broken-record"] {
        std::fs::remove_file(
            world
                .work()
                .join(format!("tasks/{BATCH}/tickets/{slug}.md")),
        )
        .expect("removed");
    }

    // Two tickets of a `same` kind with equal values: the second is that staged ticket, and
    // `check` refuses the batch while both stand.
    world.ticket(
        BATCH,
        &security(
            "token-printed",
            "a session token is printed",
            &[&one],
            "E-7",
        ),
    );
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "token-printed"],
        "",
    );
    world.ticket(
        BATCH,
        &security("cookie-printed", "a cookie is printed", &[&two], " e-7"),
    );
    let merged = world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "cookie-printed"],
        "",
    );
    assert_eq!(
        merged["matched"],
        format!("drafts:{BATCH}/tickets/token-printed"),
        "{merged}"
    );
    world.answer(
        BATCH,
        &[
            answer(&one, "security", "filed", &[], "Verified."),
            answer(&two, "security", "filed", &["no-such-ticket"], "Verified."),
        ],
    );
    // A staged ticket broken after staging.
    let mut edited = security(
        "token-printed",
        "a session token is printed",
        &[&one],
        "E-7",
    );
    edited.without = vec!["Evidence".to_owned()];
    world.ticket(BATCH, &edited);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], false);
    let problems: Vec<String> = checked["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|problem| {
            format!(
                "{}: {}",
                problem["subject"].as_str().unwrap_or_default(),
                problem["message"].as_str().unwrap_or_default()
            )
        })
        .collect();
    for expected in [
        format!("{one}: the disposition `filed` requires a ticket, and the answer names none"),
        format!("{two}: the answer names the ticket `no-such-ticket`, which has neither a staged write nor an evidence comment in this batch"),
        "ticket:token-printed: the body is missing the `## Evidence` section its ticket template requires".to_owned(),
        "ticket:undeclared-field: the ticket records the field `client`, which `fields` does not declare".to_owned(),
        "ticket:token-printed: the tickets `cookie-printed` and `token-printed` of the kind `security` carry equal `error_signature`".to_owned(),
    ] {
        assert!(problems.iter().any(|problem| problem.starts_with(&expected)), "{expected}\n{problems:#?}");
    }
    let refused = world.fails(2, &["publish", "--batch", BATCH], "");
    assert!(refused.contains("staged or changed since"), "{refused}");
    assert_eq!(
        world.items("followups"),
        before,
        "nothing reached the destination"
    );
}
