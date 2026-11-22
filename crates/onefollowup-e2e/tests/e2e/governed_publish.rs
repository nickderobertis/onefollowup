//! A publish killed while the quota governor holds it mid-batch, then run again: the second
//! publish sends only what no receipt confirms, files nothing twice, and settle confirms all.
//!
//! The killed publish is the library's `publish`, run in this process with GitHub's
//! rate-limit state doubled — the one seam a test has into the governor, since the binary
//! reads only a real destination's quota — and killed by its clock the moment the governor
//! first waits: nothing of the publish runs after that instant, as nothing would after a
//! `SIGKILL`. Every other step is the built binary over local-md.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use onefollowup::api::BatchArgs;
use onefollowup::config::{Inputs, resolve};
use onefollowup::governor::{self, Clock, Parts, Pending, Reading, Readings};
use serde_json::json;

use crate::journey::record;
use crate::world::{Ticket, World, answer};

const BATCH: &str = "nightly-2026-11-22";

/// The instant the killed publish runs at.
fn start() -> DateTime<Utc> {
    DateTime::from_timestamp(1_795_305_600, 0).expect("an instant")
}

/// A clock that kills the publish at its first wait.
struct KilledAtFirstWait;

impl Clock for KilledAtFirstWait {
    fn now(&self) -> DateTime<Utc> {
        start()
    }

    fn sleep(&self, _: Duration) -> Pending<'_, ()> {
        std::panic::panic_any(Killed)
    }
}

struct Killed;

/// GitHub's rate-limit state over the record writes local-md names `local`: 1000, floor 200,
/// 208 left — enough for the copy to land two of the batch's tickets, and the next record
/// write waits.
struct EightWritesLeft;

impl Readings for EightWritesLeft {
    fn read<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        let window = |resource: &str, remaining| Reading {
            resource: resource.to_owned(),
            limit: if resource == "local" { 1000 } else { 5000 },
            remaining,
            resets_at: start() + chrono::Duration::hours(1),
        };
        let readings = vec![
            window("local", 208),
            window("graphql", 5000),
            window("core", 5000),
        ];
        Box::pin(async move { Ok(readings) })
    }
}

/// The world's verbs' settings, resolved as the binary resolves them there.
fn settings(world: &World) -> onefollowup::Settings {
    resolve(&Inputs {
        config_file: None,
        set: Vec::new(),
        destination: None,
        drafts_source: None,
        store: None,
        environment: vec![
            (
                "HOME".to_owned(),
                world.root.join("home").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                world.root.join("home/config").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".to_owned(),
                world.root.join("home/cache").display().to_string(),
            ),
        ],
        working_directory: world.root.clone(),
        hostname: None,
    })
    .expect("the world's configuration resolves")
}

fn root_causes(world: &World) -> Vec<String> {
    let mut causes: Vec<String> = world
        .filed("followups")
        .iter()
        .map(|(_, text)| {
            record(text)["root_cause"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    causes.sort();
    causes
}

#[test]
fn a_publish_killed_while_the_gate_waits_mid_batch_resends_only_what_no_receipt_confirms() {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    // A create's lease and quiescence of one second, so the rerun may take over the create
    // the killed publish's copy left in flight without the test waiting out 120 s.
    let store = world.root.join("onetaskgraph.yaml");
    let declared = std::fs::read_to_string(&store).expect("the store");
    std::fs::write(
        &store,
        declared
            .replace(
                "root: drafts-root\n",
                "root: drafts-root\n    create_lease_seconds: 1\n",
            )
            .replace(
                "root: followups-root\n",
                "root: followups-root\n    create_quiescence_seconds: 1\n",
            ),
    )
    .expect("the store");
    let slugs = [
        "theme-resets",
        "zoom-resets",
        "font-resets",
        "grid-resets",
        "ruler-resets",
        "panel-resets",
    ];
    let mut drafts = Vec::new();
    for slug in slugs {
        drafts.push(world.draft(
            BATCH,
            &format!("the {slug} after an update"),
            &["src/a.rs"],
            "Seen.\n",
        ));
    }
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    stage(&mut world, &slugs, &drafts);

    // The publish, killed while the governor holds its fourth record write.
    let settings = settings(&world);
    let parts = Parts {
        clock: Arc::new(KilledAtFirstWait),
        readings: Some(Arc::new(EightWritesLeft)),
        notices: Arc::new(|_: &str| {}),
    };
    let killed = governor::with_parts(parts, || {
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            onefollowup::api::publish(
                &settings,
                BatchArgs {
                    batch: BATCH.to_owned(),
                },
            )
        }))
    });
    let Err(payload) = killed else {
        panic!("the publish ran to its end: the gate never waited")
    };
    assert!(payload.is::<Killed>(), "killed at the gate's wait");
    let landed = root_causes(&world);
    assert!(
        !landed.is_empty() && landed.len() < slugs.len(),
        "killed mid-batch, part of the copy landed: {landed:?}"
    );
    assert!(
        world.receipts(BATCH).is_empty(),
        "no receipt confirms anything the killed publish sent"
    );

    // Past the killed copy's one-second lease, as a rerun after a real kill would be.
    std::thread::sleep(Duration::from_secs(2));

    // The pass run again — its outbox was taken by the killed publish, so the agent stages
    // again and check passes — sends each copy once more: what landed answers unchanged,
    // nothing rewritten; only what had not landed is created; nothing is filed twice.
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    stage(&mut world, &slugs, &drafts);
    let published = world.ok("publish", &["publish", "--batch", BATCH], "");
    let outcomes: Vec<(String, String)> = published["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|one| {
            let item = one["item"].as_str().unwrap_or_default();
            let slug = slugs
                .iter()
                .find(|slug| item.contains(*slug))
                .copied()
                .unwrap_or(item);
            (
                slug.to_owned(),
                one["outcome"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert_eq!(outcomes.len(), slugs.len(), "{published}");
    for (slug, outcome) in &outcomes {
        let expected = if landed.contains(slug) {
            "unchanged"
        } else {
            "created"
        };
        assert_eq!(outcome, expected, "{slug}: {published}");
    }
    let mut every: Vec<String> = slugs.iter().map(|slug| (*slug).to_owned()).collect();
    every.sort();
    assert_eq!(root_causes(&world), every, "each ticket filed exactly once");
    assert_eq!(published["governor"], json!({"waits": []}));

    let settled = world.ok("settle", &["settle", "--batch", BATCH], "");
    assert!(settled.get("needs_attention").is_none(), "{settled}");
    assert!(
        settled.get("pending").is_none(),
        "nothing left pending: {settled}"
    );
    let receipts = world.receipts(BATCH);
    for slug in slugs {
        assert!(
            receipts.iter().any(|receipt| receipt["card"]
                .as_str()
                .is_some_and(|card| card.contains(slug))
                && receipt["error"].is_null()),
            "{slug} confirmed by a receipt: {receipts:#?}"
        );
    }
}

/// The agent's staging of the pass: one ticket per draft, each copied, each draft filed.
fn stage(world: &mut World, slugs: &[&str], drafts: &[String]) {
    let mut answers = Vec::new();
    for (slug, draft) in slugs.iter().zip(drafts) {
        world.ticket(
            BATCH,
            &Ticket::new(slug, &format!("the {slug} after an update"), &[draft]),
        );
        world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");
        answers.push(answer(draft, "root-cause", "filed", &[slug], "Verified."));
    }
    world.answer(BATCH, &answers);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
}
