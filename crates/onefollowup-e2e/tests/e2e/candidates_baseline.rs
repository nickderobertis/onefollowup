//! No duplicate lost: every open ticket onefollowup 0.14.0 rendered as a duplicate candidate for
//! a draft — the 50-draft batch's drafts at the real board's shape (`real_workload`) and the
//! real clusters of `fixtures/filing-duplicates.json` (`filing_duplicates::clusters_world`) —
//! is still rendered for it, and `check` refuses an answer that leaves any of them neither
//! related nor dismissed.
//!
//! The record of what 0.14.0 rendered, `fixtures/real-board/candidates-0.14.0.json`, was taken
//! before this release's find by running the `v0.14.0` binary over the same worlds:
//!
//!     ONEFOLLOWUP_BASELINE_BINARY=<v0.14.0's onefollowup> cargo test -p onefollowup-e2e \
//!       --test e2e candidates_baseline::record -- --ignored --exact
//!
//! which rewrites the record; it reaches nothing but this test's loopback and local files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::filing_duplicates::{self, clusters_world};
use crate::loopback_github::{LoopbackGitHub, Pricing, Quota};
use crate::prepare_cost::loopback_world;
use crate::real_workload;
use crate::world::{Ticket, World, answer};

/// Where the record of what 0.14.0 rendered is kept.
fn record_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/e2e/fixtures/real-board/candidates-0.14.0.json")
}

/// `prepare` of `batch` by `binary` in `world`: its result.
fn prepare(world: &World, binary: &Path, batch: &str) -> Value {
    let output = crate::isolated::command(binary, &world.root, world.environment())
        .args(["prepare", "--batch", batch])
        .output()
        .expect("the binary runs");
    assert!(
        output.status.success(),
        "{}: prepare: {}",
        binary.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("JSON")
}

/// Each draft's rendered candidates that are open, by draft, in the order rendered.
fn open_candidates(prepared: &Value) -> BTreeMap<String, Vec<String>> {
    prepared["duplicate_searches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|search| {
            let open = search["candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|candidate| {
                    !["done", "cancelled"]
                        .contains(&candidate["status"]["category"].as_str().unwrap_or("done"))
                })
                .filter_map(|candidate| candidate["item"].as_str().map(str::to_owned))
                .collect();
            (
                search["input"].as_str().unwrap_or_default().to_owned(),
                open,
            )
        })
        .collect()
}

/// The 50-draft batch's world on a loopback with ample quota and nothing else spending it.
fn fifty_draft_world() -> (World, tempfile::TempDir, LoopbackGitHub) {
    let github = LoopbackGitHub::serving(
        Quota {
            remaining: 5000,
            reset: chrono::Utc::now().timestamp() + 3600,
            charge: 0,
        },
        vec![
            real_workload::public_board(),
            real_workload::private_board(),
        ],
        Pricing::Published,
    );
    let (mut world, state) = loopback_world(&github);
    real_workload::record_drafts(&mut world);
    (world, state, github)
}

#[test]
#[ignore = "records what onefollowup 0.14.0 renders; run it with ONEFOLLOWUP_BASELINE_BINARY naming that binary"]
fn record() {
    let binary = PathBuf::from(
        std::env::var("ONEFOLLOWUP_BASELINE_BINARY")
            .expect("ONEFOLLOWUP_BASELINE_BINARY names onefollowup 0.14.0"),
    );
    let version = std::process::Command::new(&binary)
        .arg("--version")
        .output()
        .expect("the baseline binary runs");
    let version = String::from_utf8_lossy(&version.stdout).trim().to_owned();
    assert_eq!(version, "onefollowup 0.14.0", "the baseline is 0.14.0's");
    let (fifty, _state, _github) = fifty_draft_world();
    let fifty = open_candidates(&prepare(&fifty, &binary, real_workload::BATCH));
    let (clusters, _, _, _) = clusters_world();
    let clusters = open_candidates(&prepare(&clusters, &binary, filing_duplicates::BATCH));
    let record = json!({
        "about": "The open tickets onefollowup 0.14.0 (the v0.14.0 tag, on onetaskgraph 0.17.0) rendered as duplicate candidates for each draft, by draft, taken before onetaskgraph 0.17.3's find by candidates_baseline::record: the 50-draft batch at the real board's shape (real_workload) and the real clusters of fixtures/filing-duplicates.json (filing_duplicates::clusters_world).",
        "version": version,
        "fifty_drafts": fifty,
        "clusters": clusters,
    });
    std::fs::write(
        record_path(),
        serde_json::to_string_pretty(&record).expect("JSON") + "\n",
    )
    .expect("the record");
}

/// Every draft of `batch` in `world` filed as one new ticket, judging no candidate: `check`'s
/// result.
fn filed_judging_nothing(world: &mut World, batch: &str, drafts: &[String]) -> Value {
    let slug = "one-new-root-cause-for-every-draft";
    let named: Vec<&str> = drafts.iter().map(String::as_str).collect();
    world.ticket(
        batch,
        &Ticket::new(slug, "one new root cause for every draft", &named),
    );
    world.ok("agent", &["ticket", "copy", "--batch", batch, slug], "");
    let answers: Vec<Value> = drafts
        .iter()
        .map(|draft| answer(draft, "root-cause", "filed", &[slug], "Verified."))
        .collect();
    world.answer_as_given(batch, &answers);
    world.ok("check", &["check", "--batch", batch], "")
}

/// That every open candidate `recorded` holds is still rendered for its draft in `prepared`,
/// and that `checked` refused each of them left neither related nor dismissed.
fn holds(set: &str, recorded: &Value, prepared: &Value, checked: &Value) -> usize {
    let rendered = open_candidates(prepared);
    let problems = checked["problems"].as_array().cloned().unwrap_or_default();
    assert_eq!(checked["sound"], false, "{set}: {checked}");
    let mut held = 0;
    for (draft, items) in recorded.as_object().expect("a record by draft") {
        for item in items.as_array().into_iter().flatten() {
            let item = item.as_str().expect("an item");
            assert!(
                rendered
                    .get(draft)
                    .is_some_and(|now| now.iter().any(|one| one == item)),
                "{set}: {item}, which 0.14.0 rendered for {draft}, is rendered for it now: {:?}",
                rendered.get(draft)
            );
            assert!(
                problems.iter().any(|problem| {
                    problem["subject"] == draft.as_str()
                        && problem["message"].as_str().is_some_and(|message| {
                            message.contains(&format!("`{item}`"))
                                && message.contains("neither related nor dismissed")
                        })
                }),
                "{set}: check refuses {item} left neither related nor dismissed for {draft}: {problems:?}"
            );
            held += 1;
        }
    }
    held
}

#[test]
fn every_open_candidate_0_14_0_rendered_is_still_rendered_and_check_holds_an_answer_to_each() {
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(record_path()).expect("the record of 0.14.0's candidates"),
    )
    .expect("JSON");
    assert_eq!(record["version"], "onefollowup 0.14.0");

    let (mut fifty, _state, _github) = fifty_draft_world();
    let prepared = fifty.ok("prepare", &["prepare", "--batch", real_workload::BATCH], "");
    let drafts: Vec<String> = prepared["duplicate_searches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|search| search["input"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(drafts.len(), 50);
    let checked = filed_judging_nothing(&mut fifty, real_workload::BATCH, &drafts);
    let in_fifty = holds(
        "the 50-draft batch",
        &record["fifty_drafts"],
        &prepared,
        &checked,
    );

    let (mut clusters, mut drafts, _, fresh) = clusters_world();
    let prepared = clusters.ok(
        "prepare",
        &["prepare", "--batch", filing_duplicates::BATCH],
        "",
    );
    drafts.push(fresh);
    let checked = filed_judging_nothing(&mut clusters, filing_duplicates::BATCH, &drafts);
    let in_clusters = holds(
        "the real clusters",
        &record["clusters"],
        &prepared,
        &checked,
    );
    assert!(
        in_fifty > 0 && in_clusters > 0,
        "0.14.0 rendered open candidates in both: {in_fifty} and {in_clusters}"
    );
}
