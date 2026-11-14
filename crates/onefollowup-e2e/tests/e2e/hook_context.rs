//! A hook handed a context file: `hooks.batch_settled` names `{context_file}`, as the README's
//! worked example for an engine's `human-items raise --phase follow-up-review --context` does,
//! and settling a batch that created one ticket and updated another hands it the path of a file
//! holding the batch's tickets, a dedup key the same on every settle, and the owner
//! `hooks.owner` names — one literal, one `${VARIABLE}` — and removes the file once the hook
//! exits. An unset variable refuses `settle` before anything changes, and settling again with it
//! set recovers. A hook naming no placeholder is handed exactly what it always was.

use std::path::Path;

use serde_json::{Value, json};

use crate::journey::bound_pass;
use crate::world::{Ticket, World, answer};

const BATCH: &str = "nightly-2026-11-08";
const RUN: &str = "run-2026-11-08";

/// The README's worked example: an engine raising its follow-up review from the context file.
pub const RAISING: &str = "host_label: build-host-1
hooks:
  batch_settled: [onepipeline, human-items, raise, --phase, follow-up-review, --context, '{context_file}']
  owner:
    session: follow-ups@build-host-1
    run: ${FOLLOW_UP_RUN}
";

/// Where the README's example file puts the world's root.
const README_ROOT: &str = "/srv/follow-ups";

/// A world whose `onepipeline` records each call: its argv, its stdin, and a copy of the file
/// `--context` names, under `raised/<n>/`.
fn raising_world(config: &str) -> World {
    let mut world = World::new(&["followups"], config);
    let root = world.root.display().to_string();
    std::fs::create_dir_all(world.root.join("raised")).expect("raised");
    world.script(
        "onepipeline",
        &format!(
            "dir={root}/raised/$(ls {root}/raised | wc -l)\nmkdir -p \"$dir\"\nprintf '%s\\n' \"$@\" > \"$dir/argv\"\ncat > \"$dir/stdin\"\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = --context ]; then cp \"$2\" \"$dir/context.json\"; fi\n  shift\ndone"
        ),
    );
    world
        .variables
        .push(("FOLLOW_UP_RUN".to_owned(), RUN.to_owned()));
    world
}

/// The calls `onepipeline` recorded, in order: `(argv, stdin, context)`.
fn raised(world: &World) -> Vec<(Vec<String>, String, Option<Value>)> {
    (0..)
        .map(|at| world.root.join(format!("raised/{at}")))
        .take_while(|dir| dir.is_dir())
        .map(|dir| {
            let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap_or_default();
            (
                read("argv").lines().map(str::to_owned).collect(),
                read("stdin"),
                std::fs::read_to_string(dir.join("context.json"))
                    .ok()
                    .map(|text| serde_json::from_str(&text).expect("the context file is JSON")),
            )
        })
        .collect()
}

/// The JSON Schema the repository declares for the context file.
fn schema() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../onefollowup/schemas/hook-context.schema.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("the schema")).expect("JSON")
}

/// The README's fenced block of `language` that follows the line `marker`.
fn readme_block(marker: &str, language: &str) -> String {
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
            .expect("the README");
    let after = readme
        .split_once(marker)
        .unwrap_or_else(|| panic!("the README holds {marker:?}"))
        .1;
    let fence = format!("```{language}\n");
    let start = after.find(&fence).expect("a fenced block follows") + fence.len();
    after[start..]
        .split_once("```")
        .expect("the block closes")
        .0
        .to_owned()
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey, end to end")]
fn a_settled_hook_is_handed_a_context_file_of_the_batchs_tickets_a_stable_key_and_its_owner() {
    let mut world = raising_world(RAISING);
    // The first pass files the ticket the second updates; its settle raises once.
    let first = bound_pass(
        &mut world,
        BATCH,
        "theme-resets-after-update",
        "the theme resets after an update",
    );
    let bound = world.filed("followups");
    assert_eq!(bound.len(), 1, "{bound:?}");
    let bound_card = format!("followups:{}", bound[0].0);
    // The second pass creates one ticket and updates the first.
    let second = world.draft(
        BATCH,
        "widgets: the toolbar flickers on resize",
        &["src/toolbar.rs"],
        "The toolbar flickers while the window resizes.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut rebound = Ticket::new(
        "theme-resets-after-update",
        "the theme resets after an update",
        &[&first],
    );
    rebound.evidence.push_str(" Seen again on the next run.");
    world.ticket(BATCH, &rebound);
    world.ticket(
        BATCH,
        &Ticket::new(
            "toolbar-flickers-on-resize",
            "a resize flickers the toolbar",
            &[&second],
        ),
    );
    for slug in ["theme-resets-after-update", "toolbar-flickers-on-resize"] {
        world.ok("stage", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    // The first pass's draft keeps the answer it carries.
    world.answer(
        BATCH,
        &[answer(
            &second,
            "root-cause",
            "filed",
            &["toolbar-flickers-on-resize"],
            "Verified at the basis.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let outcomes: Vec<&str> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| one["outcome"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(outcomes, ["updated", "created"], "{published}");

    // The destination states where a person opens the updated card, as a tracker would.
    world.edit_item(
        "followups",
        &bound[0].0,
        "---\ntitle:",
        "---\nurl: https://tracker.example/widgets/issues/7\ntitle:",
    );

    // Unset, the owner's variable refuses settle before the hook runs or anything changes.
    let state = world.work().join(format!("state/{BATCH}.json"));
    let held = std::fs::read_to_string(&state).expect("the batch state");
    let variables = std::mem::take(&mut world.variables);
    let stderr = world.fails(2, &["settle", "--batch", BATCH], "");
    assert!(stderr.contains("hooks.owner.run"), "{stderr}");
    assert!(stderr.contains("`FOLLOW_UP_RUN`"), "{stderr}");
    assert!(stderr.contains("the hook was not run"), "{stderr}");
    assert_eq!(
        raised(&world).len(),
        1,
        "only the first pass's settle raised"
    );
    assert_eq!(
        std::fs::read_to_string(&state).expect("the batch state"),
        held,
        "nothing changed"
    );

    // Set, settle raises with the context file; settling again hands the same key.
    world.variables = variables;
    world.ok("settle", &["settle", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let calls = raised(&world);
    assert_eq!(calls.len(), 3, "one raise per settle");
    let created = world
        .filed("followups")
        .into_iter()
        .map(|(id, _)| format!("followups:{id}"))
        .find(|id| *id != bound_card)
        .expect("the second pass created a card");
    let validator = jsonschema::validator_for(&schema()).expect("the schema compiles");
    for (argv, stdin, context) in &calls[1..] {
        let context = context
            .as_ref()
            .expect("the hook was handed a context file");
        let errors: Vec<String> = validator
            .iter_errors(context)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{errors:?}: {context}");
        // The path replaced the placeholder, and the file is gone once the hook exited.
        assert_eq!(
            argv[..5],
            [
                "human-items",
                "raise",
                "--phase",
                "follow-up-review",
                "--context"
            ],
            "{argv:?}"
        );
        let path = Path::new(&argv[5]);
        assert!(path.is_absolute(), "{argv:?}");
        assert!(!path.exists(), "the context file is removed: {argv:?}");
        assert_eq!(
            context["owner"],
            json!({"session": "follow-ups@build-host-1", "run": RUN, "plan": null})
        );
        assert_eq!(
            context["dedup_key"],
            format!("orchestrator.follow-up/{BATCH}/batch_settled")
        );
        let tickets = context["tickets"].as_array().expect("tickets");
        assert_eq!(tickets.len(), 2, "{context}");
        assert_eq!(tickets[0]["id"], bound_card.as_str(), "{context}");
        assert_eq!(
            tickets[0]["url"], "https://tracker.example/widgets/issues/7",
            "{context}"
        );
        assert_eq!(tickets[1]["id"], created.as_str(), "{context}");
        assert!(
            tickets[1]["url"]
                .as_str()
                .is_some_and(|url| url.starts_with("file://")),
            "a ticket its source states no URL for is opened at its file: {context}"
        );
        for ticket in tickets {
            assert_ne!(ticket["priority"], "none", "{context}");
        }
        // The rest of stdin's document, as text.
        let document: Value = serde_json::from_str(stdin).expect("stdin is JSON");
        assert_eq!(context["vars"]["batch"], BATCH);
        assert_eq!(context["vars"]["writes"], document["writes"].to_string());
        assert_eq!(context["vars"]["needs_attention"], "false");
        assert_eq!(context["vars"]["receipts"], document["receipts"]);
        assert!(context["vars"].get("schema_version").is_none(), "{context}");
    }
    assert_eq!(
        calls[1].2.as_ref().map(|one| &one["dedup_key"]),
        calls[2].2.as_ref().map(|one| &one["dedup_key"])
    );
    assert_eq!(
        calls[1].2.as_ref().map(|one| &one["tickets"]),
        calls[2].2.as_ref().map(|one| &one["tickets"])
    );

    // The README's worked example is this configuration and the file this settle wrote.
    assert_eq!(
        readme_block("A worked example: the configuration", "yaml"),
        RAISING
    );
    let written = serde_json::to_string_pretty(calls[1].2.as_ref().expect("a context"))
        .expect("JSON")
        .replace(&world.root.display().to_string(), README_ROOT);
    let shown: Value =
        serde_json::from_str(&readme_block("A worked example: the context file", "json"))
            .expect("the README's example is JSON");
    assert_eq!(
        serde_json::to_string_pretty(&shown).expect("JSON"),
        written,
        "the README's example is the file the journey's settle wrote"
    );
}

#[test]
fn a_hook_naming_no_placeholder_is_handed_exactly_its_argv_and_the_settled_document() {
    let mut world = raising_world(
        "host_label: build-host-1\nhooks:\n  batch_settled: [onepipeline, --plain, '{context}']\n  owner:\n    session: follow-ups@build-host-1\n    run: ${FOLLOW_UP_RUN}\n",
    );
    bound_pass(
        &mut world,
        BATCH,
        "theme-resets-after-update",
        "the theme resets after an update",
    );
    let calls = raised(&world);
    assert_eq!(calls.len(), 1);
    let (argv, stdin, context) = &calls[0];
    assert_eq!(argv, &["--plain", "{context}"], "argv as configured");
    assert!(context.is_none());
    let document: Value = serde_json::from_str(stdin).expect("stdin is JSON");
    let receipts = world.work().join(format!("receipts/{BATCH}.jsonl"));
    assert_eq!(
        document,
        json!({
            "schema_version": 1,
            "batch": BATCH,
            "writes": 1,
            "read_back": 0,
            "confirmed": 0,
            "pending": 0,
            "needs_attention": false,
            "receipts": receipts.display().to_string(),
        })
    );
    assert!(
        !world
            .work()
            .join(format!("tasks/{BATCH}/batch_settled.context.json"))
            .exists()
    );
}

#[test]
fn a_hook_naming_the_placeholder_without_an_owner_is_refused_at_load() {
    let world = raising_world(
        "host_label: build-host-1\nhooks:\n  batch_settled: [onepipeline, --context, '{context_file}']\n",
    );
    let stderr = world.fails(2, &["config", "show"], "");
    assert!(stderr.contains("hooks.batch_settled"), "{stderr}");
    assert!(stderr.contains("hooks.owner.session"), "{stderr}");
}
