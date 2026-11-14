//! Drafts about private repositories, each verb run in process: under a store whose public boundary is active — a
//! `write_policy` whose visibility command classifies each repository — a batch's drafts
//! project is private when the drafts source is declared private, so a draft about a private
//! repository is recorded beside a public one, each keeping its own classification, and the
//! public one's ticket still reaches its destination. A drafts source declared public, or
//! declaring nothing, keeps today's public drafts project and refuses a private draft, naming
//! the visibility it declares; nothing is written.

use crate::journey::{front, inputs};
use crate::world::{REPOSITORY, Ticket, World, answer};
use serde_json::json;

const BATCH: &str = "nightly-2026-11-07";
/// The repository the visibility command classifies private.
const PRIVATE_REPOSITORY: &str = "github.com/example/ledger";

/// A world whose store's boundary is active, its drafts source declaring `visibility` (none
/// when `None`), and `followups` a destination declaring nothing.
fn bounded(visibility: Option<&str>) -> World {
    let world = World::new(&["followups"], "host_label: build-host-1\n");
    world.script(
        "repository-visibility",
        &format!(
            "case \"$(cat)\" in *{PRIVATE_REPOSITORY}*) echo '{{\"visibility\": \"private\"}}' ;; *) echo '{{\"visibility\": \"public\"}}' ;; esac"
        ),
    );
    world.script(
        "boundary-check",
        "cat > /dev/null\necho '{\"verdict\": \"pass\"}'",
    );
    let declared = visibility
        .map(|visibility| format!("    visibility: {visibility}\n"))
        .unwrap_or_default();
    // Named by path: onetaskgraph runs a policy command on its own process's `PATH`.
    let bin = world.root.join("bin").display().to_string();
    std::fs::write(
        world.root.join("onetaskgraph.yaml"),
        format!(
            "write_policy:\n  visibility_command: [{bin}/repository-visibility]\n  check_command: [{bin}/boundary-check]\nsources:\n  drafts:\n    plugin: local-md\n{declared}    config:\n      root: drafts-root\n  followups:\n    plugin: local-md\n    config:\n      root: followups-root\n"
        ),
    )
    .expect("the store");
    world
}

/// The `classification` an item's front matter records, `public` when it records none.
fn classification(text: &str) -> String {
    front(text)["classification"]
        .as_str()
        .unwrap_or("public")
        .to_owned()
}

/// The arguments of a draft about `repository`.
fn draft_args<'a>(title: &'a str, repository: &'a str) -> Vec<&'a str> {
    vec![
        "draft",
        "--batch",
        BATCH,
        "--title",
        title,
        "--repository",
        repository,
        "--author",
        "node:worker-1",
    ]
}

#[test]
fn a_drafts_source_declared_private_records_private_and_public_drafts_each_as_classified() {
    let mut world = bounded(Some("private"));
    let private = world.draft_in(
        BATCH,
        PRIVATE_REPOSITORY,
        "ledger: the nightly close double-counts refunds",
        &["src/close.rs"],
        "The nightly close counts each refund twice.\n",
    );
    let public = world.draft(
        BATCH,
        "widgets: the layout resets after a resize",
        &["src/layout.rs"],
        "The layout resets after a resize.\n",
    );

    let project =
        std::fs::read_to_string(world.root.join(format!("drafts-root/projects/{BATCH}.md")))
            .expect("the batch's drafts project");
    assert_eq!(classification(&project), "private", "{project}");
    let native = |id: &str| id.trim_start_matches("drafts:").to_owned();
    let private_task = world.read_item("drafts", &native(&private));
    let public_task = world.read_item("drafts", &native(&public));
    assert_eq!(classification(&private_task), "private", "{private_task}");
    assert_eq!(classification(&public_task), "public", "{public_task}");

    // The verify loop reads both drafts from the private project, and the public draft's
    // ticket reaches the destination public.
    inputs(&world, &json!([]));
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(prepared["drafts"], 2, "{prepared}");
    world.ticket(
        BATCH,
        &Ticket::new(
            "layout-resets-on-resize",
            "the layout resets after a resize",
            &[&public],
        ),
    );
    world.ok(
        "stage",
        &[
            "ticket",
            "copy",
            "--batch",
            BATCH,
            "layout-resets-on-resize",
        ],
        "",
    );
    world.answer(
        BATCH,
        &[
            answer(
                &public,
                "root-cause",
                "filed",
                &["layout-resets-on-resize"],
                "Verified at the basis.",
            ),
            answer(
                &private,
                "root-cause",
                "not-reproducible",
                &[],
                "The close no longer counts refunds at the basis.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_ne!(settled["needs_attention"], true, "{settled}");
    let filed = world.filed("followups");
    assert_eq!(filed.len(), 1, "{filed:?}");
    assert_eq!(classification(&filed[0].1), "public", "{}", filed[0].1);
    assert!(filed[0].1.contains(REPOSITORY), "{}", filed[0].1);
}

#[test]
fn a_drafts_source_not_declared_private_refuses_a_private_draft_naming_its_declared_visibility() {
    for (visibility, declared) in [(Some("public"), "public"), (None, "unknown")] {
        let mut world = bounded(visibility);
        world.draft(
            BATCH,
            "widgets: the layout resets after a resize",
            &["src/layout.rs"],
            "The layout resets after a resize.\n",
        );
        let before = world.items("drafts");
        let stderr = world.fails(
            2,
            &draft_args(
                "ledger: the nightly close double-counts refunds",
                PRIVATE_REPOSITORY,
            ),
            "The nightly close counts each refund twice.\n",
        );
        assert!(
            stderr.contains(&format!(
                "the drafts source `drafts` is declared `{declared}`"
            )),
            "{stderr}"
        );
        assert!(
            stderr.contains(
                "declare `visibility: private` on the drafts source `drafts` in its onetaskgraph configuration"
            ),
            "{stderr}"
        );
        assert!(stderr.contains("nothing was written"), "{stderr}");
        assert_eq!(world.items("drafts"), before, "nothing was written");
        let project =
            std::fs::read_to_string(world.root.join(format!("drafts-root/projects/{BATCH}.md")))
                .expect("the batch's drafts project");
        assert_eq!(classification(&project), "public", "{project}");
    }
}
