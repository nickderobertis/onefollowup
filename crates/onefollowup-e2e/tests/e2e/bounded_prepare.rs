//! `prepare` over the loopback GitHub's generated boards (`prepare_cost`'s), at a size these
//! journeys can run in seconds: each duplicate search stops at the 21 hits prepare keeps, its
//! draft's candidates and capping those an unlimited walk would give; its phases print their
//! progress on stderr while its stdout and records stay byte for byte what they are; and a
//! find dearer than a window leaves above its floor is followed to the result, never held.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;

use onefollowup::verify::candidates::{self, HIT_CAP, RETAINED};

use crate::duplicate_boards::{REPOSITORIES, origin};
use crate::loopback_boards::{Board, matched};
use crate::loopback_github::{LoopbackGitHub, Pricing, Quota};
use crate::prepare_cost::{
    PRIVATE_REPOSITORIES, PUBLIC_WORDS, carry_governor_state, generated, loopback_world, phases,
    private_board, private_origin, public_board,
};
use crate::world::World;

const BATCH: &str = "nightly-2026-11-26";

/// `PUBLIC_WORDS` scaled from the 2,000-ticket board to one of `tickets`.
fn scaled(tickets: usize) -> Vec<(&'static str, usize)> {
    PUBLIC_WORDS
        .iter()
        .map(|(word, many)| (*word, many * tickets / 2000))
        .collect()
}

/// A fresh window: the whole allowance, resetting `reset_in` seconds from now.
fn fresh(reset_in: i64) -> Quota {
    Quota {
        remaining: 5000,
        reset: chrono::Utc::now().timestamp() + reset_in,
        charge: 0,
    }
}

/// Record a draft about `repository` pairing two of the public board's common terms.
fn common_draft(world: &mut World, repository: &str, rare: &str) -> String {
    world.draft_in(
        BATCH,
        repository,
        &format!("the publish session loses its {rare}"),
        &["src/session.rs"],
        &format!("When a publish session runs, the {rare} is lost.\n"),
    )
}

/// Every file under `root`, by its path below it, with its bytes.
fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut held = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let below = path
                    .strip_prefix(root)
                    .expect("below the root")
                    .to_path_buf();
                held.insert(below, std::fs::read(&path).expect("readable"));
            }
        }
    }
    held
}

/// Replace everything under `root` with `held`.
fn restore(root: &Path, held: &BTreeMap<PathBuf, Vec<u8>>) {
    let _ = std::fs::remove_dir_all(root);
    for (below, bytes) in held {
        let path = root.join(below);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        std::fs::write(path, bytes).expect("written");
    }
}

#[test]
fn a_search_over_more_than_21_matches_stops_at_21_and_its_candidates_and_capping_are_an_unlimited_walk_s()
 {
    let boards: Vec<Board> = vec![public_board(2000, 470, PUBLIC_WORDS)];
    let github = LoopbackGitHub::serving(fresh(1800), boards.clone(), Pricing::Published);
    let (mut world, state) = loopback_world(&github);
    carry_governor_state(state.path(), 17);
    let draft = common_draft(&mut world, &origin("onejudge"), "watermark");
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");
    let search = prepared["duplicate_searches"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|search| search["input"] == draft.as_str())
        .cloned()
        .expect("the draft's duplicate search");
    let reads: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join(format!("state/{BATCH}.json")))
            .expect("the batch state"),
    )
    .expect("JSON");
    // Every page of every search the store was sent asks at most the 21 hits kept.
    let mut pages: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    for received in github.received() {
        for slot in 0..onetaskgraph_github_projects::SEARCH_BATCH {
            if received.variables[format!("ask{slot}")] != true {
                continue;
            }
            let text = received.variables[format!("search{slot}")]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let after = received.variables[format!("after{slot}")]
                .as_str()
                .and_then(|cursor| cursor.strip_prefix("cursor:"))
                .map_or(0, |at| at.parse::<u64>().expect("a cursor"));
            let first = received.variables[format!("first{slot}")]
                .as_u64()
                .expect("a page size");
            pages.entry(text).or_default().push((after, first));
        }
    }
    // Each query's key, as prepare made it from the input the batch state holds.
    let input: onefollowup::formats::batch::Input =
        serde_json::from_value(reads["inputs"][0].clone()).expect("the draft's input");
    let keys: BTreeMap<String, String> = candidates::plan(&input)
        .queries
        .into_iter()
        .map(|query| (query.text, query.key))
        .collect();
    let mut capped_queries = 0;
    for query in search["queries"].as_array().into_iter().flatten() {
        let asked_text = query["text"].as_str().unwrap_or_default();
        let quoted = format!("\"{asked_text}\"");
        let Some((text, asked)) = pages.iter().find(|(text, _)| text.contains(&quoted)) else {
            panic!("no search for {quoted}: {pages:?}");
        };
        let every = matched(&boards, text);
        let read: u64 = asked
            .iter()
            .map(|(after, first)| (*first).min((every.len() as u64).saturating_sub(*after)))
            .sum();
        assert!(
            asked
                .iter()
                .all(|(after, first)| after + first <= RETAINED as u64),
            "{quoted}: no page reaches past the 21st hit: {asked:?}"
        );
        let key = &keys[asked_text];
        let mut kept: Vec<String> = reads["reads"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|read| {
                read["found_by"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|found| found == key.as_str())
            })
            .map(|read| read["item"]["id"].as_str().unwrap_or_default().to_owned())
            .collect();
        kept.sort();
        // What an unlimited walk kept: the first 21 it listed.
        let mut unlimited: Vec<String> = every
            .iter()
            .take(RETAINED)
            .map(|id| format!("followups:{id}"))
            .collect();
        unlimited.sort();
        assert_eq!(kept, unlimited, "{quoted}: the hits kept");
        if every.len() > RETAINED {
            capped_queries += 1;
            // The walk stopped at 21, reported as what the store answered, and capped as when
            // the store answered every match: every count over the cap is.
            assert_eq!(read, RETAINED as u64, "{quoted}: {asked:?}");
            assert_eq!(query["hits"], RETAINED, "{query}");
            assert_eq!(query["retained"], RETAINED, "{query}");
            assert_eq!(query["capped"], true, "{query}");
        } else {
            assert_eq!(read, every.len() as u64, "{quoted}: every match read");
            assert_eq!(query["hits"], every.len(), "{query}");
            assert_eq!(query["capped"], every.len() > HIT_CAP, "{query}");
        }
    }
    assert!(
        capped_queries >= 2,
        "the draft's common terms each match more than 21: {search}"
    );
    // Its candidates are made of those reads alone, so they are an unlimited walk's too.
    let candidates = world.candidates(BATCH, &draft);
    assert!(!candidates.is_empty(), "{search}");
    for candidate in &candidates {
        assert!(
            reads["reads"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|read| read["item"]["id"] == candidate["item"]),
            "{candidate}"
        );
    }
}

#[test]
fn prepare_prints_each_phase_and_its_progress_on_stderr_and_stdout_and_records_are_as_without_it() {
    let boards: Vec<Board> = vec![
        public_board(2000, 470, PUBLIC_WORDS),
        private_board(400, 95, &scaled(2000)),
    ];
    let github = LoopbackGitHub::serving(fresh(1800), boards, Pricing::Published);
    let (mut world, state) = loopback_world(&github);
    carry_governor_state(state.path(), 17);
    let mut drafts = Vec::new();
    for at in 0..14 {
        let repository = if at < 12 {
            origin(REPOSITORIES[at % REPOSITORIES.len()])
        } else {
            private_origin(PRIVATE_REPOSITORIES[at % PRIVATE_REPOSITORIES.len()])
        };
        let (repository, title, paths, body) = generated(at, &repository);
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        drafts.push(world.draft_in(BATCH, &repository, &title, &paths, &body));
    }
    // prepare names a find's drafts in id order.
    drafts[..12].sort();
    drafts[12..].sort();
    let (before_root, before_state) = (files(&world.root), files(state.path()));

    let run = |stderr: Stdio| {
        let mut command =
            crate::isolated::command(crate::isolated::binary(), &world.root, world.environment());
        command
            .args(["prepare", "--batch", BATCH])
            .stdout(Stdio::piped())
            .stderr(stderr);
        let output = command
            .spawn()
            .expect("the binary runs")
            .wait_with_output()
            .expect("it ends");
        assert!(output.status.success(), "{output:?}");
        output
    };
    let seen = run(Stdio::piped());
    let records = files(&world.work());
    let stderr = String::from_utf8_lossy(&seen.stderr).into_owned();

    // Each phase that ran, as it started and ended, with what it covered.
    let said = phases(&stderr);
    let named: Vec<&str> = said
        .started
        .iter()
        .map(|(phase, _)| phase.as_str())
        .collect();
    assert_eq!(
        named,
        [
            "snapshot read",
            "duplicate find on followups",
            "duplicate find on private-followups",
            "kept-hit detail read"
        ],
        "no relation reads in a batch with no relations: {stderr}"
    );
    for (phase, detail) in &said.started {
        assert!(said.finished.contains_key(phase), "{phase} ended: {stderr}");
        match phase.as_str() {
            "snapshot read" => assert_eq!(detail, &format!("batch {BATCH}")),
            "duplicate find on followups" => assert!(
                detail.starts_with(&format!("12 drafts ({})", drafts[..12].join(", "))),
                "{detail}"
            ),
            "duplicate find on private-followups" => {
                assert!(
                    detail.starts_with(&format!("2 drafts ({})", drafts[12..].join(", "))),
                    "{detail}"
                );
            }
            _ => assert!(detail.contains(" items ("), "{detail}"),
        }
    }
    // A line for every 20 requests a phase sends, naming the requests and points so far.
    let charged = github.charged();
    let mut ticked = 0;
    for (phase, (sent, spent)) in &said.finished {
        let ticks = said.progress.get(phase).cloned().unwrap_or_default();
        let expected: Vec<u64> = (1..=sent / 20).map(|at| at * 20).collect();
        assert_eq!(
            ticks.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
            expected,
            "{phase}: {stderr}"
        );
        assert!(
            ticks.windows(2).all(|pair| pair[0].1 <= pair[1].1)
                && ticks.iter().all(|(_, so_far)| so_far <= spent),
            "{phase}: points so far grow to the phase's: {ticks:?} of {spent}"
        );
        ticked += ticks.len();
    }
    assert!(ticked >= 1, "a phase sent 20 requests or more: {stderr}");
    let (requests, points) = said
        .finished
        .values()
        .fold((0, 0), |(requests, points), (one, two)| {
            (requests + one, points + two)
        });
    assert_eq!(requests, charged.len() as u64, "{stderr}");
    assert_eq!(
        points,
        charged.iter().map(|one| one.points).sum::<u64>(),
        "{stderr}"
    );

    // The same batch run again from the same state, its stderr discarded: the same stdout,
    // and the same records, byte for byte.
    restore(&world.root, &before_root);
    restore(state.path(), &before_state);
    let unseen = run(Stdio::null());
    assert_eq!(
        String::from_utf8_lossy(&unseen.stdout),
        String::from_utf8_lossy(&seen.stdout)
    );
    assert_eq!(files(&world.work()), records, "every record prepare writes");
}

#[test]
fn a_find_dearer_than_a_window_leaves_above_its_floor_is_followed_to_the_result_not_held() {
    // The board's first search round costs 4,500 of the 5,000 points — more than the 4,000 a
    // window leaves above its 20% floor — the window resets 5 s after the start, and each one
    // after it lasts 10 s, so a hold for headroom is a short one. The host state carries what
    // an earlier release learned of a whole 5,319-point find, which no window could admit.
    // prepare holds on the floor until the reset and then reads its kept hits and ends.
    let boards: Vec<Board> = vec![public_board(400, 100, &scaled(400))];
    let github =
        LoopbackGitHub::serving(fresh(5), boards, Pricing::FirstSearchAt(4500)).with_period(10);
    let (mut world, state) = loopback_world(&github);
    carry_governor_state(state.path(), 17);
    let draft = common_draft(&mut world, &origin("onejudge"), "watermark");
    let mut command =
        crate::isolated::command(crate::isolated::binary(), &world.root, world.environment());
    command
        .args(["prepare", "--batch", BATCH])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("the binary runs");
    // Well past every hold a 10 s window asks for: a call held for good fails here.
    let deadline = Instant::now() + Duration::from_secs(120);
    while child.try_wait().expect("it is waited on").is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("it ends");
            panic!(
                "prepare was still held after 120 s: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let output = child.wait_with_output().expect("it ends");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let prepared: Value = serde_json::from_slice(&output.stdout).expect("JSON");
    let charged = github.charged();
    let find: u64 = charged
        .iter()
        .filter(|one| !one.searches.is_empty())
        .map(|one| one.points)
        .sum();
    assert!(find > 4000, "the find cost {find} points");
    assert!(
        charged.iter().any(|one| one.document == "ISSUE_DETAILS"),
        "its kept hits were read after it: {stderr}"
    );
    let waits = prepared["governor"]["waits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        waits.iter().any(|wait| wait["reason"] == "floor"),
        "held on the floor until the reset: {prepared}"
    );
    let search = &prepared["duplicate_searches"][0];
    assert_eq!(search["input"], draft.as_str());
    assert!(
        search["candidates"]
            .as_array()
            .is_some_and(|one| !one.is_empty()),
        "{search}"
    );
}
