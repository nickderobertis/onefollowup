//! The worked configurations under `examples/`, each run through one batch of the built
//! binary to the outcome its README walk-through states.
//!
//! Each case loads the example's `onefollowup.yaml` through `config show` (accepted with no
//! refusal) and its `onetaskgraph.yaml` through the linked store's configuration loader, with
//! no network; then runs a batch with that `onefollowup.yaml` and the example's templates and
//! scripts, its sources replaced by local-md sources of the same names (keeping
//! `assignees: true` where the example sets it), a scripted agent verifying, and a stub on
//! `PATH` standing in for each external command the example names. One test is
//! parameterized over the directories, and fails when one has no case.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::world::{COMMIT, HOST, REPOSITORY, Shape, Ticket, World, answer, filed_body};

/// Every example directory, by name.
fn examples() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .expect("the examples directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn directory(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a directory");
    for entry in std::fs::read_dir(from)
        .expect("readable")
        .filter_map(Result::ok)
    {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copied");
        }
    }
}

/// The example's sources, each with whether it holds assignees, as its `onetaskgraph.yaml`
/// declares them — read through the linked store's own loader, offline.
fn sources(name: &str) -> Vec<(String, bool)> {
    let directory = directory(name);
    let environment = vec![
        ("GH_PROJECTS_TOKEN".to_owned(), "unused".to_owned()),
        ("LINEAR_API_KEY".to_owned(), "unused".to_owned()),
        (
            "HOME".to_owned(),
            std::env::temp_dir().display().to_string(),
        ),
    ];
    let store = onefollowup::store::Store::open(Some(&directory), &directory, &environment)
        .unwrap_or_else(|error| panic!("{name}'s onetaskgraph.yaml loads: {error}"));
    let text = std::fs::read_to_string(directory.join("onetaskgraph.yaml")).expect("readable");
    let document: Value = serde_saphyr::from_str(&text).expect("YAML");
    let declared: Vec<String> = document["sources"]
        .as_object()
        .expect("sources")
        .keys()
        .cloned()
        .collect();
    let mut listed: Vec<String> = store.sources().into_iter().map(|one| one.name).collect();
    listed.sort();
    let mut expected = declared.clone();
    expected.sort();
    assert_eq!(listed, expected, "{name}: the loader declares every source");
    assert!(
        store.operations().is_empty(),
        "loading asked no source anything"
    );
    declared
        .into_iter()
        .map(|source| {
            let assigning = store.source_setting(&source, "assignees") == Some(Value::Bool(true));
            (source, assigning)
        })
        .collect()
}

/// A world running `name`'s configuration, its templates and scripts copied beside it.
pub fn world_of(name: &str) -> World {
    let declared = sources(name);
    let destinations: Vec<&str> = declared
        .iter()
        .map(|(source, _)| source.as_str())
        .filter(|source| *source != "drafts")
        .collect();
    let assigning: Vec<&str> = declared
        .iter()
        .filter(|(_, assigning)| *assigning)
        .map(|(source, _)| source.as_str())
        .collect();
    let config = std::fs::read_to_string(directory(name).join("onefollowup.yaml"))
        .expect("the example's configuration");
    let config = format!("{config}host_label: build-host-1\n");
    let mut world = World::assigning(&destinations, &assigning, &config);
    // An example naming its own templates is written in the shape they ask for.
    world.shape = match name {
        "symptoms-and-root-causes" | "typed-destinations" => Some(Shape::Lined),
        "replaced-shape" => Some(Shape::Owned),
        _ => None,
    };
    for part in ["templates", "scripts"] {
        let from = directory(name).join(part);
        if from.is_dir() {
            copy_tree(&from, &world.root.join(part));
        }
    }
    let shown = world.ok("config", &["config", "show"], "");
    assert_eq!(shown["schema_version"], 1, "{name}: {shown}");
    world.reset_phases();
    world
}

/// The front matter of an item, as YAML read into JSON.
fn front(text: &str) -> Value {
    let inner = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(front, _)| front)
        .expect("front matter");
    serde_saphyr::from_str(inner).expect("YAML")
}

fn record(text: &str) -> Value {
    front(text)["metadata"]["orchestrator.follow-up"].clone()
}

/// The filed item of `destination` whose record names `root_cause`, with its id.
pub fn filed(world: &World, destination: &str, root_cause: &str) -> (String, String) {
    world
        .filed(destination)
        .into_iter()
        .find(|(_, text)| record(text)["root_cause"] == root_cause)
        .unwrap_or_else(|| panic!("{destination} holds the ticket {root_cause}"))
}

#[test]
fn each_example_runs_a_batch_to_the_outcome_its_walk_through_states() {
    let cases: &[(&str, fn())] = &[
        ("inline-fixes", inline_fixes),
        ("replaced-shape", replaced_shape),
        ("single-root-cause", single_root_cause),
        ("symptoms-and-root-causes", symptoms_and_root_causes),
        ("typed-destinations", typed_destinations),
    ];
    let named: Vec<String> = cases.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(
        examples(),
        named,
        "every directory under examples/ has a case here, and every case a directory"
    );
    for (_, case) in cases {
        case();
    }
}

fn single_root_cause() {
    const BATCH: &str = "nightly-2026-11-10";
    let mut world = world_of("single-root-cause");
    let root = world.shell_root();
    world.script(
        "onepipeline",
        &format!(
            "echo \"$*\" >> {root}/onepipeline.log\ncat >> {root}/onepipeline.log\ncat \"$6\" >> {root}/onepipeline.log"
        ),
    );
    world.variables.push((
        "FOLLOW_UP_SESSION".to_owned(),
        "follow-ups@build-host-1".to_owned(),
    ));
    std::fs::create_dir_all(world.root.join("target/budget-results")).expect("a directory");
    std::fs::write(
        world.root.join("target/budget-results/overruns.json"),
        json!([{"id": "budget:e2e-time", "source": "budget-overrun", "summary": "the e2e tier took 12 minutes", "detail": "Over its 10-minute budget after one change."}]).to_string(),
    )
    .expect("the overruns");
    world.item(
        "followups",
        "export-older",
        &crate::world::filed_item(
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
        "a stale cache lease serves old layouts",
        &["src/cache.rs"],
        "The cache served a layout from before the save.\n",
    );
    let d2 = world.draft(
        BATCH,
        "a stale cache lease outlives its writer",
        &["src/lease.rs"],
        "The lease was still held after the writer exited.\n",
    );
    let d3 = world.draft(
        BATCH,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "café became caf.\n",
    );
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert_eq!(
        prepared["inputs"], 1,
        "the hook added the overrun: {prepared}"
    );
    world.ticket(
        BATCH,
        &Ticket::new(
            "stale-cache-lease",
            "a stale cache lease serves old layouts",
            &[&d1, &d2],
        ),
    );
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, "stale-cache-lease"],
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
        "Host build-host-1: café exported as caf.\n",
    );
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "root-cause",
                "filed",
                &["stale-cache-lease"],
                "Verified.",
            ),
            answer(
                &d2,
                "root-cause",
                "filed",
                &["stale-cache-lease"],
                "Verified.",
            ),
            answer(
                &d3,
                "root-cause",
                "filed",
                &["export-dialog-drops-unicode"],
                "Evidence.",
            ),
            answer(
                "budget:e2e-time",
                "root-cause",
                "not-reproducible",
                &[],
                "Three reruns stayed under budget.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");

    let (_, text) = filed(&world, "followups", "stale-cache-lease");
    assert_eq!(front(&text)["status"], "backlog", "at Proposal: {text}");
    let held = record(&text);
    assert_eq!(
        held["drafts"],
        json!([d1, d2]),
        "one ticket naming both drafts"
    );
    assert_eq!(
        held["priority_estimate"], "medium",
        "its priority estimated: {held}"
    );
    for absent in ["kind", "fields", "edges", "board_assignee"] {
        assert!(held.get(absent).is_none(), "{absent} is omitted: {held}");
    }
    // The destination body is the shipped default's: the authored parts, with the workaround's
    // cost, the priority calculation and the contributing locations collapsed, the values the
    // record's `impact` holds; the comment, handed no sources, is 0.5.0's opening line, the
    // evidence and the marker.
    let (id, _) = filed(&world, "followups", "stale-cache-lease");
    let (_, body) = item_parts(&world, "followups", &id);
    let authored = Ticket::new(
        "stale-cache-lease",
        "a stale cache lease serves old layouts",
        &[&d1, &d2],
    )
    .render(BATCH);
    let authored = authored
        .split_once("\n---\n")
        .map(|(_, body)| body)
        .expect("a body");
    let rendered = authored
        .replacen(
            "**Cost of the workaround.**\n- Applying it: a moment per save, by the user\n- Side effects on the outcome: none\n- Discovery: the failure names it",
            "\n<details>\n<summary>Cost of the workaround</summary>\n\n- Workaround: avoid resizing while the save indicator shows\n- Applying it: a moment per save, by the user\n- Side effects on the outcome: none\n- Discovery: the failure names it\n\n</details>\n\n<details>\n<summary>Priority calculation</summary>\n\n- Severity: high\n- Severity with the workaround: medium\n- Priority estimate: medium (severity with the workaround medium; fires intermittently; 1 occurrence; not raised)\n\n</details>",
            1,
        )
        .replacen(
            "**Contributing locations.**\n- widgets `src/layout.rs`: reads the size twice",
            "<details>\n<summary>Contributing locations</summary>\n\n- widgets `src/layout.rs`: reads the size twice\n\n</details>",
            1,
        );
    assert_eq!(body.trim_end(), rendered.trim_end(), "the destination body");
    let older = world.read_item("followups", "export-older");
    assert!(
        crate::journey::comments(&older).contains(&format!(
            "Additional evidence from follow-up run `{BATCH}`.\n\nHost build-host-1: café exported as caf.\n\n<!-- orchestrator:follow-up-comment run=\"{BATCH}\" root_cause=\"export-dialog-drops-unicode\" -->"
        )),
        "{older}"
    );
    assert_eq!(
        world.filed("followups").len(),
        2,
        "one new ticket beside the existing one"
    );
    for receipt in world.receipts(BATCH) {
        assert!(
            ["copy", "comment"].contains(&receipt["command"].as_str().unwrap_or_default()),
            "nothing is linked: {receipt}"
        );
    }
    let ops: Vec<(String, String, String, u64)> = world.destination_operations();
    assert!(
        !ops.iter().any(
            |(_, op, _, _)| ["relations", "parent-set-many", "link-add-many"]
                .contains(&op.as_str())
        ),
        "{ops:?}"
    );
    let settled =
        std::fs::read_to_string(world.root.join("onepipeline.log")).expect("the settled hook ran");
    assert!(
        settled
            .lines()
            .next()
            .and_then(
                |line| line.strip_prefix("human-items raise --phase follow-up-review --context ")
            )
            .is_some_and(|path| std::path::Path::new(path).is_absolute()),
        "the hook is handed the context file's absolute path: {settled}"
    );
    assert!(
        settled.contains("\"session\": \"follow-ups@build-host-1\""),
        "the context file names the owner: {settled}"
    );
    assert!(
        settled.contains(&format!("\"batch\":\"{BATCH}\"")),
        "{settled}"
    );
}

pub fn symptom_ticket(slug: &str, drafts: &[&str], kind: &str, fields: &[(&str, &str)]) -> Ticket {
    let mut ticket = Ticket::new(slug, &format!("{slug} fails"), drafts);
    ticket.kind = Some(kind.to_owned());
    ticket.fields = fields
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    ticket
}

#[allow(clippy::too_many_lines, reason = "one walk-through, two batches")]
fn symptoms_and_root_causes() {
    const BATCH: &str = "nightly-2026-11-10";
    let mut world = world_of("symptoms-and-root-causes");
    let d1 = world.draft(
        BATCH,
        "TIMEOUT-42 for acme",
        &["src/cache.rs"],
        "acme: TIMEOUT-42 on checkout.\n",
    );
    let d2 = world.draft(
        BATCH,
        "TIMEOUT-42 for globex",
        &["src/cache.rs"],
        "globex: TIMEOUT-42 on checkout.\n",
    );
    let d3 = world.draft(
        BATCH,
        "PAY-7 for acme",
        &["src/pay.rs"],
        "acme: PAY-7 on payment.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let clients = [
        symptom_ticket(
            "timeout-42-acme",
            &[&d1],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "acme")],
        )
        .severities("medium", "low", "retry the checkout"),
        symptom_ticket(
            "timeout-42-globex",
            &[&d2],
            "client-symptom",
            &[("error_signature", "TIMEOUT-42"), ("client", "globex")],
        )
        .severities("medium", "low", "retry the checkout"),
        symptom_ticket(
            "pay-7-acme",
            &[&d3],
            "client-symptom",
            &[("error_signature", "PAY-7"), ("client", "acme")],
        )
        .severities("medium", "low", "pay again"),
    ];
    for ticket in &clients {
        world.ticket(BATCH, ticket);
    }
    // The agent is told the two symptom targets are absent, and writes and stages them.
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let absent: Vec<(String, String)> = prepared["absent_targets"]
        .as_array()
        .expect("absent targets")
        .iter()
        .map(|one| {
            (
                one["kind"].as_str().unwrap_or_default().to_owned(),
                one["fields"]["error_signature"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect();
    assert_eq!(
        absent,
        [
            ("symptom", "PAY-7"),
            ("symptom", "TIMEOUT-42"),
            ("symptom", "TIMEOUT-42")
        ]
        .map(|(kind, value)| (kind.to_owned(), value.to_owned())),
        "{prepared}"
    );
    let symptoms = [
        symptom_ticket(
            "timeout-42",
            &[&d1, &d2],
            "symptom",
            &[("error_signature", "TIMEOUT-42")],
        )
        .severities("medium", "low", "retry the checkout"),
        symptom_ticket("pay-7", &[&d3], "symptom", &[("error_signature", "PAY-7")]).severities(
            "high",
            "medium",
            "pay again",
        ),
    ];
    for ticket in &symptoms {
        world.ticket(BATCH, ticket);
    }
    let mut cause = Ticket::new(
        "stale-cache-lease",
        "a stale cache lease serves old reads",
        &[&d1, &d2, &d3],
    )
    .severities("medium", "low", "restart the cache");
    cause.kind = Some("root-cause".to_owned());
    world.ticket(BATCH, &cause);
    for slug in [
        "timeout-42-acme",
        "timeout-42-globex",
        "pay-7-acme",
        "timeout-42",
        "pay-7",
        "stale-cache-lease",
    ] {
        let validated = world.ok("agent", &["ticket", "validate", "--batch", BATCH, slug], "");
        assert_eq!(validated["sound"], true, "{validated}");
        assert!(validated.get("absent_targets").is_none(), "{validated}");
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
    }
    for symptom in ["timeout-42", "pay-7"] {
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
                symptom,
                "--to",
                "stale-cache-lease",
            ],
            "",
        );
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
            answer(
                &d2,
                "client-symptom",
                "filed",
                &["timeout-42-globex"],
                "Verified.",
            ),
            answer(&d3, "client-symptom", "filed", &["pay-7-acme"], "Verified."),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.reset_phases();
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let ops: Vec<String> = world
        .destination_operations()
        .into_iter()
        .map(|(_, op, _, _)| op)
        .collect();
    for op in ["parent-set-many", "link-add-many"] {
        assert_eq!(
            ops.iter().filter(|one| *one == op).count(),
            1,
            "{op}: {ops:?}"
        );
    }

    // Filed: six tickets on `eng`.
    assert_eq!(world.filed("eng").len(), 6, "{published}");
    let id = |world: &World, slug: &str| format!("eng:{}", filed(world, "eng", slug).0);
    // Linked: each client-symptom a sub-issue of its symptom, derived; each symptom
    // caused-by the root cause, asserted.
    for (child, parent) in [
        ("timeout-42-acme", "timeout-42"),
        ("timeout-42-globex", "timeout-42"),
        ("pay-7-acme", "pay-7"),
    ] {
        let (_, text) = filed(&world, "eng", child);
        assert_eq!(
            front(&text)["parent"]
                .as_str()
                .map(|native| format!("eng:{native}")),
            Some(id(&world, parent)),
            "{text}"
        );
        assert_eq!(
            record(&text)["edges"],
            json!([{"relation": "occurs-for", "to": id(&world, parent)}]),
            "{text}"
        );
    }
    for symptom in ["timeout-42", "pay-7"] {
        let (_, text) = filed(&world, "eng", symptom);
        assert_eq!(
            record(&text)["edges"],
            json!([{"relation": "caused-by", "to": id(&world, "stale-cache-lease")}]),
            "{text}"
        );
    }
    let relate: Vec<Value> = world
        .receipts(BATCH)
        .into_iter()
        .filter(|receipt| receipt["command"] == "relate")
        .collect();
    assert_eq!(relate.len(), 5, "{relate:#?}");
    assert!(
        relate
            .iter()
            .all(|receipt| receipt["outcome"]["action"] == "written"),
        "{relate:#?}"
    );
    assert_eq!(
        relate
            .iter()
            .filter(|receipt| receipt["outcome"]["derived"] == true)
            .count(),
        3
    );
    // Rollups: each symptom counts its client-symptoms' occurrences; the root cause's
    // estimate is no lower than its highest symptom's.
    let estimate = |slug: &str| record(&filed(&world, "eng", slug).1)["priority_estimate"].clone();
    assert_eq!(
        estimate("timeout-42"),
        "medium",
        "three occurrences raise a low estimate"
    );
    assert_eq!(estimate("pay-7"), "medium");
    assert_eq!(
        estimate("stale-cache-lease"),
        "medium",
        "no lower than PAY-7, its own being low"
    );
    assert_eq!(
        estimate("timeout-42-acme"),
        "low",
        "a rollup moves only the `to` ticket"
    );
    let (_, cause_text) = filed(&world, "eng", "stale-cache-lease");
    assert!(
        !front(&cause_text)["labels"]
            .to_string()
            .contains("widespread"),
        "two edges: {cause_text}"
    );
    assert_eq!(
        front(&cause_text)["priority"],
        "medium",
        "two edges match no route: {cause_text}"
    );

    // A later batch: evidence for TIMEOUT-42 and acme lands on that client-symptom, and a
    // third symptom linked to the root cause makes it widespread.
    const LATER: &str = "nightly-2026-11-11";
    world.reset_phases();
    let d4 = world.draft(
        LATER,
        "TIMEOUT-42 for acme again",
        &["src/cache.rs"],
        "acme: TIMEOUT-42 again.\n",
    );
    let d5 = world.draft(
        LATER,
        "DISK-9 for initech",
        &["src/cache.rs"],
        "initech: DISK-9 on export.\n",
    );
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    let commented = world.ok(
        "agent",
        &[
            "ticket",
            "comment",
            "--batch",
            LATER,
            "--field",
            "error_signature=TIMEOUT-42",
            "--field",
            "client=acme",
        ],
        "Host build-host-1: acme saw TIMEOUT-42 again.\n",
    );
    assert_eq!(
        commented["write"]["item"],
        id(&world, "timeout-42-acme"),
        "{commented}"
    );
    world.ticket(
        LATER,
        &symptom_ticket(
            "disk-9-initech",
            &[&d5],
            "client-symptom",
            &[("error_signature", "DISK-9"), ("client", "initech")],
        )
        .severities("medium", "low", "export again"),
    );
    world.ticket(
        LATER,
        &symptom_ticket(
            "disk-9",
            &[&d5],
            "symptom",
            &[("error_signature", "DISK-9")],
        )
        .severities("medium", "low", "export again"),
    );
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    for slug in ["disk-9-initech", "disk-9"] {
        world.ok("agent", &["ticket", "copy", "--batch", LATER, slug], "");
    }
    let searched = world.ok(
        "agent",
        &[
            "ticket",
            "search",
            "--batch",
            LATER,
            "--ticket",
            "disk-9",
            "stale cache lease",
        ],
        "",
    );
    assert!(
        searched["hits"]
            .to_string()
            .contains(&id(&world, "stale-cache-lease")),
        "{searched}"
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
            "disk-9",
            "--to",
            &id(&world, "stale-cache-lease"),
        ],
        "",
    );
    world.answer(
        LATER,
        &[
            answer(
                &d4,
                "client-symptom",
                "filed",
                &["timeout-42-acme"],
                "Evidence.",
            ),
            answer(
                &d5,
                "client-symptom",
                "filed",
                &["disk-9-initech"],
                "Verified.",
            ),
        ],
    );
    // The root cause was found after prepare read the found tickets' edges: check asks for
    // that read, which the prepare it names makes.
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], false, "{checked}");
    assert!(
        checked["problems"]
            .to_string()
            .contains("prepare has not read the edges"),
        "{checked}"
    );
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATER], "");
    let (_, acme) = filed(&world, "eng", "timeout-42-acme");
    assert!(
        acme.contains(&format!("Additional evidence from follow-up run `{LATER}`")),
        "{acme}"
    );
    let (_, cause_text) = filed(&world, "eng", "stale-cache-lease");
    assert_eq!(
        front(&cause_text)["priority"],
        "high",
        "widespread-root-cause sets high on the third edge: {cause_text}"
    );
    assert_eq!(
        record(&cause_text)["priority_estimate"],
        "medium",
        "the estimate itself stands: {cause_text}"
    );
    let receipts = world.receipts(LATER);
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt["command"] == "re-estimate"
                && receipt["card"] == id(&world, "stale-cache-lease")),
        "{receipts:#?}"
    );
}

/// The front matter's list `key` of an item, as strings: a label by its name.
fn list(text: &str, key: &str) -> Vec<String> {
    front(text)[key]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|one| one.as_str().or_else(|| one["name"].as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn typed_destinations() {
    typed_batch(false);
    typed_batch(true);
}

/// The typed-destinations batch; with `paging_fails`, the on-call pager refuses.
#[allow(clippy::too_many_lines, reason = "one walk-through, and a later batch")]
fn typed_batch(paging_fails: bool) {
    const BATCH: &str = "nightly-2026-11-10";
    const ON_CALL: &str = "Priya Natarajan";
    let mut world = world_of("typed-destinations");
    let root = world.shell_root();
    // The paging CLI the notify script calls: it logs what it was handed and what the bug
    // tracker held at that moment, and names this week's on-call.
    world.script(
        "oncall",
        &format!(
            "case \"$1\" in\n  page) {{ echo \"--- page $*\"; ls {root}/bug-tracker-root/tasks; echo '--- stdin'; cat; echo; }} >> {root}/oncall.log\n    [ -f {root}/oncall.fail ] && {{ echo 'the pager is down' >&2; exit 3; }}\n    exit 0 ;;\n  who) echo '{ON_CALL}' ;;\nesac"
        ),
    );
    if paging_fails {
        std::fs::write(world.root.join("oncall.fail"), "").expect("written");
    }
    // As a filed ticket carries it: its declared fields in its content too.
    let security_body = filed_body("The image library decodes untrusted input unchecked.").replace(
        "## Evidence",
        "## Fields\n\n- error_signature: CVE-2026-1234\n\n## Evidence",
    );
    world.item(
        "security-tracker",
        "cve-older",
        &crate::world::filed_item(
            "widgets: the image library decodes untrusted input",
            "nightly-2026-10-01",
            "image-library-cve",
            &json!({"kind": "security", "fields": {"error_signature": "CVE-2026-1234"}}),
            "medium",
        ),
        &security_body,
    );
    let d1 = world.draft(
        BATCH,
        "the checkout crashes on submit",
        &["src/checkout.rs"],
        "Every submit crashes.\n",
    );
    let d2 = world.draft(
        BATCH,
        "the search box drops the last character",
        &["src/search.rs"],
        "Typing fast loses a letter.\n",
    );
    let d3 = world.draft(
        BATCH,
        "a bulk export of reports",
        &["src/export.rs"],
        "Users ask to export many reports at once.\n",
    );
    let d4 = world.draft(
        BATCH,
        "cve-2026-1234 in the image library",
        &["Cargo.lock"],
        "The scanner reports CVE-2026-1234.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut critical = Ticket::new(
        "checkout-crashes-on-submit",
        "the checkout crashes on submit",
        &[&d1],
    )
    .severities("critical", "critical", "none");
    critical.kind = Some("bug".to_owned());
    let mut high = Ticket::new(
        "search-drops-last-character",
        "the search box drops the last character",
        &[&d2],
    )
    .severities("high", "high", "none");
    high.kind = Some("bug".to_owned());
    let mut improvement = Ticket::new("bulk-report-export", "a bulk export of reports", &[&d3]);
    improvement.kind = Some("improvement".to_owned());
    let mut security = Ticket::new(
        "image-library-cve",
        "the image library decodes untrusted input",
        &[&d4],
    );
    security.kind = Some("security".to_owned());
    security.fields = vec![("error_signature".to_owned(), "cve-2026-1234 ".to_owned())];
    for ticket in [&critical, &high, &improvement, &security] {
        world.ticket(BATCH, ticket);
        world.ok(
            "agent",
            &["ticket", "copy", "--batch", BATCH, &ticket.slug],
            "",
        );
    }
    let mut answers = vec![
        answer(
            &d1,
            "bug",
            "filed",
            &["checkout-crashes-on-submit"],
            "Reproduced.",
        ),
        answer(
            &d2,
            "bug",
            "filed",
            &["search-drops-last-character"],
            "Reproduced.",
        ),
        answer(
            &d3,
            "improvement",
            "filed",
            &["bulk-report-export"],
            "Requested twice.",
        ),
        answer(
            &d4,
            "security",
            "too-low-impact",
            &[],
            "Only an internal tool decodes images.",
        ),
    ];
    world.answer(BATCH, &answers);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], false, "{checked}");
    assert!(
        checked["problems"]
            .to_string()
            .contains("the kind `security` does not allow the disposition `too-low-impact`"),
        "{checked}"
    );
    answers[3] = answer(
        &d4,
        "security",
        "filed",
        &["image-library-cve"],
        "Merged with the open finding.",
    );
    world.answer(BATCH, &answers);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");

    // The pager received the critical bug's ticket before that ticket's write.
    let paged = std::fs::read_to_string(world.root.join("oncall.log")).expect("the pager ran");
    let (listed, handed) = paged.split_once("--- stdin\n").expect("the log's parts");
    assert!(
        listed.starts_with("--- page page --schedule bugs"),
        "{paged}"
    );
    assert_eq!(
        listed.lines().count(),
        1,
        "bug-tracker held nothing when it was paged: {paged}"
    );
    let handed: Value = serde_json::from_str(handed.trim()).expect("the route command's document");
    assert_eq!(handed["route"], "page-on-critical");
    assert_eq!(handed["ticket"]["root_cause"], "checkout-crashes-on-submit");
    assert_eq!(handed["severity"], "critical");

    let (_, high_text) = filed(&world, "bug-tracker", "search-drops-last-character");
    assert_eq!(list(&high_text, "labels"), ["bug"], "{high_text}");
    assert_eq!(list(&high_text, "assignees"), ["jane.doe"], "{high_text}");
    assert_eq!(front(&high_text)["priority"], "high", "at its estimate");
    assert_eq!(record(&high_text)["board_assignee"], "jane.doe");
    let (_, improvement_text) = filed(&world, "product-triage", "bulk-report-export");
    assert_eq!(
        front(&improvement_text)["priority"],
        "low",
        "{improvement_text}"
    );
    assert!(list(&improvement_text, "assignees").is_empty());
    assert_eq!(
        world.filed("security-tracker").len(),
        1,
        "merged with the open finding"
    );
    // The merge's evidence comment, handed the ticket's sources, opens with the default
    // template's collapsed block of them.
    let older = world.read_item("security-tracker", "cve-older");
    assert!(
        older.contains(&format!(
            "<details>\n<summary>Additional evidence from follow-up run {BATCH}.</summary>\n\n- Host: {HOST}\n"
        )),
        "{older}"
    );
    if paging_fails {
        // Only the critical bug's write is withheld, receipted, and pending a retry.
        assert_eq!(world.filed("bug-tracker").len(), 1, "{published}");
        let withheld: Vec<&Value> = published["outcomes"]
            .as_array()
            .expect("outcomes")
            .iter()
            .filter(|one| one["outcome"] == "route-command-failed")
            .collect();
        assert_eq!(withheld.len(), 1, "{published}");
        assert!(
            withheld[0]["error"]
                .as_str()
                .is_some_and(|error| error.contains("the pager is down")),
            "{published}"
        );
        let pending = std::fs::read_to_string(world.work().join("feedback/pending.json"))
            .expect("a pending write");
        assert!(
            pending.contains("route-command-failed") && pending.contains("withheld"),
            "{pending}"
        );
        return;
    }
    let (critical_id, critical_text) = filed(&world, "bug-tracker", "checkout-crashes-on-submit");
    assert_eq!(
        front(&critical_text)["priority"],
        "urgent",
        "{critical_text}"
    );
    assert_eq!(list(&critical_text, "labels"), ["escalated"]);
    assert_eq!(
        list(&critical_text, "assignees"),
        [ON_CALL],
        "assigned to the on-call the command named"
    );
    assert_eq!(record(&critical_text)["board_assignee"], ON_CALL);

    // A later batch finds a person reassigned both bugs, and leaves that person's choice.
    const LATER: &str = "nightly-2026-11-11";
    let (high_id, _) = filed(&world, "bug-tracker", "search-drops-last-character");
    for id in [&critical_id, &high_id] {
        let text = world.read_item("bug-tracker", id);
        let held = list(&text, "assignees");
        world.edit_item("bug-tracker", id, &format!("- {}", held[0]), "- Sam Okafor");
    }
    let d5 = world.draft(
        LATER,
        "the checkout crashes on submit",
        &["src/checkout.rs"],
        "Seen again.\n",
    );
    let d6 = world.draft(
        LATER,
        "the search box drops the last character",
        &["src/search.rs"],
        "Seen again.\n",
    );
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    for (id, root_cause) in [
        (&critical_id, "checkout-crashes-on-submit"),
        (&high_id, "search-drops-last-character"),
    ] {
        let item = format!("bug-tracker:{id}");
        world.ok(
            "agent",
            &[
                "ticket",
                "comment",
                "--batch",
                LATER,
                "--item",
                &item,
                "--root-cause",
                root_cause,
            ],
            "Seen again at the basis.\n",
        );
        world.ok(
            "agent",
            &["ticket", "re-estimate", "--batch", LATER, &item],
            "",
        );
    }
    world.answer(
        LATER,
        &[
            answer(
                &d5,
                "bug",
                "filed",
                &["checkout-crashes-on-submit"],
                "Evidence.",
            ),
            answer(
                &d6,
                "bug",
                "filed",
                &["search-drops-last-character"],
                "Evidence.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATER], "");
    for id in [&critical_id, &high_id] {
        let text = world.read_item("bug-tracker", id);
        assert_eq!(
            list(&text, "assignees"),
            ["Sam Okafor"],
            "a person's choice stands: {text}"
        );
    }
}

fn inline_fixes() {
    const BATCH: &str = "nightly-2026-11-10";
    let mut world = world_of("inline-fixes");
    let d1 = world.draft(
        BATCH,
        "the zoom level resets",
        &["src/zoom.rs"],
        "Zoom resets.\n",
    );
    let d2 = world.draft(
        BATCH,
        "the undo stack forgets renames",
        &["src/undo.rs"],
        "Undo skips a rename.\n",
    );
    let d3 = world.draft(
        BATCH,
        "the font service times out",
        &["src/fonts.rs"],
        "The vendor's font service times out.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let undo = Ticket::new(
        "undo-forgets-renames",
        "the undo stack forgets renames",
        &[&d2],
    );
    let mut fonts = Ticket::new(
        "font-service-times-out",
        "the font service times out",
        &[&d3],
    );
    fonts.kind = Some("external-defect".to_owned());
    for ticket in [&undo, &fonts] {
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
            answer(&d1, "root-cause", "fixed-inline", &[], "Fixed in this run."),
            answer(
                &d2,
                "root-cause",
                "filed",
                &["undo-forgets-renames"],
                "Verified.",
            ),
            answer(
                &d3,
                "external-defect",
                "fixed-inline",
                &[],
                "Fixed in 0123456789abcdef0123456789abcdef01234567.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], false, "{checked}");
    let problems = checked["problems"].to_string();
    assert!(
        problems.contains("the disposition `fixed-inline` requires the change that addressed it"),
        "{problems}"
    );
    assert!(
        problems
            .contains("the kind `external-defect` does not allow the disposition `fixed-inline`"),
        "{problems}"
    );
    world.answer(
        BATCH,
        &[
            answer(
                &d1,
                "root-cause",
                "fixed-inline",
                &[],
                "Fixed in https://github.com/example/widgets/pull/12.",
            ),
            answer(
                &d2,
                "root-cause",
                "filed",
                &["undo-forgets-renames"],
                "Verified.",
            ),
            answer(
                &d3,
                "external-defect",
                "filed",
                &["font-service-times-out"],
                "Verified.",
            ),
        ],
    );
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    let filed_items = world.filed("backlog");
    assert_eq!(
        filed_items.len(),
        2,
        "nothing for the fixed draft: {filed_items:?}"
    );
    for (_, text) in &filed_items {
        assert_eq!(front(text)["status"], "backlog", "at Proposal: {text}");
        assert_eq!(front(text)["priority"], "low", "{text}");
    }
    assert_eq!(
        record(&filed(&world, "backlog", "font-service-times-out").1)["kind"],
        "external-defect"
    );
}

/// The record of the qualified `card`, read back through the store's own read.
fn stored_record(world: &World, card: &str) -> Value {
    let environment = vec![(
        "HOME".to_owned(),
        world.root.join("home").display().to_string(),
    )];
    let store = onefollowup::store::Store::open(Some(&world.root), &world.root, &environment)
        .expect("the world's store opens");
    let read = store
        .read(&[card.to_owned()], false)
        .expect("the store reads it");
    read[0].item.as_ref().expect("the store holds it").metadata["orchestrator.follow-up"].clone()
}

/// The front matter's record and the body of `destination`'s item `id`.
fn item_parts(world: &World, destination: &str, id: &str) -> (Value, String) {
    let text = world.read_item(destination, id);
    let body = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(_, body)| body.to_owned())
        .expect("front matter, then a body");
    (record(&text), body)
}

#[allow(
    clippy::too_many_lines,
    reason = "one walk-through: a batch filed, another batch's evidence, a re-copy"
)]
fn replaced_shape() {
    const BATCH: &str = "nightly-2026-11-10";
    const LATER: &str = "nightly-2026-11-11";
    const LATEST: &str = "nightly-2026-11-12";
    let mut world = world_of("replaced-shape");
    let witness = format!("{REPOSITORY} src/layout.rs:42 at {COMMIT}");

    // `config show --sources` reads the reshaped keys from the example's file.
    let shown = world.ok("config", &["config", "show", "--sources"], "");
    for key in ["kinds", "comment_templates"] {
        assert_eq!(shown["sources"][key]["layer"], "file", "{key}: {shown}");
    }
    let kind = &shown["config"]["kinds"][0];
    assert_eq!(kind["render_template"], "templates/render.md.j2");
    assert_eq!(
        kind["projections"]["root_cause"],
        "{{ ticket.section('Root cause').content }}"
    );
    assert_eq!(
        shown["config"]["comment_templates"]["evidence"],
        "templates/evidence.md.j2"
    );
    world.reset_phases();

    let d1 = world.draft(
        BATCH,
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Saving while resizing loses the layout.\n",
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let mut ticket = Ticket::new(
        "layout-save-reads-size-twice",
        "a layout save reads one window size",
        &[&d1],
    );
    ticket
        .extra
        .push(("witnesses".to_owned(), json!([witness.clone()])));
    world.ticket(BATCH, &ticket);
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
    let filed_answer = [answer(
        &d1,
        "root-cause",
        "filed",
        &[&ticket.slug],
        "Verified.",
    )];
    world.answer(BATCH, &filed_answer);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");

    let (id, _) = filed(&world, "followups", &ticket.slug);
    let card = format!("followups:{id}");
    let (held, body) = item_parts(&world, "followups", &id);
    // The record keeps every value, the projections rendered into `projected`.
    assert_eq!(held["host"], HOST);
    assert_eq!(held["basis"], json!({REPOSITORY: COMMIT}));
    assert_eq!(held["witnesses"], json!([witness]));
    assert_eq!(
        held["draft_sources"],
        json!({&d1: "author node; node worker-1"}),
        "onefollowup wrote where the draft came from: {held}"
    );
    assert_eq!(
        held["projected"],
        json!({
            "root_cause": ticket.cause,
            "who_is_affected": "Users of widgets who save layouts.",
        }),
        "a section's text and one part of a section: {held}"
    );
    assert_eq!(
        stored_record(&world, &card)["projected"],
        held["projected"],
        "the store reads each value at [projected, <name>]"
    );
    // The host, basis and witnesses shown in the body; two parts collapsed.
    assert!(
        body.contains(&format!(
            "## Provenance\n\n- Host: {HOST}\n- Basis: {REPOSITORY} at {COMMIT}\n- Witness: {witness}\n"
        )),
        "{body}"
    );
    assert!(
        body.contains(
            "<details>\n<summary>Cost of the workaround</summary>\n\n- Applying it: a moment per save, by the user\n- Side effects on the outcome: none\n- Discovery: the failure names it\n\n</details>"
        ),
        "{body}"
    );
    assert!(
        body.contains(
            "Read the window size once per save.\n\n<details>\n<summary>Contributing locations</summary>\n\n- widgets `src/layout.rs`: reads the size twice\n\n</details>"
        ),
        "{body}"
    );
    assert!(
        !body.contains("**Cost of the workaround.**")
            && !body.contains("**Contributing locations.**"),
        "the default leaves both open; this host collapses them: {body}"
    );
    // The priority is estimated from the record's `impact`, which the body never states, and
    // this host's body shows no estimate line.
    assert_eq!(
        held["impact"]["Severity with the workaround"], "medium",
        "{held}"
    );
    assert_eq!(held["priority_estimate"], "medium", "{held}");
    assert_eq!(item_priority(&world, &id), "medium");
    assert!(
        !body.contains("Priority estimate") && !body.contains("- Severity"),
        "{body}"
    );
    for section in ["Examples", "Owning batches", "Evidence"] {
        assert!(
            body.contains(&format!("## {section}\n\n")),
            "{section}: {body}"
        );
    }

    // A later batch's evidence on that ticket, with its sources, what it bears on and a
    // question, each where the example's evidence template places it.
    let d2 = world.draft(
        LATER,
        // Named as the ticket is, so prepare finds its item and reads its comments.
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Seen again on a slower disk.\n",
    );
    world.ok("prepare", &["prepare", "--batch", LATER], "");
    let searched = world.ok(
        "agent",
        &[
            "ticket",
            "search",
            "--batch",
            LATER,
            "--ticket",
            &ticket.slug,
            "window size",
        ],
        "",
    );
    assert_eq!(searched["hits"][0]["item"], card.as_str(), "{searched}");
    let parts = world.root.join("parts");
    std::fs::create_dir_all(&parts).expect("a directory");
    std::fs::write(
        parts.join("sources.json"),
        json!({"host": HOST, "basis": {REPOSITORY: COMMIT}, "drafts": [d2], "witnesses": [witness]})
            .to_string(),
    )
    .expect("the sources");
    std::fs::write(
        parts.join("bears-on.md"),
        "The fix must also cover slow disks.\n",
    )
    .expect("written");
    std::fs::write(
        parts.join("question.md"),
        "Is the slow disk a second root cause?\n",
    )
    .expect("written");
    let sources = parts.join("sources.json").display().to_string();
    let bears_on = parts.join("bears-on.md").display().to_string();
    let question = parts.join("question.md").display().to_string();
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
            "--question-file",
            &question,
        ],
        "A save on a slow disk lost the layout too.\n",
    );
    world.answer(
        LATER,
        &[answer(
            &d2,
            "root-cause",
            "filed",
            &[&ticket.slug],
            "Evidence.",
        )],
    );
    // Its re-estimate counts two occurrences: still `medium`, from the record's `impact`.
    let staged = world.ok(
        "agent",
        &["ticket", "re-estimate", "--batch", LATER, &card],
        "",
    );
    assert_eq!(staged["write"]["priority_estimate"], "medium", "{staged}");
    let checked = world.ok("check", &["check", "--batch", LATER], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATER], "");
    world.ok("settle", &["settle", "--batch", LATER], "");
    let commented = crate::journey::comments(&world.read_item("followups", &id));
    let marker = format!(
        "<!-- orchestrator:follow-up-comment run=\"{LATER}\" root_cause=\"{}\" -->",
        ticket.slug
    );
    let expected = format!(
        "**Evidence from batch {LATER}.** A save on a slow disk lost the layout too.\n\n**Bears on the ticket.** The fix must also cover slow disks.\n\n**Question.** Is the slow disk a second root cause?\n\nSources:\n- Host: {HOST}\n- Basis: {REPOSITORY} at {COMMIT}\n- Draft: {d2}; author node; node worker-1\n- Witness: {witness}\n\n{marker}"
    );
    assert!(commented.contains(&expected), "{commented}");

    // A third batch's evidence and re-estimate count three occurrences, which raise the estimate
    // one level: the destination's priority moves with it, and the body still shows no line.
    let d3 = world.draft(
        LATEST,
        "a layout save reads one window size",
        &["src/layout.rs"],
        "Seen once more.\n",
    );
    world.ok("prepare", &["prepare", "--batch", LATEST], "");
    world.ok(
        "agent",
        &["ticket", "comment", "--batch", LATEST, "--item", &card],
        "A third save lost the layout.\n",
    );
    let staged = world.ok(
        "agent",
        &["ticket", "re-estimate", "--batch", LATEST, &card],
        "",
    );
    assert_eq!(staged["write"]["priority_estimate"], "high", "{staged}");
    world.answer(
        LATEST,
        &[answer(
            &d3,
            "root-cause",
            "filed",
            &[&ticket.slug],
            "Evidence.",
        )],
    );
    let checked = world.ok("check", &["check", "--batch", LATEST], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", LATEST], "");
    world.ok("settle", &["settle", "--batch", LATEST], "");
    let (held, body) = item_parts(&world, "followups", &id);
    assert_eq!(held["priority_estimate"], "high", "{held}");
    assert_eq!(held["board_estimate"], "high", "{held}");
    assert_eq!(item_priority(&world, &id), "high");
    assert!(!body.contains("Priority estimate"), "{body}");

    // The first batch's next pass counts that comment as an occurrence its marker's batch owns,
    // and a changed root cause re-renders the body and the projections on a bound re-copy.
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let status = world.ok(
        "agent",
        &["ticket", "board-status", "--batch", BATCH, &ticket.slug],
        "",
    );
    assert!(
        status["estimate_line"]
            .as_str()
            .is_some_and(|line| line.contains("; 3 occurrences;")),
        "{status}"
    );
    ticket.cause = "Widgets save a layout from two different window sizes.".to_owned();
    world.ticket(BATCH, &ticket);
    world.ok(
        "agent",
        &["ticket", "copy", "--batch", BATCH, &ticket.slug],
        "",
    );
    world.answer(BATCH, &filed_answer);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", BATCH], "");
    world.ok("settle", &["settle", "--batch", BATCH], "");
    let (held, body) = item_parts(&world, "followups", &id);
    assert_eq!(held["projected"]["root_cause"], ticket.cause, "{held}");
    assert_eq!(
        stored_record(&world, &card)["projected"],
        json!({"root_cause": ticket.cause, "who_is_affected": "Users of widgets who save layouts."}),
        "the store reads the updated values at [projected, <name>]"
    );
    assert_eq!(
        held["projected"]["who_is_affected"],
        "Users of widgets who save layouts."
    );
    assert!(
        body.starts_with(&format!(
            "## Root cause\n\n{}\n\n## Provenance",
            ticket.cause
        )),
        "{body}"
    );
    assert!(!body.contains("Priority estimate"), "{body}");
    assert_eq!(item_priority(&world, &id), "high");
}

/// The priority `followups`' item `id` holds.
fn item_priority(world: &World, id: &str) -> String {
    let text = world.read_item("followups", id);
    front(&text)["priority"]
        .as_str()
        .expect("a priority")
        .to_owned()
}
