//! The governor at the store's real chokepoint, over a real local-md destination, on a
//! virtual clock: GitHub's rate-limit state — the doubled reading — is the only thing doubled.
//!
//! The doubled reading is a script stated in the case, or `spike-quota-burn`'s two real traces
//! (`fixtures/quota-burn/`, committed as that spike's branch holds them) replayed from their
//! start. A held destination call must send nothing until a reading admits it. For a write that is
//! read off the destination's files, unchanged at every reading taken while held; for a read
//! it is that the call answers what a person changed at the recovering reading, which a read
//! sent earlier could not have seen.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use onefollowup::governor::{self, Clock as _, Parts, Reading, Reason};
use onefollowup::store::{
    CopyBatch, EdgeWrite, HeldUpdate, Narrowing, NewTask, Search, SearchIn, Store, TaskChange,
};
use serde_json::Value;

mod support;
use support::replay::{Replay, TRACES};
use support::{DESTINATION, Scripted, T0, Virtual, instant, reading, shipped};

/// A store of two local-md sources, `drafts` (local) and the destination.
struct World {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl World {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = String::from("sources:\n");
        for source in ["drafts", DESTINATION] {
            std::fs::create_dir_all(root.join(format!("{source}-root/tasks"))).unwrap();
            store.push_str(&format!(
                "  {source}:\n    plugin: local-md\n    config:\n      root: {source}-root\n"
            ));
        }
        std::fs::write(root.join("onetaskgraph.yaml"), store).unwrap();
        Self {
            _directory: directory,
            root,
        }
    }

    fn environment(&self) -> Vec<(String, String)> {
        let home = self.root.join("home");
        vec![
            ("HOME".to_owned(), home.display().to_string()),
            (
                "XDG_CONFIG_HOME".to_owned(),
                home.join("config").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".to_owned(),
                home.join("cache").display().to_string(),
            ),
        ]
    }

    /// A store governed as a verb's is, by the system's clock and the store's own readings.
    fn store(&self) -> Store {
        Store::open(Some(&self.root), &self.root, &self.environment())
            .unwrap()
            .with_local("drafts")
    }

    /// A store whose governor reads `readings` and `clock`, its wait lines kept in `notices`.
    fn governed(
        &self,
        clock: &Arc<Virtual>,
        readings: &Arc<Scripted>,
        notices: &Arc<Mutex<Vec<String>>>,
    ) -> Store {
        let sink = Arc::clone(notices);
        let parts = Parts {
            clock: clock.clone(),
            readings: Some(readings.clone()),
            notices: Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_owned())),
        };
        governor::with_parts(parts, || self.store())
    }

    /// Every file under the destination's root, with its text.
    fn destination(&self) -> BTreeMap<PathBuf, String> {
        destination_files(&self.root)
    }

    fn task_file(&self, id: &str) -> PathBuf {
        let native = id.split_once(':').unwrap().1;
        self.root
            .join(format!("{DESTINATION}-root/tasks/{native}.md"))
    }
}

/// Every file under the destination's root in the store at `root`, with its text.
fn destination_files(root: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(directory: &Path, into: &mut BTreeMap<PathBuf, String>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, into);
            } else {
                into.insert(
                    path.clone(),
                    std::fs::read_to_string(&path).unwrap_or_default(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(&root.join(format!("{DESTINATION}-root")), &mut files);
    files
}

fn new_task(source: &str, project: &str, title: &str) -> NewTask {
    NewTask {
        source: source.to_owned(),
        project: project.to_owned(),
        title: title.to_owned(),
        body: format!("{title}, as a pass found it.\n"),
        status: "backlog".to_owned(),
        priority: "none".to_owned(),
        labels: Vec::new(),
        repositories: Vec::new(),
        metadata: BTreeMap::new(),
        assignee: None,
    }
}

/// The destination's project and `count` tasks in it, written by an ungoverned-for-quota
/// store (local-md reports no reading, so nothing waits).
fn seeded(world: &World, count: usize) -> Vec<String> {
    let store = world.store();
    store
        .ensure_project(DESTINATION, "board", "Board", &BTreeMap::new(), false)
        .unwrap();
    (0..count)
        .map(|at| {
            store
                .create_task(&new_task(DESTINATION, "board", &format!("Task {at:03}")))
                .unwrap()
                .id
        })
        .collect()
}

/// Under the floor until the GraphQL window resets at 60 s, a full window after it.
fn held_until_reset(at: DateTime<Utc>) -> Vec<Reading> {
    let graphql = if at < instant(T0 + 60) {
        reading("graphql", 900, T0 + 60)
    } else {
        reading("graphql", 5000, T0 + 3660)
    };
    vec![graphql, reading("core", 5000, T0 + 3600)]
}

/// One case: `call` made through a store held under the floor until 60 s. A write's files are
/// unchanged at every reading taken while held; `at_recovery` runs at the reading that admits
/// it. Answers what the call answered.
fn held_call<T>(
    world: &World,
    write: bool,
    at_recovery: impl Fn() + Send + Sync + 'static,
    call: impl FnOnce(&Store) -> T,
) -> T {
    let clock = Virtual::at(instant(T0));
    let readings = Scripted::new(&clock, held_until_reset);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let store = world.governed(&clock, &readings, &notices);
    let before = world.destination();
    let root = world.root.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    let recovered = Mutex::new(false);
    readings.on_read(move |now| {
        if now < instant(T0 + 60) {
            if write {
                assert_eq!(
                    destination_files(&root),
                    before,
                    "nothing was written while held"
                );
            }
        } else if !std::mem::replace(&mut *recovered.lock().unwrap(), true) {
            at_recovery();
        }
        kept.lock().unwrap().push(now);
    });
    let answered = call(&store);
    let report = store
        .governor_report()
        .expect("the governor governed the call");
    assert_eq!(report.waits.len(), 1, "{report:?}");
    assert_eq!(report.waits[0].reason, Reason::Floor);
    assert_eq!(report.waits[0].seconds, 60);
    assert_eq!(
        clock.now(),
        instant(T0 + 60),
        "it went at the recovering reading"
    );
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|read| *read < instant(T0 + 60)),
        "it was read while held"
    );
    assert_eq!(notices.lock().unwrap().len(), 1);
    assert_eq!(
        store.operations().len(),
        1,
        "the call was sent once: {:?}",
        store.operations()
    );
    answered
}

/// A store, opened on its own thread, as another process — a person's edit — would write.
fn elsewhere(root: &Path, change: impl FnOnce(&Store) + Send) {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let world = World {
                    _directory: tempfile::tempdir().unwrap(),
                    root: root.to_path_buf(),
                };
                change(&world.store());
            })
            .join()
            .unwrap();
    });
}

#[test]
fn a_held_write_sends_nothing_until_a_reading_shows_recovery_then_lands_once() {
    // ensure_project, create_task, update_task, edit_comment, copy, set_parents, add_links:
    // each its own engine call, each from a fresh store, as a fresh process sends it.
    let world = World::new();
    let ids = seeded(&world, 3);

    let project = held_call(
        &world,
        true,
        || {},
        |store| {
            store
                .ensure_project(
                    DESTINATION,
                    "other-board",
                    "Other board",
                    &BTreeMap::new(),
                    false,
                )
                .unwrap()
        },
    );
    assert_eq!(project, "other-board");

    let created = held_call(
        &world,
        true,
        || {},
        |store| {
            store
                .create_task(&new_task(DESTINATION, "board", "Created while held"))
                .unwrap()
        },
    );
    let created_files: Vec<_> = world
        .destination()
        .into_values()
        .filter(|text| text.contains("Created while held"))
        .collect();
    assert_eq!(created_files.len(), 1, "created once");
    assert_eq!(created.title, "Created while held");

    let updated = held_call(
        &world,
        true,
        || {},
        |store| {
            store
                .update_task(
                    &ids[0],
                    &TaskChange {
                        title: Some("Renamed while held".to_owned()),
                        ..TaskChange::default()
                    },
                )
                .unwrap()
        },
    );
    assert_eq!(updated.title, "Renamed while held");

    // A comment edit, from a store that has read nothing yet: held all the same.
    let comment = {
        let store = world.store();
        store
            .add_comments(
                DESTINATION,
                &[onefollowup::store::CommentPost {
                    item: ids[0].clone(),
                    body: "Posted before the hold.".to_owned(),
                }],
            )
            .unwrap();
        store.read(&[ids[0].clone()], true).unwrap()[0]
            .comments
            .clone()
            .unwrap()[0]
            .id
            .clone()
    };
    let edited = held_call(
        &world,
        true,
        || {},
        |store| {
            store
                .edit_comment(&ids[0], &comment, "Edited while held.")
                .unwrap()
        },
    );
    assert_eq!(edited.body, "Edited while held.");
    assert_eq!(
        world
            .destination()
            .values()
            .filter(|text| text.contains("Edited while held."))
            .count(),
        1,
        "edited once"
    );

    // A draft, copied into the destination: the drafts source is local, the copy governed.
    let draft = {
        let store = world.store();
        store
            .ensure_project("drafts", "batch", "Batch", &BTreeMap::new(), false)
            .unwrap();
        store
            .create_task(&new_task("drafts", "batch", "A draft to copy"))
            .unwrap()
            .id
    };
    let copied = held_call(
        &world,
        true,
        || {},
        |store| {
            store
                .copy(&CopyBatch {
                    destination: DESTINATION.to_owned(),
                    items: vec![draft.clone()],
                    create: true,
                    preserve_status: false,
                    versions: Vec::new(),
                })
                .unwrap()
        },
    );
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0].action, "created", "{copied:?}");
    assert_eq!(
        world
            .destination()
            .values()
            .filter(|text| text.contains("A draft to copy"))
            .count(),
        1
    );

    let parented = held_call(
        &world,
        true,
        || {},
        |store| {
            store
                .set_parents(&[EdgeWrite {
                    from: ids[1].clone(),
                    to: ids[0].clone(),
                    link_type: "parent".to_owned(),
                }])
                .unwrap()
        },
    );
    assert_eq!(parented.len(), 1);
    assert!(parented[0].reason.is_none(), "{parented:?}");

    let linked = held_call(
        &world,
        true,
        || {},
        |store| {
            store
                .add_links(&[EdgeWrite {
                    from: ids[2].clone(),
                    to: ids[0].clone(),
                    link_type: "relates-to".to_owned(),
                }])
                .unwrap()
        },
    );
    assert_eq!(linked.len(), 1);
    assert!(linked[0].reason.is_none(), "{linked:?}");
    let relations = world.store().relations(&ids).unwrap();
    assert_eq!(relations[1].parent.as_deref(), Some(ids[0].as_str()));
    assert_eq!(relations[2].links.len(), 1, "linked once: {relations:?}");
}

#[test]
fn a_held_read_sends_nothing_until_a_reading_shows_recovery_then_reads_once() {
    // find, list, read, comments_since, relations: each its own engine call, each answering
    // what a person changed at the recovering reading.
    let world = World::new();
    let ids = seeded(&world, 2);

    let root = world.root.clone();
    let found = held_call(
        &world,
        false,
        move || {
            elsewhere(&root, |store| {
                store
                    .create_task(&new_task(DESTINATION, "board", "Filed at recovery"))
                    .unwrap();
            });
        },
        |store| {
            store
                .find(
                    DESTINATION,
                    &[Search {
                        key: "recovery".to_owned(),
                        text: Some("Filed at recovery".to_owned()),
                        within: SearchIn::Title,
                        ..Search::default()
                    }],
                )
                .unwrap()
        },
    );
    assert_eq!(found[0].items.len(), 1, "{found:?}");

    let root = world.root.clone();
    let listed = held_call(
        &world,
        false,
        move || {
            elsewhere(&root, |store| {
                store
                    .create_task(&new_task(DESTINATION, "board", "Listed at recovery"))
                    .unwrap();
            });
        },
        |store| store.list(DESTINATION, &["backlog".to_owned()]).unwrap(),
    );
    assert!(
        listed.iter().any(|item| item.title == "Listed at recovery"),
        "{listed:?}"
    );

    let file = world.task_file(&ids[0]);
    let read = held_call(
        &world,
        false,
        move || {
            let text = std::fs::read_to_string(&file).unwrap();
            std::fs::write(
                &file,
                text.replace("Task 000", "Task 000, edited at recovery"),
            )
            .unwrap();
        },
        |store| store.read(&[ids[0].clone()], false).unwrap(),
    );
    assert_eq!(
        read[0].item.as_ref().map(|item| item.title.as_str()),
        Some("Task 000, edited at recovery")
    );

    let root = world.root.clone();
    let item = ids[1].clone();
    let listed = held_call(
        &world,
        false,
        move || {
            let item = item.clone();
            elsewhere(&root, move |store| {
                store
                    .add_comments(
                        DESTINATION,
                        &[onefollowup::store::CommentPost {
                            item,
                            body: "Commented at recovery.".to_owned(),
                        }],
                    )
                    .unwrap();
            });
        },
        |store| {
            store
                .comments_since(
                    DESTINATION,
                    &DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
                    &Narrowing::default(),
                )
                .unwrap()
        },
    );
    assert_eq!(listed.comments.len(), 1, "{listed:?}");
    assert_eq!(listed.comments[0].body, "Commented at recovery.");

    let root = world.root.clone();
    let (child, parent) = (ids[1].clone(), ids[0].clone());
    let related = held_call(
        &world,
        false,
        move || {
            let edge = EdgeWrite {
                from: child.clone(),
                to: parent.clone(),
                link_type: "parent".to_owned(),
            };
            elsewhere(&root, move |store| {
                store.set_parents(&[edge]).unwrap();
            });
        },
        |store| store.relations(&ids).unwrap(),
    );
    assert_eq!(related[1].parent.as_deref(), Some(ids[0].as_str()));
}

#[test]
fn one_update_many_of_100_waits_after_the_40th_record_until_a_reading_admits_the_41st() {
    // local-md names its record writes `local`; the doubled reading meters that resource as
    // GitHub meters one: 1000, floor 200, 240 left less every record written, resetting at
    // 90 s to a full window. Nothing reserves for others on `local` (no seeded peak, no
    // history), so the line is the floor: the 41st record would take it to 199.
    let world = World::new();
    let ids = seeded(&world, 100);
    let root = world.root.clone();
    let written = move || {
        std::fs::read_dir(root.join(format!("{DESTINATION}-root/tasks")))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                std::fs::read_to_string(entry.path())
                    .unwrap_or_default()
                    .contains("updated in one batch")
            })
            .count() as u64
    };
    let count = written.clone();
    let clock = Virtual::at(instant(T0));
    let readings = Scripted::new(&clock, move |at| {
        let written = count();
        let local = if at < instant(T0 + 90) {
            reading("local", 0, T0 + 90)
        } else {
            reading("local", 0, T0 + 3690)
        };
        let remaining = if at < instant(T0 + 90) {
            240 - written
        } else {
            1000 - (written - 40)
        };
        vec![
            Reading {
                limit: 1000,
                remaining,
                ..local
            },
            reading("graphql", 5000, T0 + 3600),
            reading("core", 5000, T0 + 3600),
        ]
    });
    let held_at = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&held_at);
    let during = written.clone();
    readings.on_read(move |now| kept.lock().unwrap().push((now, during())));
    let notices = Arc::new(Mutex::new(Vec::new()));
    let store = world.governed(&clock, &readings, &notices);
    let updates: Vec<HeldUpdate> = ids
        .iter()
        .map(|id| HeldUpdate {
            item: id.clone(),
            body: Some(format!("{id}, updated in one batch.\n")),
            ..HeldUpdate::default()
        })
        .collect();
    let answers = store.update_many(DESTINATION, &updates).unwrap();

    assert_eq!(answers.len(), 100, "every item answered");
    assert!(
        answers.iter().all(|answer| answer.reason.is_none()),
        "{answers:?}"
    );
    let answered: std::collections::BTreeSet<&str> =
        answers.iter().map(|answer| answer.item.as_str()).collect();
    assert_eq!(answered.len(), 100);
    assert_eq!(written(), 100, "every record landed");
    for id in &ids {
        let text = std::fs::read_to_string(world.task_file(id)).unwrap();
        assert_eq!(
            text.matches("updated in one batch").count(),
            1,
            "{id} written once"
        );
    }
    let reads = held_at.lock().unwrap().clone();
    let held: Vec<_> = reads
        .iter()
        .filter(|(at, _)| *at > instant(T0) && *at < instant(T0 + 90))
        .collect();
    assert!(!held.is_empty(), "read while held: {reads:?}");
    assert!(
        reads
            .iter()
            .filter(|(at, _)| *at < instant(T0 + 90))
            .all(|(_, written)| *written <= 40),
        "the 41st was not written before the reading that admits it: {reads:?}"
    );
    assert!(
        reads
            .iter()
            .any(|(at, written)| *at == instant(T0 + 90) && *written == 40),
        "{reads:?}"
    );
    let report = store.governor_report().unwrap();
    assert_eq!(report.waits.len(), 1, "{report:?}");
    assert_eq!(report.waits[0].resource, "local");
    assert_eq!(report.waits[0].seconds, 90);
    assert_eq!(report.waits[0].reason, Reason::Floor);
    assert_eq!(store.operations().len(), 1, "one update-many");
    // 240 read, less the 40 admitted since: on the estimate, at the floor.
    let line = notices.lock().unwrap()[0].clone();
    assert!(
        line.contains("on local: remaining 200 of 1000, floor 200"),
        "{line}"
    );
}

#[test]
fn one_follow_up_batch_over_local_md_which_reports_no_reading_is_admitted_without_waiting() {
    // The store's own readings: local-md reports none.
    let world = World::new();
    let clock = Virtual::at(instant(T0));
    let notices = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&notices);
    let store = governor::with_parts(
        Parts {
            clock: clock.clone(),
            readings: None,
            notices: Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_owned())),
        },
        || world.store(),
    );
    store
        .ensure_project(DESTINATION, "board", "Board", &BTreeMap::new(), false)
        .unwrap();
    let ids: Vec<String> = (0..10)
        .map(|at| {
            store
                .create_task(&new_task(DESTINATION, "board", &format!("Filed {at}")))
                .unwrap()
                .id
        })
        .collect();
    let updates: Vec<HeldUpdate> = ids
        .iter()
        .map(|id| HeldUpdate {
            item: id.clone(),
            status: Some("todo".to_owned()),
            ..HeldUpdate::default()
        })
        .collect();
    store.update_many(DESTINATION, &updates).unwrap();
    store
        .add_comments(
            DESTINATION,
            &ids.iter()
                .map(|id| onefollowup::store::CommentPost {
                    item: id.clone(),
                    body: "Evidence.".to_owned(),
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
    assert_eq!(
        clock.now(),
        instant(T0),
        "no virtual time passed in admission"
    );
    assert_eq!(
        store.governor_report().map(|report| report.waits),
        Some(Vec::new())
    );
    assert!(notices.lock().unwrap().is_empty());
    assert!(
        store.quota_reads() >= 1,
        "the destination was asked for its reading"
    );
    let destination_ops: Vec<Value> = store
        .operations()
        .iter()
        .map(|op| serde_json::to_value(op).unwrap())
        .collect();
    assert_eq!(
        destination_ops.len(),
        13,
        "the governor's reads are no store operation"
    );
}

/// A pass of `points` on `metered`, replaying the committed trace `trace_name` from its start:
/// spent at 25 points a minute in 12.5-point batches, every batch a real `update-many` through
/// the store over the local-md destination. A local-md record write is one request on
/// `local`, which the doubled reading meters as the traced `metered` window in units of one
/// batch ([`BATCH_POINTS`]): one record a batch, the pass's own records spent from it. Every
/// term of the admission rule — remaining, cost, the floor's share of the limit, the reserve's
/// peak rate times time — scales with the unit, so each admission is the one the trace's own
/// points would make, but for remaining rounding down to a whole batch, which only ever errs
/// towards waiting. Answers the store, the virtual seconds admission took, and the reading.
fn replayed_pass(trace_name: &str, metered: &str, points: u64) -> (Store, f64, Arc<Scripted>) {
    let world = World::new();
    let ids = seeded(&world, 1);
    let replay = Arc::new(Replay::of(trace_name));
    let start = replay.start();
    let clock = Virtual::at(start);
    // Any wait fails the case, at its first pause rather than after it.
    *clock.limit.lock().unwrap() = Some(start);
    let script = Arc::clone(&replay);
    let metered_name = metered.to_owned();
    let readings = Scripted::new(&clock, move |at| {
        script.readings(at, &metered_name, BATCH_POINTS)
    });
    let notices = Arc::new(Mutex::new(Vec::new()));
    // The shipped defaults, the traced resource's seeded peak standing for its `local` view in
    // that view's unit.
    let mut settings = shipped();
    let seeded_peak = settings.min_peak_per_minute[metered] / BATCH_POINTS;
    settings
        .min_peak_per_minute
        .insert("local".to_owned(), seeded_peak);
    let store = world
        .governed(&clock, &readings, &notices)
        .with_governor(&settings);
    let batches = (points as f64 / BATCH_POINTS).ceil() as usize;
    let mut admission = 0.0;
    for batch in 0..batches {
        let before = clock.now();
        let answers = store
            .update_many(
                DESTINATION,
                &[HeldUpdate {
                    item: ids[0].clone(),
                    body: Some(format!("Batch {batch} of the pass.\n")),
                    ..HeldUpdate::default()
                }],
            )
            .unwrap();
        let waited = (clock.now() - before).num_milliseconds() as f64 / 1000.0;
        assert_eq!(
            waited,
            0.0,
            "{trace_name} {metered} {points}: batch {batch} waited: {:?}",
            notices.lock().unwrap()
        );
        assert!(answers[0].reason.is_none(), "{answers:?}");
        admission += waited;
        replay.spend(metered, BATCH_POINTS, clock.now());
        // 12.5 points at 25 a minute: a batch every 30 seconds.
        clock.advance(30.0);
    }
    assert!(notices.lock().unwrap().is_empty(), "no wait line");
    assert_eq!(
        store.operations().len(),
        batches,
        "one update-many a batch, the quota reads counted in none"
    );
    let text = std::fs::read_to_string(world.task_file(&ids[0])).unwrap();
    assert!(
        text.contains(&format!("Batch {} of the pass.", batches - 1)),
        "every batch landed"
    );
    (store, admission, readings)
}

/// What one batch of a replayed pass spends, in the trace's points.
const BATCH_POINTS: f64 = 12.5;

/// With quota ample for everyone's burn, a pass of `points` on `metered` over the committed
/// trace `trace_name` is admitted at every store call without waiting.
fn admitted_without_waiting(trace_name: &str, metered: &str, points: u64) {
    let (store, admission, readings) = replayed_pass(trace_name, metered, points);
    assert_eq!(
        store.governor_report().map(|report| report.waits),
        Some(Vec::new()),
        "{trace_name} {metered} {points}: the governor object records no wait"
    );
    assert_eq!(
        admission, 0.0,
        "{trace_name} {metered} {points}: no virtual time passes in admission"
    );
    assert!(
        readings.reads().len() > 1 && store.quota_reads() > 1,
        "the doubled reading was read through the pass"
    );
}

#[test]
fn with_quota_ample_the_sample_of_2026_11_21_admits_every_store_call_of_a_full_graphql_pass_without_waiting()
 {
    admitted_without_waiting(TRACES[0], "graphql", 1500);
}

#[test]
fn with_quota_ample_the_sample_of_2026_11_21_admits_every_store_call_of_a_weekly_graphql_pass_without_waiting()
 {
    admitted_without_waiting(TRACES[0], "graphql", 150);
}

#[test]
fn with_quota_ample_the_sample_of_2026_11_21_admits_every_store_call_of_a_full_core_pass_without_waiting()
 {
    admitted_without_waiting(TRACES[0], "core", 1500);
}

#[test]
fn with_quota_ample_the_sample_of_2026_11_21_admits_every_store_call_of_a_weekly_core_pass_without_waiting()
 {
    admitted_without_waiting(TRACES[0], "core", 150);
}

#[test]
fn with_quota_ample_the_grooming_pass_of_2026_11_20_admits_every_store_call_of_a_full_graphql_pass_without_waiting()
 {
    admitted_without_waiting(TRACES[1], "graphql", 1500);
}

#[test]
fn with_quota_ample_the_grooming_pass_of_2026_11_20_admits_every_store_call_of_a_weekly_graphql_pass_without_waiting()
 {
    admitted_without_waiting(TRACES[1], "graphql", 150);
}

#[test]
fn with_quota_ample_the_grooming_pass_of_2026_11_20_admits_every_store_call_of_a_full_core_pass_without_waiting()
 {
    admitted_without_waiting(TRACES[1], "core", 1500);
}

#[test]
fn with_quota_ample_the_grooming_pass_of_2026_11_20_admits_every_store_call_of_a_weekly_core_pass_without_waiting()
 {
    admitted_without_waiting(TRACES[1], "core", 150);
}

#[test]
fn a_copy_of_more_tasks_than_a_call_may_create_goes_in_calls_the_caps_take_each_task_once() {
    // A per-minute cap of 5 and an hour's not binding: a copy creating 12 tasks is sent as calls
    // of 5, 5 and 2, each declared to create every task it carries, and every task is created
    // once and answered in order.
    let world = World::new();
    let drafted = world.store();
    drafted
        .ensure_project("drafts", "inbox", "Inbox", &BTreeMap::new(), false)
        .unwrap();
    drafted
        .ensure_project(DESTINATION, "board", "Board", &BTreeMap::new(), false)
        .unwrap();
    let drafts: Vec<String> = (0..12)
        .map(|at| {
            drafted
                .create_task(&new_task("drafts", "inbox", &format!("Draft {at:02}")))
                .unwrap()
                .id
        })
        .collect();
    let clock = Virtual::at(instant(T0));
    let readings = Scripted::new(&clock, |_| vec![reading("graphql", 5000, T0 + 3600)]);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let settings = onefollowup::config::GovernorSettings {
        max_content_writes_per_minute: 5,
        max_content_writes_per_hour: 1000,
        ..shipped()
    };
    let store = world
        .governed(&clock, &readings, &notices)
        .with_governor(&settings);
    let copied = store
        .copy(&CopyBatch {
            destination: DESTINATION.to_owned(),
            items: drafts.clone(),
            create: true,
            preserve_status: true,
            versions: Vec::new(),
        })
        .unwrap();

    let answered: Vec<(&str, &str)> = copied
        .iter()
        .map(|one| (one.source.as_str(), one.action.as_str()))
        .collect();
    let expected: Vec<(&str, &str)> = drafts.iter().map(|id| (id.as_str(), "created")).collect();
    assert_eq!(answered, expected, "every task created once, in order");
    let calls: Vec<usize> = store
        .operations()
        .iter()
        .filter(|op| op.op == "copy")
        .map(|op| op.items)
        .collect();
    assert_eq!(calls, vec![5, 5, 2], "calls of as many as the cap takes");
    let created = world
        .destination()
        .values()
        .filter(|text| text.contains("Draft "))
        .count();
    assert_eq!(created, 12, "the destination holds each task once");
}

#[test]
fn a_copy_call_refused_after_an_earlier_one_wrote_answers_its_tasks_failed_and_keeps_the_written() {
    // A per-minute cap of 2 sends a copy of 3 as calls of 2 and 1; the second names a task that
    // does not exist, which onetaskgraph refuses as a whole copy. The first call's tasks stay
    // created and answered so; the refused one answers the refusal.
    let world = World::new();
    let drafted = world.store();
    drafted
        .ensure_project("drafts", "inbox", "Inbox", &BTreeMap::new(), false)
        .unwrap();
    drafted
        .ensure_project(DESTINATION, "board", "Board", &BTreeMap::new(), false)
        .unwrap();
    let mut drafts: Vec<String> = (0..2)
        .map(|at| {
            drafted
                .create_task(&new_task("drafts", "inbox", &format!("Draft {at:02}")))
                .unwrap()
                .id
        })
        .collect();
    drafts.push("drafts:never-drafted".to_owned());
    let clock = Virtual::at(instant(T0));
    let readings = Scripted::new(&clock, |_| vec![reading("graphql", 5000, T0 + 3600)]);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let settings = onefollowup::config::GovernorSettings {
        max_content_writes_per_minute: 2,
        max_content_writes_per_hour: 1000,
        ..shipped()
    };
    let store = world
        .governed(&clock, &readings, &notices)
        .with_governor(&settings);
    let copied = store
        .copy(&CopyBatch {
            destination: DESTINATION.to_owned(),
            items: drafts.clone(),
            create: true,
            preserve_status: true,
            versions: Vec::new(),
        })
        .expect("the first call wrote, so the copy is answered item by item");

    let actions: Vec<(&str, &str)> = copied
        .iter()
        .map(|one| (one.source.as_str(), one.action.as_str()))
        .collect();
    assert_eq!(
        actions,
        vec![
            (drafts[0].as_str(), "created"),
            (drafts[1].as_str(), "created"),
            (drafts[2].as_str(), "failed"),
        ]
    );
    let (kind, message) = copied[2].error.clone().expect("the refusal");
    assert_eq!(kind, "store-refused");
    assert!(
        message.contains("no item with the id drafts:never-drafted"),
        "{message}"
    );
    let created = world
        .destination()
        .values()
        .filter(|text| text.contains("Draft "))
        .count();
    assert_eq!(created, 2, "the first call's tasks stand");
}
