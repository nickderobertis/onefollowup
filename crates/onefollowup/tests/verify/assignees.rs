//! A ticket's one assignee, from a route: written on a new ticket by the copy that creates it
//! and recorded in its record's `board_assignee`; a route command's answer overriding the
//! route's `set`; an existing ticket's written in the batch's `update-many`, held to exactly
//! what onefollowup last wrote, so a person's own assignment always stands; refused where the
//! store holds no assignee; and costing no store operation of its own.

use serde_json::{Value, json};

use crate::journey::{front, record};
use crate::world::{Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-12";
const LATER: &str = "nightly-2026-11-13";

/// Critical tickets go to whoever the rotation names today; every other to an owner.
fn assigning(owner: &str) -> String {
    format!(
        "host_label: build-host-1
routes:
  - name: rotation
    match: {{severity: [critical]}}
    set: {{assignee: {owner}}}
    command: [rotation-today]
  - name: owners
    set: {{assignee: {owner}}}
"
    )
}

/// The people an item is assigned to.
fn assignees(text: &str) -> Vec<String> {
    front(text)["assignees"]
        .as_array()
        .map(|people| {
            people
                .iter()
                .filter_map(|one| one.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The filed item whose record names `root_cause`, by its id.
fn item(world: &World, root_cause: &str) -> (String, String) {
    world
        .filed("followups")
        .into_iter()
        .find(|(_, text)| record(text)["root_cause"] == root_cause)
        .unwrap_or_else(|| panic!("followups holds {root_cause}"))
}

/// One batch of three new tickets, the first critical.
fn three_tickets(world: &mut World, batch: &str) {
    let mut answers = Vec::new();
    for (slug, severity) in [
        ("checkout-crashes", "critical"),
        ("search-drops-a-letter", "high"),
        ("export-is-slow", "medium"),
    ] {
        let title = slug.replace('-', " ");
        let draft = world.draft(batch, &title, &[], &format!("{title}.\n"));
        let mut ticket = Ticket::new(slug, &title, &[&draft]);
        ticket = if severity == "critical" {
            ticket.severities("critical", "critical", "none")
        } else {
            ticket.severities(severity, severity, "none")
        };
        world.ticket(batch, &ticket);
        answers.push(answer(&draft, "root-cause", "filed", &[slug], "Verified."));
    }
    world.ok("prepare", &["prepare", "--batch", batch], "");
    for slug in [
        "checkout-crashes",
        "search-drops-a-letter",
        "export-is-slow",
    ] {
        world.ok("agent", &["ticket", "copy", "--batch", batch, slug], "");
    }
    world.answer(batch, &answers);
    let checked = world.ok("check", &["check", "--batch", batch], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", batch], "");
    world.ok("settle", &["settle", "--batch", batch], "");
}

#[test]
fn a_new_tickets_route_assigns_it_and_a_route_commands_answer_overrides_the_route() {
    let mut world = World::assigning(
        &["followups"],
        &["drafts", "followups"],
        &assigning("alice"),
    );
    let root = world.root.display().to_string();
    world.script(
        "rotation-today",
        &format!("cat > {root}/rotation.json\necho '{{\"assignee\": \"bob\"}}'"),
    );
    three_tickets(&mut world, BATCH);
    let (_, critical) = item(&world, "checkout-crashes");
    assert_eq!(
        assignees(&critical),
        ["bob"],
        "the command's answer: {critical}"
    );
    assert_eq!(record(&critical)["board_assignee"], "bob");
    let handed: Value = serde_json::from_str(
        &std::fs::read_to_string(world.root.join("rotation.json")).expect("the command ran"),
    )
    .expect("JSON");
    assert_eq!(handed["route"], "rotation");
    for slug in ["search-drops-a-letter", "export-is-slow"] {
        let (_, text) = item(&world, slug);
        assert_eq!(assignees(&text), ["alice"], "the route's set: {text}");
        assert_eq!(record(&text)["board_assignee"], "alice");
    }
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey over two batches")]
fn a_later_batch_reassigns_only_an_item_still_holding_what_onefollowup_last_wrote() {
    let mut world = World::assigning(
        &["followups"],
        &["drafts", "followups"],
        &assigning("alice"),
    );
    world.script("rotation-today", "cat >/dev/null");
    three_tickets(&mut world, BATCH);
    // In between, a person reassigns one item and gives another a second assignee.
    let (kept, _) = item(&world, "checkout-crashes");
    let (reassigned, _) = item(&world, "search-drops-a-letter");
    let (shared, _) = item(&world, "export-is-slow");
    world.edit_item("followups", &reassigned, "- alice", "- carol");
    world.edit_item("followups", &shared, "- alice", "- alice\n- dave");
    // The routes now name erin.
    std::fs::write(world.root.join("onefollowup.yaml"), assigning("erin")).expect("written");
    let mut answers = Vec::new();
    for slug in [
        "checkout-crashes",
        "search-drops-a-letter",
        "export-is-slow",
    ] {
        let title = slug.replace('-', " ");
        let draft = world.draft(LATER, &title, &[], "Seen again.\n");
        answers.push(answer(&draft, "root-cause", "filed", &[slug], "Evidence."));
    }
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    for (id, slug) in [
        (&kept, "checkout-crashes"),
        (&reassigned, "search-drops-a-letter"),
        (&shared, "export-is-slow"),
    ] {
        let card = format!("followups:{id}");
        world.ok(
            "agent",
            &[
                "ticket",
                "comment",
                "--batch",
                LATER,
                "--item",
                &card,
                "--root-cause",
                slug,
            ],
            "Seen again at the basis.\n",
        );
        world.ok(
            "agent",
            &["ticket", "re-estimate", "--batch", LATER, &card],
            "",
        );
    }
    world.answer(LATER, &answers);
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", LATER], "");

    let text = world.read_item("followups", &kept);
    assert_eq!(assignees(&text), ["erin"], "replaced: {text}");
    assert_eq!(record(&text)["board_assignee"], "erin");
    let text = world.read_item("followups", &reassigned);
    assert_eq!(
        assignees(&text),
        ["carol"],
        "a person's choice stands: {text}"
    );
    assert_eq!(
        record(&text)["board_assignee"],
        "alice",
        "nothing landed: {text}"
    );
    let text = world.read_item("followups", &shared);
    assert_eq!(
        assignees(&text),
        ["alice", "dave"],
        "both people stand: {text}"
    );
    let assigned: Vec<(String, String, String)> = world
        .receipts(LATER)
        .into_iter()
        .filter(|receipt| receipt["command"] == "assign")
        .map(|receipt| {
            (
                receipt["card"].as_str().unwrap_or_default().to_owned(),
                receipt["outcome"]["action"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                receipt["outcome"]["error"]["kind"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect();
    assert_eq!(
        assigned,
        [
            (
                format!("followups:{kept}"),
                "written".to_owned(),
                String::new()
            ),
            (
                format!("followups:{reassigned}"),
                "held".to_owned(),
                "assignee-held".to_owned()
            ),
            (
                format!("followups:{shared}"),
                "held".to_owned(),
                "assignee-held".to_owned()
            ),
        ],
        "{published}"
    );
    let journal = world.receipts(LATER);
    let held = journal
        .iter()
        .find(|receipt| {
            receipt["card"] == format!("followups:{reassigned}") && receipt["command"] == "assign"
        })
        .expect("the held assignee's receipt");
    assert_eq!(held["outcome"]["when"], json!(["alice"]), "{held}");
    // The re-estimates landed beside them.
    assert_eq!(
        journal
            .iter()
            .filter(|receipt| receipt["command"] == "re-estimate"
                && receipt["outcome"]["error"].is_null())
            .count(),
        3,
        "{journal:#?}"
    );
    let settled = world.ok("settle", &["settle", "--batch", LATER], "");
    assert_eq!(
        settled["pending"],
        Value::Null,
        "a held assignee is no pending write: {settled}"
    );
}

#[test]
fn a_route_naming_several_assignees_is_refused_naming_the_key() {
    let world = World::new(
        &["followups"],
        "host_label: build-host-1\nroutes:\n  - {name: owners, set: {assignees: [alice, bob]}}\n",
    );
    let refused = world.fails(2, &["config", "show"], "");
    assert!(
        refused.contains("routes[0].set.assignees")
            && refused.contains("name one person with `assignee`"),
        "{refused}"
    );
}

#[test]
fn check_refuses_a_route_assigning_where_a_source_holds_no_assignee() {
    for (assigning_sources, source, role) in [
        (&["drafts"][..], "followups", "destination"),
        (&["followups"][..], "drafts", "drafts source"),
    ] {
        let mut world = World::assigning(&["followups"], assigning_sources, &assigning("alice"));
        world.script("rotation-today", "cat >/dev/null");
        let draft = world.draft(BATCH, "export is slow", &[], "Slow.\n");
        world.ok("prepare", &["prepare", "--batch", BATCH], "");
        world.ticket(
            BATCH,
            &Ticket::new("export-is-slow", "export is slow", &[&draft]),
        );
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, "export-is-slow"],
            "",
        );
        world.answer(
            BATCH,
            &[answer(
                &draft,
                "root-cause",
                "filed",
                &["export-is-slow"],
                "Verified.",
            )],
        );
        let checked = world.ok("check", &["check", "--batch", BATCH], "");
        assert_eq!(checked["sound"], false, "{checked}");
        let problems = checked["problems"].to_string();
        assert!(
            problems.contains(&format!("the route `owners` assigns `alice` to tickets of the kind `root-cause`, and its {role} source `{source}` does not set `config.assignees: true`")),
            "{problems}"
        );
        assert!(
            problems.contains("\"subject\":\"route:owners\""),
            "{problems}"
        );
        world.fails(2, &["publish", "--batch", BATCH], "");
        assert!(world.filed("followups").is_empty(), "publish wrote nothing");
    }
}

/// One batch: two new tickets and evidence with its re-estimate on another batch's item,
/// answering the destination store operations it made.
fn one_batch(config: &str) -> (World, Vec<(String, String, String, u64)>) {
    let mut world = World::assigning(&["followups"], &["drafts", "followups"], config);
    world.script("rotation-today", "cat >/dev/null");
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
    let d1 = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &[],
        "café.\n",
    );
    let d2 = world.draft(BATCH, "the zoom level resets", &[], "Zoom.\n");
    let d3 = world.draft(BATCH, "the undo stack forgets renames", &[], "Undo.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    for (slug, title, draft) in [
        ("zoom-level-resets", "the zoom level resets", &d2),
        (
            "undo-forgets-renames",
            "the undo stack forgets renames",
            &d3,
        ),
    ] {
        world.ticket(BATCH, &Ticket::new(slug, title, &[draft]));
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    world.ok(
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
        "café again.\n",
    );
    world.ok(
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
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "root-cause",
                "filed",
                &["export-dialog-drops-unicode"],
                "Evidence.",
            ),
            answer(
                &d2,
                "root-cause",
                "filed",
                &["zoom-level-resets"],
                "Verified.",
            ),
            answer(
                &d3,
                "root-cause",
                "filed",
                &["undo-forgets-renames"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let operations = world.destination_operations();
    (world, operations)
}

#[test]
fn an_assignee_costs_no_store_operation_and_a_configuration_assigning_nobody_writes_none() {
    let (assigned, with) = one_batch(&assigning("alice"));
    let (unassigned, without) = one_batch("host_label: build-host-1\n");
    let count = |operations: &[(String, String, String, u64)]| {
        operations
            .iter()
            .map(|(_, op, source, _)| format!("{op} {source}"))
            .collect::<Vec<_>>()
    };
    assert_eq!(count(&with), count(&without), "{with:?}\n{without:?}");
    for (_, text) in assigned.filed("followups") {
        assert_eq!(assignees(&text), ["alice"], "{text}");
        assert_eq!(record(&text)["board_assignee"], "alice");
    }
    for (_, text) in unassigned.filed("followups") {
        assert!(assignees(&text).is_empty(), "{text}");
        assert!(record(&text).get("board_assignee").is_none(), "{text}");
    }
    assert!(
        !unassigned
            .receipts(BATCH)
            .iter()
            .any(|receipt| receipt["command"] == "assign")
    );
}
