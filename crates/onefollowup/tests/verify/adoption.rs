//! `settle` reading back only what no answer confirmed, and a host adopting onefollowup by
//! configuration alone: its own destination source, status vocabulary, rubric and templates.

use serde_json::{Value, json};

use crate::journey::{comments, front};
use crate::world::{COMMIT, HOST, REPOSITORY, Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-07";

#[cfg(unix)]
fn writable(path: &std::path::Path, writable: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if writable { 0o644 } else { 0o444 }),
    )
    .expect("permissions");
}

#[cfg(unix)]
#[test]
fn settle_reads_back_exactly_the_write_whose_answer_was_lost() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    world.seed("followups", 30);
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
    let first = world.draft(
        BATCH,
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Lost.\n",
    );
    let second = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "café.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    world.ticket(
        BATCH,
        &Ticket::new(
            "layout-save-reads-size-twice",
            "a layout save reads one window size",
            &[&first],
        ),
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "copy",
            "--batch",
            BATCH,
            "layout-save-reads-size-twice",
        ],
        "",
    );
    world.answer(
        BATCH,
        &[
            answer(
                &first,
                "root-cause",
                "filed",
                &["layout-save-reads-size-twice"],
                "Verified.",
            ),
            answer(
                &second,
                "root-cause",
                "not-reproducible",
                &[],
                "Not yet verified.",
            ),
        ],
    );
    world.ok("check", &["check", "--batch", BATCH], "");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    assert_eq!(
        world.receipts(BATCH).len(),
        1,
        "the first publish was receipted"
    );

    // The agent goes on: one further write, its evidence on another batch's item.
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
        "Host build-host-1: café exported as caf.\n",
    );
    world.answer(
        BATCH,
        &[
            answer(
                &first,
                "root-cause",
                "filed",
                &["layout-save-reads-size-twice"],
                "Verified.",
            ),
            answer(
                &second,
                "root-cause",
                "filed",
                &["export-dialog-drops-unicode"],
                "Evidence on the item.",
            ),
        ],
    );
    world.ok("check", &["check", "--batch", BATCH], "");
    let journal = world.work().join(format!("receipts/{BATCH}.jsonl"));
    writable(&journal, false);
    let failed = world.fails(1, &["publish", "--batch", BATCH], "");
    writable(&journal, true);
    assert!(
        failed.contains("could not record their 1 receipts"),
        "{failed}"
    );
    assert!(failed.contains("run `onefollowup settle"), "{failed}");
    assert_eq!(
        world.receipts(BATCH).len(),
        1,
        "the answer was not recorded"
    );
    assert!(
        comments(&world.read_item("followups", "export-older")).contains("café exported as caf"),
        "the write landed"
    );

    world.reset_phases();
    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(settled["read_back"], 1, "{settled}");
    assert_eq!(settled["confirmed"], 1, "{settled}");
    assert_eq!(settled["needs_attention"], Value::Null, "{settled}");
    let operations = world.destination_operations();
    assert_eq!(
        operations,
        [(
            "settle".to_owned(),
            "read".to_owned(),
            "followups".to_owned(),
            1
        )],
        "exactly that one item was read back"
    );
    assert!(
        !world.work().join("feedback/pending.json").exists(),
        "nothing is pending"
    );
}

/// A host's own policy: its destination named explicitly, its status vocabulary, rubric and
/// templates in a directory of its own.
const ADOPTED: &str = "host_label: build-host-1
destination: team-platform
task_template: policy/task.md.j2
default_kind: incident
kinds:
  - name: incident
    description: One incident's cause.
    ticket_template: policy/ticket.md.j2
statuses:
  proposal: {category: backlog, description: Awaiting triage., protected: false, selected: false}
  accepted: {category: todo, description: Ready for the team., protected: true, selected: true}
  deferred: {category: draft, description: Parked., protected: true, selected: false}
  queued: {category: queued, description: Claimed., protected: true, selected: false}
  in-progress: {category: in-progress, description: Being fixed., protected: true, selected: false}
  done: {category: done, description: Fixed., protected: true, selected: false}
  cancelled: {category: cancelled, description: Dropped., protected: false, selected: false}
rubric:
  levels:
    - {name: sev1, priority: urgent, users: everyone is down, development: everyone is blocked, resources: large spend}
    - {name: sev2, priority: high, users: some are down, development: some are blocked, resources: real spend}
    - {name: sev3, priority: low, users: an annoyance, development: a nuisance, resources: little spend}
  areas: {users: Customers, development: Engineers, resources: Spend}
  guidance: Judge by the worst affected area.
  impact_lines: [Severity, Mitigation, Residual severity]
  no_workaround: n/a
  impact_parts: [Who, Cost]
  workaround_costs: [Effort]
";

const TASK: &str = "# Incidents of {{ batch }}\n\nAnswer every input in {{ answers }}; tickets go under {{ tickets }}.\n\n{% for draft in drafts %}- {{ draft.id }}: {{ draft.title }}\n{% endfor %}";

const TICKET: &str = "## Summary\n\n<what happened>\n\n## Impact\n\n<{{ rubric.impact_parts | join(', ') }}, then the lines {{ rubric.impact_lines | join(', ') }}>\n\n## Evidence\n\n<the host, and what was seen>\n";

#[test]
fn a_host_adopts_the_loop_by_configuration_alone_from_any_directory() {
    let mut world = World::new(&["team-platform"], ADOPTED);
    let store = std::fs::read_to_string(world.root.join("onetaskgraph.yaml")).expect("the store");
    std::fs::write(
        world.root.join("onetaskgraph.yaml"),
        store.replace(
            "      root: team-platform-root\n",
            "      root: team-platform-root\n      status_mapping: {Triage: backlog, Ready: todo, Parked: draft, Claimed: queued, Fixing: in-progress, Fixed: done, Dropped: cancelled}\n",
        ),
    )
    .expect("the store");
    std::fs::create_dir_all(world.root.join("policy")).expect("policy");
    std::fs::write(world.root.join("policy/task.md.j2"), TASK).expect("the task template");
    std::fs::write(world.root.join("policy/ticket.md.j2"), TICKET).expect("the ticket template");
    world.seed("team-platform", 20);

    let draft = world.draft(
        BATCH,
        "the queue worker drops jobs",
        &["src/queue.rs"],
        "Jobs vanish.\n",
    );
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let task = std::fs::read_to_string(prepared["task_template"].as_str().expect("a path"))
        .expect("the task");
    assert!(
        task.starts_with(&format!("# Incidents of {BATCH}")),
        "the host's task template: {task}"
    );
    assert!(task.contains(&format!("- {draft}: the queue worker drops jobs")));

    // From a subdirectory, with the configuration named, every verb finds the same files.
    let nested = world.root.join("services/queue");
    std::fs::create_dir_all(&nested).expect("a subdirectory");
    let output = world.run_in(
        &nested,
        &[
            "--config",
            "../../onefollowup.yaml",
            "prepare",
            "--batch",
            BATCH,
        ],
        "",
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    let again: Value = serde_json::from_str(&output.stdout).expect("JSON");
    assert_eq!(
        again["answers"], prepared["answers"],
        "the same answers file"
    );
    assert_eq!(
        again["task_template"], prepared["task_template"],
        "the same task"
    );

    let record = json!({
        "schema": 14, "root_cause": "queue-drops-jobs", "repository": REPOSITORY,
        "created_by_run": BATCH, "owning_runs": [BATCH], "drafts": [draft],
        "basis": {REPOSITORY: COMMIT}, "verified_at": "2026-11-07T10:00:00Z", "host": HOST,
        "frequency": "consistent",
    });
    let ticket = format!(
        "---\ntitle: \"widgets: the queue worker drops jobs\"\nmetadata:\n  orchestrator.follow-up: {record}\n---\n## Summary\n\nJobs vanish from the queue.\n\n## Impact\n\n**Who.** Customers with queued jobs.\n**Cost.** A lost job each time.\n\n- Severity: sev2\n- Mitigation: retry the job\n- Residual severity: sev3\n\n## Evidence\n\nHost {HOST}: the worker log shows the drop.\n"
    );
    let path = world
        .work()
        .join(format!("tasks/{BATCH}/tickets/queue-drops-jobs.md"));
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the tickets directory");
    std::fs::write(&path, ticket).expect("the ticket");
    let validated = world.ok(
        "agent",
        &["ticket", "validate", "--batch", BATCH, "queue-drops-jobs"],
        "",
    );
    assert_eq!(validated["sound"], true, "{validated}");
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "queue-drops-jobs"],
        "",
    );
    world.answer(
        BATCH,
        &[answer(
            &draft,
            "incident",
            "filed",
            &["queue-drops-jobs"],
            "Verified.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(settled["read_back"], 0);
    let filed = world.filed("team-platform");
    assert_eq!(filed.len(), 1);
    let card = &filed[0].1;
    assert!(
        front(card)["status"]
            .as_str()
            .is_some_and(|status| status.eq_ignore_ascii_case("Triage")),
        "the host's own status vocabulary: {card}"
    );
    assert_eq!(
        front(card)["priority"],
        "medium",
        "sev3 estimates low, raised one level as it fires consistently"
    );
    assert!(card.contains("- Priority estimate: medium (severity with the workaround sev3; fires consistently; 1 occurrence; raised one level because it fires consistently)"), "{card}");
    for absent in ["Root cause", "Suggested fix", "Owning batches"] {
        assert!(
            !card.contains(&format!("## {absent}")),
            "the host's template has no {absent}"
        );
    }
}
