//! Each draft's duplicates searched where its ticket would land, each verb run in process: a public destination
//! `followups` whose route by repository sends a ticket about a private repository to
//! `private-followups`. Each holds an earlier ticket repeating one draft — and a decoy
//! worded like the other's — so a draft's search reaching the wrong source would render that
//! source's decoy. `prepare` searches the public draft on the public folder alone and the
//! private one on the private folder alone, on a first pass and on the continuing pass after.

use serde_json::{Value, json};

use crate::world::{COMMIT, HOST, World};

const BATCH: &str = "nightly-2026-11-26";
/// The private repository the route sends to `private-followups`.
const PRIVATE: &str = "github.com/example-private/ledger";
const PUBLIC: &str = "github.com/example/widgets";

/// Write `destination`'s item `id`: an open follow-up ticket `batch` filed about
/// `github.com/example/<repository>`, as `crates/onefollowup-e2e`'s `duplicate_boards::ticket`
/// writes one.
fn ticket(
    world: &World,
    destination: &str,
    id: &str,
    (repository, title, root_cause): (&str, &str, &str),
    batch: &str,
    _status: &str,
    body: &str,
) {
    let origin = format!("github.com/example/{repository}");
    let record = json!({
        "schema": 14,
        "root_cause": root_cause,
        "repository": origin,
        "created_by_run": batch,
        "owning_runs": [batch],
        "drafts": [format!("drafts:{batch}/drafts/x")],
        "basis": {origin.clone(): COMMIT},
        "verified_at": "2026-10-01T11:30:00Z",
        "host": HOST,
        "priority_estimate": "medium",
        "board_estimate": "medium",
        "frequency": "intermittent",
        "impact": {
            "Severity": "high",
            "Workaround": "re-run the failed step",
            "Severity with the workaround": "medium",
        },
    });
    world.item(
        destination,
        id,
        &format!(
            "title: {}\nstatus: backlog\npriority: medium\nmetadata:\n  orchestrator.follow-up: {record}",
            Value::from(title)
        ),
        &format!("{body}\n"),
    );
}

/// A world of `followups`, routing every `example-private` repository's ticket to
/// `private-followups`, each holding one earlier duplicate and one decoy.
fn routed() -> World {
    let world = World::new(
        &["followups", "private-followups"],
        "host_label: build-host-1\n",
    );
    std::fs::write(
        world.root.join("onetaskgraph.yaml"),
        "sources:
  drafts:
    plugin: local-md
    config:
      root: drafts-root
  followups:
    plugin: local-md
    config:
      root: followups-root
    routes:
      - repositories: [\"github.com/example-private/*\"]
        to: private-followups
  private-followups:
    plugin: local-md
    config:
      root: private-followups-root
",
    )
    .expect("the store");
    // The public draft's earlier duplicate, and a public ticket worded like the private draft.
    ticket(
        &world,
        "followups",
        "export-unicode",
        (
            "widgets",
            "widgets: the export dialog drops unicode titles",
            "export-dialog-drops-unicode-titles",
        ),
        "nightly-2026-10-01",
        "open",
        "## Root cause\n\nThe export dialog drops unicode titles.",
    );
    ticket(
        &world,
        "followups",
        "payroll-decoy",
        (
            "widgets",
            "widgets: the payroll ledger rounds cents twice",
            "payroll-ledger-rounds-cents-twice",
        ),
        "nightly-2026-10-02",
        "open",
        "## Root cause\n\nThe payroll ledger rounds cents twice.",
    );
    // The private draft's earlier duplicate, and a private ticket worded like the public draft.
    ticket(
        &world,
        "private-followups",
        "ledger-cents",
        (
            "ledger",
            "ledger: the payroll export rounds cents twice",
            "payroll-export-rounds-cents-twice",
        ),
        "nightly-2026-10-03",
        "open",
        "## Root cause\n\nThe payroll export rounds cents twice.",
    );
    ticket(
        &world,
        "private-followups",
        "unicode-decoy",
        (
            "ledger",
            "ledger: the export dialog drops unicode titles",
            "ledger-export-dialog-drops-unicode-titles",
        ),
        "nightly-2026-10-04",
        "open",
        "## Root cause\n\nThe export dialog drops unicode titles.",
    );
    world
}

/// The items `input`'s searches found, from the batch state `prepare` wrote.
fn found_for(world: &World, input: &str) -> Vec<String> {
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join(format!("state/{BATCH}.json")))
            .expect("prepare wrote the batch state"),
    )
    .expect("JSON");
    state["reads"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|read| {
            read["found_by"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|key| {
                    key.as_str().is_some_and(|key| {
                        key == format!("input:{input}")
                            || key.starts_with(&format!("term:{input}:"))
                            || key.starts_with(&format!("file:{input}:"))
                    })
                })
        })
        .map(|read| read["item"]["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[test]
fn a_private_draft_is_searched_only_where_its_ticket_routes_and_a_public_one_only_on_the_public_source()
 {
    let mut world = routed();
    let public = world.draft_in(
        BATCH,
        PUBLIC,
        "the export dialog drops unicode titles",
        &["src/export.rs"],
        "Exporting a report titled in Japanese writes an empty title.\n",
    );
    let private = world.draft_in(
        BATCH,
        PRIVATE,
        "the payroll export rounds cents twice",
        &["src/payroll.rs"],
        "A payroll export rounds each line's cents, then the total's again.\n",
    );
    for pass in ["first", "continuing"] {
        world.reset_phases();
        let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
        let items = |input: &str| -> Vec<String> {
            world
                .candidates(BATCH, input)
                .iter()
                .map(|candidate| candidate["item"].as_str().unwrap_or_default().to_owned())
                .collect()
        };
        let (public_items, private_items) = (items(&public), items(&private));
        assert!(
            public_items.contains(&"followups:export-unicode".to_owned())
                && public_items.iter().all(|id| id.starts_with("followups:")),
            "{pass}: the public draft's duplicate, and nothing private: {public_items:?}"
        );
        assert!(
            private_items.contains(&"private-followups:ledger-cents".to_owned())
                && private_items
                    .iter()
                    .all(|id| id.starts_with("private-followups:")),
            "{pass}: the private draft's duplicate, where its ticket lands, and nothing public: {private_items:?}"
        );
        // No search of either draft reached the other's source: what each found is its own
        // source's alone, though the other's holds a ticket worded like it.
        let found_public = found_for(&world, &public);
        let found_private = found_for(&world, &private);
        assert!(
            !found_public.is_empty() && found_public.iter().all(|id| id.starts_with("followups:")),
            "{pass}: {found_public:?}"
        );
        assert!(
            !found_private.is_empty()
                && found_private
                    .iter()
                    .all(|id| id.starts_with("private-followups:")),
            "{pass}: {found_private:?}"
        );
        // One find a source, carrying exactly the searches of the draft routed there.
        let queries = |input: &str| {
            prepared["duplicate_searches"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|search| search["input"] == input)
                .and_then(|search| search["queries"].as_array())
                .map_or(0, Vec::len)
        };
        let finds: Vec<(String, u64)> = prepared["store_operations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|operation| operation["op"] == "find" && operation["destination"] == true)
            .map(|operation| {
                (
                    operation["source"].as_str().unwrap_or_default().to_owned(),
                    operation["searches"].as_u64().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            finds,
            [
                ("followups".to_owned(), queries(&public) as u64),
                ("private-followups".to_owned(), queries(&private) as u64),
            ],
            "{pass}: {}",
            prepared["store_operations"]
        );
    }
}
