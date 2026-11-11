//! The rules each verb holds its input to, through the library: a draft, a ticket file, a
//! hook's inputs and the agent's tools each refused naming what breaks; a route command's
//! override applied and a malformed one refused; and `settle` reading back writes whose answers
//! were all lost.

use serde_json::{Value, json};

use crate::journey::{HOOKED, front, hooks, inputs, record};
use crate::world::{COMMIT, HOST, REPOSITORY, Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-09";

fn draft_args<'a>(title: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "draft",
        "--batch",
        BATCH,
        "--title",
        title,
        "--repository",
        REPOSITORY,
    ];
    args.extend_from_slice(extra);
    args
}

#[test]
fn a_draft_is_refused_naming_what_breaks_and_each_author_form_is_recorded() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    for (args, body, refused) in [
        (
            vec![
                "draft",
                "--batch",
                "not a batch",
                "--title",
                "t",
                "--repository",
                REPOSITORY,
            ],
            "Body.\n",
            "is not a batch id",
        ),
        (
            draft_args("  ", &[]),
            "Body.\n",
            "a draft's title is one line",
        ),
        (
            draft_args("two\nlines", &[]),
            "Body.\n",
            "a draft's title is one line",
        ),
        (
            vec![
                "draft",
                "--batch",
                BATCH,
                "--title",
                "t",
                "--repository",
                "widgets",
            ],
            "Body.\n",
            "is not a normalized repository origin",
        ),
        (
            draft_args("t", &["--stamp", "colour=blue"]),
            "Body.\n",
            "`colour` is not a stamp a draft records",
        ),
        (
            draft_args("t", &["--stamp", "session=a\u{7}b"]),
            "Body.\n",
            "the stamp `session` carries a control character",
        ),
        (
            draft_args("t", &[]),
            " \n",
            "a draft's body, read from stdin, is empty",
        ),
        (
            draft_args("t", &["--author", "node:"]),
            "Body.\n",
            "is not `node[:<node>]`",
        ),
        (
            draft_args("t", &["--author", "monitor"]),
            "Body.\n",
            "is not `node[:<node>]`",
        ),
        (
            draft_args("t", &["--author", "manager:m"]),
            "Body.\n",
            "is not `node[:<node>]`",
        ),
        (
            draft_args("t", &["--author", "robot"]),
            "Body.\n",
            "is not `node[:<node>]`",
        ),
    ] {
        let stderr = world.fails(2, &args, body);
        assert!(stderr.contains(refused), "{args:?}: {stderr}");
    }
    assert!(
        world.filed("drafts").is_empty(),
        "no refused draft was recorded"
    );

    for (extra, author) in [
        (
            vec!["--author", "monitor:m-1"],
            json!({"kind": "monitor", "member": "m-1"}),
        ),
        (
            vec!["--author", "pacemaker", "--stamp", "member=p-2"],
            json!({"kind": "pacemaker", "member": "p-2"}),
        ),
        (vec!["--author", "manager"], json!({"kind": "manager"})),
        (
            vec!["--stamp", "node=n-3"],
            json!({"kind": "node", "node": "n-3"}),
        ),
        (vec![], json!({"kind": "node", "node": "unresolved"})),
    ] {
        let printed = world.ok("draft", &draft_args("an author form", &extra), "Body.\n");
        let path = printed["path"].as_str().expect("the draft's file");
        let text = std::fs::read_to_string(path).expect("the draft");
        let recorded = &front(&text)["metadata"]["orchestrator.follow-up-draft"]["author"];
        for (key, value) in author.as_object().expect("an object") {
            assert_eq!(&recorded[key], value, "{extra:?}: {text}");
        }
    }
}

#[test]
fn prepare_refuses_a_hook_that_cannot_run_or_prints_a_malformed_input_set() {
    let mut world = World::new(&["followups"], HOOKED);
    hooks(&world);
    world.draft(BATCH, "a draft", &["src/a.rs"], "Body.\n");
    for (printed, refused) in [
        (
            json!({"schema_version": 2, "inputs": []}),
            "schema_version 2",
        ),
        (
            json!({"schema_version": 1, "inputs": [{"id": "has space", "source": "s", "summary": "x", "detail": ""}]}),
            "with an empty or spaced id",
        ),
        (
            json!({"schema_version": 1, "inputs": [
                {"id": "budget:a", "source": "s", "summary": "x", "detail": ""},
                {"id": "budget:a", "source": "s", "summary": "y", "detail": ""},
            ]}),
            "the id `budget:a` twice",
        ),
    ] {
        std::fs::write(world.root.join("inputs.json"), printed.to_string()).expect("written");
        let stderr = world.fails(1, &["prepare", "--batch", BATCH], "");
        assert!(stderr.contains("hooks.batch_preparing"), "{stderr}");
        assert!(stderr.contains(refused), "{stderr}");
    }
    let stderr = world.fails(
        1,
        &[
            "--set",
            "hooks.batch_preparing=[no-such-hook-anywhere]",
            "prepare",
            "--batch",
            BATCH,
        ],
        "",
    );
    assert!(stderr.contains("could not run"), "{stderr}");
    assert!(
        !world.work().join(format!("tasks/{BATCH}")).exists(),
        "nothing was snapshotted"
    );

    // Drafts recorded under an alias join the batch; an alias that is no batch id is refused.
    inputs(&world, &json!([]));
    world.draft(
        "nightly-alias",
        "an aliased draft",
        &["src/b.rs"],
        "Body.\n",
    );
    let stderr = world.fails(
        2,
        &["prepare", "--batch", BATCH, "--alias", "not an id"],
        "",
    );
    assert!(stderr.contains("is not a batch id"), "{stderr}");
    let prepared = world.ok(
        "prepare",
        &[
            "prepare",
            "--batch",
            BATCH,
            "--alias",
            "nightly-alias",
            "--alias",
            "nightly-alias",
            "--alias",
            BATCH,
        ],
        "",
    );
    assert_eq!(prepared["drafts"], 2, "{prepared}");
}

/// What `ticket validate` finds of the batch's ticket file `slug`.
fn problems(world: &mut World, slug: &str) -> Vec<String> {
    let validated = world.ok("agent", &["ticket", "validate", "--batch", BATCH, slug], "");
    validated["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|one| one.as_str().unwrap_or_default().to_owned())
        .collect()
}

fn raw(world: &World, slug: &str, text: &str) {
    let path = world
        .work()
        .join(format!("tasks/{BATCH}/tickets/{slug}.md"));
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the tickets directory");
    std::fs::write(path, text).expect("the ticket");
}

fn assert_finds(found: &[String], wanted: &[&str], what: &str) {
    for one in wanted {
        assert!(
            found.iter().any(|problem| problem.contains(one)),
            "{what}: no problem says {one:?}: {found:#?}"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines, reason = "one rule per case, in one table")]
fn validate_names_every_rule_a_ticket_file_breaks() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    let draft = world.draft(BATCH, "a draft", &["src/a.rs"], "Body.\n");
    let stderr = world.fails(2, &["ticket", "validate", "--batch", BATCH, "x"], "");
    assert!(stderr.contains("there is no ticket `x`"), "{stderr}");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");

    // The file's shape.
    for (slug, text, wanted) in [
        (
            "no-front-matter",
            "Just prose.\n".to_owned(),
            vec!["opens with no front matter"],
        ),
        (
            "not-a-mapping",
            "---\n- one\n- two\n---\nBody.\n".to_owned(),
            vec!["is not a YAML mapping"],
        ),
        (
            "stray-keys",
            "---\nlabels: [x]\nstatus: sideways\nmetadata:\n  other: 1\n---\n## Root cause\n\nX.\n"
                .to_owned(),
            vec![
                "carries `labels`, which a ticket does not",
                "carries no `title` string",
                "is not a status role",
                "the metadata carries `other`",
                "carries no `orchestrator.follow-up` record object",
            ],
        ),
        (
            "no-metadata",
            "---\ntitle: \"widgets: x\"\n---\nBody.\n".to_owned(),
            vec!["carries no `metadata` mapping"],
        ),
        (
            "broken-record",
            "---\ntitle: \"widgets: x\"\nmetadata:\n  orchestrator.follow-up: {schema: 14}\n---\nBody.\n"
                .to_owned(),
            vec!["its `orchestrator.follow-up` record:"],
        ),
    ] {
        raw(&world, slug, &text);
        assert_finds(&problems(&mut world, slug), &wanted, slug);
    }

    // The record and the title.
    let sound = Ticket::new("a-sound-ticket", "a sound ticket", &[&draft]);
    raw(
        &world,
        "Not_A_Slug",
        &sound.render(BATCH).replace(
            "title: \"widgets: a sound ticket\"",
            &format!("title: \"widgets: {}\\a\"", "x".repeat(200)),
        ),
    );
    assert_finds(
        &problems(&mut world, "Not_A_Slug"),
        &[
            "is not a root-cause slug",
            "the title is longer than",
            "the title carries a control character",
        ],
        "the slug and title",
    );
    let mut wrong = Ticket::new("record-breaks-rules", "a broken record", &[&draft]);
    wrong.extra = vec![
        ("root_cause".to_owned(), json!("another-cause")),
        ("owning_runs".to_owned(), json!(["another-batch"])),
        ("drafts".to_owned(), json!([])),
        (
            "basis".to_owned(),
            json!({"not an origin": COMMIT, "github.com/example/other": "abc"}),
        ),
        ("verified_at".to_owned(), json!("yesterday")),
        ("host".to_owned(), json!("not a host!")),
    ];
    world.ticket(BATCH, &wrong);
    assert_finds(
        &problems(&mut world, "record-breaks-rules"),
        &[
            "the record's root cause `another-cause` is not the file's",
            "does not name this batch",
            "names no draft or input",
            "names no commit for its repository",
            "the basis names `not an origin`",
            "is not a 40-character commit",
            "is not an RFC 3339 time",
            "is not a host name",
        ],
        "the record",
    );
    let mut stranger = Ticket::new("names-a-stranger", "a stranger", &["drafts:elsewhere/x"]);
    stranger.extra = vec![("repository".to_owned(), json!("widgets"))];
    world.ticket(BATCH, &stranger);
    assert_finds(
        &problems(&mut world, "names-a-stranger"),
        &[
            "is not a normalized origin",
            "which is not one of the batch's inputs",
        ],
        "a stranger",
    );
    let mut misnamed = Ticket::new("misnamed-title", "misnamed", &[&draft]);
    misnamed.repository = "github.com/example/gadgets".to_owned();
    raw(
        &world,
        "misnamed-title",
        &misnamed
            .render(BATCH)
            .replace("title: \"gadgets: ", "title: \"widgets: "),
    );
    assert_finds(
        &problems(&mut world, "misnamed-title"),
        &["the title does not open with `gadgets: `"],
        "the title's repository",
    );
    let mut unknown = Ticket::new("unknown-kind", "an unknown kind", &[&draft]);
    unknown.kind = Some("chore".to_owned());
    world.ticket(BATCH, &unknown);
    assert_finds(
        &problems(&mut world, "unknown-kind"),
        &["the record's kind `chore` is not a configured kind"],
        "the kind",
    );

    // The body against the ticket template and the rubric.
    raw(
        &world,
        "sections-out-of-order",
        &sound
            .render(BATCH)
            .replacen("## Root cause", "## Stray notes\n\nX.\n\n## Root cause", 1)
            .replace("## Examples\n\na sound ticket.", "## Examples\n")
            .replace(
                "## Owning batches",
                "## Root cause\n\nAgain.\n\n## Owning batches",
            ),
    );
    assert_finds(
        &problems(&mut world, "sections-out-of-order"),
        &[
            "`## Stray notes` is not a section its ticket template has",
            "`## Root cause` is out of the order its ticket template states",
            "`## Examples` is empty",
        ],
        "the sections",
    );
    let mut no_impact = Ticket::new("no-impact", "no impact", &[&draft]);
    no_impact.without = vec!["Impact".to_owned()];
    world.ticket(BATCH, &no_impact);
    assert_finds(
        &problems(&mut world, "no-impact"),
        &[
            "missing the `## Impact` section its ticket template requires, before `## Examples`",
            "no `## Impact` section, so there is no severity",
        ],
        "no impact",
    );
    raw(
        &world,
        "impact-out-of-shape",
        &sound
            .render(BATCH)
            .replace("**What it costs them.**", "**What it costs.**")
            .replace("- Workaround: ", "- Work-around: "),
    );
    assert_finds(
        &problems(&mut world, "impact-out-of-shape"),
        &[
            "does not carry its `**What it costs them.**` part",
            "where the rubric states each of",
        ],
        "the impact's shape",
    );
    for (slug, severities, wanted) in [
        (
            "levels-off-the-rubric",
            ("dire", "mild", "restart"),
            "which is not one of the rubric's levels",
        ),
        (
            "empty-workaround",
            ("high", "medium", ""),
            "the workaround line is empty",
        ),
        (
            "raised-by-workaround",
            ("medium", "high", "restart"),
            "is above the severity",
        ),
        (
            "no-workaround-lowered",
            ("high", "medium", "none"),
            "with a workaround of `none`",
        ),
        (
            "workaround-not-lowering",
            ("high", "high", "restart"),
            "an acceptable workaround lowers the severity",
        ),
    ] {
        let (severity, mitigated, workaround) = severities;
        let ticket = Ticket::new(slug, slug, &[&draft]).severities(severity, mitigated, workaround);
        world.ticket(BATCH, &ticket);
        assert_finds(&problems(&mut world, slug), &[wanted], slug);
    }

    raw(
        &world,
        "empty-title",
        &sound
            .render(BATCH)
            .replace("title: \"widgets: a sound ticket\"", "title: \"  \""),
    );
    assert_finds(
        &problems(&mut world, "empty-title"),
        &["the title is empty"],
        "an empty title",
    );
    // A heading inside a fence is the fence's text, not a section.
    let mut fenced = Ticket::new("fenced-evidence", "fenced evidence", &[&draft]);
    fenced.evidence =
        "The log:\n\n```text\n## Root cause\n````\nstill fenced\n```\n\nSeen once.".to_owned();
    world.ticket(BATCH, &fenced);
    assert_eq!(
        problems(&mut world, "fenced-evidence"),
        Vec::<String>::new()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let tickets = world.work().join(format!("tasks/{BATCH}/tickets"));
        std::fs::set_permissions(&tickets, std::fs::Permissions::from_mode(0o000))
            .expect("unreadable");
        let ran = world.run(&["check", "--batch", BATCH], "");
        std::fs::set_permissions(&tickets, std::fs::Permissions::from_mode(0o755))
            .expect("readable");
        assert_eq!(ran.code, 1, "{}", ran.stdout);
        assert!(ran.stderr.contains("could not be read"), "{}", ran.stderr);
    }

    // A ticket named by its file validates as by its root cause.
    world.ticket(BATCH, &sound);
    let path = world
        .work()
        .join(format!("tasks/{BATCH}/tickets/a-sound-ticket.md"));
    let validated = world.ok(
        "agent",
        &[
            "ticket",
            "validate",
            "--batch",
            BATCH,
            path.to_str().expect("a path"),
        ],
        "",
    );
    assert_eq!(validated["sound"], true, "{validated}");
}

#[test]
fn the_agents_tools_refuse_what_the_batch_did_not_read() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
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
    let draft = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "café.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let ticket = Ticket::new(
        "unicode-export",
        "the export dialog drops unicode titles",
        &[&draft],
    );
    world.ticket(BATCH, &ticket);
    for (args, stdin, refused) in [
        (
            vec![
                "ticket",
                "search",
                "--batch",
                BATCH,
                "--ticket",
                "unicode-export",
                "unicode",
                "--query",
                "  ",
            ],
            "",
            "question 2 is narrowed by nothing",
        ),
        (
            vec![
                "ticket",
                "board-status",
                "--batch",
                BATCH,
                "followups:nowhere",
            ],
            "",
            "prepare did not read `followups:nowhere`",
        ),
        (
            vec![
                "ticket",
                "comment",
                "--batch",
                BATCH,
                "--item",
                "followups:export-older",
                "--root-cause",
                "Not A Slug",
            ],
            "Evidence.\n",
            "is not a root-cause slug",
        ),
        (
            vec![
                "ticket",
                "comment",
                "--batch",
                BATCH,
                "--item",
                "followups:nowhere",
                "--root-cause",
                "unicode-export",
            ],
            "Evidence.\n",
            "is not an item this batch read",
        ),
        (
            vec![
                "ticket",
                "comment",
                "--batch",
                BATCH,
                "--item",
                "followups:export-older",
                "--root-cause",
                "unicode-export",
            ],
            "  \n",
            "the evidence, read from stdin, is empty",
        ),
    ] {
        let stderr = world.fails(2, &args, stdin);
        assert!(stderr.contains(refused), "{args:?}: {stderr}");
    }

    // A search reaching an item prepare did not read adds it to what the batch read.
    world.item(
        "followups",
        "late-arrival",
        &filed_item(
            "widgets: a late arrival about unicode",
            "nightly-2026-10-02",
            "late-arrival",
            &json!({}),
            "low",
        ),
        &filed_body("Unicode again."),
    );
    let found = world.ok(
        "agent",
        &[
            "ticket",
            "search",
            "--batch",
            BATCH,
            "--ticket",
            "unicode-export",
            "late arrival",
        ],
        "",
    );
    assert_eq!(
        found["hits"][0]["item"], "followups:late-arrival",
        "{found}"
    );
    let status = world.ok(
        "agent",
        &[
            "ticket",
            "board-status",
            "--batch",
            BATCH,
            "followups:late-arrival",
        ],
        "",
    );
    assert_eq!(status["title"], "widgets: a late arrival about unicode");

    // This batch's own item takes its evidence through its ticket's copy, not a comment.
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "unicode-export"],
        "",
    );
    world.answer(
        BATCH,
        &[answer(
            &draft,
            "root-cause",
            "filed",
            &["unicode-export"],
            "Verified.",
        )],
    );
    world.ok("check", &["check", "--batch", BATCH], "");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let own = world
        .filed("followups")
        .into_iter()
        .find(|(_, text)| record(text)["created_by_run"] == BATCH)
        .map(|(id, _)| format!("followups:{id}"))
        .expect("the batch's item");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let stderr = world.fails(
        2,
        &[
            "ticket",
            "comment",
            "--batch",
            BATCH,
            "--item",
            &own,
            "--root-cause",
            "unicode-export",
        ],
        "Evidence.\n",
    );
    assert!(stderr.contains("is this batch's own item"), "{stderr}");
}

const ROUTED: &str = "host_label: build-host-1
routes:
  - name: overridden
    match: {severity: [high]}
    command: [override]
  - name: garbled
    match: {severity: [medium]}
    command: [garble]
";

#[test]
fn a_route_commands_override_is_applied_and_a_malformed_one_withholds_its_ticket() {
    let mut world = World::new(&["followups"], ROUTED);
    world.script(
        "override",
        "cat > /dev/null\necho '{\"status_role\": \"accepted\", \"priority\": \"urgent\", \"labels\": [\"paged\"]}'",
    );
    world.script("garble", "cat > /dev/null\necho 'not an override'");
    let stderr = world.fails(2, &["publish", "--batch", BATCH], "");
    assert!(stderr.contains("has not been prepared"), "{stderr}");
    let urgent = world.draft(
        BATCH,
        "saving crashes the app",
        &["src/save.rs"],
        "Crash.\n",
    );
    let garbled = world.draft(BATCH, "exports drop a row", &["src/export.rs"], "Row.\n");
    let withdrawn = world.draft(BATCH, "a flaky test", &["tests/a.rs"], "Flaky.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let stderr = world.fails(2, &["publish", "--batch", BATCH], "");
    assert!(stderr.contains("no `check` of batch"), "{stderr}");
    world.answer(BATCH, &[]);

    let mut unjudged = Ticket::new("save-crashes", "saving crashes the app", &[&urgent]);
    unjudged.frequency = None;
    world.ticket(BATCH, &unjudged);
    world.ticket(
        BATCH,
        &Ticket::new("export-drops-row", "exports drop a row", &[&garbled]).severities(
            "medium",
            "low",
            "re-export",
        ),
    );
    let mut never =
        Ticket::new("flaky-test", "a flaky test", &[&withdrawn]).severities("low", "low", "none");
    never.status = Some("cancelled".to_owned());
    world.ticket(BATCH, &never);
    for slug in ["save-crashes", "export-drops-row", "flaky-test"] {
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    world.answer(
        BATCH,
        &[
            answer(
                &urgent,
                "root-cause",
                "filed",
                &["save-crashes"],
                "Verified.",
            ),
            answer(
                &garbled,
                "root-cause",
                "filed",
                &["export-drops-row"],
                "Verified.",
            ),
            answer(
                &withdrawn,
                "root-cause",
                "filed",
                &["flaky-test"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let outcomes: Vec<(&str, &str)> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| {
            (
                one["outcome"].as_str().unwrap_or_default(),
                one["error"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(outcomes[0].0, "created", "{published}");
    assert_eq!(outcomes[1].0, "route-command-failed", "{published}");
    assert!(outcomes[1].1.contains("is not an override"), "{published}");
    assert_eq!(outcomes[2].0, "nothing-to-withdraw", "{published}");
    let filed = world.filed("followups");
    assert_eq!(filed.len(), 1, "only the overridden ticket was written");
    let held = front(&filed[0].1);
    assert_eq!(held["priority"], "urgent", "{held}");
    assert_eq!(held["status"], "todo", "the route's role, accepted: {held}");
    assert_eq!(held["labels"][0]["name"], "paged", "{held}");
    let kinds: Vec<Value> = world
        .receipts(BATCH)
        .iter()
        .map(|receipt| receipt["outcome"].clone())
        .collect();
    assert_eq!(kinds.len(), 3, "every outcome is receipted: {kinds:?}");

    // An outbox with nothing staged publishes nothing.
    let mut quiet = World::new(&["followups"], "host_label: build-host-1\n");
    let only = quiet.draft("nightly-quiet", "nothing to file", &["src/q.rs"], "Q.\n");
    quiet.ok("prepare", &["prepare", "--batch", "nightly-quiet"], "");
    quiet.answer(
        "nightly-quiet",
        &[answer(
            &only,
            "root-cause",
            "not-reproducible",
            &[],
            "Not seen.",
        )],
    );
    quiet.ok("check", &["check", "--batch", "nightly-quiet"], "");
    let nothing = quiet.ok("publish", &["publish", "--batch", "nightly-quiet"], "");
    assert_eq!(nothing["outcomes"], json!([]), "{nothing}");
    assert!(quiet.filed("followups").is_empty());
}

#[cfg(unix)]
#[test]
fn settle_reads_back_every_write_whose_answer_was_lost_and_pends_one_that_never_landed() {
    use std::os::unix::fs::PermissionsExt as _;

    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
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
    world.item(
        "followups",
        "gone-later",
        &filed_item(
            "widgets: the importer skips blank rows",
            "nightly-2026-10-02",
            "importer-skips-blank-rows",
            &json!({}),
            "low",
        ),
        &filed_body("The importer skips blank rows."),
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
    let third = world.draft(
        BATCH,
        "the importer skips blank rows",
        &["src/import.rs"],
        "Rows.\n",
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
        &format!("Host {HOST}: café exported as caf.\n"),
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
    world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            BATCH,
            "--item",
            "followups:gone-later",
            "--root-cause",
            "importer-skips-blank-rows",
        ],
        &format!("Host {HOST}: a blank row skipped.\n"),
    );
    world.answer(
        BATCH,
        &[
            answer(
                &third,
                "root-cause",
                "filed",
                &["importer-skips-blank-rows"],
                "Evidence on the item.",
            ),
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

    // A person deletes one item, and no receipt can be written: every other write lands and
    // no answer is recorded.
    std::fs::remove_file(world.root.join("followups-root/tasks/gone-later.md")).expect("removed");
    let receipts = world.work().join("receipts");
    std::fs::create_dir_all(&receipts).expect("the receipts directory");
    std::fs::set_permissions(&receipts, std::fs::Permissions::from_mode(0o555)).expect("read-only");
    let failed = world.fails(1, &["publish", "--batch", BATCH], "");
    std::fs::set_permissions(&receipts, std::fs::Permissions::from_mode(0o755))
        .expect("writable again");
    assert!(failed.contains("run `onefollowup settle"), "{failed}");
    assert_eq!(world.filed("followups").len(), 2, "the copy landed");

    world.reset_phases();
    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(settled["read_back"], 4, "{settled}");
    assert_eq!(
        settled["confirmed"], 3,
        "each write that landed was found: {settled}"
    );
    assert_eq!(settled["pending"], 1, "{settled}");
    assert_eq!(settled["needs_attention"], true, "{settled}");
    let pending: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join("feedback/pending.json")).expect("pending.json"),
    )
    .expect("JSON");
    assert_eq!(
        pending["entries"][0]["item"], "followups:gone-later",
        "{pending}"
    );
    assert_eq!(pending["entries"][0]["write"], "comment", "{pending}");
    assert_eq!(pending["entries"][0]["state"], "unknown", "{pending}");
    let operations = world.destination_operations();
    let ops: Vec<&str> = operations.iter().map(|(_, op, _, _)| op.as_str()).collect();
    assert_eq!(
        ops,
        ["read", "find"],
        "one read of the items named, one search by origin for the new ticket: {operations:?}"
    );
}

#[test]
fn a_task_template_that_cannot_be_found_read_or_rendered_is_named() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    world.draft(BATCH, "a draft", &["src/a.rs"], "Body.\n");
    std::fs::create_dir_all(world.root.join("templates/partial.md.j2")).expect("a directory");
    std::fs::write(
        world.root.join("templates/broken.md.j2"),
        "{% if %}never closed\n",
    )
    .expect("written");
    std::fs::write(
        world.root.join("templates/includes.md.j2"),
        "{% include \"partial.md.j2\" %}\n",
    )
    .expect("written");
    std::fs::write(
        world.root.join("templates/includes-nothing.md.j2"),
        "{% include \"absent.md.j2\" %}\n",
    )
    .expect("written");
    for (template, named) in [
        ("builtin:absent.md.j2", "builtin:task-template.md.j2"),
        ("templates/absent.md.j2", "could not be read"),
        ("templates/broken.md.j2", "broken.md.j2"),
        (
            "templates/includes.md.j2",
            "partial.md.j2 could not be read",
        ),
        ("templates/includes-nothing.md.j2", "absent.md.j2"),
    ] {
        let set = format!("task_template={template}");
        let ran = world.run(&["--set", &set, "prepare", "--batch", BATCH], "");
        assert_ne!(ran.code, 0, "{template}: {}", ran.stdout);
        assert!(ran.stderr.contains(named), "{template}: {}", ran.stderr);
    }
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
}

/// A host whose `security` kind finds duplicates by `error_signature`.
const SAME: &str = "host_label: build-host-1
fields:
  error_signature:
    description: The error's signature, as the log prints it.
kinds:
  - name: root-cause
    description: One root cause.
  - name: security
    description: A security finding.
    duplicates: {same: [error_signature]}
";

#[test]
fn a_re_prepare_asks_for_each_written_tickets_values_and_check_names_stray_answers_and_files() {
    let mut world = World::new(&["followups"], SAME);
    world.seed("followups", 5);
    world.item(
        "followups",
        "signed-elsewhere",
        &filed_item(
            "widgets: an unrelated title",
            "nightly-2026-10-05",
            "unrelated-title",
            &json!({"kind": "security", "fields": {"error_signature": "TokenLeak: bearer"}}),
            "low",
        ),
        &filed_body("Nothing in common but the signature."),
    );
    let first = world.draft(
        BATCH,
        "a token leaks to the log",
        &["src/auth.rs"],
        "Leak.\n",
    );
    let second = world.draft(BATCH, "a token leaks again", &["src/auth.rs"], "Leak.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    for (slug, draft, signature) in [
        ("token-leaks", &first, "TokenLeak: bearer"),
        ("token-leaks-again", &second, " tokenleak: BEARER "),
        ("token-unsigned", &second, " "),
    ] {
        let mut ticket = Ticket::new(slug, slug, &[draft]);
        ticket.kind = Some("security".to_owned());
        ticket.fields = vec![("error_signature".to_owned(), signature.to_owned())];
        world.ticket(BATCH, &ticket);
    }
    let mut stray = Ticket::new("stray-field", "stray field", &[&first]);
    stray.fields = vec![("colour".to_owned(), "blue".to_owned())];
    world.ticket(BATCH, &stray);

    // A re-prepare searches each written ticket's value, once however it is spelled.
    world.reset_phases();
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let finds = world.destination_operations();
    assert_eq!(
        finds
            .iter()
            .map(|(_, op, _, items)| (op.as_str(), *items))
            .collect::<Vec<_>>(),
        [("find", 1), ("read", 1)],
        "the value search found the item its title search could not: {finds:?}"
    );

    // The open item carrying the signature takes the ticket's evidence.
    let matched = world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "token-leaks"],
        "",
    );
    assert_eq!(
        matched["matched"], "followups:signed-elsewhere",
        "{matched}"
    );
    assert_eq!(matched["write"]["op"], "comment", "{matched}");
    world.ticket(
        BATCH,
        &Ticket::new("plain-ticket", "a plain ticket", &[&first]),
    );
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "plain-ticket"],
        "",
    );
    std::fs::remove_file(
        world
            .work()
            .join(format!("tasks/{BATCH}/tickets/plain-ticket.md")),
    )
    .expect("removed");
    world.answer(
        BATCH,
        &[
            answer(&first, "root-cause", "filed", &["token-leaks"], "Verified."),
            answer(&second, "root-cause", "not-reproducible", &[], "Not seen."),
            answer(
                "drafts:no-such-input",
                "root-cause",
                "not-reproducible",
                &[],
                "?",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], false, "{checked}");
    let said: Vec<String> = checked["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|one| format!("{}: {}", one["subject"], one["message"]))
        .collect();
    assert_finds(
        &said,
        &[
            "the answer names no input of this batch",
            "the ticket `plain-ticket` it copies is not under the batch's tickets",
            "the ticket records the field `colour`",
        ],
        "check",
    );
    let stderr = world.fails(2, &["publish", "--batch", BATCH], "");
    assert!(stderr.contains("no `check` of batch"), "{stderr}");
}

#[test]
fn evidence_and_re_estimates_on_items_gone_or_changed_since_prepare_are_each_receipted() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    for (id, cause) in [("gone-item", "gone-cause"), ("moved-item", "moved-cause")] {
        world.item(
            "followups",
            id,
            &filed_item(
                &format!("widgets: the {} fails", cause.replace('-', " ")),
                "nightly-2026-10-01",
                cause,
                &json!({}),
                "medium",
            ),
            &filed_body(cause),
        );
    }
    let draft = world.draft(BATCH, "the gone cause fails", &["src/a.rs"], "Seen.\n");
    let other = world.draft(BATCH, "the moved cause fails", &["src/b.rs"], "Seen.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    for (item, cause) in [
        ("followups:gone-item", "gone-cause"),
        ("followups:moved-item", "moved-cause"),
    ] {
        world.ok(
            "agent",
            &[
                "ticket",
                "comment",
                "--batch",
                BATCH,
                "--item",
                item,
                "--root-cause",
                cause,
            ],
            &format!("Host {HOST}: seen again.\n"),
        );
        world.ok(
            "agent",
            &["ticket", "re-estimate", "--batch", BATCH, item],
            "",
        );
    }
    world.answer(
        BATCH,
        &[
            answer(&draft, "root-cause", "filed", &["gone-cause"], "Evidence."),
            answer(&other, "root-cause", "filed", &["moved-cause"], "Evidence."),
        ],
    );
    world.ok("check", &["check", "--batch", BATCH], "");
    std::fs::remove_file(world.root.join("followups-root/tasks/gone-item.md")).expect("removed");
    world.edit_item(
        "followups",
        "moved-item",
        "priority: medium",
        "priority: high",
    );
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let outcomes: Vec<(&str, &str, &str)> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| {
            (
                one["op"].as_str().unwrap_or_default(),
                one["item"].as_str().unwrap_or_default(),
                one["outcome"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("comment", "followups:gone-item", "error"),
            ("re-estimate", "followups:gone-item", "error"),
            ("comment", "followups:moved-item", "written"),
            ("re-estimate", "followups:moved-item", "version-moved"),
        ],
        "{published}"
    );
    let journal = world.receipts(BATCH);
    assert_eq!(
        journal.len(),
        4,
        "each outcome is its own entry: {journal:?}"
    );
    for (receipt, (command, item, kind)) in journal.iter().zip([
        ("comment", "followups:gone-item", Some("error")),
        ("re-estimate", "followups:gone-item", Some("error")),
        ("comment", "followups:moved-item", None),
        ("re-estimate", "followups:moved-item", Some("version-moved")),
    ]) {
        assert_eq!(receipt["command"], command, "{receipt}");
        assert_eq!(receipt["card"], item, "{receipt}");
        match kind {
            Some(kind) => {
                assert_eq!(receipt["outcome"]["error"]["kind"], kind, "{receipt}");
                assert!(receipt["outcome"].get("id").is_none(), "{receipt}");
            }
            None => assert!(receipt["outcome"].get("error").is_none(), "{receipt}"),
        }
    }
    assert!(
        world
            .read_item("followups", "moved-item")
            .contains("priority: high"),
        "the person's priority stands"
    );

    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(
        settled["read_back"], 0,
        "every outcome was answered: {settled}"
    );
    assert_eq!(settled["pending"], 3, "{settled}");
    assert_eq!(settled["needs_attention"], true, "{settled}");

    // The next pass reads the comment that landed.
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let status = world.ok(
        "agent",
        &[
            "ticket",
            "board-status",
            "--batch",
            BATCH,
            "followups:moved-item",
        ],
        "",
    );
    assert!(
        status["comments"]
            .as_array()
            .is_some_and(|comments| comments.iter().any(|comment| comment["body"]
                .as_str()
                .is_some_and(|body| body.contains("seen again")))),
        "{status}"
    );
    let pending: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join("feedback/pending.json")).expect("pending.json"),
    )
    .expect("JSON");
    let writes: Vec<(&str, &str)> = pending["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["item"].as_str().unwrap_or_default(),
                entry["write"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        writes,
        [
            ("followups:gone-item", "comment"),
            ("followups:gone-item", "priority"),
            ("followups:moved-item", "priority"),
        ],
        "{pending}"
    );
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey, end to end")]
fn a_withdrawal_a_restaged_comment_and_a_kind_removed_before_publish_are_each_handled() {
    let mut world = World::new(&["followups"], SAME);
    world.item(
        "followups",
        "signed-elsewhere",
        &filed_item(
            "widgets: a token leaks to the log",
            "nightly-2026-10-05",
            "token-leaks",
            &json!({"kind": "security", "fields": {"error_signature": "TokenLeak: bearer"}}),
            "low",
        ),
        &filed_body("A token leaks."),
    );
    let first = world.draft(BATCH, "a flaky resize test", &["tests/r.rs"], "Flaky.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut flaky = Ticket::new("flaky-resize", "a flaky resize test", &[&first]);
    flaky.extra = vec![("fields".to_owned(), json!({}))];
    world.ticket(BATCH, &flaky);
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "flaky-resize"],
        "",
    );
    world.answer(
        BATCH,
        &[answer(
            &first,
            "root-cause",
            "filed",
            &["flaky-resize"],
            "Verified.",
        )],
    );
    world.ok("check", &["check", "--batch", BATCH], "");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let (card, text) = world
        .filed("followups")
        .into_iter()
        .find(|(id, _)| id != "signed-elsewhere")
        .expect("the flaky ticket's card");
    assert!(
        record(&text).get("fields").is_none(),
        "an empty `fields` is not recorded: {text}"
    );

    // The next pass withdraws it, and stages evidence twice on another batch's item.
    let second = world.draft(
        BATCH,
        "a token leaks to the log",
        &["src/auth.rs"],
        "Leak.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    flaky.status = Some("cancelled".to_owned());
    world.ticket(BATCH, &flaky);
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "flaky-resize"],
        "",
    );
    let mut bare = Ticket::new("token-leaks-bare", "a token leaks to the log", &[&second]);
    bare.kind = Some("security".to_owned());
    bare.fields = vec![("error_signature".to_owned(), "tokenleak: bearer".to_owned())];
    bare.without = vec!["Evidence".to_owned()];
    world.ticket(BATCH, &bare);
    let stderr = world.fails(
        2,
        &["ticket", "copy", "--batch", BATCH, "token-leaks-bare"],
        "",
    );
    assert!(
        stderr.contains("is not sound, so nothing was staged")
            && stderr.contains("missing the `## Evidence` section"),
        "a ticket with no evidence to stage on the matched item is refused: {stderr}"
    );
    let comment = |world: &mut World, evidence: &str| {
        world.ok(
            "agent",
            &[
                "ticket",
                "comment",
                "--batch",
                BATCH,
                "--item",
                "followups:signed-elsewhere",
                "--root-cause",
                "token-leaks",
            ],
            evidence,
        )
    };
    let staged = comment(&mut world, "Host build-host-1: one leak.\n");
    let again = comment(&mut world, "Host build-host-1: two leaks.\n");
    assert_eq!(
        staged["index"], again["index"],
        "restaged in place: {again}"
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "re-estimate",
            "--batch",
            BATCH,
            "followups:signed-elsewhere",
        ],
        "",
    );
    world.answer(
        BATCH,
        &[answer(
            &second,
            "security",
            "filed",
            &["token-leaks"],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");

    // The host drops the security kind before publishing: the re-estimate of an item recording
    // it is refused, receipted, and the batch's other writes land.
    let published = world.ok(
        "publish",
        &[
            "--set",
            "kinds=[{name: root-cause, description: One root cause.}]",
            "publish",
            "--batch",
            BATCH,
        ],
        "",
    );
    let outcomes: Vec<(&str, &str)> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| {
            (
                one["op"].as_str().unwrap_or_default(),
                one["outcome"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("copy", "updated"),
            ("comment", "written"),
            ("re-estimate", "invalid")
        ],
        "{published}"
    );
    assert!(
        published["outcomes"][2]["error"].as_str().is_some_and(
            |error| error.contains("records the kind `security`, which is not configured")
        ),
        "{published}"
    );
    assert_eq!(
        front(&world.read_item("followups", &card))["status"],
        "cancelled",
        "the withdrawal wrote its status"
    );
    let stderr = world.fails(
        1,
        &[
            "--set",
            "hooks.batch_settled=[no-such-hook-anywhere]",
            "settle",
            "--batch",
            BATCH,
        ],
        "",
    );
    assert!(stderr.contains("could not run"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_destination_refusing_every_write_leaves_each_receipted_and_pending() {
    use std::os::unix::fs::PermissionsExt as _;

    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
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
            "layout-save",
            "a layout save reads one window size",
            &[&first],
        ),
    );
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "layout-save"],
        "",
    );
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
        &format!("Host {HOST}: again.\n"),
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
            answer(&first, "root-cause", "filed", &["layout-save"], "Verified."),
            answer(
                &second,
                "root-cause",
                "filed",
                &["export-dialog-drops-unicode"],
                "Evidence.",
            ),
        ],
    );
    world.ok("check", &["check", "--batch", BATCH], "");
    let before = world.items("followups");

    // The destination's directory refuses every write.
    let tasks = world.root.join("followups-root/tasks");
    std::fs::set_permissions(&tasks, std::fs::Permissions::from_mode(0o555)).expect("read-only");
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    std::fs::set_permissions(&tasks, std::fs::Permissions::from_mode(0o755)).expect("writable");
    let outcomes: Vec<(&str, &str)> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| {
            (
                one["op"].as_str().unwrap_or_default(),
                one["outcome"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            ("copy", "store-refused"),
            ("comment", "error"),
            ("re-estimate", "error"),
        ],
        "{published}"
    );
    assert_eq!(world.items("followups"), before, "nothing was written");
    let journal = world.receipts(BATCH);
    let kinds: Vec<(&str, &str)> = journal
        .iter()
        .map(|receipt| {
            (
                receipt["command"].as_str().unwrap_or_default(),
                receipt["outcome"]["error"]["kind"]
                    .as_str()
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("copy", "store-refused"),
            ("comment", "error"),
            ("re-estimate", "error"),
        ],
        "{journal:?}"
    );

    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(
        settled["read_back"], 0,
        "every refusal was answered: {settled}"
    );
    assert_eq!(settled["pending"], 3, "{settled}");
    assert_eq!(settled["needs_attention"], true, "{settled}");
}
