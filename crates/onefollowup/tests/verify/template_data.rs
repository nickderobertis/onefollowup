//! Every variable README.md's "Templates" documents reaches the templates that render with it.
//! A render template printing each ticket variable files a symptom carrying a declared field,
//! a relation's edge and witnesses; another batch's evidence comment, through an evidence
//! template printing each comment variable, carries its sources, bears-on paragraph and
//! question; and a person's comment on the ticket is answered through a reply template
//! printing each reply variable, whose reply — owned by its marker alone, in no form 0.5.0
//! wrote — `comments settle` verifies, re-rendering the changed ticket on the way.

use std::collections::BTreeMap;

use serde_json::json;

use crate::answering::{account, comment, item_text, launches, pending, quoted, response};
use crate::world::{COMMIT, HOST, REPOSITORY, Ticket, World, answer};

const BATCH: &str = "nightly-2026-11-10";
const LATER: &str = "nightly-2026-11-11";

const CONFIG: &str = "host_label: build-host-1
fields:
  client: {description: The client that saw it.}
comment_templates:
  evidence: vars/evidence.md.j2
  reply: vars/reply.md.j2
kinds:
  - {name: root-cause, description: One root cause., render_template: vars/render.md.j2}
  - name: symptom
    description: One symptom.
    render_template: vars/render.md.j2
    projections:
      fix_lead: '{{ ticket.section(\"Suggested fix\").lead }}'
      fix_parts: '{{ ticket.section(\"Suggested fix\").parts | tojson }}'
      fix_lines: '{{ ticket.section(\"Suggested fix\").lines | tojson }}'
relations:
  - {name: caused-by, from: symptom, to: root-cause, cardinality: many-to-many, decided_by: agent, store: link}
";

/// Each ticket and render variable, one `VAR <name>: <value>` line apiece.
const RENDER: &str = r#"{%- set impact = ticket.section("Impact") -%}
{%- set fix = ticket.section("Suggested fix") -%}
## Root cause

{{ ticket.section("Root cause").content }}

## Variables

VAR rubric: {{ rubric.levels | map(attribute="name") | join(",") }}
VAR estimate: {{ estimate.raise_at }}/{{ estimate.raise_cap }}
VAR fields: {{ fields | tojson }}
VAR declared_fields: {{ declared_fields | tojson }}
VAR kind: {{ kind.name }}
VAR batch: {{ batch }}
VAR destination: {{ destination }}
VAR namespace: {{ namespace }}
VAR ticket.title: {{ ticket.title }}
VAR ticket.slug: {{ ticket.slug }}
VAR ticket.body: {{ ticket.body | length }}
VAR ticket.sections: {{ ticket.sections | map(attribute="name") | join(",") }}
VAR section.name: {{ impact.name }}
VAR section.content: {{ impact.content | length }}
VAR section.lead: {{ fix.lead | length }}
VAR section.parts: {{ impact.parts[0].label }}={{ impact.parts[0].text }}
VAR section.lines: {{ impact.lines[0].label }}={{ impact.lines[0].value }}|{{ impact.lines[0].text }}
VAR fix.parts: {{ fix.parts | map(attribute="label") | join(",") }}
VAR section.subsections: {{ fix.subsections[0].heading }}={{ fix.subsections[0].content }}
VAR record: {{ record | tojson }}
VAR record.edges: {{ record.edges | tojson }}
VAR record.witnesses: {{ record.witnesses | tojson }}
VAR record.fields: {{ record.fields | tojson }}
VAR estimate_line.level: {{ estimate_line.level }}
VAR estimate_line.line: {{ estimate_line.line }}
VAR estimate_line.mitigated: {{ estimate_line.mitigated }}
VAR estimate_line.frequency: {{ estimate_line.frequency }}
VAR estimate_line.occurrences: {{ estimate_line.occurrences }}
VAR estimate_line.raise: {{ estimate_line.raise }}

{{ collapsed("Impact", impact.content) }}
"#;

/// Each evidence comment variable.
const EVIDENCE: &str = r"Evidence.
VAR comment.batch: {{ comment.batch }}
VAR comment.root_cause: {{ comment.root_cause }}
VAR comment.evidence: {{ comment.evidence | trim }}
VAR comment.bears_on: {{ comment.bears_on }}
VAR comment.question: {{ comment.question }}
VAR comment.sources.host: {{ comment.sources.host }}
VAR comment.sources.basis: {{ comment.sources.basis | tojson }}
VAR comment.sources.drafts: {{ comment.sources.drafts | tojson }}
VAR comment.sources.witnesses: {{ comment.sources.witnesses | tojson }}
VAR comment.sources.draft_sources: {{ comment.sources.draft_sources | tojson }}
VAR comment.sources.transcripts: {{ comment.sources.transcripts | tojson }}
VAR comment.kind: {{ kind.name }}
VAR comment.destination: {{ destination }}
VAR comment.fields: {{ fields | tojson }}
VAR comment.declared_fields: {{ declared_fields | tojson }}
";

/// Each reply variable.
const REPLY: &str = r"{{ comment.response }}
VAR reply.batch: {{ comment.batch }}
VAR reply.root_cause: {{ comment.root_cause }}
VAR reply.answers: {{ comment.answers }}
VAR reply.verdict: {{ comment.verdict }}
VAR reply.url: {{ comment.url }}
VAR reply.author: {{ comment.author }}
VAR reply.response: {{ comment.response }}
";

/// The symptom's `## Suggested fix` content.
const FIX: &str = "Read the window size once per save.

It is one change.

**Contributing locations.** The save path.

It reads the size twice.
- widgets `src/layout.rs`: reads the size twice

### widgets — `github.com/example/widgets`

Read the size once.

**Interface.** None changes.

Behaviour only.
";

/// Every `VAR <name>: <value>` line of `text`.
fn variables(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("VAR "))
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_owned(), value.trim().to_owned()))
        .collect()
}

fn populated(found: &BTreeMap<String, String>, names: &[&str], text: &str) {
    for name in names {
        let value = found
            .get(*name)
            .unwrap_or_else(|| panic!("{name} is printed: {text}"));
        assert!(
            !["", "none", "null", "[]", "{}", "="].contains(&value.as_str()),
            "{name} is populated, not `{value}`: {text}"
        );
    }
}

/// Each variable README.md's "Templates" table names, in its order.
fn documented() -> Vec<String> {
    let readme = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"),
    )
    .expect("the README");
    let table = readme
        .split_once("### Templates\n")
        .and_then(|(_, rest)| rest.split_once("\n\n| Variable | What it is |\n|---|---|\n"))
        .map(|(_, rest)| rest.split("\n\n").next().unwrap_or_default().to_owned())
        .expect("the Templates table");
    table
        .lines()
        .filter_map(|row| row.strip_prefix("| `"))
        .filter_map(|row| row.split_once('`'))
        .map(|(name, _)| name.to_owned())
        .collect()
}

#[test]
fn every_variable_the_readme_documents_is_printed_here() {
    let printed = variables(&format!("{RENDER}{EVIDENCE}{REPLY}"));
    let documented = documented();
    assert!(documented.len() >= 10, "{documented:?}");
    for name in &documented {
        assert!(
            printed.keys().any(|key| key == name
                || key.starts_with(&format!("{name}."))
                || (name == "comment" && key.starts_with("reply."))
                || (name == "ticket" && key.starts_with("section."))),
            "README.md documents `{name}`, which no template here prints"
        );
    }
}

/// The fenced reply shape the feedback file quotes under `label`.
fn shape(feedback: &str, label: &str) -> String {
    let opening = format!("- {label}:\n\n````text\n");
    let start = feedback.find(&opening).expect("the shape is quoted") + opening.len();
    let end = feedback[start..].find("\n````").expect("it closes") + start;
    feedback[start..end].to_owned()
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey: a ticket, evidence, a reply"
)]
fn every_documented_variable_reaches_its_template_populated() {
    let mut world = World::new(&["followups"], CONFIG);
    std::fs::create_dir_all(world.root.join("vars")).expect("a directory");
    for (name, text) in [
        ("render.md.j2", RENDER),
        ("evidence.md.j2", EVIDENCE),
        ("reply.md.j2", REPLY),
    ] {
        std::fs::write(world.root.join("vars").join(name), text).expect("a template");
    }
    let witness = format!("{REPOSITORY} src/checkout.rs:7 at {COMMIT}");
    let d1 = world.draft(
        BATCH,
        "checkout times out for acme",
        &["src/checkout.rs"],
        "acme: a timeout.\n",
    );
    let d2 = world.draft(
        BATCH,
        "a stale cache lease",
        &["src/cache.rs"],
        "The lease outlived its writer.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut symptom = Ticket::new("checkout-times-out", "checkout times out for acme", &[&d1]);
    symptom.kind = Some("symptom".to_owned());
    symptom.fields = vec![("client".to_owned(), "acme".to_owned())];
    symptom
        .extra
        .push(("witnesses".to_owned(), json!([witness])));
    let mut cause = Ticket::new("stale-cache-lease", "a stale cache lease", &[&d2]);
    cause.kind = Some("root-cause".to_owned());
    for ticket in [&symptom, &cause] {
        world.ticket(BATCH, ticket);
    }
    // The symptom's suggested fix: introductory prose, a part of two paragraphs and a labelled
    // line, and a `###` subsection holding another part.
    let path = world
        .work()
        .join(format!("tasks/{BATCH}/tickets/{}.md", symptom.slug));
    let text = std::fs::read_to_string(&path).expect("the ticket");
    std::fs::write(
        &path,
        text.replacen(
            "**Contributing locations.**\n- widgets `src/layout.rs`: reads the size twice\n\n### Layout save — `widgets`\n\nRead the window size once per save. A save then never mixes two window sizes.\n",
            FIX,
            1,
        ),
    )
    .expect("written");
    for slug in [&cause.slug, &symptom.slug] {
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    world.ok(
        "agent",
        &[
            "ticket",
            "relate",
            "--batch",
            BATCH,
            "--relation",
            "caused-by",
            "--from",
            &symptom.slug,
            "--to",
            &cause.slug,
        ],
        "",
    );
    world.answer(
        BATCH,
        &[
            answer(&d1, "symptom", "filed", &[&symptom.slug], "Verified."),
            answer(&d2, "root-cause", "filed", &[&cause.slug], "Verified."),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");

    let (id, text) = crate::shapes::filed(&world, "followups", &symptom.slug);
    let card = format!("followups:{id}");
    let found = variables(&text);
    populated(
        &found,
        &[
            "rubric",
            "estimate",
            "fields",
            "declared_fields",
            "kind",
            "batch",
            "destination",
            "namespace",
            "ticket.title",
            "ticket.slug",
            "ticket.body",
            "ticket.sections",
            "section.name",
            "section.content",
            "section.lead",
            "section.parts",
            "section.lines",
            "section.subsections",
            "record",
            "record.edges",
            "record.witnesses",
            "record.fields",
            "estimate_line.level",
            "estimate_line.line",
            "estimate_line.mitigated",
            "estimate_line.frequency",
            "estimate_line.occurrences",
            "estimate_line.raise",
        ],
        &text,
    );
    assert_eq!(found["kind"], "symptom");
    assert_eq!(found["fields"], r#"{"client":"acme"}"#);
    assert_eq!(
        found["declared_fields"],
        r#"{"client":{"description":"The client that saw it."}}"#
    );
    assert_eq!(found["batch"], BATCH);
    assert_eq!(found["destination"], "followups");
    assert_eq!(found["namespace"], "orchestrator");
    assert_eq!(found["ticket.slug"], symptom.slug);
    assert_eq!(
        found["section.parts"],
        "Who is affected=Users of widgets who save layouts."
    );
    assert_eq!(
        found["section.lines"],
        "Applying it=a moment per save, by the user|- Applying it: a moment per save, by the user"
    );
    assert_eq!(
        found["section.subsections"],
        "widgets — `github.com/example/widgets`=Read the size once."
    );
    assert!(found["record.edges"].contains("caused-by"), "{text}");
    // The section's lead, its parts (every paragraph, the subsection's included) and its
    // labelled line, as the render template and the projections are given them.
    assert_eq!(found["fix.parts"], "Contributing locations,Interface");
    let projected = &crate::journey::record(&text)["projected"];
    assert_eq!(
        projected["fix_lead"], "Read the window size once per save.\n\nIt is one change.",
        "{projected}"
    );
    let parse = |key: &str| -> serde_json::Value {
        serde_json::from_str(projected[key].as_str().expect("a rendered value")).expect("JSON")
    };
    assert_eq!(
        parse("fix_parts"),
        json!([
            {"label": "Contributing locations",
             "text": "The save path.\n\nIt reads the size twice.\n- widgets `src/layout.rs`: reads the size twice"},
            {"label": "Interface", "text": "None changes.\n\nBehaviour only."},
        ])
    );
    assert_eq!(
        parse("fix_lines"),
        json!([{"label": "widgets `src/layout.rs`", "value": "reads the size twice",
                "text": "- widgets `src/layout.rs`: reads the size twice"}])
    );
    assert_eq!(found["record.witnesses"], json!([witness]).to_string());
    assert_eq!(found["estimate_line.frequency"], "intermittent");
    assert!(
        text.contains("<details>\n<summary>Impact</summary>\n\n**Who is affected.**"),
        "{text}"
    );

    // Another batch's evidence comment, through the evidence template.
    let d3 = world.ok(
        "draft",
        &[
            "draft",
            "--batch",
            LATER,
            "--title",
            "checkout times out for acme again",
            "--repository",
            REPOSITORY,
            "--path",
            "src/checkout.rs",
            "--author",
            "node:worker-2",
            "--stamp",
            "transcript=onepipeline transcript nightly-2026-11-11 worker-2",
        ],
        "Again.\n",
    )["draft"]
        .as_str()
        .expect("a draft id")
        .to_owned();
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    world.ok(
        "agent",
        &[
            "ticket",
            "search",
            "--batch",
            LATER,
            "--ticket",
            &symptom.slug,
            "checkout times out",
        ],
        "",
    );
    let parts = world.root.join("parts");
    std::fs::create_dir_all(&parts).expect("a directory");
    std::fs::write(
        parts.join("sources.json"),
        json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [d3], "witnesses": [witness]})
            .to_string(),
    )
    .expect("written");
    std::fs::write(parts.join("bears-on.md"), "It narrows the client.\n").expect("written");
    std::fs::write(parts.join("question.md"), "Is globex affected too?\n").expect("written");
    let file = |name: &str| parts.join(name).display().to_string();
    world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--item",
            &card,
            "--sources-file",
            &file("sources.json"),
            "--bears-on-file",
            &file("bears-on.md"),
            "--question-file",
            &file("question.md"),
        ],
        "acme timed out again.\n",
    );
    world.answer(
        LATER,
        &[answer(
            &d3,
            "symptom",
            "filed",
            &[&symptom.slug],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATER], "");
    world.ok("settle", &["settle", "--batch", LATER], "");
    let commented = item_text(&world, &card);
    let found = variables(&commented);
    populated(
        &found,
        &[
            "comment.batch",
            "comment.root_cause",
            "comment.evidence",
            "comment.bears_on",
            "comment.question",
            "comment.sources.host",
            "comment.sources.basis",
            "comment.sources.drafts",
            "comment.sources.witnesses",
            "comment.sources.draft_sources",
            "comment.sources.transcripts",
            "comment.kind",
            "comment.destination",
            "comment.fields",
            "comment.declared_fields",
        ],
        &commented,
    );
    assert_eq!(found["comment.batch"], LATER);
    assert_eq!(found["comment.kind"], "symptom");

    // A person's comment, answered through the reply template.
    comment(
        &world,
        &card,
        "c-alice",
        "alice",
        "2026-11-12T10:00:00Z",
        "Does this hit globex?",
    );
    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let launched = launches(&world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    ));
    let (batch, feedback) = &launched[0];
    assert_eq!(batch, BATCH);
    let feedback_text = std::fs::read_to_string(feedback).expect("the feedback file");
    let confirms = shape(&feedback_text, "Reply shape if it confirms");
    let found = variables(&confirms);
    populated(
        &found,
        &[
            "reply.batch",
            "reply.root_cause",
            "reply.answers",
            "reply.verdict",
            "reply.url",
            "reply.author",
            "reply.response",
        ],
        &confirms,
    );
    assert_eq!(found["reply.verdict"], "confirms");
    assert_eq!(found["reply.author"], "alice");
    assert_eq!(
        variables(&shape(&feedback_text, "Reply shape if it does not"))["reply.verdict"],
        "does-not-confirm"
    );
    let one = &quoted(feedback)[0];
    crate::answering::edit_ticket(&one.ticket, "acme", "acme and globex");
    let reply = format!(
        "{}\n\n{}",
        confirms.replace(
            "<your reply, in your words>",
            "Yes: globex too, now in the ticket."
        ),
        one.confirms
    );
    comment(
        &world,
        &card,
        "c-alice-reply",
        "follow-up-agent",
        "2026-11-12T11:00:00Z",
        &reply,
    );
    account(
        feedback,
        batch,
        &[response(one, "c-alice-reply", "confirms", "Named globex.")],
    );
    let feedback_path = feedback.display().to_string();
    let checked = world.ok(
        "check",
        &["comments", "check", "--feedback", &feedback_path],
        "",
    );
    assert_eq!(checked["sound"], true, "{checked}");
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(
        (
            &settled["settled"][0]["verified"],
            &settled["settled"][0]["pending"]
        ),
        (&json!(1), &json!(0)),
        "the reply is verified: {settled}"
    );
    assert!(pending(&world).is_empty(), "{:?}", pending(&world));
    let after = item_text(&world, &card);
    assert!(
        after.contains("VAR ticket.title: widgets: checkout times out for acme and globex"),
        "the changed ticket is rendered through its template: {after}"
    );
}
