//! Filing against a crowded destination, modelled on the `followups` board's 2,038 items:
//! 2,000 tickets across 6 repositories, about 470 open, where the board-frequent words the
//! spike's misses recorded (`shape` on 290 tickets, `private` on 122, `plans` on 91,
//! `initial` on 66) sit on hundreds of unrelated tickets, and each duplicate draft's own
//! distinctive terms sit on unrelated tickets too — one of them on more than the hit cap.
//! onefollowup holds no corpus to rank words by rarity, so this is what shows the shipped
//! stoplist, the per-query hit cap and the ranking keeping recall: every duplicate's earlier
//! member is rendered and related, and the batch's destination work stays a handful of
//! batched store calls, recorded as `follow-up-batch-store-operations`' second workload.

use onefollowup::formats::batch::Input;
use onefollowup::verify::candidates::{self, BOUND, HIT_CAP, QueryKind};
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
        // The capped query found more than reached the task: the rest of its candidates rank
        // below the bound, under the ones sharing more of the draft's terms and file.
        for query in &capped {
            let only_it = rendered
                .iter()
                .filter(|one| shared(one) == 0 && capped_by(one) > 0)
                .count();
            let hits = usize::try_from(query["hits"].as_u64().unwrap_or_default()).unwrap_or(0);
            assert!(
                hits > HIT_CAP && only_it < hits,
                "{}: {query} ranks its candidates last: {search}",
                cluster.later
            );
        }
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
