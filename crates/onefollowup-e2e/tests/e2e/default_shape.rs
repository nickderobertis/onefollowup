//! The shipped default's ticket and comment shape: a verifying agent's plain authored ticket,
//! its impact values in the record alone, rendered with the workaround's cost, the priority
//! calculation and the contributing locations collapsed; the one-unit and the several-unit fix,
//! each enforced; an evidence comment's sources collapsed under its opening; and the estimate
//! line found by its label and replaced in place, wherever a body carries it.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::answering::{account, comment, launches, quoted, response};
use crate::journey::{comments, front, record};
use crate::world::{COMMIT, HOST, Shape, Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-20";
const LATER: &str = "nightly-2026-11-21";
const SERVICE: &str = "github.com/example/some-service";
const DEFAULT: &str = "host_label: build-host-1\n";

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../onefollowup/tests/fixtures/default-shape")
        .join(name);
    let text = std::fs::read_to_string(&path).expect("the fixture");
    text.split_once("-->\n")
        .map(|(_, body)| body.to_owned())
        .expect("a body under the fixture's note")
}

/// The body of a stored item's or ticket file's text.
fn body(text: &str) -> String {
    text.strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(_, body)| body.to_owned())
        .expect("front matter, then a body")
}

/// A draft about `repository` naming its transcript, as an engine's node records one.
fn draft(world: &mut World, batch: &str, repository: &str, title: &str) -> String {
    let transcript = format!("transcript=onepipeline transcript {batch} worker-1");
    let printed = world.ok(
        "draft",
        &[
            "draft",
            "--batch",
            batch,
            "--title",
            title,
            "--repository",
            repository,
            "--author",
            "node:worker-1",
            "--stamp",
            &transcript,
            "--path",
            "src/cursor.py",
        ],
        "Seen while listing.\n",
    );
    printed["draft"].as_str().expect("a draft id").to_owned()
}

/// `ticket`'s file with `authored` as its body.
fn write_ticket(world: &World, batch: &str, ticket: &Ticket, authored: &str) -> PathBuf {
    let rendered = ticket.render(batch);
    let (head, _) = rendered.split_once("\n---\n").expect("front matter");
    let path = world
        .work()
        .join(format!("tasks/{batch}/tickets/{}.md", ticket.slug));
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the tickets directory");
    std::fs::write(&path, format!("{head}\n---\n{authored}")).expect("the ticket");
    path
}

/// A ticket about the listing service, its impact the fixtures' values.
fn listing(slug: &str, title: &str, drafts: &[&str]) -> Ticket {
    let mut ticket = Ticket::new(slug, title, drafts).severities(
        "high",
        "low",
        "read the listing twice and merge the pages",
    );
    ticket.repository = SERVICE.to_owned();
    ticket
}

/// The stored text of `destination`'s item filed for `root_cause`.
fn filed(world: &World, destination: &str, root_cause: &str) -> (String, String) {
    world
        .filed(destination)
        .into_iter()
        .find(|(_, text)| record(text)["root_cause"] == root_cause)
        .unwrap_or_else(|| panic!("{root_cause} was filed"))
}

/// Every `- <label>:` line `text` carries outside its blocks' summaries.
fn labelled<'a>(text: &'a str, label: &str) -> Vec<&'a str> {
    text.lines()
        .filter(|line| line.starts_with(&format!("- {label}:")))
        .collect()
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey: two tickets filed, evidence, a re-estimate, a reply"
)]
fn the_default_files_both_fix_forms_collapsed_and_collapses_an_evidence_comments_sources() {
    let mut world = World::new(&["followups"], DEFAULT);
    let d1 = draft(
        &mut world,
        BATCH,
        SERVICE,
        "a listing skips its last page when its count changes",
    );
    let d2 = draft(
        &mut world,
        BATCH,
        SERVICE,
        "an export of a listing comes back one page short",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let one = listing(
        "listing-skips-a-page",
        "a listing skips its last page when its count changes",
        &[&d1],
    );
    let two = listing(
        "listing-and-export-skip-a-page",
        "an export of a listing comes back one page short",
        &[&d2],
    );
    for (ticket, name) in [(&one, "one-unit"), (&two, "two-unit")] {
        let authored = fixture(&format!("{name}.authored.md"));
        assert!(
            labelled(&authored, "Severity").is_empty() && !authored.contains("<details>"),
            "the agent writes no severity line and no block: {authored}"
        );
        write_ticket(&world, BATCH, ticket, &authored);
        let validated = world.ok(
            "agent",
            &["ticket", "validate", "--batch", BATCH, &ticket.slug],
            "",
        );
        assert_eq!(validated["sound"], true, "{name}: {validated}");
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }
    world.answer(
        BATCH,
        &[
            answer(&d1, "root-cause", "filed", &[&one.slug], "Verified."),
            answer(&d2, "root-cause", "filed", &[&two.slug], "Verified."),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");

    // Each destination body is the authored one as the shipped render writes it: the two-unit
    // form byte for byte what ai-orchestrator 8bf72a30's `collapsed()` writes of it.
    // (A local-md item's file closes its body with one newline of its own.)
    let (two_id, two_text) = filed(&world, "followups", &two.slug);
    assert_eq!(
        body(&two_text).strip_suffix('\n'),
        Some(fixture("two-unit.collapsed.md").as_str())
    );
    let (_, one_text) = filed(&world, "followups", &one.slug);
    assert_eq!(
        body(&one_text).strip_suffix('\n'),
        Some(fixture("one-unit.rendered.md").as_str())
    );
    for (text, opening) in [
        (
            &one_text,
            "## Suggested fix\n\n<details>\n<summary>Contributing locations</summary>\n\n- some-service `src/cursor.py`: stops one page early when the count changes.\n\n</details>\n\n### Listing — `some-service` (`src/cursor.py`)\n\n`list` pages by the cursor the previous page returned, so no page is skipped when the count changes.",
        ),
        (
            &two_text,
            "## Suggested fix\n\nPage the listing by cursor in the service and follow the cursor in the export, so no page is skipped when the count changes.\n\n<details>\n<summary>Contributing locations</summary>",
        ),
    ] {
        let shown = body(text);
        assert!(shown.contains(opening), "{shown}");
        assert!(!shown.contains("## Owning batches"), "{shown}");
        let evidence = shown.split_once("## Evidence\n\n").expect("evidence").1;
        assert_eq!(
            evidence.trim_end(),
            "A nightly export of the widgets listing came back one page short twice this week, each time while items were being added.",
            "prose and nothing the record holds"
        );
        let impact = shown
            .split_once("## Impact\n\n")
            .and_then(|(_, rest)| rest.split_once("\n\n## Examples"))
            .expect("an impact section")
            .0;
        assert!(
            impact.ends_with("<details>\n<summary>Priority calculation</summary>\n\n- Severity: high\n- Severity with the workaround: low\n- Priority estimate: low (severity with the workaround low; fires intermittently; 1 occurrence; not raised)\n\n</details>"),
            "{impact}"
        );
        assert!(
            impact.contains("<details>\n<summary>Cost of the workaround</summary>\n\n- Workaround: read the listing twice and merge the pages\n- Applying it:"),
            "the workaround line opens the cost block: {impact}"
        );
        let held = record(text);
        for key in ["draft_sources", "transcripts", "witnesses", "impact"] {
            assert!(held.get(key).is_some(), "{key}: {held}");
        }
        assert_eq!(
            held["impact"],
            json!({"Severity": "high", "Workaround": "read the listing twice and merge the pages", "Severity with the workaround": "low"})
        );
        assert_eq!(front(text)["priority"], "low");
    }

    // A later batch's evidence, handed its sources, opens with one collapsed block of them;
    // its re-estimate rewrites the estimate line inside the priority calculation, in place.
    let card = format!("followups:{two_id}");
    let d3 = draft(
        &mut world,
        LATER,
        SERVICE,
        "an export of a listing comes back one page short",
    );
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    let parts = world.root.join("parts");
    std::fs::create_dir_all(&parts).expect("a directory");
    let witness = format!("{SERVICE} src/cursor.py:41 at {COMMIT}");
    std::fs::write(
        parts.join("sources.json"),
        json!({"host": HOST, "basis": {SERVICE: COMMIT}, "drafts": [d3], "witnesses": [witness]})
            .to_string(),
    )
    .expect("the sources");
    std::fs::write(
        parts.join("bears-on.md"),
        "The export needs the cursor too.\n",
    )
    .expect("written");
    let sources = parts.join("sources.json").display().to_string();
    let bears_on = parts.join("bears-on.md").display().to_string();
    world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--item",
            &card,
            "--sources-file",
            &sources,
            "--bears-on-file",
            &bears_on,
        ],
        "The export came back one page short again.\n",
    );
    world.ok(
        "agent",
        &["ticket", "re-estimate", "--batch", LATER, &card],
        "",
    );
    world.answer(
        LATER,
        &[answer(
            &d3,
            "root-cause",
            "filed",
            &[&two.slug],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATER], "");
    world.ok("settle", &["settle", "--batch", LATER], "");
    let text = world.read_item("followups", &two_id);
    let marker = format!(
        "<!-- orchestrator:follow-up-comment run=\"{LATER}\" root_cause=\"{}\" -->",
        two.slug
    );
    let expected = format!(
        "<details>\n<summary>Additional evidence from follow-up run {LATER}.</summary>\n\n- Host: {HOST}\n- Basis: {SERVICE} at {COMMIT}\n- Draft: {d3}; author node; node worker-1; transcript: onepipeline transcript {LATER} worker-1\n- Witness: {witness}\n\n</details>\n\nThe export came back one page short again.\n\nBears on the ticket: The export needs the cursor too.\n\n{marker}"
    );
    assert!(comments(&text).contains(&expected), "{text}");
    let moved = fixture("two-unit.collapsed.md").replace(
        "- Priority estimate: low (severity with the workaround low; fires intermittently; 1 occurrence; not raised)",
        "- Priority estimate: low (severity with the workaround low; fires intermittently; 2 occurrences; not raised)",
    );
    assert_eq!(
        body(&text)
            .split("\n## Comments\n")
            .next()
            .map(str::trim_end),
        Some(moved.trim_end()),
        "only the estimate line moved, in place"
    );

    // A reply the comment pass writes carries no block: it reads as 0.5.0's.
    comment(
        &world,
        &card,
        "c-alice",
        "alice",
        "2026-11-22T10:00:00Z",
        "Does the export need its own ticket?",
    );
    let gathered = world.ok("gather", &["comments", "gather"], "");
    let snapshot = gathered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let launched = launches(&world.ok(
        "launches",
        &["comments", "launches", "--snapshot", &snapshot],
        "",
    ));
    let (owner, feedback) = &launched[0];
    assert_eq!(owner, BATCH);
    let asked = &quoted(feedback)[0];
    assert!(
        asked.opening.starts_with(&format!(
            "Reply from follow-up run `{BATCH}` to @alice's comment: "
        )),
        "{asked:?}"
    );
    let feedback_text = std::fs::read_to_string(feedback).expect("the feedback file");
    let shape = feedback_text
        .split_once("Reply shape if it does not")
        .expect("the reply's shape is quoted")
        .1;
    assert!(
        !shape[..shape.find("\n````\n").unwrap_or(shape.len())].contains("<details>"),
        "{feedback_text}"
    );
    let reply = crate::answering::post_reply(
        &world,
        asked,
        "does-not-confirm",
        "No: the export follows the same cursor.",
    );
    account(
        feedback,
        owner,
        &[response(asked, &reply, "does-not-confirm", "Answered.")],
    );
    let feedback_path = feedback.display().to_string();
    let checked = world.ok(
        "check",
        &["comments", "check", "--feedback", &feedback_path],
        "",
    );
    assert_eq!(checked["sound"], true, "{checked}");
    let settled = world.ok(
        "settle",
        &["comments", "settle", "--snapshot", &snapshot],
        "",
    );
    assert_eq!(settled["settled"][0]["verified"], 1, "{settled}");
    let text = world.read_item("followups", &two_id);
    let posted = comments(&text);
    let posted = posted
        .split_once(&asked.opening)
        .expect("the reply is posted")
        .1;
    assert!(!posted.contains("<details>"), "{posted}");
}

/// What `ticket validate` finds of `slug`'s file.
fn problems(world: &mut World, batch: &str, slug: &str) -> Vec<String> {
    let validated = world.ok("agent", &["ticket", "validate", "--batch", batch, slug], "");
    validated["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|one| one.as_str().unwrap_or_default().to_owned())
        .collect()
}

/// That `ticket validate`, `ticket copy` and, over a staged sound copy edited after it, `check`
/// each refuse `slug`'s `unsound` body, naming `wanted`; and that `sound` is accepted.
fn refused_everywhere(
    world: &mut World,
    batch: &str,
    ticket: &Ticket,
    sound: &str,
    unsound: &str,
    wanted: &str,
) {
    let path = write_ticket(world, batch, ticket, unsound);
    let found = problems(world, batch, &ticket.slug);
    assert!(
        found.iter().any(|one| one.contains(wanted)),
        "validate names {wanted:?}: {found:#?}"
    );
    let stderr = world.fails(2, &["ticket", "copy", "--batch", batch, &ticket.slug], "");
    assert!(stderr.contains(wanted), "copy names {wanted:?}: {stderr}");
    write_ticket(world, batch, ticket, sound);
    assert_eq!(problems(world, batch, &ticket.slug), Vec::<String>::new());
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", batch, &ticket.slug],
        "",
    );
    std::fs::write(
        &path,
        std::fs::read_to_string(&path)
            .expect("the ticket")
            .replace(sound, unsound),
    )
    .expect("edited after its copy was staged");
    let checked = world.ok("check", &["check", "--batch", batch], "");
    assert!(
        checked["problems"].to_string().contains(wanted),
        "check names {wanted:?}: {checked}"
    );
    write_ticket(world, batch, ticket, sound);
}

#[test]
#[allow(clippy::too_many_lines, reason = "every form of the fix, each way")]
fn the_fix_units_check_holds_each_fix_form_both_ways_and_a_kind_may_turn_it_off() {
    let mut world = World::new(&["followups"], DEFAULT);
    let d1 = draft(&mut world, BATCH, SERVICE, "a listing skips a page");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let ticket = listing("listing-skips-a-page", "a listing skips a page", &[&d1]);
    let one = fixture("one-unit.authored.md");
    let two = fixture("two-unit.authored.md");
    let overview = "Page the listing by cursor in the service and follow the cursor in the export, so no page is skipped when the count changes.\n\n";
    // An overview before the only unit is refused, naming the one-unit form.
    let one_with_overview = one.replacen(
        "## Suggested fix\n\n",
        &format!("## Suggested fix\n\n{overview}"),
        1,
    );
    refused_everywhere(
        &mut world,
        BATCH,
        &ticket,
        &one,
        &one_with_overview,
        "has one unit, so nothing but its **Contributing locations.** part comes before it",
    );
    let found = problems(&mut world, BATCH, &ticket.slug);
    assert!(found.is_empty(), "{found:#?}");
    write_ticket(&world, BATCH, &ticket, &one_with_overview);
    let found = problems(&mut world, BATCH, &ticket.slug);
    assert!(
        found.iter().any(|one| one.contains(
            "move what it says that the unit does not, such as why the change removes the root cause, into the unit's opening sentence or two"
        )),
        "{found:#?}"
    );
    // Two units with no overview are refused, naming the several-unit form.
    let two_without_overview = two.replacen(overview, "", 1);
    refused_everywhere(
        &mut world,
        BATCH,
        &ticket,
        &two,
        &two_without_overview,
        "has 2 units, so it opens with an overview paragraph",
    );
    // A fix with no unit is refused, naming both forms.
    let no_unit = two.replace(
        "\n\n### Listing — `some-service` (`src/cursor.py`)\n\n`list` pages by the cursor the previous page returned.\n\n### Export — `some-export`\n\nThe export follows the cursor until the listing reports no next page.",
        "",
    );
    write_ticket(&world, BATCH, &ticket, &no_unit);
    let found = problems(&mut world, BATCH, &ticket.slug);
    assert!(
        found.iter().any(|one| one.contains("carries no unit subsection. With one unit it is the **Contributing locations.** part then the unit")
            && one.contains("with two or more units an overview paragraph")),
        "{found:#?}"
    );
    // A unit heading out of its form, and locations listed after the units, are refused too.
    write_ticket(
        &world,
        BATCH,
        &ticket,
        &one.replace(
            "### Listing — `some-service` (`src/cursor.py`)",
            "### Listing",
        ),
    );
    assert!(
        problems(&mut world, BATCH, &ticket.slug)
            .iter()
            .any(|one| one.contains("subsection `### Listing` is not headed")),
    );
    write_ticket(
        &world,
        BATCH,
        &ticket,
        &one.replace(
            "- some-service `src/cursor.py`: stops one page early when the count changes.",
            "the cursor stops early",
        ),
    );
    assert!(
        problems(&mut world, BATCH, &ticket.slug)
            .iter()
            .any(|one| one.contains("part carries no bullet")),
    );

    // With `fix-units` left out of the kind's checks, both one-unit forms are accepted.
    let unchecked = "host_label: build-host-1\nkinds:\n  - name: root-cause\n    description: One root cause.\n    render_template: builtin:ticket-render.md.j2\n    checks: [headings, title, impact, evidence-prose, witnesses]\n";
    std::fs::write(world.root.join("onefollowup.yaml"), unchecked).expect("written");
    for body in [&one, &one_with_overview] {
        write_ticket(&world, BATCH, &ticket, body);
        assert_eq!(
            problems(&mut world, BATCH, &ticket.slug),
            Vec::<String>::new()
        );
    }
}

#[test]
fn evidence_prose_witnesses_and_a_command_check_refuse_what_they_name() {
    let mut world = World::new(&["followups"], DEFAULT);
    let d1 = draft(&mut world, BATCH, SERVICE, "a listing skips a page");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let ticket = listing("listing-skips-a-page", "a listing skips a page", &[&d1]);
    let one = fixture("one-unit.authored.md");
    let evidence = "A nightly export of the widgets listing came back one page short twice this week, each time while items were being added.";
    write_ticket(
        &world,
        BATCH,
        &ticket,
        &one.replace(
            evidence,
            &format!(
                "- Host: {HOST}\n- Basis: {SERVICE} at {COMMIT}\n\nSeen in {d1}; read it with `onepipeline transcript {BATCH} worker-1`."
            ),
        ),
    );
    let found = problems(&mut world, BATCH, &ticket.slug);
    assert!(
        found.iter().any(|one| one.contains("`## Evidence` carries a `Host:` line, a `Basis:` line, a qualified draft id, a 40-character commit, a transcript command, which the record holds")),
        "{found:#?}"
    );
    let mut bare = ticket.clone();
    bare.extra = vec![("witnesses".to_owned(), json!([]))];
    write_ticket(&world, BATCH, &bare, &one);
    assert!(
        problems(&mut world, BATCH, &ticket.slug)
            .iter()
            .any(|one| one.contains("the record carries no `witnesses`")),
    );

    // A command check is handed the ticket on stdin; what it prints refuses the ticket.
    world.script(
        "shape-check",
        "input=$(cat)\ncase \"$input\" in\n  *'\"rendered_body\":\"## Root cause'*'\"kind\":\"root-cause\"'*) ;;\n  *) echo \"not handed the ticket: $input\"; exit 1 ;;\nesac\ncase \"$input\" in\n  *Listing*) echo 'a unit is named Listing'; echo 'name units after what changes'; exit 1 ;;\nesac\n",
    );
    let commanded = "host_label: build-host-1\nkinds:\n  - name: root-cause\n    description: One root cause.\n    render_template: builtin:ticket-render.md.j2\n    checks: [headings, {command: [shape-check]}]\n";
    std::fs::write(world.root.join("onefollowup.yaml"), commanded).expect("written");
    write_ticket(&world, BATCH, &ticket, &one);
    let found = problems(&mut world, BATCH, &ticket.slug);
    assert_eq!(
        found,
        ["a unit is named Listing", "name units after what changes"],
        "each line it printed"
    );
    write_ticket(
        &world,
        BATCH,
        &ticket,
        &one.replace("### Listing — ", "### Paging — "),
    );
    assert_eq!(
        problems(&mut world, BATCH, &ticket.slug),
        Vec::<String>::new()
    );
    let silent = commanded.replace("shape-check", "fails-silently");
    world.script("fails-silently", "exit 3\n");
    std::fs::write(world.root.join("onefollowup.yaml"), silent).expect("written");
    assert_eq!(
        problems(&mut world, BATCH, &ticket.slug),
        ["the check command `fails-silently` exited 3 and printed nothing"]
    );
    std::fs::write(
        world.root.join("onefollowup.yaml"),
        commanded.replace("shape-check", "/no/such/check"),
    )
    .expect("written");
    assert!(
        problems(&mut world, BATCH, &ticket.slug)[0]
            .starts_with("the check command `/no/such/check` did not run"),
    );
}

/// `ticket` with its record's `impact` replaced by `impact`, or left out for `None`.
pub fn with_impact(ticket: &Ticket, impact: Option<&Value>) -> Ticket {
    let mut held = ticket.clone();
    held.impact = false;
    if let Some(impact) = impact {
        held.extra.push(("impact".to_owned(), impact.clone()));
    }
    held
}

/// Writes a ticket's file, its record's `impact` as given.
pub type Writer<'a> = &'a dyn Fn(&World, Option<&Value>);

/// The four ways an estimated ticket's `impact` is refused, each refused by `ticket validate`,
/// `ticket copy` and `check`, naming the label; `write` writes the ticket `slug` in its host's
/// shape with the `impact` given.
pub fn impact_refusals(
    world: &mut World,
    batch: &str,
    slug: &str,
    labels: [&str; 3],
    write: Writer<'_>,
) {
    let [severity, workaround, mitigated] = labels;
    let sound = json!({severity: "high", workaround: "retry", mitigated: "medium"});
    for (impact, wanted) in [
        (None, "the record carries no `impact`".to_owned()),
        (
            Some(json!({severity: "high", mitigated: "medium"})),
            format!("the record's `impact` carries no `{workaround}`"),
        ),
        (
            Some(json!({severity: "high", workaround: "retry", mitigated: "medium", "Blast radius": "wide"})),
            "the record's `impact` carries `Blast radius`, which is not one of the rubric's impact lines".to_owned(),
        ),
        (
            Some(json!({severity: "dire", workaround: "retry", mitigated: "medium"})),
            format!("the record's `impact` gives `{severity}` as `dire`, which is not one of the rubric's levels"),
        ),
    ] {
        write(world, impact.as_ref());
        let found = problems(world, batch, slug);
        assert!(
            found.iter().any(|one| one.contains(&wanted)),
            "validate names {wanted:?}: {found:#?}"
        );
        let stderr = world.fails(2, &["ticket", "copy", "--batch", batch, slug], "");
        assert!(stderr.contains(&wanted), "copy names {wanted:?}: {stderr}");
        write(world, Some(&sound));
        world.ok("agent", &["ticket", "copy", "--batch", batch, slug], "");
        write(world, impact.as_ref());
        let checked = world.ok("check", &["check", "--batch", batch], "");
        assert!(
            checked["problems"].to_string().contains(&wanted),
            "check names {wanted:?}: {checked}"
        );
    }
    write(world, Some(&sound));
}

#[test]
fn the_default_refuses_an_estimated_ticket_whose_impact_is_missing_or_off_the_rubric() {
    let mut world = World::new(&["followups"], DEFAULT);
    let d1 = world.draft(
        BATCH,
        "a layout save reads one size",
        &["src/a.rs"],
        "Seen.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let ticket = Ticket::new("layout-save", "a layout save reads one size", &[&d1]);
    impact_refusals(
        &mut world,
        BATCH,
        &ticket.slug,
        ["Severity", "Workaround", "Severity with the workaround"],
        &|world, impact| world.ticket(BATCH, &with_impact(&ticket, impact)),
    );
}

#[test]
fn the_replaced_shape_example_refuses_an_estimated_ticket_whose_impact_is_off_the_rubric() {
    let mut world = crate::examples::world_of("replaced-shape");
    let d1 = world.draft(
        BATCH,
        "a layout save reads one size",
        &["src/a.rs"],
        "Seen.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut ticket = Ticket::new("layout-save", "a layout save reads one size", &[&d1]);
    ticket.shape = Shape::Owned;
    ticket.extra.push((
        "witnesses".to_owned(),
        json!([format!(
            "{} src/layout.rs:42 at {COMMIT}",
            ticket.repository
        )]),
    ));
    impact_refusals(
        &mut world,
        BATCH,
        &ticket.slug,
        ["Severity", "Workaround", "Severity with the workaround"],
        &|world, impact| world.ticket(BATCH, &with_impact(&ticket, impact)),
    );
}

/// `destination`'s item `id`, written as 0.5.0 wrote one: `body`, and a record with no `impact`.
fn legacy_item(world: &World, id: &str, body: &str, extra: &Value) {
    world.item(
        "followups",
        id,
        &filed_item(
            &format!("widgets: the {id} item"),
            "nightly-2026-10-01",
            id,
            extra,
            "medium",
        ),
        body,
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "every place an estimate line sits, in one batch"
)]
fn re_estimate_and_publish_replace_the_estimate_line_in_place_wherever_it_sits() {
    let mut world = World::new(&["followups"], DEFAULT);
    // A 0.5.0 body: its lines in `## Impact`, the estimate line after them; another with
    // Linear's `* ` bullets; and one whose record carries `impact` and whose body carries no
    // estimate line at all.
    let legacy = filed_body("A legacy item.");
    legacy_item(&world, "legacy", &legacy, &json!({}));
    let starred = legacy.replace("\n- ", "\n* ");
    legacy_item(&world, "starred", &starred, &json!({}));
    let lineless = "## Root cause\n\nA lineless item.\n\n## Evidence\n\nSeen once.\n";
    legacy_item(
        &world,
        "lineless",
        lineless,
        &json!({"impact": {"Severity": "critical", "Workaround": "none", "Severity with the workaround": "critical"}}),
    );
    let d1 = world.draft(BATCH, "the legacy item", &["src/a.rs"], "Seen.\n");
    let d2 = world.draft(BATCH, "the starred item", &["src/a.rs"], "Seen.\n");
    let d3 = world.draft(BATCH, "the lineless item", &["src/a.rs"], "Seen.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut answers = Vec::new();
    for (item, draft) in [("legacy", &d1), ("starred", &d2), ("lineless", &d3)] {
        let card = format!("followups:{item}");
        world.ok(
            "agent",
            &["ticket", "comment", "--batch", BATCH, "--item", &card],
            "Seen again.\n",
        );
        world.ok(
            "agent",
            &["ticket", "re-estimate", "--batch", BATCH, &card],
            "",
        );
        answers.push(answer(draft, "root-cause", "filed", &[item], "Evidence."));
    }
    world.answer(BATCH, &answers);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let again =
        "(severity with the workaround medium; fires intermittently; 2 occurrences; not raised)";
    for (item, written, line) in [
        ("legacy", &legacy, "- Priority estimate: medium "),
        ("starred", &starred, "* Priority estimate: medium "),
    ] {
        let text = world.read_item("followups", item);
        let once =
            "(severity with the workaround medium; fires intermittently; 1 occurrence; not raised)";
        assert_eq!(
            body(&text)
                .split("\n## Comments\n")
                .next()
                .map(str::trim_end),
            Some(
                written
                    .replace(&format!("{line}{once}"), &format!("{line}{again}"))
                    .trim_end()
            ),
            "{item}: the line rewritten in place, its bullet kept"
        );
        // The legacy read's values are written into `impact`.
        assert_eq!(
            record(&text)["impact"],
            json!({"Severity": "high", "Workaround": "retry the export", "Severity with the workaround": "medium"}),
            "{item}"
        );
    }
    let text = world.read_item("followups", "lineless");
    assert_eq!(
        body(&text)
            .split("\n## Comments\n")
            .next()
            .map(str::trim_end),
        Some(lineless.trim_end()),
        "a body with no estimate line is unchanged"
    );
    assert_eq!(record(&text)["priority_estimate"], "urgent");
    assert_eq!(front(&text)["priority"], "urgent", "the priority moved");
}

#[test]
fn a_bound_copy_of_a_ticket_0_5_0_wrote_reads_its_lines_into_impact() {
    // A host whose kind keeps 0.5.0's body: no render template, its lines in `## Impact`.
    let config = "host_label: build-host-1\nkinds:\n  - name: root-cause\n    description: One root cause.\n    checks: [title]\n";
    let mut world = World::new(&["followups"], config);
    world.shape = Some(Shape::Lined);
    let d1 = world.draft(
        BATCH,
        "a layout save reads one size",
        &["src/a.rs"],
        "Seen.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let ticket = Ticket::new("layout-save", "a layout save reads one size", &[&d1]);
    // A new ticket carries `impact`, whatever lines its body carries.
    let mut unrecorded = ticket.clone();
    unrecorded.impact = false;
    world.ticket(BATCH, &unrecorded);
    let found = problems(&mut world, BATCH, "layout-save");
    assert!(
        found
            .iter()
            .any(|one| one.contains("the record carries no `impact`")),
        "{found:#?}"
    );
    world.ticket(BATCH, &ticket);
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "layout-save"],
        "",
    );
    let filed_answer = [answer(
        &d1,
        "root-cause",
        "filed",
        &["layout-save"],
        "Verified.",
    )];
    world.answer(BATCH, &filed_answer);
    world.ok("check", &["check", "--batch", BATCH], "");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let (id, text) = filed(&world, "followups", "layout-save");
    let before = body(&text);
    assert!(
        before.contains("- Severity with the workaround: medium\n- Priority estimate: medium (severity with the workaround medium; fires intermittently; 1 occurrence; not raised)"),
        "{before}"
    );

    // As 0.5.0 left it: neither the item's record nor the ticket file carries `impact`.
    let path = world.root.join(format!("followups-root/tasks/{id}.md"));
    let stored = std::fs::read_to_string(&path).expect("the item");
    let mut in_impact = false;
    let kept: Vec<&str> = stored
        .split('\n')
        .filter(|line| {
            if *line == "    impact:" {
                in_impact = true;
                return false;
            }
            in_impact = in_impact && line.starts_with("      ");
            !in_impact
        })
        .collect();
    std::fs::write(&path, kept.join("\n")).expect("written");
    assert!(
        record(&world.read_item("followups", &id))
            .get("impact")
            .is_none(),
        "{stored}"
    );
    let mut legacy = ticket.clone();
    legacy.impact = false;
    legacy.frequency = Some("consistent".to_owned());
    world.ticket(BATCH, &legacy);

    // Its batch again: bound, the copy reads the body's lines, and writes them into `impact`.
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(
        problems(&mut world, BATCH, "layout-save"),
        Vec::<String>::new()
    );
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "layout-save"],
        "",
    );
    world.answer(BATCH, &filed_answer);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let text = world.read_item("followups", &id);
    assert_eq!(
        record(&text)["impact"],
        json!({"Severity": "high", "Workaround": "avoid resizing while the save indicator shows", "Severity with the workaround": "medium"})
    );
    assert_eq!(
        body(&text),
        before.replace(
            "- Priority estimate: medium (severity with the workaround medium; fires intermittently; 1 occurrence; not raised)",
            "- Priority estimate: high (severity with the workaround medium; fires consistently; 1 occurrence; raised one level because it fires consistently)",
        ),
        "only the estimate line moved"
    );
    assert_eq!(front(&text)["priority"], "high");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "each edge of the checks and the estimate line"
)]
fn each_check_names_a_missing_section_and_a_body_the_legacy_read_cannot_read_is_estimated_as_before()
 {
    let mut world = World::new(&["followups"], DEFAULT);
    let d1 = draft(&mut world, BATCH, SERVICE, "a listing skips a page");
    // A 0.5.0 item whose `- Severity:` line is written twice: the legacy read cannot read it, so
    // it is estimated from its `## Impact` as 0.5.0 estimated it, and keeps no `impact`.
    let doubled = filed_body("A doubled item.").replace(
        "## Evidence\n\nSeen once.",
        "## Evidence\n\nSeen once.\n- Severity: low",
    );
    legacy_item(&world, "doubled", &doubled, &json!({}));
    let d2 = world.draft(BATCH, "the doubled item", &["src/a.rs"], "Seen.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let ticket = listing("listing-skips-a-page", "a listing skips a page", &[&d1]);
    let one = fixture("one-unit.authored.md");
    for (body, wanted) in [
        (
            one.replace("## Suggested fix", "## Remedy"),
            "the body has no `## Suggested fix` section, which carries the fix and its units",
        ),
        (
            one.replace("## Evidence", "## Notes"),
            "the body has no `## Evidence` section, which says what happened",
        ),
        (
            one.replace("## Impact", "## Cost"),
            "the body has no `## Impact` section, which carries the rubric's labelled parts",
        ),
        (
            one.replace("**Contributing locations.**\n", ""),
            "carries no **Contributing locations.** part before its first unit",
        ),
        (
            one.replace(
                "`list` pages by the cursor the previous page returned, so no page is skipped when the count changes.",
                "",
            ),
            "subsection `### Listing — `some-service` (`src/cursor.py`)` is empty",
        ),
    ] {
        write_ticket(&world, BATCH, &ticket, &body);
        let found = problems(&mut world, BATCH, &ticket.slug);
        assert!(
            found.iter().any(|one| one.contains(wanted)),
            "{wanted:?}: {found:#?}"
        );
    }
    // A heading-shaped line inside a fenced block is the block's text, not a unit.
    write_ticket(
        &world,
        BATCH,
        &ticket,
        &one.replace(
            "so no page is skipped when the count changes.",
            "so no page is skipped when the count changes.\n\n```text\n### Not a unit — `fenced`\n```",
        ),
    );
    assert_eq!(
        problems(&mut world, BATCH, &ticket.slug),
        Vec::<String>::new()
    );

    // The title check's parameters: no prefix, and a limit of its own.
    let titled = "host_label: build-host-1\nkinds:\n  - name: root-cause\n    description: One root cause.\n    checks: [{check: title, prefix: null, limit: 30}]\n";
    std::fs::write(world.root.join("onefollowup.yaml"), titled).expect("written");
    let mut untitled = ticket.clone();
    untitled.repository = "github.com/example/elsewhere".to_owned();
    write_ticket(&world, BATCH, &untitled, &one);
    let found = problems(&mut world, BATCH, &ticket.slug);
    assert!(
        found
            .iter()
            .any(|one| one.contains("the title is longer than 30 characters"))
            && !found.iter().any(|one| one.contains("does not open with")),
        "{found:#?}"
    );

    // An estimate line template whose line does not open with its label refuses the estimate.
    std::fs::write(world.root.join("line.j2"), "Priority is {{ level }}\n").expect("written");
    std::fs::write(
        world.root.join("onefollowup.yaml"),
        "host_label: build-host-1\nestimate: {line_template: line.j2}\n",
    )
    .expect("written");
    let stderr = world.fails(
        2,
        &[
            "ticket",
            "re-estimate",
            "--batch",
            BATCH,
            "followups:doubled",
        ],
        "",
    );
    assert!(
        stderr.contains("`estimate.line_template` rendered `Priority is medium`, which is not one line opening with `- Priority estimate:`"),
        "{stderr}"
    );
    std::fs::write(world.root.join("onefollowup.yaml"), DEFAULT).expect("written");
    world.ok(
        "agent",
        &[
            "ticket",
            "re-estimate",
            "--batch",
            BATCH,
            "followups:doubled",
        ],
        "",
    );
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "root-cause",
                "not-reproducible",
                &[],
                "Not seen again.",
            ),
            answer(
                &d2,
                "root-cause",
                "not-reproducible",
                &[],
                "Not seen again.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let text = world.read_item("followups", "doubled");
    assert!(record(&text).get("impact").is_none(), "{text}");
    assert_eq!(record(&text)["priority_estimate"], "medium");
}
