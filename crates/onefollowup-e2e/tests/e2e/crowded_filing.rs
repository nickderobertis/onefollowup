//! Filing against a crowded destination, modelled on the `followups` board's 2,038 items:
//! 2,000 tickets across 6 repositories, about 470 open, where the board-frequent words the
//! spike's misses recorded (`shape` on 290 tickets, `private` on 122, `plans` on 91,
//! `initial` on 66) sit on hundreds of unrelated tickets, and each duplicate draft's own
//! distinctive terms sit on unrelated tickets too — one of them on more than the hit cap.
//! onefollowup holds no corpus to rank words by rarity, so this is what shows the shipped
//! stoplist, the per-query hit cap and the ranking keeping recall: every duplicate's earlier
//! member is rendered and related, and the batch's destination work stays a handful of
//! batched store calls, recorded as `follow-up-batch-store-operations`' second workload.

use std::collections::BTreeSet;

use onefollowup::formats::batch::Input;
use onefollowup::verify::candidates::{self, BOUND, HIT_CAP, QueryKind, RETAINED};
use serde_json::{Value, json};

use crate::budget::BUDGET;
use crate::duplicate_boards::{self, Crowd, clusters, origin};
use crate::filing_duplicates::{Fate, assert_fates, file, target};
use crate::telemetry;
use crate::world::World;

const BATCH: &str = "nightly-2026-11-22";

/// The board-frequent words, and how many tickets of the `followups` board each was on.
const BOARD_WORDS: [(&str, usize); 4] = [
    ("shape", 290),
    ("private", 122),
    ("plans", 91),
    ("initial", 66),
];

/// The tickets the destination holds, and how many are open.
const TICKETS: usize = 2000;
const OPEN: usize = 470;

/// The terms `prepare` searches for a draft, as its plan names them.
fn terms(id: &str, repository: &str, title: &str, body: &str, paths: &[String]) -> Vec<String> {
    candidates::plan(&Input {
        id: id.to_owned(),
        source: "draft".to_owned(),
        title: title.to_owned(),
        repository: Some(origin(repository)),
        paths: paths.to_vec(),
        body: body.to_owned(),
        author: None,
        transcript: None,
    })
    .queries
    .into_iter()
    .filter(|query| query.kind == QueryKind::Term)
    .map(|query| query.text)
    .collect()
}

#[test]
fn a_crowded_destination_still_renders_and_relates_every_duplicate_within_a_handful_of_store_calls()
{
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    let clusters: Vec<_> = clusters().into_iter().take(10).collect();
    let mut earlier_of = Vec::new();
    for cluster in &clusters {
        let placed: Vec<(String, String, String)> = cluster
            .earlier
            .iter()
            .map(|earlier| {
                let (item, root_cause) = duplicate_boards::place(&world, "followups", earlier);
                (item, root_cause, earlier.status.clone())
            })
            .collect();
        earlier_of.push(placed);
    }
    let placed = earlier_of.iter().flatten().count();
    let placed_open = earlier_of
        .iter()
        .flatten()
        .filter(|(_, _, status)| status == "open")
        .count();
    let fresh = [
        (
            "onevcs",
            "a branch name ending in a dot truncates the publish summary",
            "Publishing a branch whose name ends in a dot prints a summary that cuts the name short, so a reviewer cannot find the change from it.",
            vec!["crates/onevcs/src/summary.rs".to_owned()],
        ),
        (
            "onebudgetspec",
            "budget ids with capitals pass the linter and break the reporter",
            "A budget id written with capital letters passes the schema linter, then the reporter rejects it mid-run and the whole budgets check goes red without naming the id.",
            vec!["crates/onebudgetspec/src/reporter.rs".to_owned()],
        ),
    ];

    // Every duplicate's terms on unrelated tickets: the one its earlier members lack on more
    // than the hit cap, the rest on a few each; the new drafts' terms on a few each.
    let mut crowded: Vec<(String, usize)> = BOARD_WORDS
        .iter()
        .map(|(word, many)| ((*word).to_owned(), *many))
        .collect();
    let mut capped_term = Vec::new();
    for (cluster, placed) in clusters.iter().zip(&earlier_of) {
        let terms = terms(
            "draft",
            &cluster.repository,
            &cluster.draft.title,
            &cluster.draft.body,
            &cluster.draft.paths,
        );
        let lacking = terms
            .iter()
            .find(|term| {
                cluster.earlier.iter().all(|earlier| {
                    !format!("{} {}", earlier.title, earlier.body)
                        .to_lowercase()
                        .contains(term.as_str())
                })
            })
            .unwrap_or(&terms[0])
            .clone();
        assert!(!placed.is_empty());
        for (at, term) in terms.iter().enumerate() {
            let many = if *term == lacking {
                HIT_CAP + 15
            } else {
                2 + at
            };
            match crowded.iter_mut().find(|(word, _)| word == term) {
                Some((_, held)) => *held = (*held).max(many),
                None => crowded.push((term.clone(), many)),
            }
        }
        capped_term.push(lacking);
    }
    for (repository, title, body, paths) in &fresh {
        for (at, term) in terms("draft", repository, title, body, paths)
            .iter()
            .enumerate()
        {
            if !crowded.iter().any(|(word, _)| word == term) {
                crowded.push((term.clone(), 3 + at));
            }
        }
    }
    let words: Vec<(&str, usize)> = crowded
        .iter()
        .map(|(word, many)| (word.as_str(), *many))
        .collect();
    duplicate_boards::crowd(
        &world,
        "followups",
        &Crowd {
            count: TICKETS - placed,
            open: OPEN - placed_open,
            words: &words,
        },
    );
    assert_eq!(world.items("followups").len(), TICKETS);

    let mut drafts = Vec::new();
    for cluster in &clusters {
        let paths: Vec<&str> = cluster.draft.paths.iter().map(String::as_str).collect();
        drafts.push(world.draft_in(
            BATCH,
            &origin(&cluster.repository),
            &cluster.draft.title,
            &paths,
            &format!("{}\n", cluster.draft.body),
        ));
    }
    let mut fresh_drafts = Vec::new();
    for (repository, title, body, paths) in &fresh {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        fresh_drafts.push(world.draft_in(
            BATCH,
            &origin(repository),
            title,
            &paths,
            &format!("{body}\n"),
        ));
    }

    world.reset_phases();
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let searches = prepared["duplicate_searches"]
        .as_array()
        .expect("duplicate searches")
        .clone();
    let search_of = |draft: &str| -> Value {
        searches
            .iter()
            .find(|search| search["input"] == draft)
            .cloned()
            .unwrap_or_else(|| panic!("{draft} was searched"))
    };

    // No query of the batch searches a stoplisted word, and the board-frequent words the
    // drafts carry are among those it removed.
    let mut removed: Vec<String> = Vec::new();
    for search in &searches {
        for word in search["stoplisted"].as_array().into_iter().flatten() {
            let word = word.as_str().unwrap_or_default().to_owned();
            assert!(
                !search["queries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|query| query["kind"] != "title" && query["text"] == word.as_str()),
                "the stoplisted {word} is searched: {search}"
            );
            removed.push(word);
        }
    }
    for (word, _) in BOARD_WORDS {
        assert!(
            searches.iter().all(|search| {
                !search["queries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|query| query["kind"] != "title" && query["text"] == word)
            }),
            "the board-frequent {word} is searched by no query"
        );
    }
    assert!(
        removed.iter().any(|word| word == "shape") && removed.iter().any(|word| word == "plans"),
        "the drafts' board-frequent words were removed: {removed:?}"
    );

    let mut fates = Vec::new();
    let mut recorded = Vec::new();
    let mut bounded = 0;
    let mut retained_total = 0;
    for (((cluster, draft), placed), lacking) in clusters
        .iter()
        .zip(&drafts)
        .zip(&earlier_of)
        .zip(&capped_term)
    {
        let search = search_of(draft);
        let rendered = search["candidates"].as_array().expect("candidates").clone();
        let capped: Vec<&Value> = search["queries"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|query| query["capped"] == true)
            .collect();
        assert!(
            capped.iter().any(|query| query["text"] == lacking.as_str()),
            "{}: its term {lacking} reached the cap: {search}",
            cluster.later
        );
        for (item, _, _) in placed {
            assert!(
                rendered.iter().any(|one| one["item"] == item.as_str()),
                "{} ({}): its earlier member {item} is rendered: {search}",
                cluster.later,
                cluster.cluster
            );
        }
        // Ranked by the uncapped queries shared, then the capped: a candidate only the capped
        // query found ranks below every one sharing more of the draft's terms and file.
        let shared = |one: &Value| one["shared"].as_array().map_or(0, Vec::len);
        let capped_by = |one: &Value| one["capped"].as_array().map_or(0, Vec::len);
        for pair in rendered.windows(2) {
            assert!(
                (shared(&pair[0]), capped_by(&pair[0])) >= (shared(&pair[1]), capped_by(&pair[1])),
                "{}: ranked best first: {search}",
                cluster.later
            );
        }
        let earliest = placed
            .iter()
            .filter_map(|(item, _, _)| rendered.iter().position(|one| one["item"] == item.as_str()))
            .min()
            .expect("an earlier member is rendered");
        for below in rendered.iter().filter(|one| shared(one) == 0) {
            let at = rendered
                .iter()
                .position(|one| one == below)
                .expect("rendered");
            assert!(
                at > earliest,
                "{}: {below} ranks below the earlier member",
                cluster.later
            );
        }
        // A query matching past the cap is answered only `RETAINED` of its hits — the store
        // stops there — and keeps them all, as every other keeps all of its own: the draft's
        // candidates are drawn from those kept alone, and what only a capped query found ranks
        // below the bound's better-sharing candidates.
        let count = |query: &Value, key: &str| {
            usize::try_from(query[key].as_u64().unwrap_or_default()).unwrap_or(usize::MAX)
        };
        let mut kept = 0;
        for query in search["queries"].as_array().into_iter().flatten() {
            if query["capped"] == true {
                assert!(
                    count(query, "hits") == RETAINED && count(query, "retained") == RETAINED,
                    "{}: {query} is answered and keeps {RETAINED} hits, the store stopping there: {search}",
                    cluster.later
                );
            } else {
                assert_eq!(
                    count(query, "retained"),
                    count(query, "hits"),
                    "{}: {query} keeps every hit under the cap",
                    cluster.later
                );
            }
            kept += count(query, "retained");
        }
        let only_capped = rendered
            .iter()
            .filter(|one| shared(one) == 0 && capped_by(one) > 0)
            .count();
        let answered: usize = capped.iter().map(|query| count(query, "hits")).sum();
        assert!(
            only_capped < answered,
            "{}: the capped queries' candidates rank last: {search}",
            cluster.later
        );
        retained_total += kept;
        let matching_now =
            usize::try_from(search["matching"].as_u64().unwrap_or_default()).unwrap_or(usize::MAX);
        assert!(
            matching_now <= kept,
            "{}: its {matching_now} candidates come from the {kept} hits kept: {search}",
            cluster.later
        );
        let matching = search["matching"].as_u64().unwrap_or_default();
        if usize::try_from(matching).unwrap_or_default() > BOUND {
            assert_eq!(rendered.len(), BOUND, "{search}");
            bounded += 1;
        }
        let fate = target(&rendered, placed);
        let Fate::Related { item, .. } = &fate else {
            unreachable!("a duplicate is related");
        };
        recorded.push(json!({
            "cluster": cluster.cluster,
            "later": cluster.later,
            "draft": draft,
            "related_to": item,
            "stoplisted": search["stoplisted"],
            "capped": capped,
            "matching": matching,
            "candidates": rendered,
        }));
        fates.push((draft.clone(), fate));
    }
    assert!(
        bounded >= 1,
        "a draft's matching candidates exceed the bound of {BOUND}"
    );
    for (at, draft) in fresh_drafts.iter().enumerate() {
        let search = search_of(draft);
        recorded.push(json!({
            "draft": draft,
            "stoplisted": search["stoplisted"],
            "matching": search["matching"],
            "candidates": search["candidates"],
        }));
        fates.push((
            draft.clone(),
            Fate::Filed {
                slug: format!("crowded-new-root-cause-{at}"),
            },
        ));
    }
    // The overflow stays out of the store's answer and the batch's reads: the find stops each
    // query at the hits it keeps, and prepare's comments read reads, of what it answered, the
    // candidates every draft renders — each one read with its comments — and nothing more.
    let prepared_ops: Vec<(String, u64)> = world
        .destination_operations()
        .into_iter()
        .filter(|(phase, _, _, _)| phase == "prepare")
        .map(|(_, op, _, items)| (op, items))
        .collect();
    let items_of = |op: &str| {
        prepared_ops
            .iter()
            .find(|(one, _)| one == op)
            .map(|(_, items)| *items)
            .unwrap_or_else(|| panic!("prepare made a {op}: {prepared_ops:?}"))
    };
    let (answered, read) = (items_of("find"), items_of("read"));
    let fresh_kept: u64 = fresh_drafts
        .iter()
        .flat_map(|draft| {
            search_of(draft)["queries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|query| query["retained"].as_u64().unwrap_or_default())
        .sum();
    let kept_bound = u64::try_from(retained_total).unwrap_or(u64::MAX) + fresh_kept;
    let state: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join(format!("state/{BATCH}.json")))
            .expect("the batch state"),
    )
    .expect("JSON");
    let reads = state["reads"].as_array().cloned().unwrap_or_default();
    let commented: BTreeSet<&str> = reads
        .iter()
        .filter(|one| !one["comments"].is_null())
        .filter_map(|one| one["item"]["id"].as_str())
        .collect();
    let rendered: BTreeSet<&str> = searches
        .iter()
        .flat_map(|search| search["candidates"].as_array().into_iter().flatten())
        .filter_map(|candidate| candidate["item"].as_str())
        .collect();
    assert_eq!(
        commented, rendered,
        "prepare read with comments exactly the candidates the drafts render"
    );
    assert!(
        read == rendered.len() as u64 && read < answered && answered <= kept_bound,
        "prepare read {read} items of the {answered} its find was answered with, the find stopping each query at the hits it keeps ({kept_bound}): {prepared_ops:?}"
    );
    assert_eq!(
        reads.len() as u64,
        answered,
        "the batch keeps every item the find answered"
    );
    let find_searches: Vec<u64> = world
        .phases
        .iter()
        .flat_map(|(_, operations)| operations)
        .filter(|operation| operation["op"] == "find" && operation["destination"] == true)
        .filter_map(|operation| operation["searches"].as_u64())
        .collect();
    telemetry::record_journey(
        "filing-duplicates-crowded",
        &json!({"batch": BATCH, "find_searches": find_searches, "drafts": recorded}),
    );

    file(&mut world, BATCH, &fates);
    assert_fates(&world, "followups", BATCH, &fates);

    let operations = world.destination_operations();
    let detail = operations
        .iter()
        .map(|(phase, op, source, items)| format!("{phase}: {op} on {source} ({items} items)"))
        .collect::<Vec<_>>()
        .join("; ");
    telemetry::record_part(
        BUDGET,
        "crowded-destination",
        operations.len(),
        &format!(
            "{} destination store operations from prepare to settle, the drafts source excluded, filing 12 drafts against 2,000 tickets, prepare's find carrying {} searches: {detail}",
            operations.len(),
            find_searches
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(" and ")
        ),
    );
}
