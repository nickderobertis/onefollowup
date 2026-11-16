//! The edges of rendering through configured templates: every way a `--sources-file` or a part
//! file is refused with nothing staged; a render template or a projection that cannot render a
//! ticket refusing its `ticket copy`, receipted, with nothing staged; and a `same` duplicate's
//! merge staging the kind's `evidence_section` as its comment's evidence, with the ticket's
//! record as its sources, or refusing a ticket carrying no such section.

use serde_json::{Value, json};

use crate::world::{COMMIT, HOST, REPOSITORY, Ticket, World, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-10";
const ITEM: &str = "followups:export-older";

fn staged(world: &World) -> usize {
    std::fs::read_to_string(world.work().join(format!("outbox/{BATCH}.json")))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|outbox| outbox["writes"].as_array().map(Vec::len))
        .unwrap_or(0)
}

/// A world holding another batch's item and `files`, and this batch prepared with one draft
/// over it.
fn prepared(config: &str, files: &[(&str, &str)]) -> (World, String) {
    let mut world = World::new(&["followups"], config);
    for (name, text) in files {
        std::fs::write(world.root.join(name), text).expect("written");
    }
    world.item(
        "followups",
        "export-older",
        &filed_item(
            "widgets: the export dialog drops unicode titles",
            "nightly-2026-10-01",
            "export-dialog-drops-unicode",
            &json!({}),
            "medium",
        ),
        &filed_body("The export dialog drops unicode titles."),
    );
    let draft = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "café became caf.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    (world, draft)
}

#[test]
fn every_unsound_sources_or_part_file_is_refused_with_nothing_staged() {
    let (world, draft) = prepared("host_label: build-host-1\n", &[]);
    let parts = world.root.join("parts");
    std::fs::create_dir_all(&parts).expect("a directory");
    let sources = parts.join("sources.json").display().to_string();
    let missing = parts.join("missing.md").display().to_string();
    let other = "89abcdef0123456789abcdef0123456789abcdef";
    let comment = |extra: &[&str]| {
        let mut args = vec!["ticket", "comment", "--batch", BATCH, "--item", ITEM];
        args.extend_from_slice(extra);
        world.fails(2, &args, "Seen again.\n")
    };
    for (flag, says) in [
        ("--sources-file", "--sources-file"),
        ("--bears-on-file", "--bears-on-file"),
        ("--question-file", "--question-file"),
    ] {
        let refused = comment(&[flag, &missing]);
        assert!(
            refused.contains(&format!("{says} {missing} could not be read")),
            "{refused}"
        );
    }
    for (text, says) in [
        ("not JSON".to_owned(), "is not one JSON object holding exactly"),
        ("[]".to_owned(), "is not one JSON object holding exactly"),
        (
            json!({"host": "not a host!", "basis": {REPOSITORY: COMMIT}, "drafts": [draft], "witnesses": []}).to_string(),
            "is not a host name",
        ),
        (
            json!({"host": HOST, "basis": [REPOSITORY], "drafts": [draft], "witnesses": []}).to_string(),
            "`basis` is not an object of origin to commit",
        ),
        (
            json!({"host": HOST, "basis": {}, "drafts": [draft], "witnesses": []}).to_string(),
            "`basis` names no repository",
        ),
        (
            json!({"host": HOST, "basis": {"not an origin": "abc"}, "drafts": [draft], "witnesses": []}).to_string(),
            "is not a 40-character commit",
        ),
        (
            json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": draft, "witnesses": []}).to_string(),
            "`drafts` is not a list of draft ids",
        ),
        (
            json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [draft], "witnesses": "one"}).to_string(),
            "`witnesses` is not a list of witnesses",
        ),
        (
            json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [draft], "witnesses": ["src/export.rs:3"]}).to_string(),
            "is not `<origin> <path>:<line> at <commit>`",
        ),
        (
            json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [draft], "witnesses": [format!("github.com/example/gadgets src/a.rs:1 at {other}")]}).to_string(),
            "names `github.com/example/gadgets`, which the basis does not",
        ),
    ] {
        std::fs::write(&sources, &text).expect("written");
        let refused = comment(&["--sources-file", &sources]);
        assert!(refused.contains(says), "{text}: {refused}");
        assert_eq!(staged(&world), 0, "{text}");
    }
    // An empty part is no part: the comment stages with neither paragraph.
    std::fs::write(parts.join("empty.md"), "\n").expect("written");
    let empty = parts.join("empty.md").display().to_string();
    let mut world = world;
    let staged_comment = world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            BATCH,
            "--item",
            ITEM,
            "--bears-on-file",
            &empty,
        ],
        "Seen again.\n",
    );
    assert_eq!(
        staged_comment["write"]["body"],
        format!(
            "Additional evidence from follow-up run `{BATCH}`.\n\nSeen again.\n\n<!-- orchestrator:follow-up-comment run=\"{BATCH}\" root_cause=\"export-dialog-drops-unicode\" -->\n"
        )
    );
}

#[test]
fn a_template_that_cannot_render_a_ticket_refuses_its_copy() {
    for (key, template) in [
        (
            "render_template: broken.md.j2",
            "{{ ticket.title | no_such_filter }}",
        ),
        (
            "projections: {broken: \"{{ no_such_function() }}\"}",
            "## unused\n",
        ),
    ] {
        let (mut world, draft) = prepared(
            &format!(
                "host_label: build-host-1\nkinds:\n  - name: root-cause\n    description: One root cause.\n    {key}\n"
            ),
            &[("broken.md.j2", template)],
        );
        let slug = "export-dialog-drops-unicode-again";
        world.ticket(
            BATCH,
            &Ticket::new(slug, "the export dialog drops unicode titles", &[&draft]),
        );
        let refused = world.fails(2, &["ticket", "copy", "--batch", BATCH, slug], "");
        assert!(
            refused.contains("does not render, so nothing was staged"),
            "{key}: {refused}"
        );
        assert!(
            refused.contains("broken"),
            "the template is named: {refused}"
        );
        assert_eq!(staged(&world), 0, "{key}");
        let receipts = world.receipts(BATCH);
        let last = receipts.last().expect("the refusal is receipted");
        assert_eq!(
            (
                &last["outcome"]["action"],
                &last["outcome"]["error"]["kind"]
            ),
            (&json!("failed"), &json!("invalid")),
            "{key}: {last}"
        );
        assert!(
            !refused.contains("Referenced variables"),
            "the refusal names the error, not every variable: {refused}"
        );
        world.reset_phases();
    }
}

#[test]
fn a_same_duplicate_stages_its_kinds_evidence_section_with_its_records_sources() {
    let config = |section: &str| {
        format!(
            "host_label: build-host-1
fields:
  error_signature: {{description: The error's signature.}}
comment_templates:
  evidence: evidence.md.j2
kinds:
  - name: root-cause
    description: One root cause.
  - name: security
    description: A security finding.
    duplicates: {{same: [error_signature]}}
    evidence_section: {section}
"
        )
    };
    let mut world = World::new(&["followups"], &config("Examples"));
    std::fs::write(
        world.root.join("evidence.md.j2"),
        "{{ comment.evidence }}\nHost: {{ comment.sources.host }}\nWitnesses: {{ comment.sources.witnesses | join(\", \") }}\nSources: {{ comment.sources.draft_sources | tojson }}\n",
    )
    .expect("written");
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
        "## Root cause\n\nTokens leak.\n\n## Evidence\n\nSeen once.\n",
    );
    let draft = world.draft(BATCH, "tokens leak again", &["src/log.rs"], "Leaked.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let witness = format!("{REPOSITORY} src/log.rs:9 at {COMMIT}");
    let mut ticket = Ticket::new("tokens-leak-again", "tokens leak again", &[&draft]);
    ticket.kind = Some("security".to_owned());
    ticket.fields = vec![("error_signature".to_owned(), "E-1042".to_owned())];
    ticket
        .extra
        .push(("witnesses".to_owned(), json!([witness])));
    world.ticket(BATCH, &ticket);
    let merged = world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, &ticket.slug],
        "",
    );
    assert_eq!(merged["matched"], "followups:token-leak", "{merged}");
    assert_eq!(
        merged["write"]["body"],
        format!(
            "tokens leak again.\nHost: {HOST}\nWitnesses: {witness}\nSources: {{\"{draft}\":\"author node; node worker-1\"}}\n\n<!-- orchestrator:follow-up-comment run=\"{BATCH}\" root_cause=\"tokens-leak-into-logs\" -->\n"
        ),
        "the kind's `Examples` as the evidence, the record's host, witnesses and drafts' sources"
    );

    // A kind whose evidence section the ticket does not carry refuses the merge.
    std::fs::write(
        world.root.join("onefollowup.yaml"),
        config("Related tickets"),
    )
    .expect("written");
    let refused = world.fails(2, &["ticket", "copy", "--batch", BATCH, &ticket.slug], "");
    assert!(
        refused.contains("carries no `## Related tickets` to stage there"),
        "{refused}"
    );
}
