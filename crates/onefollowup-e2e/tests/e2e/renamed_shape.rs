//! A host that renames every section of the default kind's ticket — no `## Impact`, no `impact`
//! check — and shows the severity values as a table in a section of another name, its agents
//! supplying them only in the record's `impact` (`tests/renamed-shape/`): the priority, its
//! re-estimate and its own estimate line still come from the record.

use std::path::{Path, PathBuf};

use crate::default_shape::{impact_refusals, with_impact};
use crate::journey::{front, record};
use crate::world::{Ticket, World, answer};

const BATCH: &str = "nightly-2026-11-20";
const LATER: &str = "nightly-2026-11-21";

fn directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/renamed-shape")
}

/// A world configured as the renamed-shape host, its templates beside its configuration.
fn renamed_world() -> World {
    let config =
        std::fs::read_to_string(directory().join("onefollowup.yaml")).expect("the configuration");
    let world = World::new(
        &["followups"],
        &format!("{config}host_label: build-host-1\n"),
    );
    std::fs::create_dir_all(world.root.join("templates")).expect("a directory");
    for entry in std::fs::read_dir(directory().join("templates"))
        .expect("the templates")
        .filter_map(Result::ok)
    {
        std::fs::copy(
            entry.path(),
            world.root.join("templates").join(entry.file_name()),
        )
        .expect("copied");
    }
    world
}

/// This host's ticket, its impact `(severity, workaround, mitigated)` in the record alone.
fn renamed_ticket(
    world: &World,
    batch: &str,
    slug: &str,
    title: &str,
    drafts: &[&str],
    impact: (&str, &str, &str),
) -> Ticket {
    let ticket = Ticket::new(slug, title, drafts).severities(impact.0, impact.2, impact.1);
    write_renamed(world, batch, &ticket);
    ticket
}

/// `ticket`'s file in this host's body.
fn write_renamed(world: &World, batch: &str, ticket: &Ticket) {
    let (slug, title) = (&ticket.slug, &ticket.title);
    let rendered = ticket.render(batch);
    let (head, _) = rendered.split_once("\n---\n").expect("front matter");
    let body = format!(
        "## Guarantee\n\nA saved layout is the one on screen.\n\n## Who pays\n\nEvery user who saves a layout while resizing loses it.\n\n## Remedy\n\n**Where it lives.**\n- widgets `src/layout.rs`: reads the size twice\n\n### Layout save — `widgets`\n\nRead the size once per save, so a save never mixes two sizes.\n\n## What happened\n\nThe {title} on a nightly run.\n"
    );
    let path = world
        .work()
        .join(format!("tasks/{batch}/tickets/{slug}.md"));
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the tickets directory");
    std::fs::write(&path, format!("{head}\n---\n{body}")).expect("the ticket");
}

fn body(text: &str) -> String {
    text.strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(_, body)| body.to_owned())
        .expect("front matter, then a body")
}

/// The `## Weighing` section of a stored item's body: its table, then its estimate line.
fn weighing(text: &str) -> String {
    body(text)
        .split_once("## Weighing\n\n")
        .and_then(|(_, rest)| rest.split_once("\n## Remedy"))
        .map(|(section, _)| section.trim().to_owned())
        .expect("a weighing section")
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey: two tickets filed, one re-estimated"
)]
fn a_host_renaming_every_section_is_estimated_from_the_records_impact() {
    let mut world = renamed_world();
    let shown = world.ok("config", &["config", "show"], "");
    assert_eq!(
        shown["config"]["estimate"]["line_label"],
        "Estimated priority"
    );
    let d1 = world.draft(
        BATCH,
        "a layout save reads one size",
        &["src/layout.rs"],
        "Seen.\n",
    );
    let d2 = world.draft(
        BATCH,
        "a layout save corrupts the file",
        &["src/layout.rs"],
        "Seen.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let medium = renamed_ticket(
        &world,
        BATCH,
        "layout-save",
        "a layout save reads one size",
        &[&d1],
        ("high", "retry the save", "medium"),
    );
    let urgent = renamed_ticket(
        &world,
        BATCH,
        "layout-corrupts",
        "a layout save corrupts the file",
        &[&d2],
        ("critical", "none", "critical"),
    );
    for ticket in [&medium, &urgent] {
        let validated = world.ok(
            "agent",
            &["ticket", "validate", "--batch", BATCH, &ticket.slug],
            "",
        );
        assert_eq!(validated["sound"], true, "{validated}");
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }
    world.answer(
        BATCH,
        &[
            answer(&d1, "root-cause", "filed", &["layout-save"], "Verified."),
            answer(
                &d2,
                "root-cause",
                "filed",
                &["layout-corrupts"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");

    let filed = world.filed("followups");
    let text_of = |slug: &str| {
        filed
            .iter()
            .find(|(_, text)| record(text)["root_cause"] == slug)
            .cloned()
            .unwrap_or_else(|| panic!("{slug} was filed"))
    };
    let (id, text) = text_of("layout-save");
    assert_eq!(
        front(&text)["priority"],
        "medium",
        "the rubric's level for `medium`"
    );
    assert_eq!(
        weighing(&text),
        "| Severity | Workaround | Severity with the workaround |\n|---|---|---|\n| high | retry the save | medium |\n\nEstimated priority: medium — medium with the workaround, 1 occurrence"
    );
    let (_, other) = text_of("layout-corrupts");
    assert_eq!(
        front(&other)["priority"],
        "urgent",
        "its own `impact`, its own level"
    );
    assert!(
        weighing(&other)
            .ends_with("Estimated priority: urgent — critical with the workaround, 1 occurrence")
    );
    for text in [&text, &other] {
        let shown = body(text);
        for label in [
            "Severity",
            "Workaround",
            "Severity with the workaround",
            "Priority estimate",
        ] {
            assert!(
                !shown
                    .lines()
                    .any(|line| line.starts_with(&format!("- {label}:"))),
                "no default-form `- {label}:` line: {shown}"
            );
        }
        assert!(!shown.contains("## Impact"), "{shown}");
    }

    // A later batch's evidence, and its re-estimate counting two occurrences, which raise the
    // estimate at this host's `raise_at`: the priority moves and the line is rewritten in place.
    let card = format!("followups:{id}");
    let d3 = world.draft(
        LATER,
        "a layout save reads one size",
        &["src/layout.rs"],
        "Again.\n",
    );
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    world.ok(
        "agent",
        &["ticket", "comment", "--batch", LATER, "--item", &card],
        "Seen again on a slower disk.\n",
    );
    let staged = world.ok(
        "agent",
        &["ticket", "re-estimate", "--batch", LATER, &card],
        "",
    );
    assert_eq!(staged["write"]["priority_estimate"], "high", "{staged}");
    world.answer(
        LATER,
        &[answer(
            &d3,
            "root-cause",
            "filed",
            &["layout-save"],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATER], "");
    world.ok("settle", &["settle", "--batch", LATER], "");
    let after = world.read_item("followups", &id);
    assert_eq!(front(&after)["priority"], "high");
    assert_eq!(record(&after)["priority_estimate"], "high");
    assert_eq!(
        weighing(&after),
        "| Severity | Workaround | Severity with the workaround |\n|---|---|---|\n| high | retry the save | medium |\n\nEstimated priority: high — medium with the workaround, 2 occurrences, raised one level because it has 2 or more occurrences"
    );
    assert_eq!(
        body(&after).split("\n## Comments\n").next().map(str::trim_end),
        Some(
            body(&text)
                .replace(
                    "Estimated priority: medium — medium with the workaround, 1 occurrence",
                    "Estimated priority: high — medium with the workaround, 2 occurrences, raised one level because it has 2 or more occurrences",
                )
                .trim_end()
        ),
        "only the estimate line moved"
    );
}

#[test]
fn the_renamed_shape_refuses_an_estimated_ticket_whose_impact_is_off_the_rubric() {
    let mut world = renamed_world();
    let d1 = world.draft(
        BATCH,
        "a layout save reads one size",
        &["src/a.rs"],
        "Seen.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let ticket = renamed_ticket(
        &world,
        BATCH,
        "layout-save",
        "a layout save reads one size",
        &[&d1],
        ("high", "retry", "medium"),
    );
    impact_refusals(
        &mut world,
        BATCH,
        &ticket.slug,
        ["Severity", "Workaround", "Severity with the workaround"],
        &|world, impact| write_renamed(world, BATCH, &with_impact(&ticket, impact)),
    );
}
