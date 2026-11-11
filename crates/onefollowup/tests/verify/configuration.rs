//! The triage configuration driving the loop, with no relations configured: the shipped
//! default, a multi-kind host routing kinds to their own destinations through ordered routes,
//! and the estimate model's exact priorities.

use serde_json::{Value, json};

use crate::journey::{front, record};
use crate::world::{Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-06";

#[test]
fn the_shipped_default_is_one_root_cause_kind_and_four_dispositions() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    let kinds = world.ok("kinds", &["kinds"], "");
    let names: Vec<&str> = kinds["kinds"]
        .as_array()
        .expect("kinds")
        .iter()
        .map(|kind| kind["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, ["root-cause"]);
    assert_eq!(kinds["kinds"][0]["duplicates"], "agent");
    let dispositions: Vec<&str> = kinds["dispositions"]
        .as_array()
        .expect("dispositions")
        .iter()
        .map(|one| one["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        dispositions,
        [
            "filed",
            "not-reproducible",
            "already-fixed",
            "too-low-impact"
        ]
    );
    assert_eq!(kinds["kinds"][0]["dispositions"], json!(dispositions));
}

/// A host of bugs, improvements and security findings, each kind with its own destination
/// but improvements, and ordered routes over bugs.
const TRIAGE: &str = "host_label: build-host-1
destination: improvements
default_kind: bug
fields:
  error_signature:
    description: The error's signature, as the log prints it.
dispositions:
  - {name: filed, description: Filed., requires: [ticket]}
  - {name: not-reproducible, description: It does not hold., requires: [reason]}
  - {name: too-low-impact, description: Below the bar., requires: [reason]}
  - {name: wont-fix, description: It holds and will not be fixed., requires: [reason]}
  - {name: fixed-upstream, description: A change upstream fixed it., requires: [change]}
default_dispositions: [filed, not-reproducible, too-low-impact]
kinds:
  - name: bug
    description: A defect.
    destination: bugs
    dispositions: [filed, not-reproducible, wont-fix, fixed-upstream]
  - name: improvement
    description: An improvement.
    priority: {fixed: low}
  - name: security
    description: A security finding.
    destination: security
    duplicates: {same: [error_signature]}
routes:
  - name: on-call
    match: {kind: [bug], severity: [critical]}
    set: {status_role: accepted, priority: urgent, labels: [incident]}
    command: [notify]
  - name: high-bugs
    match: {kind: [bug], severity: [high]}
    set: {labels: [triage]}
  - name: gated
    match: {kind: [bug], path: [\"src/gated/**\"]}
    command: [gate]
";

fn kind(mut ticket: Ticket, kind: &str) -> Ticket {
    ticket.kind = Some(kind.to_owned());
    ticket
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey, end to end")]
fn kinds_destinations_dispositions_and_ordered_routes_drive_the_loop() {
    let mut world = World::new(&["improvements", "bugs", "security", "gadget-bugs"], TRIAGE);
    // The store's own route on the bugs destination: a gadgets bug lands in gadget-bugs.
    let store = std::fs::read_to_string(world.root.join("onetaskgraph.yaml")).expect("the store");
    std::fs::write(
        world.root.join("onetaskgraph.yaml"),
        store.replace(
            "  bugs:\n    plugin: local-md\n    config:\n      root: bugs-root\n",
            "  bugs:\n    plugin: local-md\n    config:\n      root: bugs-root\n    routes:\n      - {repositories: [github.com/example/gadgets], to: gadget-bugs}\n",
        ),
    )
    .expect("the store");
    let root = world.root.display().to_string();
    world.script(
        "notify",
        &format!("cat >> {root}/notify.log\necho >> {root}/notify.log"),
    );
    world.script("gate", "echo 'the gate is closed' >&2\nexit 4");

    let crash = world.draft(
        BATCH,
        "saving crashes the app",
        &["src/save.rs"],
        "Crash.\n",
    );
    let high = world.draft(
        BATCH,
        "exports drop the last row",
        &["src/export.rs"],
        "Row lost.\n",
    );
    let gated = world.draft(
        BATCH,
        "the gated panel misdraws",
        &["src/gated/panel.rs"],
        "Misdrawn.\n",
    );
    let gadget = world.draft_in(
        BATCH,
        "github.com/example/gadgets",
        "gadget sync stalls",
        &["src/sync.rs"],
        "Stalls.\n",
    );
    let idea = world.draft(
        BATCH,
        "the palette could remember colours",
        &["src/palette.rs"],
        "An idea.\n",
    );
    let tokens = world.draft(
        BATCH,
        "a token is logged on login",
        &["src/login.rs"],
        "Token.\n",
    );
    let cookie = world.draft(
        BATCH,
        "a token is logged on logout",
        &["src/logout.rs"],
        "Token.\n",
    );
    let upstream = world.draft(BATCH, "fonts render blurry", &["src/font.rs"], "Blurry.\n");
    let vague = world.draft(BATCH, "icons render blurry", &["src/icon.rs"], "Blurry.\n");
    let declined_idea = world.draft(
        BATCH,
        "the palette could sort colours",
        &["src/palette.rs"],
        "Idea.\n",
    );
    let declined_bug = world.draft(
        BATCH,
        "a tooltip flickers",
        &["src/tooltip.rs"],
        "Flicker.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");

    let tickets = [
        kind(
            Ticket::new("save-crashes", "saving crashes the app", &[&crash])
                .severities("critical", "critical", "none"),
            "bug",
        ),
        kind(
            Ticket::new("export-drops-row", "exports drop the last row", &[&high]),
            "bug",
        ),
        kind(
            Ticket::new(
                "gated-panel-misdraws",
                "the gated panel misdraws",
                &[&gated],
            )
            .severities("medium", "low", "reopen the panel"),
            "bug",
        ),
        {
            let mut ticket = kind(
                Ticket::new("gadget-sync-stalls", "gadget sync stalls", &[&gadget]),
                "bug",
            );
            ticket.repository = "github.com/example/gadgets".to_owned();
            ticket
        },
        kind(
            Ticket::new(
                "palette-remembers-colours",
                "the palette could remember colours",
                &[&idea],
            ),
            "improvement",
        ),
    ];
    for ticket in &tickets {
        world.ticket(BATCH, ticket);
        let staged = world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
        assert_eq!(staged["write"]["op"], "copy", "{staged}");
    }
    let status = world.ok(
        "agent",
        &["ticket", "board-status", "--batch", BATCH, "save-crashes"],
        "",
    );
    assert_eq!(
        status["status_role"], "accepted",
        "the first route's set: {status}"
    );
    assert_eq!(status["priority"], "urgent", "{status}");
    let status = world.ok(
        "agent",
        &[
            "ticket",
            "board-status",
            "--batch",
            BATCH,
            "palette-remembers-colours",
        ],
        "",
    );
    assert_eq!(
        status["priority"], "low",
        "an improvement's fixed priority: {status}"
    );
    for (slug, draft, signature) in [
        ("token-on-login", &tokens, "E-9"),
        ("token-on-logout", &cookie, " e-9 "),
    ] {
        let mut ticket = kind(Ticket::new(slug, "a token is logged", &[draft]), "security");
        ticket.fields = vec![("error_signature".to_owned(), signature.to_owned())];
        world.ticket(BATCH, &ticket);
    }
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "token-on-login"],
        "",
    );
    let merged = world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "token-on-logout"],
        "",
    );
    assert_eq!(
        merged["matched"],
        format!("drafts:{BATCH}/tickets/token-on-login"),
        "{merged}"
    );
    std::fs::remove_file(
        world
            .work()
            .join(format!("tasks/{BATCH}/tickets/token-on-logout.md")),
    )
    .expect("folded in");

    let answers = |upstream_detail: &str, idea_disposition: &str| {
        vec![
            answer(&crash, "bug", "filed", &["save-crashes"], "Verified."),
            answer(&high, "bug", "filed", &["export-drops-row"], "Verified."),
            answer(
                &gated,
                "bug",
                "filed",
                &["gated-panel-misdraws"],
                "Verified.",
            ),
            answer(
                &gadget,
                "bug",
                "filed",
                &["gadget-sync-stalls"],
                "Verified.",
            ),
            answer(
                &idea,
                "improvement",
                "filed",
                &["palette-remembers-colours"],
                "Verified.",
            ),
            answer(
                &tokens,
                "security",
                "filed",
                &["token-on-login"],
                "Verified.",
            ),
            answer(
                &cookie,
                "security",
                "filed",
                &["token-on-login"],
                "The same signature.",
            ),
            answer(
                &upstream,
                "bug",
                "fixed-upstream",
                &[],
                "Fixed by https://github.com/example/fonts/pull/12.",
            ),
            answer(&vague, "bug", "fixed-upstream", &[], upstream_detail),
            answer(
                &declined_idea,
                "improvement",
                idea_disposition,
                &[],
                "Not worth it.",
            ),
            answer(&declined_bug, "bug", "wont-fix", &[], "Not worth it."),
        ]
    };
    world.answer(BATCH, &answers("Fixed upstream somewhere.", "wont-fix"));
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
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
    assert_eq!(problems.len(), 2, "{problems:#?}");
    assert_eq!(problems[0].0, vague);
    assert!(
        problems[0]
            .1
            .starts_with("the disposition `fixed-upstream` requires the change"),
        "{problems:#?}"
    );
    assert_eq!(problems[1].0, declined_idea);
    assert!(
        problems[1].1.starts_with("the kind `improvement` does not allow the disposition `wont-fix`; it allows `filed`, `not-reproducible`, `too-low-impact`"),
        "a kind listing no dispositions takes default_dispositions: {problems:#?}"
    );
    world.answer(
        BATCH,
        &answers("Fixed by 0123abc upstream.", "too-low-impact"),
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");

    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let gated_outcome = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .find(|one| one["outcome"] == "route-command-failed")
        .expect("the gated route withheld its ticket");
    assert!(
        gated_outcome["error"]
            .as_str()
            .is_some_and(|error| error.contains("the gate is closed"))
    );
    let copies: Vec<&Value> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .filter(|one| one["outcome"] == "created")
        .collect();
    assert_eq!(copies.len(), 5, "every other ticket landed: {published}");
    let gated_receipt = world
        .receipts(BATCH)
        .into_iter()
        .find(|receipt| receipt["outcome"]["error"]["kind"] == "route-command-failed")
        .expect("receipted");
    assert_eq!(
        gated_receipt["ticket"],
        format!("drafts:{BATCH}/tickets/gated-panel-misdraws")
    );

    let bugs = world.filed("bugs");
    assert_eq!(
        bugs.len(),
        2,
        "the critical and the high bug; the gated one withheld: {bugs:#?}"
    );
    let crashing = bugs
        .iter()
        .map(|(_, text)| text)
        .find(|text| record(text)["root_cause"] == "save-crashes")
        .expect("the critical bug");
    assert_eq!(
        front(crashing)["status"],
        "todo",
        "the route's accepted role"
    );
    assert_eq!(front(crashing)["priority"], "urgent");
    assert!(
        crashing.contains("incident"),
        "the route's labels: {crashing}"
    );
    assert_eq!(
        record(crashing)["kind"],
        Value::Null,
        "the default kind is not recorded"
    );
    let exporting = bugs
        .iter()
        .map(|(_, text)| text)
        .find(|text| record(text)["root_cause"] == "export-drops-row")
        .expect("the high bug");
    assert!(
        exporting.contains("triage"),
        "the next route's labels: {exporting}"
    );
    assert_eq!(front(exporting)["priority"], "medium");
    let gadgets = world.filed("gadget-bugs");
    assert_eq!(
        gadgets.len(),
        1,
        "the store's route on the bugs destination applied"
    );
    assert_eq!(record(&gadgets[0].1)["root_cause"], "gadget-sync-stalls");
    let improvements = world.filed("improvements");
    assert_eq!(
        improvements.len(),
        1,
        "the improvement on the top-level destination"
    );
    assert_eq!(front(&improvements[0].1)["priority"], "low");
    assert_eq!(record(&improvements[0].1)["kind"], "improvement");
    let security = world.filed("security");
    assert_eq!(
        security.len(),
        1,
        "two findings of one signature are one ticket"
    );
    assert_eq!(
        record(&security[0].1)["fields"],
        json!({"error_signature": "E-9"})
    );

    let notified = std::fs::read_to_string(world.root.join("notify.log")).expect("notify ran");
    let notified: Vec<Value> = notified
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).expect("JSON"))
        .collect();
    assert_eq!(
        notified.len(),
        1,
        "the notify command ran for its one ticket"
    );
    assert_eq!(notified[0]["route"], "on-call");
    assert_eq!(notified[0]["kind"], "bug");
    assert_eq!(notified[0]["severity"], "critical");
    assert_eq!(notified[0]["ticket"]["root_cause"], "save-crashes");
}

/// The estimate model, raising at two occurrences.
const ESTIMATE: &str = "host_label: build-host-1\nestimate: {raise_at: 2, raise_cap: high}\n";

#[test]
fn the_estimate_model_computes_each_priority_from_recorded_facts() {
    let mut world = World::new(&["followups"], ESTIMATE);
    for (id, frequency, evidence) in [
        ("quiet-older", "intermittent", 0),
        ("steady-older", "consistent", 1),
    ] {
        let body = filed_body(&format!("The {id} item drops unicode."))
            .replace("Severity with the workaround: medium", "Severity with the workaround: low")
            .replace(
                "- Priority estimate: medium (severity with the workaround medium; fires intermittently; 1 occurrence; not raised)",
                "- Priority estimate: low (severity with the workaround low; fires intermittently; 1 occurrence; not raised)",
            );
        let comments = if evidence == 0 {
            String::new()
        } else {
            "\n## Comments\n\n<!-- onetaskgraph:comment id=\"20261002T000000Z-1\" author=\"other\" created_at=\"2026-10-02T00:00:00Z\" updated_at=\"2026-10-02T00:00:00Z\" -->\n### other — 2026-10-02T00:00:00Z\n\nAdditional evidence from follow-up run `nightly-2026-10-02`.\n\nSeen.\n\n<!-- orchestrator:follow-up-comment run=\"nightly-2026-10-02\" root_cause=\"steady-older\" -->\n\n<!-- /onetaskgraph:comment -->\n".to_owned()
        };
        world.item(
            "followups",
            id,
            &filed_item(
                &format!("widgets: the {id} item drops unicode"),
                "nightly-2026-10-01",
                id,
                &json!({"frequency": frequency, "priority_estimate": "low", "board_estimate": "low"}),
                "low",
            ),
            &format!("{body}{comments}"),
        );
    }
    let mut drafts = Vec::new();
    for title in [
        "a low one is rare",
        "a medium one is rare",
        "a low one is steady",
        "a high one is steady",
        "a critical one stays critical",
        "the quiet-older item drops unicode",
        "the steady-older item drops unicode",
    ] {
        drafts.push(world.draft(BATCH, title, &["src/a.rs"], "Seen.\n"));
    }
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let cases = [
        (
            "low-rare",
            ("medium", "low", "retry"),
            "intermittent",
            "low (severity with the workaround low; fires intermittently; 1 occurrence; not raised)",
        ),
        (
            "medium-rare",
            ("high", "medium", "retry"),
            "intermittent",
            "medium (severity with the workaround medium; fires intermittently; 1 occurrence; not raised)",
        ),
        (
            "low-steady",
            ("medium", "low", "retry"),
            "consistent",
            "medium (severity with the workaround low; fires consistently; 1 occurrence; raised one level because it fires consistently)",
        ),
        (
            "high-steady",
            ("high", "high", "none"),
            "consistent",
            "high (severity with the workaround high; fires consistently; 1 occurrence; already high, where a raise is capped, so not raised though it fires consistently)",
        ),
        (
            "critical-stays",
            ("critical", "critical", "none"),
            "intermittent",
            "urgent (severity with the workaround critical; fires intermittently; 1 occurrence; not raised)",
        ),
    ];
    let mut answers = Vec::new();
    for ((slug, (severity, mitigated, workaround), frequency, line), draft) in
        cases.iter().zip(&drafts)
    {
        let mut ticket = Ticket::new(slug, "an estimated ticket", &[draft])
            .severities(severity, mitigated, workaround);
        ticket.frequency = Some((*frequency).to_owned());
        world.ticket(BATCH, &ticket);
        let status = world.ok(
            "agent",
            &["ticket", "board-status", "--batch", BATCH, slug],
            "",
        );
        assert_eq!(
            status["estimate_line"],
            format!("- Priority estimate: {line}"),
            "{slug}"
        );
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
        answers.push(answer(draft, "root-cause", "filed", &[slug], "Verified."));
    }
    for (item, draft) in [("quiet-older", &drafts[5]), ("steady-older", &drafts[6])] {
        world.ok(
            "agent",
            &[
                "ticket",
                "comment",
                "--batch",
                BATCH,
                "--item",
                &format!("followups:{item}"),
                "--root-cause",
                item,
            ],
            "Seen again.\n",
        );
        world.ok(
            "agent",
            &[
                "ticket",
                "re-estimate",
                "--batch",
                BATCH,
                &format!("followups:{item}"),
            ],
            "",
        );
        answers.push(answer(
            draft,
            "root-cause",
            "filed",
            &[item],
            "Evidence on the item.",
        ));
    }
    world.answer(BATCH, &answers);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    for (slug, _, _, line) in cases {
        let (_, text) = world
            .filed("followups")
            .into_iter()
            .find(|(_, text)| record(text)["root_cause"] == slug)
            .expect("filed");
        let level = line.split(' ').next().unwrap_or_default();
        assert_eq!(front(&text)["priority"], level, "{slug}");
        assert_eq!(record(&text)["priority_estimate"], level, "{slug}");
        assert!(
            text.contains(&format!("- Priority estimate: {line}")),
            "{text}"
        );
    }
    let quiet = world.read_item("followups", "quiet-older");
    assert!(quiet.contains("- Priority estimate: medium (severity with the workaround low; fires intermittently; 2 occurrences; raised one level because it has 2 or more occurrences)"), "{quiet}");
    assert_eq!(
        front(&quiet)["priority"],
        "medium",
        "the raised priority the count implies"
    );
    let steady = world.read_item("followups", "steady-older");
    assert!(steady.contains("- Priority estimate: medium (severity with the workaround low; fires consistently; 3 occurrences; raised one level because it fires consistently and it has 2 or more occurrences)"), "raised once when both hold: {steady}");
    assert_eq!(record(&steady)["priority_estimate"], "medium");
}
