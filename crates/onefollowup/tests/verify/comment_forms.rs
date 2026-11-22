//! A comment is owned by its marker alone. Three evidence comments on another batch's ticket —
//! one in 0.5.0's form, one copied from a new-form evidence comment on the `followups` board,
//! and one rendered by the `replaced-shape` example's evidence template — each count as an
//! occurrence owned by the batch and root cause its marker names; and a batch's own
//! `ticket comment` there adds its comment and leaves each of the three byte for byte.
//!
//! `evidence-board-form.md` is ai-orchestrator#2319's comment `6893792448` on the `followups`
//! board as read on 2026-11-15, with its run, its host and its root cause replaced by this
//! journey's (`nightly-2026-11-09`, `build-host-1`, `export-dialog-drops-unicode`).

use std::path::Path;

use serde_json::json;

use crate::answering::{comment, item_text};
use crate::world::{World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-12";
const ROOT_CAUSE: &str = "export-dialog-drops-unicode";
const ITEM: &str = "followups:export-older";

fn marker(batch: &str) -> String {
    format!("<!-- orchestrator:follow-up-comment run=\"{batch}\" root_cause=\"{ROOT_CAUSE}\" -->")
}

/// The example's evidence template rendered for `batch`, as its `ticket comment` would.
fn example_form(batch: &str) -> String {
    let config = onefollowup::config::defaults("build-host-1");
    let template = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/replaced-shape/templates/evidence.md.j2");
    onefollowup::verify::render::comment(
        &onefollowup::verify::render::Common {
            config: &config,
            kind: &config.kinds[0],
            batch,
            destination: "followups",
        },
        &onefollowup::verify::template::Located::File(template),
        &json!({
            "batch": batch, "root_cause": ROOT_CAUSE,
            "evidence": "café exported as caf on a second machine.",
            "bears_on": null, "question": null,
            "sources": {
                "host": "build-host-1",
                "basis": {"github.com/example/widgets": "0123456789abcdef0123456789abcdef01234567"},
                "drafts": [], "witnesses": [], "draft_sources": {}, "transcripts": {},
            },
        }),
        &onefollowup::formats::markers::CommentMarker {
            batch: batch.to_owned(),
            root_cause: ROOT_CAUSE.to_owned(),
            reply: None,
            groom: None,
        },
    )
    .expect("the example's template renders")
}

#[test]
fn every_comment_form_is_owned_by_its_marker_and_left_as_it_stands() {
    // Four occurrences raise the estimate: the item's own and its three comments.
    let mut world = World::new(
        &["followups"],
        "host_label: build-host-1\nestimate: {raise_at: 4}\n",
    );
    world.item(
        "followups",
        "export-older",
        &filed_item(
            "widgets: the export dialog drops unicode titles",
            "nightly-2026-10-01",
            ROOT_CAUSE,
            &json!({}),
            "medium",
        ),
        &filed_body("The export dialog drops unicode titles."),
    );
    world.item(
        "followups",
        "own-item",
        &filed_item(
            "widgets: the export dialog drops unicode file names",
            BATCH,
            "export-dialog-drops-unicode-names",
            &json!({}),
            "medium",
        ),
        &filed_body("The export dialog drops unicode file names."),
    );
    let old_form = format!(
        "Additional evidence from follow-up run `nightly-2026-11-01`.\n\nSeen again.\n\n{}\n",
        marker("nightly-2026-11-01")
    );
    let board_form = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../onefollowup/tests/fixtures/comment-markers/evidence-board-form.md"),
    )
    .expect("the board's comment");
    assert!(board_form.starts_with("<details>\n<summary>Additional evidence"));
    let example = example_form(BATCH);
    for (id, body) in [
        ("c-old", &old_form),
        ("c-board", &board_form),
        ("c-example", &example),
    ] {
        comment(
            &world,
            ITEM,
            id,
            "follow-up-bot",
            "2026-11-12T10:00:00Z",
            body.trim_end(),
        );
    }
    let seeded = item_text(&world, ITEM);

    let draft = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "café became caf.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    // Each comment counts, whichever form it takes: with only two, three occurrences would
    // leave the estimate at `medium`.
    let estimated = world.ok(
        "agent",
        &["ticket", "re-estimate", "--batch", BATCH, ITEM],
        "",
    );
    assert_eq!(
        estimated["write"]["priority_estimate"], "high",
        "the item's own occurrence and its three comments: {estimated}"
    );

    // The batch's own comment is added, and the three stand as they were.
    world.ok(
        "agent",
        &["ticket", "comment", "--batch", BATCH, "--item", ITEM],
        "Host build-host-1: café exported as caf again.\n",
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "search",
            "--batch",
            BATCH,
            "--ticket",
            "names",
            "unicode file names",
        ],
        "",
    );
    let refused = world.fails(
        2,
        &[
            "ticket",
            "comment",
            "--batch",
            BATCH,
            "--item",
            "followups:own-item",
        ],
        "Seen.\n",
    );
    assert!(refused.contains("is this batch's own item"), "{refused}");
    world.answer(
        BATCH,
        &[answer(
            &draft,
            "root-cause",
            "filed",
            &[ROOT_CAUSE],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let after = item_text(&world, ITEM);
    let seeded_comments = seeded
        .split_once("\n## Comments\n")
        .map(|(_, comments)| comments.trim_end().to_owned())
        .expect("the seeded comments");
    assert!(
        after.contains(&seeded_comments),
        "each seeded comment is byte for byte as it was:\n{seeded_comments}\n---\n{after}"
    );
    assert_eq!(
        after.matches(&marker(BATCH)).count(),
        2,
        "the batch's seeded comment and the one it added: {after}"
    );
    assert!(
        after.contains(&format!(
            "Additional evidence from follow-up run `{BATCH}`.\n\nHost build-host-1: café exported as caf again.\n\n{}",
            marker(BATCH)
        )),
        "{after}"
    );
}
