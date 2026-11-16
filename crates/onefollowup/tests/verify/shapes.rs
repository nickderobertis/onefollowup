//! The `replaced-shape` worked example's journey through the library, in process: the example's
//! `onefollowup.yaml` and templates, its sources replaced by local-md sources of the same names.
//! (`crates/onefollowup-e2e/tests/e2e/examples.rs` runs it against the built binary.)

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::journey::front;
use crate::world::{COMMIT, HOST, REPOSITORY, Ticket, World, answer};

fn directory(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

/// A world running `name`'s configuration, its templates copied beside it.
pub fn world_of(name: &str) -> World {
    let config = std::fs::read_to_string(directory(name).join("onefollowup.yaml"))
        .expect("the example's configuration");
    let world = World::new(
        &["followups"],
        &format!("{config}host_label: build-host-1\n"),
    );
    let templates = directory(name).join("templates");
    std::fs::create_dir_all(world.root.join("templates")).expect("a directory");
    for entry in std::fs::read_dir(&templates)
        .expect("the templates")
        .filter_map(Result::ok)
    {
        std::fs::copy(
            entry.path(),
            world.root.join("templates").join(entry.file_name()),
        )
        .expect("copied");
    }
    world
}

fn record(text: &str) -> Value {
    front(text)["metadata"]["orchestrator.follow-up"].clone()
}

/// The filed item of `destination` whose record names `root_cause`, with its id.
pub fn filed(world: &World, destination: &str, root_cause: &str) -> (String, String) {
    world
        .filed(destination)
        .into_iter()
        .find(|(_, text)| record(text)["root_cause"] == root_cause)
        .unwrap_or_else(|| panic!("{destination} holds the ticket {root_cause}"))
}

#[test]
fn the_replaced_shape_example_runs_its_walk_through() {
    replaced_shape();
}

/// The record of the qualified `card`, read back through the store's own read.
fn stored_record(world: &World, card: &str) -> Value {
    let environment = vec![(
        "HOME".to_owned(),
        world.root.join("home").display().to_string(),
    )];
    let store = onefollowup::store::Store::open(Some(&world.root), &world.root, &environment)
        .expect("the world's store opens");
    let read = store
        .read(&[card.to_owned()], false)
        .expect("the store reads it");
    read[0].item.as_ref().expect("the store holds it").metadata["orchestrator.follow-up"].clone()
}

/// The front matter's record and the body of `destination`'s item `id`.
fn item_parts(world: &World, destination: &str, id: &str) -> (Value, String) {
    let text = world.read_item(destination, id);
    let body = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(_, body)| body.to_owned())
        .expect("front matter, then a body");
    (record(&text), body)
}

#[allow(
    clippy::too_many_lines,
    reason = "one walk-through: a batch filed, another batch's evidence, a re-copy"
)]
fn replaced_shape() {
    const BATCH: &str = "nightly-2026-11-10";
    const LATER: &str = "nightly-2026-11-11";
    let mut world = world_of("replaced-shape");
    let witness = format!("{REPOSITORY} src/layout.rs:42 at {COMMIT}");

    // `config show --sources` reads the reshaped keys from the example's file.
    let shown = world.ok("config", &["config", "show", "--sources"], "");
    for key in ["kinds", "comment_templates"] {
        assert_eq!(shown["sources"][key]["layer"], "file", "{key}: {shown}");
    }
    let kind = &shown["config"]["kinds"][0];
    assert_eq!(kind["render_template"], "templates/render.md.j2");
    assert_eq!(
        kind["projections"]["root_cause"],
        "{{ ticket.section('Root cause').content }}"
    );
    assert_eq!(
        shown["config"]["comment_templates"]["evidence"],
        "templates/evidence.md.j2"
    );
    world.reset_phases();

    let d1 = world.draft(
        BATCH,
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Saving while resizing loses the layout.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut ticket = Ticket::new(
        "layout-save-reads-size-twice",
        "a layout save reads one window size",
        &[&d1],
    );
    ticket
        .extra
        .push(("witnesses".to_owned(), json!([witness.clone()])));
    world.ticket(BATCH, &ticket);
    let validated = world.ok(
        "agent",
        &["ticket", "validate", "--batch", BATCH, &ticket.slug],
        "",
    );
    assert_eq!(validated["sound"], true, "{validated}");
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, &ticket.slug],
        "",
    );
    let filed_answer = [answer(
        &d1,
        "root-cause",
        "filed",
        &[&ticket.slug],
        "Verified.",
    )];
    world.answer(BATCH, &filed_answer);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");

    let (id, _) = filed(&world, "followups", &ticket.slug);
    let card = format!("followups:{id}");
    let (held, body) = item_parts(&world, "followups", &id);
    // The record keeps every value, the projections rendered into `projected`.
    assert_eq!(held["host"], HOST);
    assert_eq!(held["basis"], json!({REPOSITORY: COMMIT}));
    assert_eq!(held["witnesses"], json!([witness]));
    assert_eq!(
        held["draft_sources"],
        json!({&d1: "author node; node worker-1"}),
        "onefollowup wrote where the draft came from: {held}"
    );
    assert_eq!(
        held["projected"],
        json!({
            "root_cause": ticket.cause,
            "who_is_affected": "Users of widgets who save layouts.",
        }),
        "a section's text and one part of a section: {held}"
    );
    assert_eq!(
        stored_record(&world, &card)["projected"],
        held["projected"],
        "the store reads each value at [projected, <name>]"
    );
    // The host, basis and witnesses shown in the body; two parts collapsed.
    assert!(
        body.contains(&format!(
            "## Provenance\n\n- Host: {HOST}\n- Basis: {REPOSITORY} at {COMMIT}\n- Witness: {witness}\n"
        )),
        "{body}"
    );
    assert!(
        body.contains(
            "<details>\n<summary>Cost of the workaround</summary>\n\n- Applying it: a moment per save, by the user\n- Side effects on the outcome: none\n- Discovery: the failure names it\n\n</details>"
        ),
        "{body}"
    );
    assert!(
        body.contains(
            "Read the window size once per save.\n\n<details>\n<summary>Contributing locations</summary>\n\n- widgets `src/layout.rs`: reads the size twice\n\n</details>"
        ),
        "{body}"
    );
    assert!(
        !body.contains("**Cost of the workaround.**")
            && !body.contains("**Contributing locations.**"),
        "the default leaves both open; this host collapses them: {body}"
    );
    assert!(
        body.contains("- Severity with the workaround: medium\n- Priority estimate: medium (severity with the workaround medium; fires intermittently; 1 occurrence; not raised)\n"),
        "{body}"
    );
    for section in ["Examples", "Owning batches", "Evidence"] {
        assert!(
            body.contains(&format!("## {section}\n\n")),
            "{section}: {body}"
        );
    }

    // A later batch's evidence on that ticket, with its sources, what it bears on and a
    // question, each where the example's evidence template places it.
    let d2 = world.draft(
        LATER,
        "a layout save reads one window size again",
        &["src/layout.rs"],
        "Seen again on a slower disk.\n",
    );
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    let searched = world.ok(
        "agent",
        &[
            "ticket",
            "search",
            "--batch",
            LATER,
            "--ticket",
            &ticket.slug,
            "window size",
        ],
        "",
    );
    assert_eq!(searched["hits"][0]["item"], card.as_str(), "{searched}");
    let parts = world.root.join("parts");
    std::fs::create_dir_all(&parts).expect("a directory");
    std::fs::write(
        parts.join("sources.json"),
        json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [d2], "witnesses": [witness]})
            .to_string(),
    )
    .expect("the sources");
    std::fs::write(
        parts.join("bears-on.md"),
        "The fix must also cover slow disks.\n",
    )
    .expect("written");
    std::fs::write(
        parts.join("question.md"),
        "Is the slow disk a second root cause?\n",
    )
    .expect("written");
    let sources = parts.join("sources.json").display().to_string();
    let bears_on = parts.join("bears-on.md").display().to_string();
    let question = parts.join("question.md").display().to_string();
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
            &sources,
            "--bears-on-file",
            &bears_on,
            "--question-file",
            &question,
        ],
        "A save on a slow disk lost the layout too.\n",
    );
    world.answer(
        LATER,
        &[answer(
            &d2,
            "root-cause",
            "filed",
            &[&ticket.slug],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATER], "");
    world.ok("settle", &["settle", "--batch", LATER], "");
    let commented = crate::journey::comments(&world.read_item("followups", &id));
    let marker = format!(
        "<!-- orchestrator:follow-up-comment run=\"{LATER}\" root_cause=\"{}\" -->",
        ticket.slug
    );
    let expected = format!(
        "**Evidence from batch {LATER}.** A save on a slow disk lost the layout too.\n\n**Bears on the ticket.** The fix must also cover slow disks.\n\n**Question.** Is the slow disk a second root cause?\n\nSources:\n- Host: {HOST}\n- Basis: {REPOSITORY} at {COMMIT}\n- Draft: {d2}; author node; node worker-1\n- Witness: {witness}\n\n{marker}"
    );
    assert!(commented.contains(&expected), "{commented}");

    // The first batch's next pass counts that comment as an occurrence its marker's batch owns,
    // and a changed root cause re-renders the body and the projections on a bound re-copy.
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let status = world.ok(
        "agent",
        &["ticket", "board-status", "--batch", BATCH, &ticket.slug],
        "",
    );
    assert!(
        status["estimate_line"]
            .as_str()
            .is_some_and(|line| line.contains("; 2 occurrences;")),
        "{status}"
    );
    ticket.cause = "Widgets save a layout from two different window sizes.".to_owned();
    world.ticket(BATCH, &ticket);
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, &ticket.slug],
        "",
    );
    world.answer(BATCH, &filed_answer);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let (held, body) = item_parts(&world, "followups", &id);
    assert_eq!(held["projected"]["root_cause"], ticket.cause, "{held}");
    assert_eq!(
        stored_record(&world, &card)["projected"],
        json!({"root_cause": ticket.cause, "who_is_affected": "Users of widgets who save layouts."}),
        "the store reads the updated values at [projected, <name>]"
    );
    assert_eq!(
        held["projected"]["who_is_affected"],
        "Users of widgets who save layouts."
    );
    assert!(
        body.starts_with(&format!(
            "## Root cause\n\n{}\n\n## Provenance",
            ticket.cause
        )),
        "{body}"
    );
    assert!(body.contains("; 2 occurrences;"), "{body}");
}
