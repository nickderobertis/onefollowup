//! A ticket the `followups` board holds in the shape ai-orchestrator 8bf72a30 wrote — read from
//! onetaskgraph#4039 (`tests/fixtures/board/onetaskgraph-4039.md` in the library crate, its
//! record relabelled schema 14) — filed by its own batch over local-md and copied again once it
//! is bound, under a render template that writes the board's collapsed shape. The bound copy
//! keeps every record key the board held, with its value, and writes the body exactly as the
//! board holds it.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::journey::{front, record};
use crate::shapes::filed;
use crate::world::{World, answer};

/// The batch the board's record names as creating and owning it.
const BATCH: &str =
    "Move-follow-up-ticket-metadata-onto-board-fields-and-collapse-its-long-optional-parts";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../onefollowup/tests/fixtures/board")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(fixtures().join(name)).expect("the fixture")
}

/// The body under an item's front matter.
fn body(text: &str) -> String {
    text.strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(_, body)| body.to_owned())
        .expect("front matter, then a body")
}

fn configuration(render: bool) -> String {
    let mut config = String::from(
        "host_label: build-host-1\nkinds:\n  - name: root-cause\n    description: One root cause.\n    ticket_template: board/ticket.md.j2\n",
    );
    if render {
        config.push_str("    render_template: board/render.md.j2\n");
    }
    config
}

/// One pass of the batch: prepare, the ticket copied, its draft answered, checked, published
/// and settled.
fn pass(world: &mut World, slug: &str, draft: &str) {
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(prepared["drafts"], 1, "{prepared}");
    let validated = world.ok("agent", &["ticket", "validate", "--batch", BATCH, slug], "");
    assert_eq!(validated["sound"], true, "{validated}");
    world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    world.answer(
        BATCH,
        &[answer(draft, "root-cause", "filed", &[slug], "Verified.")],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
}

#[test]
fn a_board_ticket_read_from_onetaskgraph_4039_survives_a_bound_re_copy_whole() {
    let as_read = read("onetaskgraph-4039.md");
    let board_front = front(&as_read);
    let board_record = board_front["metadata"]["orchestrator.follow-up"].clone();
    let board_body = body(&as_read);
    let slug = board_record["root_cause"].as_str().expect("a root cause");
    let draft = board_record["drafts"][0].as_str().expect("one draft");
    assert_eq!(board_record["schema"], 14);
    for key in ["draft_sources", "transcripts", "witnesses"] {
        assert!(
            board_record.get(key).is_some(),
            "the board's record holds {key}"
        );
    }

    let mut world = World::new(&["followups"], &configuration(false));
    std::fs::create_dir_all(world.root.join("board")).expect("a directory");
    for name in ["ticket.md.j2", "render.md.j2"] {
        std::fs::copy(fixtures().join(name), world.root.join("board").join(name)).expect("copied");
    }
    // The draft the board's record stands on, at its own id, its author and transcript the
    // ones the record names.
    let native = draft.strip_prefix("drafts:").expect("a drafts item");
    let path = world.root.join(format!("drafts-root/tasks/{native}.md"));
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
    let draft_record = json!({
        "schema": 1, "run": BATCH,
        "author": {"kind": "node", "node": "follow-up-ticket-fields", "member": null},
        "dispatch": {"scratch": null, "session": null, "cwd": "/srv/work", "branch": null, "head": null},
        "transcript": format!("onepipeline transcript {BATCH} follow-up-ticket-fields"),
        "repository": "github.com/nickderobertis/onetaskgraph",
        "paths": ["crates/onetaskgraph-github-projects/src/lib.rs"],
        "drafted_at": "2026-11-14T00:34:47Z",
    });
    std::fs::write(
        &path,
        format!(
            "---\ntitle: \"a projected metadata field with no value costs an extra read\"\nstatus: todo\npriority: none\nmetadata:\n  orchestrator.follow-up-draft: {draft_record}\n---\nThe board reread its fields.\n"
        ),
    )
    .expect("the draft");

    // The ticket file: the board's title and record, and the body its verifying agent authors.
    // Its agent records the impact values its body's lines state, as this release's records
    // carry them.
    let mut authored_record = board_record.clone();
    authored_record["impact"] = json!({
        "Severity": "low",
        "Workaround": "Keep source drafts available so transcript projections remain populated where a transcript exists.",
        "Severity with the workaround": "low",
    });
    let authored = read("onetaskgraph-4039.authored.md");
    let authored = authored
        .split_once("-->\n")
        .map(|(_, body)| body)
        .expect("the authored body under its note");
    assert!(!authored.contains("<details>"));
    let ticket_path = world
        .work()
        .join(format!("tasks/{BATCH}/tickets/{slug}.md"));
    std::fs::create_dir_all(ticket_path.parent().expect("a parent")).expect("a directory");
    std::fs::write(
        &ticket_path,
        format!(
            "---\ntitle: {}\nmetadata:\n  orchestrator.follow-up: {authored_record}\n---\n{authored}",
            board_front["title"]
        ),
    )
    .expect("the ticket");

    // The first pass files it in the authored shape.
    pass(&mut world, slug, draft);
    let (id, text) = filed(&world, "followups", slug);
    assert!(!body(&text).contains("<details>"), "{text}");

    // Bound now, the ticket is copied again under the board's render template.
    std::fs::write(world.root.join("onefollowup.yaml"), configuration(true)).expect("written");
    pass(&mut world, slug, draft);
    let copies: Vec<Value> = world
        .receipts(BATCH)
        .into_iter()
        .filter(|receipt| receipt["command"] == "copy")
        .collect();
    assert_eq!(
        copies
            .last()
            .map(|receipt| receipt["outcome"]["action"].clone()),
        Some(json!("updated")),
        "the bound copy wrote the card: {copies:?}"
    );

    let text = world.read_item("followups", &id);
    assert_eq!(
        body(&text).trim_end(),
        board_body.trim_end(),
        "every section, part and line as the board holds it"
    );
    let held = record(&text);
    for (key, value) in board_record.as_object().expect("an object") {
        assert_eq!(&held[key], value, "{key}: {held}");
    }
}
