//! The main journey: a batch of drafts and hook inputs prepared over a destination of 400
//! other items, verified by a scripted agent that searches, validates and stages, checked,
//! published in a handful of batched store calls and settled — and a second batch whose write
//! staged after its check refuses `publish` until `check` passes again.

use onefollowup::formats::Versioned as _;
use onefollowup::formats::ticket::TicketRecord;
use serde_json::{Value, json};

use crate::world::{Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-01";

/// The configuration of a host with both hooks on `PATH`.
pub const HOOKED: &str = "host_label: build-host-1\nhooks:\n  batch_preparing: [inputs-hook]\n  batch_settled: [settled-hook]\n";

/// The hooks: each logs its stdin, and the preparing hook prints `inputs.json`.
pub fn hooks(world: &World) {
    let root = world.root.display();
    world.script(
        "inputs-hook",
        &format!(
            "cat >> {root}/preparing.log\necho >> {root}/preparing.log\ncat {root}/inputs.json"
        ),
    );
    world.script(
        "settled-hook",
        &format!("cat >> {root}/settled.log\necho >> {root}/settled.log"),
    );
}

/// What the preparing hook prints next.
pub fn inputs(world: &World, inputs: &Value) {
    std::fs::write(
        world.root.join("inputs.json"),
        json!({"schema_version": 1, "inputs": inputs}).to_string(),
    )
    .expect("written");
}

/// The front matter of an item, as YAML read into JSON.
pub fn front(text: &str) -> Value {
    let inner = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(front, _)| front)
        .expect("front matter");
    serde_saphyr::from_str(inner).expect("YAML")
}

/// The record an item's front matter carries.
pub fn record(text: &str) -> Value {
    front(text)["metadata"]["orchestrator.follow-up"].clone()
}

/// The comments section of an item's file.
pub fn comments(text: &str) -> String {
    text.split_once("## Comments")
        .map(|(_, comments)| comments.to_owned())
        .unwrap_or_default()
}

/// A setup pass of `batch`: one draft filed as `slug`, checked, published and settled, so the
/// batch's later passes hold a ticket bound to an existing card.
pub fn bound_pass(world: &mut World, batch: &str, slug: &str, title: &str) -> String {
    inputs(world, &json!([]));
    let draft = world.draft(
        batch,
        title,
        &["src/theme.rs"],
        "The theme resets after an update.\n",
    );
    world.ok("setup", &["prepare", "--batch", batch], "");
    world.ticket(batch, &Ticket::new(slug, title, &[&draft]));
    world.ok("setup", &["ticket", "copy", "--batch", batch, slug], "");
    world.answer(
        batch,
        &[answer(
            &draft,
            "root-cause",
            "filed",
            &[slug],
            "Verified at the basis.",
        )],
    );
    let checked = world.ok("setup", &["check", "--batch", batch], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("setup", &["publish", "--batch", batch], "");
    world.ok("setup", &["settle", "--batch", batch], "");
    draft
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey, end to end")]
fn a_batch_is_prepared_verified_checked_published_and_settled_in_a_handful_of_store_calls() {
    let mut world = World::new(&["followups"], HOOKED);
    hooks(&world);
    world.seed("followups", 400);
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

    // An earlier pass of the batch filed one ticket, now bound to its card.
    let d0 = bound_pass(
        &mut world,
        BATCH,
        "theme-resets-after-update",
        "the theme resets after an update",
    );
    let bound_card = world
        .filed("followups")
        .into_iter()
        .find(|(id, _)| id != "export-older")
        .map(|(id, _)| format!("followups:{id}"))
        .expect("the earlier pass filed its card");
    std::fs::remove_file(world.root.join("preparing.log")).expect("the setup's log");
    std::fs::remove_file(world.root.join("settled.log")).expect("the setup's log");
    world.reset_phases();

    // Six drafts, and two inputs the hook supplies.
    inputs(
        &world,
        &json!([
            {"id": "budget:gate-time", "source": "budget-overrun", "summary": "the gate took 40 minutes", "detail": "Over its 30-minute budget."},
            {"id": "budget:e2e-time", "source": "budget-overrun", "summary": "the e2e tier took 12 minutes", "detail": "Over its 10-minute budget."},
        ]),
    );
    let d1 = world.draft(
        BATCH,
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Saving while resizing loses the layout.\n",
    );
    let d2 = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "café became caf.\n",
    );
    let d3 = world.draft(
        BATCH,
        "a resize flickers the toolbar",
        &["src/toolbar.rs"],
        "The toolbar flickers.\n",
    );
    let d4 = world.draft(
        BATCH,
        "the undo stack forgets renames",
        &["src/undo.rs"],
        "Undo skips a rename.\n",
    );
    let d5 = world.draft(
        BATCH,
        "the splash screen is slow",
        &["src/splash.rs"],
        "Two seconds.\n",
    );
    let d6 = world.draft(
        BATCH,
        "a resize test is flaky",
        &["tests/resize.rs"],
        "Retries green.\n",
    );

    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(
        prepared["drafts"], 7,
        "the six drafts and the earlier pass's: {prepared}"
    );
    assert_eq!(prepared["inputs"], 2, "{prepared}");
    let preparing =
        std::fs::read_to_string(world.root.join("preparing.log")).expect("the hook ran");
    assert_eq!(
        preparing
            .lines()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>(),
        [json!({"schema_version": 1, "batch": BATCH}).to_string()],
        "the hook received the batch id on stdin once"
    );
    let task = std::fs::read_to_string(prepared["task_template"].as_str().expect("a path"))
        .expect("the task");
    for input in [
        &d1,
        &d2,
        &d3,
        &d4,
        &d5,
        &d6,
        "budget:gate-time",
        "budget:e2e-time",
    ] {
        assert!(task.contains(input), "the task lists {input}");
    }
    assert!(
        task.contains("budget-overrun"),
        "each input's source reaches the task"
    );
    let after_prepare = world.items("followups");

    // The scripted agent: search, write, validate and stage three new tickets, re-copy the
    // bound one, and stage evidence and its re-estimate on another batch's item.
    let searched = world.ok(
        "agent",
        &[
            "ticket",
            "search",
            "--batch",
            BATCH,
            "--ticket",
            "layout-save-reads-size-twice",
            "window size",
            "--query",
            "layout save",
        ],
        "",
    );
    let hits: Vec<&str> = searched["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["item"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        hits,
        [bound_card.as_str()],
        "only the batch's own card says it: {searched}"
    );
    let tickets = [
        Ticket::new(
            "layout-save-reads-size-twice",
            "a layout save reads one window size",
            &[&d1],
        ),
        Ticket::new(
            "toolbar-flickers-on-resize",
            "a resize flickers the toolbar",
            &[&d3],
        ),
        Ticket::new(
            "undo-forgets-renames",
            "the undo stack forgets renames",
            &[&d4],
        ),
    ];
    for ticket in &tickets {
        world.ticket(BATCH, ticket);
        let validated = world.ok(
            "agent",
            &["ticket", "validate", "--batch", BATCH, &ticket.slug],
            "",
        );
        assert_eq!(validated["sound"], true, "{validated}");
        let staged = world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
        assert_eq!(staged["write"]["op"], "copy");
    }
    let mut rebound = Ticket::new(
        "theme-resets-after-update",
        "the theme resets after an update",
        &[&d0],
    );
    rebound.evidence.push_str(" Seen again on the next run.");
    world.ticket(BATCH, &rebound);
    let status = world.ok(
        "agent",
        &[
            "ticket",
            "board-status",
            "--batch",
            BATCH,
            "theme-resets-after-update",
        ],
        "",
    );
    assert_eq!(status["item"], bound_card, "{status}");
    world.ok(
        "agent",
        &[
            "ticket",
            "copy",
            "--batch",
            BATCH,
            "theme-resets-after-update",
        ],
        "",
    );
    let commented = world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            BATCH,
            "--item",
            "followups:export-older",
            "--root-cause",
            "export-dialog-drops-unicode",
        ],
        "Host build-host-1: café exported as caf at the basis.\n",
    );
    assert_eq!(commented["write"]["op"], "comment");
    let estimated = world.ok(
        "agent",
        &[
            "ticket",
            "re-estimate",
            "--batch",
            BATCH,
            "followups:export-older",
        ],
        "",
    );
    assert_eq!(
        estimated["write"]["priority_estimate"], "medium",
        "two occurrences do not reach raise_at: {estimated}"
    );
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "root-cause",
                "filed",
                &["layout-save-reads-size-twice"],
                "Verified.",
            ),
            answer(
                &d2,
                "root-cause",
                "filed",
                &["export-dialog-drops-unicode"],
                "Evidence on the existing item.",
            ),
            answer(
                &d3,
                "root-cause",
                "filed",
                &["toolbar-flickers-on-resize"],
                "Verified.",
            ),
            answer(
                &d4,
                "root-cause",
                "filed",
                &["undo-forgets-renames"],
                "Verified.",
            ),
            answer(
                &d5,
                "root-cause",
                "not-reproducible",
                &[],
                "Under a second at the basis.",
            ),
            answer(
                &d6,
                "root-cause",
                "too-low-impact",
                &[],
                "It retries green; nobody is affected.",
            ),
            answer(
                "budget:gate-time",
                "root-cause",
                "already-fixed",
                &[],
                "The basis already splits the gate.",
            ),
            answer(
                "budget:e2e-time",
                "root-cause",
                "not-reproducible",
                &[],
                "Ten minutes at the basis.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    assert_eq!(
        world.items("followups"),
        after_prepare,
        "nothing reached the destination before publish"
    );

    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let outcomes: Vec<(String, String)> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| {
            (
                one["op"].as_str().unwrap_or_default().to_owned(),
                one["outcome"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("copy", "created"),
            ("copy", "created"),
            ("copy", "created"),
            ("copy", "updated"),
            ("comment", "written"),
            ("re-estimate", "written"),
        ]
        .map(|(op, outcome)| (op.to_owned(), outcome.to_owned())),
        "{published}"
    );
    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(
        settled["read_back"], 0,
        "every write's answer named its version: {settled}"
    );
    assert_eq!(settled["needs_attention"], Value::Null, "{settled}");
    let summary =
        std::fs::read_to_string(world.root.join("settled.log")).expect("the settled hook ran");
    let summary: Vec<Value> = summary
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("the summary is JSON"))
        .collect();
    assert_eq!(
        summary.len(),
        1,
        "the settled hook received the summary once"
    );
    assert_eq!(summary[0]["batch"], BATCH);
    assert_eq!(summary[0]["writes"], 6);

    // The store operations, each attributed to its phase: nothing reached the destination
    // before publish but prepare's two reads and the agent's one search.
    let operations = world.destination_operations();
    let named: Vec<(&str, &str)> = operations
        .iter()
        .map(|(phase, op, _, _)| (phase.as_str(), op.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("prepare", "find"),
            ("prepare", "read"),
            ("agent", "find"),
            ("publish", "copy"),
            ("publish", "copy"),
            ("publish", "update-many"),
            ("publish", "comment-add-many"),
        ],
        "{operations:?}"
    );
    crate::telemetry::record(
        "follow-up-main-journey-store-operations",
        operations.len(),
        &operations
            .iter()
            .map(|(phase, op, source, items)| format!("{phase}: {op} {source} ({items} items)"))
            .collect::<Vec<_>>()
            .join("; "),
    );

    // The destination holds exactly what the receipts name, in the v1 formats.
    let receipts = world.receipts(BATCH);
    let pass: Vec<&Value> = receipts.iter().skip(1).collect();
    assert_eq!(pass.len(), 6, "{receipts:#?}");
    let created: Vec<String> = pass
        .iter()
        .filter(|receipt| receipt["outcome"]["action"] == "created")
        .map(|receipt| receipt["card"].as_str().expect("a card").to_owned())
        .collect();
    assert_eq!(created.len(), 3);
    let mut filed: Vec<String> = world
        .filed("followups")
        .into_iter()
        .map(|(id, _)| format!("followups:{id}"))
        .collect();
    filed.sort();
    let mut named_cards: Vec<String> = created.clone();
    named_cards.push(bound_card.clone());
    named_cards.push("followups:export-older".to_owned());
    named_cards.sort();
    assert_eq!(filed, named_cards, "exactly the cards the receipts name");
    for card in &created {
        let text = world.read_item("followups", card.trim_start_matches("followups:"));
        let held = record(&text);
        assert_eq!(held["schema"], 14);
        for absent in ["kind", "fields", "edges", "board_assignee"] {
            assert!(
                held.get(absent).is_none(),
                "{absent} is omitted under the default configuration: {held}"
            );
        }
        let parsed = TicketRecord::parse(&held.to_string()).expect("a schema-14 record");
        assert_eq!(
            serde_json::from_str::<Value>(&parsed.render()).expect("JSON"),
            held,
            "the record round-trips"
        );
        assert!(text.contains("- Priority estimate: medium (severity with the workaround medium; fires intermittently; 1 occurrence; not raised)"), "{text}");
        assert_eq!(
            front(&text)["status"],
            "backlog",
            "a new ticket lands at its kind's initial status"
        );
    }
    let older = world.read_item("followups", "export-older");
    assert!(
        comments(&older).contains(&format!(
            "<!-- orchestrator:follow-up-comment run=\"{BATCH}\" root_cause=\"export-dialog-drops-unicode\" -->"
        )),
        "{older}"
    );
    assert!(comments(&older).contains(&format!(
        "Additional evidence from follow-up run `{BATCH}`."
    )));
    assert!(older.contains("- Priority estimate: medium (severity with the workaround medium; fires intermittently; 2 occurrences; not raised)"), "{older}");
    let bound_text = world.read_item("followups", bound_card.trim_start_matches("followups:"));
    assert!(
        bound_text.contains("Seen again on the next run."),
        "the bound card was re-copied: {bound_text}"
    );
    for (receipt, outcome) in pass
        .iter()
        .zip(["created", "created", "created", "updated"])
    {
        assert_eq!(receipt["command"], "copy");
        assert_eq!(receipt["outcome"]["action"], outcome);
        assert!(
            receipt["ticket"]
                .as_str()
                .is_some_and(|ticket| ticket.starts_with("drafts:"))
        );
    }
    assert_eq!(pass[4]["command"], "comment");
    assert_eq!(pass[4]["card"], "followups:export-older");
    assert_eq!(pass[5]["command"], "re-estimate");
    assert_eq!(pass[5]["outcome"]["priority_estimate"], "medium");
}

#[test]
fn a_write_staged_after_check_refuses_publish_until_check_passes_again() {
    const SECOND: &str = "nightly-2026-11-02";
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    world.seed("followups", 20);
    let draft = world.draft(
        SECOND,
        "the zoom level resets",
        &["src/zoom.rs"],
        "Zoom resets.\n",
    );
    world.ok("prepare", &["prepare", "--batch", SECOND], "");
    world.ticket(
        SECOND,
        &Ticket::new("zoom-level-resets", "the zoom level resets", &[&draft]),
    );
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", SECOND, "zoom-level-resets"],
        "",
    );
    world.answer(
        SECOND,
        &[answer(
            &draft,
            "root-cause",
            "filed",
            &["zoom-level-resets"],
            "Verified.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", SECOND], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ticket(
        SECOND,
        &Ticket::new(
            "zoom-ignores-the-wheel",
            "the zoom ignores the wheel",
            &[&draft],
        ),
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "copy",
            "--batch",
            SECOND,
            "zoom-ignores-the-wheel",
        ],
        "",
    );
    let before = world.items("followups");
    let refused = world.fails(2, &["publish", "--batch", SECOND], "");
    assert!(
        refused.contains(&format!(
            "outbox[1]: the copy of drafts:{SECOND}/tickets/zoom-ignores-the-wheel"
        )),
        "publish names the unchecked write: {refused}"
    );
    assert_eq!(
        world.items("followups"),
        before,
        "nothing was written to the destination"
    );
    let checked = world.ok("check", &["check", "--batch", SECOND], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", SECOND], "");
    assert_eq!(
        published["outcomes"].as_array().map(Vec::len),
        Some(2),
        "{published}"
    );
    assert_eq!(world.filed("followups").len(), 2);
}
