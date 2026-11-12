//! Relations over the `examples/symptoms-and-root-causes` configuration, on local-md
//! sources: every edge held to the configured kinds and relations, match-decided edges derived
//! from exact `same` fields and never asserted, each edge written once however often a batch
//! retries, a lost link end reported and restored, rollups one hop into the `to` ticket only,
//! evidence on the most specific ticket, and routes counting edges. A configuration of two
//! destinations shows a parent refused across them while a link across them is written.

use std::path::Path;

use serde_json::{Value, json};

use crate::journey::{front, record};
use crate::world::{Ticket, World, answer, filed_body, filed_item};

const BATCH: &str = "nightly-2026-11-10";
const LATER: &str = "nightly-2026-11-11";

/// A world running the symptoms-and-root-causes example's configuration and templates.
pub fn example_world() -> World {
    let directory =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/symptoms-and-root-causes");
    let config = std::fs::read_to_string(directory.join("onefollowup.yaml"))
        .expect("the example's configuration");
    let world = World::new(&["eng"], &format!("{config}host_label: build-host-1\n"));
    std::fs::create_dir_all(world.root.join("templates")).expect("a directory");
    for entry in std::fs::read_dir(directory.join("templates"))
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

/// A ticket of `kind` recording `fields`.
pub fn kinded(slug: &str, drafts: &[&str], kind: &str, fields: &[(&str, &str)]) -> Ticket {
    let mut ticket = Ticket::new(slug, &format!("{slug} fails"), drafts);
    ticket.kind = Some(kind.to_owned());
    ticket.fields = fields
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    ticket
}

/// The item of `destination` whose record names `root_cause`, as its qualified id and text.
pub fn filed_as(world: &World, destination: &str, root_cause: &str) -> (String, String) {
    world
        .filed(destination)
        .into_iter()
        .find(|(_, text)| record(text)["root_cause"] == root_cause)
        .map(|(id, text)| (format!("{destination}:{id}"), text))
        .unwrap_or_else(|| panic!("{destination} holds {root_cause}"))
}

/// Seed an existing ticket of another batch on `eng`: its kind, fields and severities.
fn seed(world: &World, id: &str, root_cause: &str, kind: &str, fields: &Value, priority: &str) {
    let mut extra = json!({"kind": kind});
    if fields.as_object().is_some_and(|map| !map.is_empty()) {
        extra["fields"] = fields.clone();
    }
    let mut body = filed_body(&format!("{root_cause} fails for its clients."));
    if let Some(map) = fields.as_object()
        && !map.is_empty()
    {
        let lines: String = map
            .iter()
            .map(|(name, value)| format!("- {name}: {}\n", value.as_str().unwrap_or_default()))
            .collect();
        body = body.replace("## Evidence", &format!("## Fields\n\n{lines}\n## Evidence"));
    }
    world.item(
        "eng",
        id,
        &filed_item(
            &format!("widgets: {root_cause} fails"),
            "nightly-2026-10-01",
            root_cause,
            &extra,
            priority,
        ),
        &body,
    );
}

fn outbox(world: &World, batch: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(world.work().join(format!("outbox/{batch}.json")))
            .expect("the outbox"),
    )
    .expect("JSON")
}

fn write_outbox(world: &World, batch: &str, outbox: &Value) {
    std::fs::write(
        world.work().join(format!("outbox/{batch}.json")),
        serde_json::to_string_pretty(outbox).expect("JSON") + "\n",
    )
    .expect("written");
}

fn problems(checked: &Value) -> String {
    checked["problems"].to_string()
}

fn ops(world: &World) -> Vec<String> {
    world
        .phases
        .iter()
        .flat_map(|(_, operations)| operations.iter())
        .map(|operation| operation["op"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey, every refusal in turn")]
fn every_edge_is_held_to_the_configured_kinds_and_relations() {
    let mut world = example_world();
    let d1 = world.draft(BATCH, "TIMEOUT-42 for acme", &[], "acme: TIMEOUT-42.\n");
    let d2 = world.draft(BATCH, "PAY-7 for acme", &[], "acme: PAY-7.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    world.ticket(
        BATCH,
        &kinded(
            "timeout-42-acme",
            &[&d1],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "acme")],
        ),
    );
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "timeout-42-acme"],
        "",
    );

    // prepare and ticket validate tell the agent the symptom target is absent.
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(
        prepared["absent_targets"],
        json!([{"ticket": "timeout-42-acme", "relation": "occurs-for", "kind": "symptom", "fields": {"error_signature": "TIMEOUT-42"}}]),
        "{prepared}"
    );
    let task = std::fs::read_to_string(prepared["task_template"].as_str().expect("a path"))
        .expect("the task");
    assert!(
        task.contains("`occurs-for` (a `client-symptom` to the `symptom` carrying its `error_signature`: derived from those fields, never staged by hand)")
            && task.contains("`caused-by` (a `symptom` to a `root-cause`, as you judge it: stage each edge with `onefollowup ticket relate --relation caused-by`)"),
        "the task states each relation: {task}"
    );
    let validated = world.ok(
        "agent",
        &["ticket", "validate", "--batch", BATCH, "timeout-42-acme"],
        "",
    );
    assert_eq!(validated["sound"], true, "{validated}");
    assert_eq!(validated["absent_targets"][0]["kind"], "symptom");

    // check refuses the staged client-symptom whose symptom neither exists nor is staged,
    // naming the kind and fields, and publish then writes nothing.
    world.answer(
        BATCH,
        &[answer(
            &d1,
            "client-symptom",
            "filed",
            &["timeout-42-acme"],
            "Verified.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], false, "{checked}");
    assert!(
        problems(&checked).contains("the `symptom` ticket its `occurs-for` edge derives, the one whose `error_signature` is `TIMEOUT-42`, neither exists nor is staged"),
        "{checked}"
    );
    world.fails(2, &["publish", "--batch", BATCH], "");
    assert!(world.filed("eng").is_empty(), "publish wrote nothing");

    // The agent writes the symptoms and two root causes.
    let tickets = [
        kinded(
            "timeout-42",
            &[&d1],
            "symptom",
            &[("error_signature", "TIMEOUT-42")],
        ),
        kinded("pay-7", &[&d2], "symptom", &[("error_signature", "PAY-7")]),
        kinded(
            "pay-7-acme",
            &[&d2],
            "client-symptom",
            &[("error_signature", "PAY-7"), ("client", "acme")],
        ),
        kinded("stale-cache-lease", &[&d1, &d2], "root-cause", &[]),
        kinded("slow-dns", &[&d1], "root-cause", &[]),
    ];
    for ticket in &tickets {
        world.ticket(BATCH, ticket);
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }

    // ticket relate refuses a match-decided relation, an unknown one, and wrong kinds.
    for (args, refused) in [
        (
            ["occurs-for", "timeout-42-acme", "timeout-42"],
            "the relation `occurs-for` is decided by matching `error_signature`: its edges are derived from the tickets' fields, never asserted",
        ),
        (
            ["blames", "timeout-42", "stale-cache-lease"],
            "`blames` is not a configured relation",
        ),
        (
            ["caused-by", "timeout-42-acme", "stale-cache-lease"],
            "the relation `caused-by` goes from a `symptom` to a `root-cause`, and its `from` end `drafts:nightly-2026-11-10/tickets/timeout-42-acme` is a `client-symptom`",
        ),
        (
            ["caused-by", "timeout-42", "nowhere"],
            "`nowhere` is neither a ticket of this batch nor an item it read",
        ),
    ] {
        let stderr = world.fails(
            2,
            &[
                "ticket",
                "relate",
                "--batch",
                BATCH,
                "--relation",
                args[0],
                "--from",
                args[1],
                "--to",
                args[2],
            ],
            "",
        );
        assert!(
            stderr.contains(refused) && stderr.contains("nothing was staged"),
            "{stderr}"
        );
    }
    // A symptom linked to two root causes.
    for (from, to) in [
        ("timeout-42", "stale-cache-lease"),
        ("timeout-42", "slow-dns"),
        ("pay-7", "stale-cache-lease"),
    ] {
        let staged = world.ok(
            "agent",
            &[
                "ticket",
                "relate",
                "--batch",
                BATCH,
                "--relation",
                "caused-by",
                "--from",
                from,
                "--to",
                to,
            ],
            "",
        );
        assert_eq!(staged["write"]["op"], "relate", "{staged}");
    }
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "client-symptom",
                "filed",
                &["timeout-42-acme"],
                "Verified.",
            ),
            answer(&d2, "client-symptom", "filed", &["pay-7-acme"], "Verified."),
        ],
    );
    let sound = outbox(&world, BATCH);

    // An outbox edited directly to hold each broken write: check refuses it, and publish
    // writes nothing.
    let mut mystery = kinded("mystery", &[&d1], "mystery", &[]);
    mystery.title = "a mystery".to_owned();
    world.ticket(BATCH, &mystery);
    for (edit, refused) in [
        (
            json!({"op": "copy", "ticket": "drafts:nightly-2026-11-10/tickets/mystery", "destination": "eng"}),
            vec!["the record's kind `mystery` is not a configured kind"],
        ),
        (
            json!({"op": "relate", "relation": "blames", "from": "drafts:nightly-2026-11-10/tickets/timeout-42", "to": "drafts:nightly-2026-11-10/tickets/slow-dns"}),
            vec!["`blames` is not a configured relation"],
        ),
        (
            json!({"op": "relate", "relation": "caused-by", "from": "drafts:nightly-2026-11-10/tickets/stale-cache-lease", "to": "drafts:nightly-2026-11-10/tickets/timeout-42"}),
            vec![
                "the relation `caused-by` goes from a `symptom` to a `root-cause`, and its `from` end `drafts:nightly-2026-11-10/tickets/stale-cache-lease` is a `root-cause`",
            ],
        ),
        (
            json!({"op": "relate", "relation": "occurs-for", "from": "drafts:nightly-2026-11-10/tickets/timeout-42-acme", "to": "drafts:nightly-2026-11-10/tickets/pay-7"}),
            vec![
                "the relation `occurs-for` is decided by matching `error_signature`",
                "the relation `occurs-for` is many-to-one, and `drafts:nightly-2026-11-10/tickets/timeout-42-acme` is the `from` end of 2 of its edges",
            ],
        ),
    ] {
        let mut edited = sound.clone();
        edited["writes"]
            .as_array_mut()
            .expect("writes")
            .push(edit.clone());
        write_outbox(&world, BATCH, &edited);
        let checked = world.ok("check", &["check", "--batch", BATCH], "");
        assert_eq!(checked["sound"], false, "{edit}: {checked}");
        for message in refused {
            assert!(problems(&checked).contains(message), "{edit}: {checked}");
        }
        world.fails(2, &["publish", "--batch", BATCH], "");
        assert!(
            world.filed("eng").is_empty(),
            "{edit}: publish wrote nothing"
        );
    }
    std::fs::remove_file(
        world
            .work()
            .join(format!("tasks/{BATCH}/tickets/mystery.md")),
    )
    .expect("removed");
    write_outbox(&world, BATCH, &sound);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");

    // Each client-symptom's parent derived with no agent assertion; the symptom linked to
    // both root causes.
    assert!(
        !sound.to_string().contains("occurs-for"),
        "the outbox asserts no occurs-for: {sound}"
    );
    for (child, parent) in [("timeout-42-acme", "timeout-42"), ("pay-7-acme", "pay-7")] {
        let (_, text) = filed_as(&world, "eng", child);
        let (parent_id, _) = filed_as(&world, "eng", parent);
        assert_eq!(
            front(&text)["parent"]
                .as_str()
                .map(|native| format!("eng:{native}")),
            Some(parent_id),
            "{text}"
        );
    }
    let (_, symptom) = filed_as(&world, "eng", "timeout-42");
    let (stale, _) = filed_as(&world, "eng", "stale-cache-lease");
    let (dns, _) = filed_as(&world, "eng", "slow-dns");
    assert_eq!(
        record(&symptom)["edges"],
        json!([{"relation": "caused-by", "to": stale}, {"relation": "caused-by", "to": dns}]),
        "{symptom}"
    );
    let links = front(&symptom)["metadata"]["onetaskgraph.links"].to_string();
    assert!(links.contains(&stale) && links.contains(&dns), "{symptom}");
    let relate: Vec<&Value> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .filter(|one| one["op"] == "relate")
        .collect();
    assert_eq!(relate.len(), 5, "{published}");
    assert!(
        relate.iter().all(|one| one["outcome"] == "written"),
        "{published}"
    );
    world.ok("settle", &["settle", "--batch", BATCH], "");
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey over two batches")]
fn duplicates_merge_by_exact_fields_and_evidence_lands_on_the_most_specific_ticket() {
    let mut world = example_world();
    seed(
        &world,
        "symptom-timeout",
        "timeout-42",
        "symptom",
        &json!({"error_signature": "TIMEOUT-42"}),
        "medium",
    );
    seed(
        &world,
        "client-timeout-acme",
        "timeout-42-acme",
        "client-symptom",
        &json!({"error_signature": "TIMEOUT-42", "client": "acme"}),
        "medium",
    );
    let d1 = world.draft(BATCH, "TIMEOUT-42 again", &[], "TIMEOUT-42 again.\n");
    let d2 = world.draft(BATCH, "PAY-7 for acme", &[], "PAY-7.\n");
    let d3 = world.draft(BATCH, "pay-7 for globex", &[], "pay-7.\n");
    let d4 = world.draft(BATCH, "a stale lease", &[], "Stale.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");

    // A symptom whose error signature an existing symptom carries adds no card.
    world.ticket(
        BATCH,
        &kinded(
            "timeout-again",
            &[&d1],
            "symptom",
            &[("error_signature", " timeout-42 ")],
        ),
    );
    let staged = world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "timeout-again"],
        "",
    );
    assert_eq!(staged["matched"], "eng:symptom-timeout", "{staged}");
    assert_eq!(staged["write"]["op"], "comment", "{staged}");
    // Two symptoms whose error signatures differ only in case are one.
    world.ticket(
        BATCH,
        &kinded("pay-7", &[&d2], "symptom", &[("error_signature", "PAY-7")]),
    );
    world.ticket(
        BATCH,
        &kinded(
            "pay-7-again",
            &[&d3],
            "symptom",
            &[("error_signature", "pay-7")],
        ),
    );
    world.ok("agent", &["ticket", "copy", "--batch", BATCH, "pay-7"], "");
    let staged = world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "pay-7-again"],
        "",
    );
    assert_eq!(
        staged["matched"], "drafts:nightly-2026-11-10/tickets/pay-7",
        "{staged}"
    );
    std::fs::remove_file(
        world
            .work()
            .join(format!("tasks/{BATCH}/tickets/pay-7-again.md")),
    )
    .expect("the merged ticket is deleted");
    // Two root causes stating the same prose that the agent did not judge one stay two.
    let mut first = kinded("stale-lease-reads", &[&d4], "root-cause", &[]);
    first.cause = "A stale cache lease serves old reads.".to_owned();
    let mut second = kinded("stale-lease-writes", &[&d4], "root-cause", &[]);
    second.cause = first.cause.clone();
    for ticket in [&first, &second] {
        world.ticket(BATCH, ticket);
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }
    world.answer(
        BATCH,
        &[
            answer(&d1, "symptom", "filed", &["timeout-again"], "Evidence."),
            answer(&d2, "symptom", "filed", &["pay-7"], "Verified."),
            answer(&d3, "symptom", "filed", &["pay-7"], "The same symptom."),
            answer(
                &d4,
                "root-cause",
                "filed",
                &["stale-lease-reads", "stale-lease-writes"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let filed: Vec<String> = world
        .filed("eng")
        .into_iter()
        .map(|(_, text)| {
            record(&text)["root_cause"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    let mut filed = filed;
    filed.sort();
    assert_eq!(
        filed,
        [
            "pay-7",
            "stale-lease-reads",
            "stale-lease-writes",
            "timeout-42",
            "timeout-42-acme"
        ],
        "no card for the duplicates, two for the root causes"
    );
    world.ok("settle", &["settle", "--batch", BATCH], "");

    // Evidence observed with both fields lands on the client-symptom; naming the symptom
    // instead is refused while that client-symptom exists.
    let d5 = world.draft(LATER, "TIMEOUT-42 for acme", &[], "acme again.\n");
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    let refused = world.fails(
        2,
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--item",
            "eng:symptom-timeout",
            "--field",
            "error_signature=TIMEOUT-42",
            "--field",
            "client=acme",
        ],
        "acme saw it again.\n",
    );
    assert!(
        refused.contains("`eng:client-timeout-acme`, a `client-symptom`, is the most specific existing ticket the observed fields name, and `eng:symptom-timeout` is less specific"),
        "{refused}"
    );
    let unknown = world.fails(
        2,
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--field",
            "region=eu",
        ],
        "acme saw it again.\n",
    );
    assert!(
        unknown.contains("`region` is not a declared field"),
        "{unknown}"
    );
    let unfiled = world.fails(
        2,
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--field",
            "error_signature=DISK-9",
        ],
        "disk.\n",
    );
    assert!(
        unfiled.contains("no existing ticket of a `same` kind carries `error_signature` `DISK-9`"),
        "{unfiled}"
    );
    let commented = world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--field",
            "error_signature=timeout-42",
            "--field",
            "client=ACME",
        ],
        "acme saw it again.\n",
    );
    assert_eq!(commented["write"]["item"], "eng:client-timeout-acme");
    assert_eq!(commented["write"]["root_cause"], "timeout-42-acme");
    let named = world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--item",
            "eng:client-timeout-acme",
            "--field",
            "error_signature=TIMEOUT-42",
            "--field",
            "client=acme",
        ],
        "acme saw it again.\n",
    );
    assert_eq!(
        named["index"], commented["index"],
        "the same comment, restaged"
    );
    world.answer(
        LATER,
        &[answer(
            &d5,
            "client-symptom",
            "filed",
            &["timeout-42-acme"],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATER], "");
    assert!(
        world
            .read_item("eng", "client-timeout-acme")
            .contains(&format!("Additional evidence from follow-up run `{LATER}`"))
    );
    assert!(
        !world
            .read_item("eng", "symptom-timeout")
            .contains(&format!("run `{LATER}`"))
    );
}

/// One batch over the example: a client-symptom under its symptom, the symptom caused by a
/// root cause; checked, published and settled.
fn first_batch(world: &mut World) -> (String, String, String) {
    let d1 = world.draft(BATCH, "timeout-42 fails", &[], "acme: TIMEOUT-42.\n");
    let d2 = world.draft(BATCH, "a stale cache lease", &[], "The lease is stale.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    for ticket in [
        kinded(
            "timeout-42-acme",
            &[&d1],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "acme")],
        ),
        kinded(
            "timeout-42",
            &[&d1],
            "symptom",
            &[("error_signature", "TIMEOUT-42")],
        ),
        kinded("stale-cache-lease", &[&d2], "root-cause", &[]),
    ] {
        world.ticket(BATCH, &ticket);
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }
    world.ok(
        "agent",
        &[
            "ticket",
            "relate",
            "--batch",
            BATCH,
            "--relation",
            "caused-by",
            "--from",
            "timeout-42",
            "--to",
            "stale-cache-lease",
        ],
        "",
    );
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "client-symptom",
                "filed",
                &["timeout-42-acme"],
                "Verified.",
            ),
            answer(
                &d2,
                "root-cause",
                "filed",
                &["stale-cache-lease"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    (
        filed_as(world, "eng", "timeout-42-acme").0,
        filed_as(world, "eng", "timeout-42").0,
        filed_as(world, "eng", "stale-cache-lease").0,
    )
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one journey over a retry and a later batch"
)]
fn an_edge_is_written_once_and_a_lost_link_end_is_reported_then_restored() {
    let mut world = example_world();
    let (_, symptom, cause) = first_batch(&mut world);

    // A retried publish writes no edge that exists, and receipts each as existing.
    for slug in ["timeout-42-acme", "timeout-42", "stale-cache-lease"] {
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    world.ok(
        "agent",
        &[
            "ticket",
            "relate",
            "--batch",
            BATCH,
            "--relation",
            "caused-by",
            "--from",
            "timeout-42",
            "--to",
            "stale-cache-lease",
        ],
        "",
    );
    let before = world.items("eng");
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let retried = world.ok("publish", &["publish", "--batch", BATCH], "");
    assert_eq!(world.items("eng"), before, "the retry wrote nothing");
    let edges: Vec<&Value> = retried["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .filter(|one| one["op"] == "relate")
        .collect();
    assert_eq!(edges.len(), 2, "{retried}");
    assert!(
        edges.iter().all(|one| one["outcome"] == "exists"),
        "{retried}"
    );
    let journal = world.receipts(BATCH);
    let receipted: Vec<&Value> = journal
        .iter()
        .filter(|receipt| receipt["command"] == "relate")
        .collect();
    assert_eq!(receipted.len(), 4, "{journal:#?}");
    assert!(
        receipted[2..]
            .iter()
            .all(|receipt| receipt["outcome"]["action"] == "exists")
    );
    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(settled["pending"], Value::Null, "{settled}");

    // A whole-body rewrite of the root cause's file loses its end of the link; the next
    // batch's prepare reports it, and a publish stating that edge again restores it.
    let (_, cause_text) = filed_as(&world, "eng", "stale-cache-lease");
    let native = cause.trim_start_matches("eng:");
    let rewritten: String = cause_text
        .lines()
        .filter(|line| !line.contains("onetaskgraph.links"))
        .take_while(|line| *line != "## Links")
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(
        world.root.join(format!("eng-root/tasks/{native}.md")),
        rewritten,
    )
    .expect("rewritten");
    world.reset_phases();
    let d3 = world.draft(LATER, "timeout-42 fails", &[], "globex: TIMEOUT-42.\n");
    let d4 = world.draft(LATER, "stale-cache-lease fails", &[], "Still stale.\n");
    let prepared = world.ok("prepare", &["prepare", "--batch", LATER], "");
    assert_eq!(
        prepared["missing_links"],
        json!([{"item": symptom, "relation": "caused-by", "other": cause}]),
        "{prepared}"
    );
    world.ticket(
        LATER,
        &kinded(
            "timeout-42-globex",
            &[&d3],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "globex")],
        ),
    );
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", LATER, "timeout-42-globex"],
        "",
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "relate",
            "--batch",
            LATER,
            "--relation",
            "caused-by",
            "--from",
            &symptom,
            "--to",
            &cause,
        ],
        "",
    );
    world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--item",
            &cause,
            "--root-cause",
            "stale-cache-lease",
        ],
        "Still stale.\n",
    );
    world.answer(
        LATER,
        &[
            answer(
                &d3,
                "client-symptom",
                "filed",
                &["timeout-42-globex"],
                "Verified.",
            ),
            answer(
                &d4,
                "root-cause",
                "filed",
                &["stale-cache-lease"],
                "Evidence.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", LATER], "");
    let restored = world.read_item("eng", native);
    assert!(
        restored.contains("onetaskgraph.links") && restored.contains(&symptom),
        "{restored}"
    );
    let edge = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .find(|one| one["op"] == "relate" && one["item"] == symptom)
        .cloned()
        .unwrap_or_default();
    assert_eq!(edge["outcome"], "written", "{published}");
    let (globex, _) = filed_as(&world, "eng", "timeout-42-globex");
    assert_eq!(
        front(&world.read_item("eng", globex.trim_start_matches("eng:")))["parent"],
        symptom.trim_start_matches("eng:"),
        "its parent derived from the existing symptom"
    );

    // The whole batch made one relations read, one parent write and one link write.
    let made = ops(&world);
    for op in ["relations", "parent-set-many", "link-add-many"] {
        assert_eq!(
            made.iter().filter(|one| *one == op).count(),
            1,
            "{op}: {made:?}"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines, reason = "one journey, each rollup in turn")]
fn rollups_move_only_the_direct_to_ticket_and_keep_a_persons_priority() {
    let mut world = example_world();
    // An existing symptom estimating medium at one occurrence, whose priority a person set
    // to low.
    seed(
        &world,
        "symptom-timeout",
        "timeout-42",
        "symptom",
        &json!({"error_signature": "TIMEOUT-42"}),
        "low",
    );
    let d1 = world.draft(BATCH, "TIMEOUT-42 for acme", &[], "acme.\n");
    let d2 = world.draft(BATCH, "TIMEOUT-42 for globex", &[], "globex.\n");
    let d3 = world.draft(BATCH, "a stale cache lease", &[], "Stale.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    for ticket in [
        kinded(
            "timeout-42-acme",
            &[&d1],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "acme")],
        )
        .severities("high", "medium", "retry"),
        kinded(
            "timeout-42-globex",
            &[&d2],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "globex")],
        )
        .severities("high", "medium", "retry"),
        kinded("stale-cache-lease", &[&d3], "root-cause", &[]).severities(
            "medium",
            "low",
            "restart the cache",
        ),
    ] {
        world.ticket(BATCH, &ticket);
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }
    world.ok(
        "agent",
        &[
            "ticket",
            "relate",
            "--batch",
            BATCH,
            "--relation",
            "caused-by",
            "--from",
            "eng:symptom-timeout",
            "--to",
            "stale-cache-lease",
        ],
        "",
    );
    // max: the root cause is no lower than the symptom's own estimate, medium — not the
    // high its client-symptoms roll up into the symptom, a rollup being one hop.
    let status = world.ok(
        "agent",
        &[
            "ticket",
            "board-status",
            "--batch",
            BATCH,
            "stale-cache-lease",
        ],
        "",
    );
    assert_eq!(status["priority_estimate"], "medium", "{status}");
    assert!(
        status["estimate_line"]
            .as_str()
            .is_some_and(|line| line.contains("held at medium, its related tickets' highest")),
        "{status}"
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert!(
        problems(&checked).contains("prepare has not read the edges `eng:symptom-timeout`"),
        "{checked}"
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "client-symptom",
                "filed",
                &["timeout-42-acme"],
                "Verified.",
            ),
            answer(
                &d2,
                "client-symptom",
                "filed",
                &["timeout-42-globex"],
                "Verified.",
            ),
            answer(
                &d3,
                "root-cause",
                "filed",
                &["stale-cache-lease"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");

    // occurrences: the symptom's two client-symptoms bring it to three, raising its
    // estimate to high; the person's priority on it stands.
    let symptom = world.read_item("eng", "symptom-timeout");
    assert_eq!(record(&symptom)["priority_estimate"], "high", "{symptom}");
    assert_eq!(
        front(&symptom)["priority"],
        "low",
        "a person's priority stands"
    );
    assert!(
        symptom.contains("3 occurrences; raised one level"),
        "{symptom}"
    );
    assert_eq!(
        record(&symptom)["edges"],
        json!([{"relation": "caused-by", "to": filed_as(&world, "eng", "stale-cache-lease").0}]),
        "the edge is recorded on its existing `from`: {symptom}"
    );
    // Neither rollup moves a `from` ticket.
    for client in ["timeout-42-acme", "timeout-42-globex"] {
        assert_eq!(
            record(&filed_as(&world, "eng", client).1)["priority_estimate"],
            "medium"
        );
    }
    let (_, cause) = filed_as(&world, "eng", "stale-cache-lease");
    assert_eq!(record(&cause)["priority_estimate"], "medium", "{cause}");
    assert_eq!(front(&cause)["priority"], "medium");
    world.ok("settle", &["settle", "--batch", BATCH], "");

    // Closing a ticket closes no related ticket: withdrawing a client-symptom leaves its
    // symptom open.
    let mut withdrawn = kinded(
        "timeout-42-acme",
        &[&d1],
        "client-symptom",
        &[("error_signature", "TIMEOUT-42"), ("client", "acme")],
    )
    .severities("high", "medium", "retry");
    withdrawn.status = Some("cancelled".to_owned());
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    world.ticket(BATCH, &withdrawn);
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "timeout-42-acme"],
        "",
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let (_, client) = filed_as(&world, "eng", "timeout-42-acme");
    assert_eq!(front(&client)["status"], "cancelled", "{client}");
    let symptom = world.read_item("eng", "symptom-timeout");
    assert_eq!(front(&symptom)["status"], "backlog", "{symptom}");
}

#[test]
fn a_route_counting_edges_matches_on_the_third_staged_edge_and_not_the_second() {
    let mut world = example_world();
    let d1 = world.draft(BATCH, "three symptoms of one lease", &[], "Three.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut cause = kinded("stale-cache-lease", &[&d1], "root-cause", &[])
        .severities("medium", "low", "restart");
    cause.title = "a stale cache lease".to_owned();
    world.ticket(BATCH, &cause);
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "stale-cache-lease"],
        "",
    );
    let mut priorities = Vec::new();
    for signature in ["TIMEOUT-42", "PAY-7", "DISK-9"] {
        let slug = signature.to_lowercase();
        world.ticket(
            BATCH,
            &kinded(&slug, &[&d1], "symptom", &[("error_signature", signature)])
                .severities("medium", "low", "retry"),
        );
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, &slug], "");
        world.ok(
            "agent",
            &[
                "ticket",
                "relate",
                "--batch",
                BATCH,
                "--relation",
                "caused-by",
                "--from",
                &slug,
                "--to",
                "stale-cache-lease",
            ],
            "",
        );
        let status = world.ok(
            "agent",
            &[
                "ticket",
                "board-status",
                "--batch",
                BATCH,
                "stale-cache-lease",
            ],
            "",
        );
        priorities.push(status["priority"].as_str().unwrap_or_default().to_owned());
    }
    assert_eq!(
        priorities,
        ["low", "low", "high"],
        "widespread-root-cause matches on the third staged edge"
    );
    world.answer(
        BATCH,
        &[answer(
            &d1,
            "root-cause",
            "filed",
            &["stale-cache-lease"],
            "Verified.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let (_, cause) = filed_as(&world, "eng", "stale-cache-lease");
    assert_eq!(front(&cause)["priority"], "high", "{cause}");
    assert!(
        front(&cause)["labels"].to_string().contains("widespread"),
        "{cause}"
    );
}

const TWO_DESTINATIONS: &str = r"host_label: build-host-1
destination: one
default_kind: task
kinds:
  - {name: task, description: Work on one destination, destination: one}
  - {name: epic, description: Work on another, destination: two}
relations:
  - {name: filed-under, from: task, to: epic, cardinality: many-to-one, decided_by: agent, store: parent}
  - {name: see-also, from: task, to: epic, cardinality: many-to-many, decided_by: agent, store: link}
";

#[test]
fn a_parent_across_sources_is_refused_while_a_link_across_them_is_written() {
    let mut world = World::new(&["one", "two"], TWO_DESTINATIONS);
    let d1 = world.draft(BATCH, "a task", &[], "A task.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut task = Ticket::new("a-task", "a task", &[&d1]);
    task.kind = Some("task".to_owned());
    world.ticket(BATCH, &task);
    for slug in ["epic-one", "epic-two"] {
        let mut epic = Ticket::new(slug, slug, &[&d1]);
        epic.kind = Some("epic".to_owned());
        world.ticket(BATCH, &epic);
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    world.ok("agent", &["ticket", "copy", "--batch", BATCH, "a-task"], "");
    let relate = |world: &mut World, relation: &str, to: &str| {
        world.run(
            &[
                "ticket",
                "relate",
                "--batch",
                BATCH,
                "--relation",
                relation,
                "--from",
                "a-task",
                "--to",
                to,
            ],
            "",
        )
    };
    assert_eq!(relate(&mut world, "filed-under", "epic-one").code, 0);
    // A second edge of a many-to-one relation from one ticket is refused.
    let second = relate(&mut world, "filed-under", "epic-two");
    assert_eq!(second.code, 2);
    assert!(
        second.stderr.contains("the relation `filed-under` is many-to-one, and `drafts:nightly-2026-11-10/tickets/a-task` already has its edge to `drafts:nightly-2026-11-10/tickets/epic-one`"),
        "{}",
        second.stderr
    );
    world.answer(
        BATCH,
        &[answer(
            &d1,
            "task",
            "filed",
            &["a-task", "epic-one", "epic-two"],
            "Verified.",
        )],
    );
    // And so is one written straight into the outbox.
    let sound = outbox(&world, BATCH);
    let mut edited = sound.clone();
    edited["writes"].as_array_mut().expect("writes").push(json!({"op": "relate", "relation": "filed-under", "from": "drafts:nightly-2026-11-10/tickets/a-task", "to": "drafts:nightly-2026-11-10/tickets/epic-two"}));
    write_outbox(&world, BATCH, &edited);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert!(
        problems(&checked).contains("the relation `filed-under` is many-to-one, and `drafts:nightly-2026-11-10/tickets/a-task` is the `from` end of 2 of its edges"),
        "{checked}"
    );
    // The parent itself crosses sources: refused at check, with publish writing nothing.
    write_outbox(&world, BATCH, &sound);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], false, "{checked}");
    assert!(
        problems(&checked).contains("is held as the store's parent, which joins two items of one source, and `drafts:nightly-2026-11-10/tickets/a-task` is on `one` while `drafts:nightly-2026-11-10/tickets/epic-one` is on `two`"),
        "{checked}"
    );
    world.fails(2, &["publish", "--batch", BATCH], "");
    assert!(world.filed("one").is_empty() && world.filed("two").is_empty());
    // A link across them is written.
    let mut linked = sound.clone();
    let writes = linked["writes"].as_array_mut().expect("writes");
    writes.retain(|write| write["op"] != "relate");
    write_outbox(&world, BATCH, &linked);
    assert_eq!(relate(&mut world, "see-also", "epic-two").code, 0);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let (task_id, task_text) = filed_as(&world, "one", "a-task");
    let (epic_id, _) = filed_as(&world, "two", "epic-two");
    assert!(
        front(&task_text)["metadata"]["onetaskgraph.links"]
            .to_string()
            .contains(&epic_id),
        "{task_text}"
    );
    let edge = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .find(|one| one["op"] == "relate")
        .cloned()
        .unwrap_or_default();
    assert_eq!(edge["outcome"], "written", "{published}");
    assert_eq!(edge["item"], task_id);
}

#[test]
fn an_edge_whose_end_was_withheld_is_receipted_and_left_pending() {
    let mut world = example_world();
    let config = std::fs::read_to_string(world.root.join("onefollowup.yaml")).expect("read");
    // A root cause also pages its owner, who is not answering.
    let config = config.replace(
        "host_label: build-host-1\n",
        "  - name: page-owner\n    match: {kind: [root-cause]}\n    command: [page-owner]\nhost_label: build-host-1\n",
    );
    std::fs::write(world.root.join("onefollowup.yaml"), config).expect("written");
    world.script(
        "page-owner",
        "cat >/dev/null\necho 'the pager is down' >&2\nexit 3",
    );
    let d1 = world.draft(BATCH, "timeout-42 fails", &[], "TIMEOUT-42.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    for ticket in [
        kinded(
            "timeout-42",
            &[&d1],
            "symptom",
            &[("error_signature", "TIMEOUT-42")],
        ),
        kinded("stale-cache-lease", &[&d1], "root-cause", &[]),
    ] {
        world.ticket(BATCH, &ticket);
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }
    world.ok(
        "agent",
        &[
            "ticket",
            "relate",
            "--batch",
            BATCH,
            "--relation",
            "caused-by",
            "--from",
            "timeout-42",
            "--to",
            "stale-cache-lease",
        ],
        "",
    );
    world.answer(
        BATCH,
        &[answer(
            &d1,
            "symptom",
            "filed",
            &["timeout-42", "stale-cache-lease"],
            "Verified.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
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
    assert!(
        outcomes.contains(&("copy".to_owned(), "route-command-failed".to_owned()))
            && outcomes.contains(&("relate".to_owned(), "endpoint-not-written".to_owned())),
        "{published}"
    );
    assert_eq!(world.filed("eng").len(), 1, "only the symptom landed");
    let (_, symptom) = filed_as(&world, "eng", "timeout-42");
    assert!(
        record(&symptom).get("edges").is_none(),
        "no edge is recorded to a ticket with no item: {symptom}"
    );
    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert_eq!(settled["pending"], 2, "{settled}");
    let pending = std::fs::read_to_string(world.work().join("feedback/pending.json"))
        .expect("the pending writes");
    assert!(
        pending.contains("endpoint-not-written") && pending.contains("route-command-failed"),
        "{pending}"
    );
}

#[test]
fn an_edge_from_a_ticket_found_to_be_an_existing_one_starts_at_that_item() {
    let mut world = example_world();
    seed(
        &world,
        "symptom-timeout",
        "timeout-42",
        "symptom",
        &json!({"error_signature": "TIMEOUT-42"}),
        "medium",
    );
    let d1 = world.draft(BATCH, "TIMEOUT-42 again", &[], "TIMEOUT-42.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    world.ticket(
        BATCH,
        &kinded(
            "timeout-again",
            &[&d1],
            "symptom",
            &[("error_signature", "TIMEOUT-42")],
        ),
    );
    world.ticket(
        BATCH,
        &kinded("stale-cache-lease", &[&d1], "root-cause", &[]),
    );
    for slug in ["timeout-again", "stale-cache-lease"] {
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    let staged = world.ok(
        "agent",
        &[
            "ticket",
            "relate",
            "--batch",
            BATCH,
            "--relation",
            "caused-by",
            "--from",
            "timeout-again",
            "--to",
            "stale-cache-lease",
        ],
        "",
    );
    assert_eq!(staged["write"]["from"], "eng:symptom-timeout", "{staged}");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "timeout-again"],
        "",
    );
    world.answer(
        BATCH,
        &[answer(
            &d1,
            "symptom",
            "filed",
            &["timeout-again", "stale-cache-lease"],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let (cause, _) = filed_as(&world, "eng", "stale-cache-lease");
    let symptom = world.read_item("eng", "symptom-timeout");
    assert_eq!(
        record(&symptom)["edges"],
        json!([{"relation": "caused-by", "to": cause}]),
        "the existing symptom records its new edge: {symptom}"
    );
    assert!(
        front(&symptom)["metadata"]["onetaskgraph.links"]
            .to_string()
            .contains(&cause),
        "{symptom}"
    );
}

const ALERTS: &str = r"host_label: build-host-1
destination: teams
default_kind: alert
fields:
  code: {description: The alert's code}
  team: {description: The team it paged}
kinds:
  - {name: alert, description: One team's alert, duplicates: {same: [code, team]}, destination: teams}
  - {name: incident, description: The incident across teams, duplicates: {same: [code]}, destination: incidents}
relations:
  - {name: raised-for, from: alert, to: incident, cardinality: many-to-one, decided_by: {match: [code]}, store: link}
";

#[test]
fn a_derived_link_across_two_destinations_finds_its_target_on_the_other_one() {
    let mut world = World::new(&["teams", "incidents"], ALERTS);
    let d1 = world.draft(BATCH, "disk full on the payments team", &[], "DISK-9.\n");
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut alert = kinded(
        "disk-9-payments",
        &[&d1],
        "alert",
        &[("code", "DISK-9"), ("team", "payments")],
    );
    alert.kind = Some("alert".to_owned());
    world.ticket(BATCH, &alert);
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(
        prepared["absent_targets"][0]["kind"], "incident",
        "{prepared}"
    );
    let searched: Vec<&Value> = prepared["store_operations"]
        .as_array()
        .expect("operations")
        .iter()
        .filter(|operation| operation["op"] == "find" && operation["source"] == "incidents")
        .collect();
    assert_eq!(
        searched.len(),
        1,
        "the target is looked for on its own destination: {prepared}"
    );
    world.ticket(
        BATCH,
        &kinded("disk-9", &[&d1], "incident", &[("code", "DISK-9")]),
    );
    for slug in ["disk-9-payments", "disk-9"] {
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    world.answer(
        BATCH,
        &[answer(
            &d1,
            "alert",
            "filed",
            &["disk-9-payments"],
            "Verified.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let (incident, _) = filed_as(&world, "incidents", "disk-9");
    let (_, alert) = filed_as(&world, "teams", "disk-9-payments");
    assert_eq!(
        record(&alert)["edges"],
        json!([{"relation": "raised-for", "to": incident}]),
        "{alert}"
    );
}
