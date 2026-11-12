//! A host adopting the comment pass by configuration alone, with none of the default policy:
//! its destination source named explicitly (no store routes, no default destination), drafts in
//! a local-md `drafts` source, its own status vocabulary, rubric and templates in a directory of
//! its own — its task template rendering the feedback mode itself.

use serde_json::json;

use crate::answering::{
    account, comment, destination_ops, edit_ticket, item_text, launches, post_reply, quoted,
    replies_to, response, watermark,
};
use crate::journey::front;
use crate::world::{COMMIT, HOST, REPOSITORY, World, answer};

const BATCH: &str = "incidents-2026-11-07";
const DESTINATION: &str = "team-platform";

/// The host's policy: the verify loop's adoption configuration, with which statuses' comments
/// it answers stated in its own words.
const ADOPTED: &str = "host_label: build-host-1
destination: team-platform
task_template: policy/task.md.j2
default_kind: incident
kinds:
  - name: incident
    description: One incident's cause.
    ticket_template: policy/ticket.md.j2
statuses:
  proposal: {category: backlog, description: Awaiting triage., protected: false, selected: false, answered: true}
  accepted: {category: todo, description: Ready for the team., protected: true, selected: true, answered: false}
  deferred: {category: draft, description: Parked., protected: true, selected: false, answered: true}
  queued: {category: queued, description: Claimed., protected: true, selected: false, answered: false}
  in-progress: {category: in-progress, description: Being fixed., protected: true, selected: false, answered: false}
  done: {category: done, description: Fixed., protected: true, selected: false, answered: false}
  cancelled: {category: cancelled, description: Dropped., protected: false, selected: false, answered: false}
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

/// The host's task template, both modes in its own words.
const TASK: &str = "{% if mode == \"feedback\" %}# Comments for {{ batch }}

Account for every comment in {{ responses }}, then run `{{ check }}`.
{% for quoted in comments %}
{{ quoted.anchor }}
- Its ticket: `{{ quoted.ticket }}`
- What onefollowup last wrote of it: `{{ quoted.last_written }}`
- Reply opening: `{{ quoted.reply_opening }}`
- Marker if it confirms: `{{ quoted.marker_confirms }}`
- Marker if it does not: `{{ quoted.marker_does_not_confirm }}`

{{ quoted.body }}
{% endfor %}{% else %}# Incidents of {{ batch }}

Answer every input in {{ answers }}; tickets go under {{ tickets }}.
{% endif %}";

const TICKET: &str = "## Summary\n\n<what happened>\n\n## Impact\n\n<{{ rubric.impact_parts | join(', ') }}, then the lines {{ rubric.impact_lines | join(', ') }}>\n\n## Evidence\n\n<the host, and what was seen>\n";

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the host's setup, its batch, then its comment pass"
)]
fn a_host_answers_comments_by_configuration_alone() {
    let mut world = World::new(&[DESTINATION], ADOPTED);
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
    world.seed(DESTINATION, 12);

    let draft = world.draft(
        BATCH,
        "the queue worker drops jobs",
        &["src/queue.rs"],
        "Jobs vanish.\n",
    );
    world.ok("file", &["prepare", "--batch", BATCH], "");
    let record = json!({
        "schema": 14, "root_cause": "queue-drops-jobs", "repository": REPOSITORY,
        "created_by_run": BATCH, "owning_runs": [BATCH], "drafts": [draft],
        "basis": {REPOSITORY: COMMIT}, "verified_at": "2026-11-07T10:00:00Z", "host": HOST,
        "frequency": "intermittent",
    });
    let ticket = format!(
        "---\ntitle: \"widgets: the queue worker drops jobs\"\nmetadata:\n  orchestrator.follow-up: {record}\n---\n## Summary\n\nJobs vanish from the queue.\n\n## Impact\n\n**Who.** Customers with queued jobs.\n**Cost.** A lost job each time.\n\n- Severity: sev2\n- Mitigation: retry the job\n- Residual severity: sev3\n\n## Evidence\n\nHost {HOST}: the worker log shows the drop.\n"
    );
    let path = world
        .work()
        .join(format!("tasks/{BATCH}/tickets/queue-drops-jobs.md"));
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the tickets directory");
    std::fs::write(&path, ticket).expect("the ticket");
    world.ok(
        "file",
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
    let checked = world.ok("file", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("file", &["publish", "--batch", BATCH], "");
    world.ok("file", &["settle", "--batch", BATCH], "");
    let card = format!("{DESTINATION}:{}", world.filed(DESTINATION)[0].0);
    comment(
        &world,
        &card,
        "20261107T120000Z-1",
        "pat",
        "2026-11-07T12:00:00Z",
        "This happens after every restart, not at random.",
    );
    world.reset_phases();

    let gathered = world.ok("gather", &["comments", "gather"], "");
    assert_eq!(gathered["selected"], json!({BATCH: 1}), "{gathered}");
    assert_eq!(destination_ops(&world), ["comments-since"]);
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
    let text = std::fs::read_to_string(feedback).expect("the feedback file");
    assert!(
        text.contains(&format!("# Comments for {BATCH}")),
        "the host's own feedback task: {text}"
    );
    let one = &quoted(feedback)[0];
    edit_ticket(
        &one.ticket,
        "Jobs vanish from the queue.",
        "Jobs vanish from the queue after every restart.",
    );
    let reply = post_reply(
        &world,
        one,
        "confirms",
        "Every restart, as you say: the summary now says so.",
    );
    account(
        feedback,
        batch,
        &[response(one, &reply, "confirms", "Named the restart.")],
    );
    let checked = world.ok(
        "check",
        &[
            "comments",
            "check",
            "--feedback",
            &feedback.display().to_string(),
        ],
        "",
    );
    assert_eq!(checked["sound"], true, "{checked}");
    world.reset_phases();
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(
        destination_ops(&world),
        ["read", "update-many"],
        "{settled}"
    );

    assert_eq!(replies_to(&world, &card, "20261107T120000Z-1"), 1);
    let item = item_text(&world, &card);
    assert!(item.contains("after every restart."), "{item}");
    assert!(
        item.contains("severity with the workaround sev3; fires intermittently; 2 occurrences"),
        "{item}"
    );
    assert!(
        front(&item)["status"]
            .as_str()
            .is_some_and(|status| status.eq_ignore_ascii_case("Triage")),
        "the host's own status vocabulary stands: {item}"
    );
    assert!(
        watermark(&world, DESTINATION).is_some(),
        "the host's destination's watermark moved"
    );
}
