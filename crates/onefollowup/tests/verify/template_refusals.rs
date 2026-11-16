//! What the renderer alone writes is refused wherever an agent writes it: a ticket's authored
//! text, and every part handed to `ticket comment`, carrying a `<details>` or `</details>` line
//! or a comment-marker line is refused with nothing staged — under the shipped default and under
//! the `replaced-shape` example alike — as are an authored record supplying `projected`, and a
//! sources file missing a key, naming a draft outside the batch, or holding a witness at a
//! commit its basis does not record.

use serde_json::{Value, json};

use crate::shapes::world_of;
use crate::world::{COMMIT, HOST, REPOSITORY, Ticket, World, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-10";

/// The lines only the renderer or onefollowup writes, each with what its refusal says.
const RESERVED: [(&str, &str); 3] = [
    ("<details>", "only the renderer writes a collapsed block"),
    ("</details>", "only the renderer writes a collapsed block"),
    (
        "<!-- orchestrator:follow-up-comment run=\"x\" root_cause=\"y\" -->",
        "only onefollowup writes a marker",
    ),
];

fn staged(world: &World) -> usize {
    std::fs::read_to_string(world.work().join(format!("outbox/{BATCH}.json")))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|outbox| outbox["writes"].as_array().map(Vec::len))
        .unwrap_or(0)
}

fn refusals(mut world: World, configuration: &str) {
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
        "café became caf.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let slug = "export-dialog-drops-unicode-again";

    // Authored ticket text: refused at validate and at copy, nothing staged.
    for (line, says) in RESERVED {
        let mut ticket = Ticket::new(slug, "the export dialog drops unicode titles", &[&draft]);
        ticket.evidence = format!("Seen on {HOST}.\n\n{line}\nmore\n");
        world.ticket(BATCH, &ticket);
        let validated = world.ok("agent", &["ticket", "validate", "--batch", BATCH, slug], "");
        assert_eq!(
            validated["sound"], false,
            "{configuration}: {line}: {validated}"
        );
        assert!(
            validated.to_string().contains(says),
            "{configuration}: {validated}"
        );
        let refused = world.fails(2, &["ticket", "copy", "--batch", BATCH, slug], "");
        assert!(refused.contains(says), "{configuration}: {line}: {refused}");
        assert_eq!(staged(&world), 0, "{configuration}: nothing was staged");
    }
    // An authored record supplying `projected`.
    let mut ticket = Ticket::new(slug, "the export dialog drops unicode titles", &[&draft]);
    ticket
        .extra
        .push(("projected".to_owned(), json!({"root_cause": "mine"})));
    world.ticket(BATCH, &ticket);
    let refused = world.fails(2, &["ticket", "copy", "--batch", BATCH, slug], "");
    assert!(
        refused.contains("`projected`, which onefollowup alone writes"),
        "{configuration}: {refused}"
    );
    assert_eq!(staged(&world), 0);

    // Each part handed to `ticket comment`.
    let parts = world.root.join("parts");
    std::fs::create_dir_all(&parts).expect("a directory");
    let path = |name: &str| parts.join(name).display().to_string();
    let comment = |extra: &[&str], evidence: &str| {
        let mut args = vec![
            "ticket",
            "comment",
            "--batch",
            BATCH,
            "--item",
            "followups:export-older",
        ];
        args.extend_from_slice(extra);
        world.fails(2, &args, evidence)
    };
    std::fs::write(parts.join("plain.md"), "A plain paragraph.\n").expect("written");
    for (line, says) in RESERVED {
        let text = format!("Seen again.\n{line}\n");
        std::fs::write(parts.join("reserved.md"), &text).expect("written");
        for (extra, evidence) in [
            (vec![], text.as_str()),
            (
                vec!["--bears-on-file", &path("reserved.md")],
                "Seen again.\n",
            ),
            (
                vec!["--question-file", &path("reserved.md")],
                "Seen again.\n",
            ),
        ] {
            let refused = comment(&extra, evidence);
            assert!(
                refused.contains(says),
                "{configuration}: {extra:?}: {refused}"
            );
            assert!(refused.contains("nothing was staged"), "{refused}");
            assert_eq!(staged(&world), 0);
        }
    }

    // A sources file holding other than the record's shape, a draft outside the batch, or a
    // witness at another commit.
    let witness = format!("{REPOSITORY} src/export.rs:3 at {COMMIT}");
    for (sources, says) in [
        (
            json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [draft]}),
            "holding exactly `host`, `basis`, `drafts`, `witnesses`",
        ),
        (
            json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": ["drafts:someone-elses"], "witnesses": [witness]}),
            "the draft `drafts:someone-elses`, which is not one of batch",
        ),
        (
            json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [draft], "witnesses": [format!("{REPOSITORY} src/export.rs:3 at 89abcdef0123456789abcdef0123456789abcdef")]}),
            "where the basis records",
        ),
    ] {
        std::fs::write(parts.join("sources.json"), sources.to_string()).expect("written");
        let refused = comment(&["--sources-file", &path("sources.json")], "Seen again.\n");
        assert!(
            refused.contains(says),
            "{configuration}: {sources}: {refused}"
        );
        assert_eq!(staged(&world), 0);
    }

    // The same parts, sound, stage the comment.
    std::fs::write(
        parts.join("sources.json"),
        json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [draft], "witnesses": [witness]})
            .to_string(),
    )
    .expect("written");
    let sources = path("sources.json");
    let plain = path("plain.md");
    world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            BATCH,
            "--item",
            "followups:export-older",
            "--sources-file",
            &sources,
            "--bears-on-file",
            &plain,
            "--question-file",
            &plain,
        ],
        "Seen again.\n",
    );
    assert_eq!(staged(&world), 1, "{configuration}");
}

#[test]
fn reserved_lines_and_bad_sources_are_refused_with_nothing_staged() {
    refusals(
        World::new(&["followups"], "host_label: build-host-1\n"),
        "the shipped default",
    );
    refusals(world_of("replaced-shape"), "the replaced-shape example");
}
