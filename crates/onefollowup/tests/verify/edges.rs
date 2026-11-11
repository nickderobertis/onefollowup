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

    world.ticket(
        BATCH,
        &Ticket::new("save-crashes", "saving crashes the app", &[&urgent]),
    );
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
fn settle_reads_back_a_new_ticket_a_comment_and_a_re_estimate_whose_answers_were_all_lost() {
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

    // No receipt can be written: every write lands and no answer is recorded.
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
    assert_eq!(settled["read_back"], 3, "{settled}");
    assert_eq!(
        settled["confirmed"], 3,
        "each write was found landed: {settled}"
    );
    assert!(settled.get("needs_attention").is_none(), "{settled}");
    let operations = world.destination_operations();
    let ops: Vec<&str> = operations.iter().map(|(_, op, _, _)| op.as_str()).collect();
    assert_eq!(
        ops,
        ["read", "find"],
        "one read of the items named, one search by origin for the new ticket: {operations:?}"
    );
}
