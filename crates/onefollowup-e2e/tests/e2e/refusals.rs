//! What `prepare` and `check` refuse: a `batch_preparing` hook that fails or prints what is
//! not the inputs shape, before anything is snapshotted or read; and every answer that breaks
//! the configured kinds and dispositions.

use serde_json::{Value, json};

use crate::world::{World, answer};

const BATCH: &str = "nightly-2026-11-05";

#[test]
fn a_failing_or_misspoken_preparing_hook_fails_prepare_before_any_snapshot_or_read() {
    let world = World::new(
        &["followups"],
        "host_label: build-host-1\nhooks:\n  batch_preparing: [inputs-hook]\n",
    );
    // A destination nothing can read: had prepare reached it, it would fail on the store.
    std::fs::remove_dir_all(world.root.join("followups-root")).expect("removed");
    for (script, says) in [
        (
            "echo 'no budgets file' >&2\nexit 3",
            "exited 3: no budgets file",
        ),
        (
            "echo '{\"inputs\": \"three\"}'",
            "printed a document that is not the inputs shape",
        ),
        (
            "echo 'not json at all'",
            "printed a document that is not the inputs shape",
        ),
    ] {
        world.script("inputs-hook", script);
        let failed = world.fails(1, &["prepare", "--batch", BATCH], "");
        assert!(
            failed.contains("the `hooks.batch_preparing` command `inputs-hook`"),
            "{failed}"
        );
        assert!(failed.contains(says), "{failed}");
        assert!(failed.contains("nothing was snapshotted"), "{failed}");
        assert!(
            !world.work().join(format!("state/{BATCH}.json")).exists(),
            "no snapshot was written"
        );
        assert!(!world.work().join(format!("tasks/{BATCH}/task.md")).exists());
    }
}

/// A host with a second kind that lists its own dispositions, and default dispositions.
const KINDS: &str = "host_label: build-host-1
dispositions:
  - name: filed
    description: Filed.
    requires: [ticket]
  - name: not-reproducible
    description: It does not hold.
    requires: [reason]
  - name: wont-fix
    description: It holds and will not be fixed.
    requires: [reason]
default_dispositions: [filed, not-reproducible]
kinds:
  - name: root-cause
    description: One root cause.
  - name: chore
    description: A chore.
    dispositions: [filed, not-reproducible, wont-fix]
";

#[test]
fn check_refuses_every_answer_that_breaks_the_kinds_and_dispositions() {
    let mut world = World::new(&["followups"], KINDS);
    let mut inputs = Vec::new();
    for at in 0..9 {
        inputs.push(world.draft(
            BATCH,
            &format!("draft number {at} drops data"),
            &["src/a.rs"],
            "Seen.\n",
        ));
    }
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut no_kind = answer(
        &inputs[0],
        "root-cause",
        "not-reproducible",
        &[],
        "Not at the basis.",
    );
    no_kind.as_object_mut().expect("an object").remove("kind");
    world.answer(
        BATCH,
        &[
            no_kind,
            answer(
                &inputs[1],
                "nonsense",
                "not-reproducible",
                &[],
                "Not at the basis.",
            ),
            answer(&inputs[2], "root-cause", "wont-fix", &[], "Not worth it."),
            answer(&inputs[3], "chore", "wont-fix", &[], "Not worth it."),
            answer(
                &inputs[5],
                "root-cause",
                "not-reproducible",
                &[],
                "Not at the basis.",
            ),
            answer(&inputs[5], "root-cause", "wont-fix", &[], "Not worth it."),
            answer(&inputs[6], "root-cause", "deferred-forever", &[], "Later."),
            answer(&inputs[7], "root-cause", "not-reproducible", &[], ""),
        ],
    );
    // An earlier pass's carried answer with no kind is refused the same way.
    std::fs::write(
        world.work().join(format!("dispositions/{BATCH}.carried.json")),
        serde_json::to_string_pretty(&json!({
            "schema": 1,
            "run": BATCH,
            "dispositions": [{"draft": inputs[8], "disposition": "not-reproducible", "root_causes": [], "detail": "Not at the basis."}],
        }))
        .expect("JSON")
            + "\n",
    )
    .expect("written");
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], false);
    let problems: Vec<(String, String)> = checked["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|problem: &Value| {
            (
                problem["subject"].as_str().unwrap_or_default().to_owned(),
                problem["message"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let expected = [
        (0, "the answer names no kind"),
        (1, "the kind `nonsense` is not a configured kind"),
        (
            2,
            "the kind `root-cause` does not allow the disposition `wont-fix`",
        ),
        (4, "the input has no disposition"),
        (5, "the input carries 2 dispositions"),
        (
            6,
            "the disposition `deferred-forever` is not a configured disposition",
        ),
        (7, "the disposition `not-reproducible` requires a reason"),
        (8, "the answer names no kind"),
    ];
    for (at, message) in expected {
        assert!(
            problems
                .iter()
                .any(|(subject, said)| subject == &inputs[at] && said.starts_with(message)),
            "input {at}: {message}\n{problems:#?}"
        );
    }
    assert!(
        !problems.iter().any(|(subject, _)| subject == &inputs[3]),
        "`wont-fix` is accepted for a kind allowing it: {problems:#?}"
    );
    assert_eq!(problems.len(), expected.len(), "{problems:#?}");
    let refused = world.fails(2, &["publish", "--batch", BATCH], "");
    assert!(refused.contains("nothing was written"), "{refused}");
}
