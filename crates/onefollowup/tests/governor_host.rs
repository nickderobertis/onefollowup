//! The governor's host-level state, shared by several onefollowup processes: each "process" is
//! a store of its own — its own work directory and its own local-md destination — on a thread
//! of its own, every one pointed at one `governor.state_dir`, all on one simulated clock.
//! GitHub's rate-limit state is the only thing doubled: the local-md destinations are real,
//! and every content-creating write is read back off their files at the instant it landed.
//! A process is killed as `SIGKILL` would kill it: at a wait, nothing of it runs after.
//!
//! The admission rule's guarantee across processes is driven through the governor's two
//! seams, as the store drives them, against one doubled GitHub both processes spend from.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use onefollowup::config::GovernorSettings;
use onefollowup::governor::{
    self, Clock as _, Governor, Kind, Parts, Pending, Reading, Readings, Reason, Request,
};
use onefollowup::store::{CommentPost, NewTask, Store};

mod support;
use support::{Killed, Participant, Simulated, T0, instant, reading, shipped};

/// GitHub's rate-limit state as ample throughout: full windows resetting on the hour, which
/// nothing the floor or the seeded reserve holds.
struct Ample(Arc<Simulated>);

impl Readings for Ample {
    fn read<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        let hour = (self.0.now().timestamp() - T0) / 3600;
        let reset = T0 + (hour + 1) * 3600;
        let readings = vec![
            reading("graphql", 5000, reset),
            reading("core", 5000, reset),
        ];
        Box::pin(async move { Ok(readings) })
    }
}

/// One onefollowup process's world: a work directory holding a store of `drafts` and the
/// local-md destination `destination`, with tasks to comment on.
struct Process {
    _directory: tempfile::TempDir,
    root: PathBuf,
    destination: String,
    tasks: Vec<String>,
}

impl Process {
    fn new(destination: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_path_buf();
        let mut store = String::from("sources:\n");
        for source in ["drafts", destination] {
            std::fs::create_dir_all(root.join(format!("{source}-root/tasks"))).unwrap();
            store.push_str(&format!(
                "  {source}:\n    plugin: local-md\n    config:\n      root: {source}-root\n"
            ));
        }
        std::fs::write(root.join("onetaskgraph.yaml"), store).unwrap();
        let mut process = Self {
            _directory: directory,
            root,
            destination: destination.to_owned(),
            tasks: Vec::new(),
        };
        // Seeded by a store with no state directory: its writes count in no host state.
        let seeding = Store::open(Some(&process.root), &process.root, &process.environment())
            .unwrap()
            .with_local("drafts");
        seeding
            .ensure_project(destination, "board", "Board", &BTreeMap::new(), false)
            .unwrap();
        process.tasks = (0..5)
            .map(|at| {
                seeding
                    .create_task(&NewTask {
                        source: destination.to_owned(),
                        project: "board".to_owned(),
                        title: format!("Ticket {at}"),
                        body: "Seen.\n".to_owned(),
                        status: "backlog".to_owned(),
                        priority: "none".to_owned(),
                        labels: Vec::new(),
                        repositories: Vec::new(),
                        metadata: BTreeMap::new(),
                        assignee: None,
                    })
                    .unwrap()
                    .id
            })
            .collect();
        process
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

    /// The process's store, governed by `settings` on `clock`, its wait lines in `notices`.
    fn store(
        &self,
        clock: &Arc<Simulated>,
        settings: &GovernorSettings,
        notices: &Arc<Mutex<Vec<String>>>,
    ) -> Store {
        let sink = Arc::clone(notices);
        let parts = Parts {
            clock: clock.clone(),
            readings: Some(Arc::new(Ample(Arc::clone(clock)))),
            notices: Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_owned())),
        };
        governor::with_parts(parts, || {
            Store::open(Some(&self.root), &self.root, &self.environment())
                .unwrap()
                .with_local("drafts")
                .with_governor(settings)
        })
    }

    /// The evidence comment `number` of this process: its body names both.
    fn post(&self, number: usize) -> CommentPost {
        CommentPost {
            item: self.tasks[number % self.tasks.len()].clone(),
            body: format!("evidence {} {number:03}.", self.destination),
        }
    }

    /// Every evidence comment on the destination, by body, with how often each appears.
    fn written(&self) -> BTreeMap<String, usize> {
        let mut written = BTreeMap::new();
        let tasks = self.root.join(format!("{}-root/tasks", self.destination));
        for entry in std::fs::read_dir(tasks).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default();
            for line in text.lines() {
                if let Some(at) = line.find("evidence ") {
                    let body = line[at..].trim_end().to_owned();
                    *written.entry(body).or_insert(0) += 1;
                }
            }
        }
        written
    }

    fn count(&self) -> usize {
        self.written().values().sum()
    }
}

/// When each content-creating write landed, read off every destination's files each time the
/// clock is about to move, so a write is stamped with the instant it was made at.
#[derive(Clone)]
struct Landed {
    counts: Arc<Mutex<Vec<usize>>>,
    at: Arc<Mutex<Vec<DateTime<Utc>>>>,
}

impl Landed {
    fn watch(clock: &Arc<Simulated>, processes: Vec<Arc<Process>>) -> Self {
        let landed = Self {
            counts: Arc::new(Mutex::new(vec![0; processes.len()])),
            at: Arc::new(Mutex::new(Vec::new())),
        };
        let watching = landed.clone();
        clock.on_advance(move |now| watching.take(&processes, now));
        landed
    }

    fn take(&self, processes: &[Arc<Process>], now: DateTime<Utc>) {
        let mut counts = self.counts.lock().unwrap();
        for (seen, process) in counts.iter_mut().zip(processes) {
            let count = process.count();
            for _ in *seen..count {
                self.at.lock().unwrap().push(now);
            }
            *seen = count;
        }
    }

    /// Every write so far, the last instant's included.
    fn all(&self, processes: &[Arc<Process>], now: DateTime<Utc>) -> Vec<DateTime<Utc>> {
        self.take(processes, now);
        self.at.lock().unwrap().clone()
    }
}

/// How many of `writes` fall in each instant.
fn per_instant(writes: &[DateTime<Utc>]) -> Vec<(i64, usize)> {
    let mut counted: BTreeMap<i64, usize> = BTreeMap::new();
    for at in writes {
        *counted.entry(at.timestamp() - T0).or_default() += 1;
    }
    counted.into_iter().collect()
}

/// The most of `writes` in any rolling `span` seconds.
fn most_in_any(writes: &[DateTime<Utc>], span: i64) -> usize {
    let mut sorted = writes.to_vec();
    sorted.sort();
    (0..sorted.len())
        .map(|first| {
            sorted[first..]
                .iter()
                .take_while(|at| (**at - sorted[first]).num_seconds() < span)
                .count()
        })
        .max()
        .unwrap_or(0)
}

/// The shipped settings with the host state in `directory`.
fn sharing(directory: &Path) -> GovernorSettings {
    GovernorSettings {
        state_dir: Some(directory.to_path_buf()),
        ..shipped()
    }
}

/// Run `posts` of `process` as one `comment add-many` on a thread of its own taking part in
/// `clock`: `Ok` with what it answered, or `Err` when the case killed it.
fn publish(
    process: &Arc<Process>,
    clock: &Arc<Simulated>,
    settings: &GovernorSettings,
    posts: Vec<CommentPost>,
    notices: &Arc<Mutex<Vec<String>>>,
    killed_at: Option<DateTime<Utc>>,
) -> std::thread::JoinHandle<Result<Option<governor::Report>, ()>> {
    let process = Arc::clone(process);
    let clock_of = Arc::clone(clock);
    let settings = settings.clone();
    let notices = Arc::clone(notices);
    // Joined before the thread starts, so the clock never moves past a process not yet begun.
    let participant: Participant = clock.join();
    std::thread::spawn(move || {
        let _taking_part = participant;
        if let Some(at) = killed_at {
            clock_of.kill(std::thread::current().id(), at);
        }
        let ran = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let store = process.store(&clock_of, &settings, &notices);
            let answers = store.add_comments(&process.destination, &posts).unwrap();
            assert!(
                answers.iter().all(|answer| answer.reason.is_none()),
                "{answers:?}"
            );
            store.governor_report()
        }));
        match ran {
            Ok(report) => Ok(report),
            Err(payload) if payload.is::<Killed>() => Err(()),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

fn posts(process: &Process, numbers: impl IntoIterator<Item = usize>) -> Vec<CommentPost> {
    numbers
        .into_iter()
        .map(|number| process.post(number))
        .collect()
}

/// The posts of `numbers` not yet on `process`'s destination: what a restarted publish sends.
fn unwritten(process: &Process, numbers: impl IntoIterator<Item = usize>) -> Vec<CommentPost> {
    let written = process.written();
    numbers
        .into_iter()
        .map(|number| process.post(number))
        .filter(|post| !written.contains_key(&post.body))
        .collect()
}

fn each_once(process: &Process, numbers: impl IntoIterator<Item = usize>) {
    let written = process.written();
    let expected: BTreeSet<String> = numbers
        .into_iter()
        .map(|number| process.post(number).body)
        .collect();
    let landed: BTreeSet<String> = written.keys().cloned().collect();
    assert_eq!(landed, expected, "every comment landed");
    assert!(
        written.values().all(|times| *times == 1),
        "every comment written exactly once: {written:?}"
    );
}

#[test]
fn one_comment_add_many_over_the_minute_cap_writes_up_to_it_waits_for_the_window_then_the_rest() {
    // One process, the per-minute cap 10 and the hour's not binding: 25 comments go 10 at
    // once, 10 once the first ten leave the minute, then the last 5.
    let state = tempfile::tempdir().unwrap();
    let settings = GovernorSettings {
        max_content_writes_per_minute: 10,
        max_content_writes_per_hour: 1000,
        ..sharing(state.path())
    };
    let clock = Simulated::at(instant(T0));
    let alpha = Arc::new(Process::new("alpha"));
    let landed = Landed::watch(&clock, vec![Arc::clone(&alpha)]);
    let notices = Arc::new(Mutex::new(Vec::new()));
    let report = publish(
        &alpha,
        &clock,
        &settings,
        posts(&alpha, 0..25),
        &notices,
        None,
    )
    .join()
    .unwrap()
    .unwrap()
    .unwrap();

    let writes = landed.all(&[Arc::clone(&alpha)], clock.now());
    assert_eq!(per_instant(&writes), vec![(0, 10), (60, 10), (120, 5)]);
    each_once(&alpha, 0..25);
    let reasons: Vec<(Reason, u64)> = report
        .waits
        .iter()
        .map(|wait| (wait.reason, wait.seconds))
        .collect();
    assert_eq!(
        reasons,
        vec![
            (Reason::ContentPerMinute, 60),
            (Reason::ContentPerMinute, 60)
        ]
    );
    assert_eq!(report.waits[0].resource, "local");
    let line = notices.lock().unwrap()[0].clone();
    assert!(
        line.contains(
            "holding content-creating writes to alpha on local: 10 in the last minute across this host, max_content_writes_per_minute 10; resuming by 2026-11-22T00:01:00Z"
        ),
        "{line}"
    );
}

/// Two processes and a restart sharing one content-write window of `span` seconds, capped at
/// 10, the other cap not binding.
fn a_shared_window_holds_both_processes_and_a_restart(span: i64, reason: Reason) {
    let state = tempfile::tempdir().unwrap();
    let (minute, hour) = if span == 60 { (10, 1000) } else { (1000, 10) };
    let settings = GovernorSettings {
        max_content_writes_per_minute: minute,
        max_content_writes_per_hour: hour,
        ..sharing(state.path())
    };
    let clock = Simulated::at(instant(T0));
    let alpha = Arc::new(Process::new("alpha"));
    let beta = Arc::new(Process::new("beta"));
    let both = vec![Arc::clone(&alpha), Arc::clone(&beta)];
    let landed = Landed::watch(&clock, both.clone());
    let notices = Arc::new(Mutex::new(Vec::new()));
    let tenth = span / 6;

    // Alpha writes 6 at once; a sixth of the window later beta's 6 find room for 4, and beta
    // waits; killed while it waits, it wrote nothing more.
    publish(
        &alpha,
        &clock,
        &settings,
        posts(&alpha, 0..6),
        &notices,
        None,
    )
    .join()
    .unwrap()
    .unwrap();
    clock.advance(tenth);
    let killed = publish(
        &beta,
        &clock,
        &settings,
        posts(&beta, 0..6),
        &notices,
        Some(instant(T0 + 2 * tenth)),
    )
    .join()
    .unwrap();
    assert!(killed.is_err(), "beta was killed while it waited");
    assert_eq!(clock.now(), instant(T0 + 2 * tenth));
    assert_eq!(beta.count(), 4, "beta filled the window and wrote no more");

    // The window is full of both processes' writes: beta restarted and alpha's next pass
    // both wait, and write once alpha's first six leave the window.
    let restarted = publish(
        &beta,
        &clock,
        &settings,
        unwritten(&beta, 0..6),
        &notices,
        None,
    );
    let again = publish(
        &alpha,
        &clock,
        &settings,
        posts(&alpha, 6..8),
        &notices,
        None,
    );
    let restarted = restarted.join().unwrap().unwrap().unwrap();
    let again = again.join().unwrap().unwrap().unwrap();

    let writes = landed.all(&both, clock.now());
    assert_eq!(
        per_instant(&writes),
        vec![(0, 6), (tenth, 4), (span, 4)],
        "nothing was written while the shared window was full"
    );
    for report in [&restarted, &again] {
        assert_eq!(report.waits.len(), 1, "{report:?}");
        assert_eq!(report.waits[0].reason, reason);
        assert_eq!(
            report.waits[0].seconds,
            u64::try_from(span - 2 * tenth).unwrap()
        );
    }
    each_once(&alpha, 0..8);
    each_once(&beta, 0..6);
}

#[test]
fn the_minute_window_is_shared_by_two_processes_and_kept_across_a_restart() {
    a_shared_window_holds_both_processes_and_a_restart(60, Reason::ContentPerMinute);
}

#[test]
fn the_hour_window_is_shared_by_two_processes_and_kept_across_a_restart() {
    a_shared_window_holds_both_processes_and_a_restart(3600, Reason::ContentPerHour);
}

#[test]
fn two_processes_of_150_comments_and_a_restart_stay_inside_the_shipped_caps_host_wide() {
    let state = tempfile::tempdir().unwrap();
    let settings = sharing(state.path());
    assert_eq!(settings.max_content_writes_per_minute, 40);
    assert_eq!(settings.max_content_writes_per_hour, 250);
    let clock = Simulated::at(instant(T0));
    let alpha = Arc::new(Process::new("alpha"));
    let beta = Arc::new(Process::new("beta"));
    let both = vec![Arc::clone(&alpha), Arc::clone(&beta)];
    let landed = Landed::watch(&clock, both.clone());
    let notices = Arc::new(Mutex::new(Vec::new()));

    // Whichever process's last writes wait for the hour is killed mid-hour while it waits,
    // and restarted; the other finishes inside the hour.
    let runs: Vec<_> = both
        .iter()
        .map(|process| {
            let run = publish(
                process,
                &clock,
                &settings,
                posts(process, 0..150),
                &notices,
                Some(instant(T0 + 1800)),
            );
            (Arc::clone(process), run)
        })
        .collect();
    let killed: Vec<Arc<Process>> = runs
        .into_iter()
        .filter_map(|(process, run)| run.join().unwrap().is_err().then_some(process))
        .collect();
    assert_eq!(killed.len(), 1, "one process was killed mid-hour");
    assert_eq!(clock.now(), instant(T0 + 1800));
    let victim = &killed[0];
    assert!(victim.count() < 150, "killed with comments left to write");
    let restarted = publish(
        victim,
        &clock,
        &settings,
        unwritten(victim, 0..150),
        &notices,
        None,
    );
    restarted.join().unwrap().unwrap();

    let writes = landed.all(&both, clock.now());
    assert_eq!(writes.len(), 300);
    assert!(most_in_any(&writes, 60) <= 40, "{:?}", per_instant(&writes));
    assert!(
        most_in_any(&writes, 3600) <= 250,
        "{:?}",
        per_instant(&writes)
    );
    assert_eq!(
        most_in_any(&writes, 3600),
        250,
        "the hour's cap was reached"
    );
    each_once(&alpha, 0..150);
    each_once(&beta, 0..150);
    assert!(
        clock.now() >= instant(T0 + 3600),
        "the last 50 waited for the hour"
    );
}

/// What the doubled GitHub saw, in the order it saw it.
#[derive(Debug, Clone, Copy)]
enum Event {
    /// A request sent, by which process.
    Sent(DateTime<Utc>, usize),
    /// A response, to which process, with the GraphQL remaining it reported.
    Answered(DateTime<Utc>, usize, i64),
    /// A `GET /rate_limit`.
    Read(DateTime<Utc>),
}

/// One GitHub both processes spend from: GraphQL at `remaining` of 5,000 until the reset at
/// an hour, a full window after it; core always full. Every request costs [`HINT_COST`], its
/// hint plus one. `spike` is another consumer's spend: at that instant, remaining drops to that figure.
struct Backend {
    clock: Arc<Simulated>,
    remaining: Mutex<i64>,
    spike: Mutex<Option<(DateTime<Utc>, i64)>>,
    events: Mutex<Vec<Event>>,
    /// Requests in flight now, and the most ever, by process.
    in_flight: Mutex<[u32; 2]>,
    most_in_flight: Mutex<[u32; 2]>,
}

const RESET: i64 = T0 + 3600;

/// What each request of an `update-many` is hinted at, and what it costs: one point more.
const HINT: u64 = 16;
const HINT_COST: i64 = 17;

impl Backend {
    fn readings(&self) -> Vec<Reading> {
        let now = self.clock.now();
        let mut remaining = self.remaining.lock().unwrap();
        let mut spike = self.spike.lock().unwrap();
        if let Some((at, to)) = *spike
            && now >= at
        {
            *remaining = to;
            *spike = None;
        }
        if now >= instant(RESET) {
            vec![
                reading("graphql", 5000, RESET + 3600),
                reading("core", 5000, RESET + 3600),
            ]
        } else {
            vec![
                reading("graphql", u64::try_from(*remaining).unwrap(), RESET),
                reading("core", 5000, RESET),
            ]
        }
    }

    fn graphql(readings: &[Reading]) -> i64 {
        i64::try_from(readings[0].remaining).unwrap()
    }

    fn log(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }

    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

impl Readings for Backend {
    fn read<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        let readings = self.readings();
        self.log(Event::Read(self.clock.now()));
        Box::pin(async move { Ok(readings) })
    }
}

/// One process publishing one `update-many` of `writes`, each a GraphQL request hinted
/// [`HINT`] that
/// takes a second to answer, through `governor`'s seams as the store drives them.
fn update_many(process: usize, governor: &Governor, backend: &Backend, writes: usize) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        governor
            .admit_call(&["followups".to_owned()], Kind::Write)
            .await;
        for _ in 0..writes {
            governor
                .before_request(&Request {
                    resource: "graphql".to_owned(),
                    cost_hint: Some(HINT),
                    content_creating: false,
                })
                .await;
            backend.log(Event::Sent(backend.clock.now(), process));
            {
                let mut in_flight = backend.in_flight.lock().unwrap();
                in_flight[process] += 1;
                let mut most = backend.most_in_flight.lock().unwrap();
                most[process] = most[process].max(in_flight[process]);
            }
            backend.clock.sleep(Duration::from_secs(1)).await;
            if backend.clock.now() < instant(RESET) {
                *backend.remaining.lock().unwrap() -= HINT_COST;
            }
            let readings = backend.readings();
            backend.in_flight.lock().unwrap()[process] -= 1;
            backend.log(Event::Answered(
                backend.clock.now(),
                process,
                Backend::graphql(&readings),
            ));
            governor.after_response(&readings);
            governor.request_ended();
        }
        governor.end_call();
    });
}

/// Two processes sharing one host state, each publishing one `update-many` of 100 on one
/// doubled GitHub: the backend once both are done, and each one's report.
fn two_update_manies(
    settings: &GovernorSettings,
    start: i64,
    spike: Option<(i64, i64)>,
) -> (Arc<Backend>, Vec<governor::Report>) {
    let clock = Simulated::at(instant(T0));
    let backend = Arc::new(Backend {
        clock: Arc::clone(&clock),
        remaining: Mutex::new(start),
        spike: Mutex::new(spike.map(|(at, to)| (instant(at), to))),
        events: Mutex::new(Vec::new()),
        in_flight: Mutex::new([0, 0]),
        most_in_flight: Mutex::new([0, 0]),
    });
    let runs: Vec<_> = (0..2)
        .map(|process| {
            let participant = clock.join();
            let backend = Arc::clone(&backend);
            let settings = settings.clone();
            std::thread::spawn(move || {
                let _taking_part = participant;
                let governor = Governor::new(
                    settings,
                    backend.clock.clone(),
                    backend.clone(),
                    Arc::new(|line: &str| eprintln!("{line}")),
                );
                update_many(process, &governor, &backend, 100);
                governor.report().unwrap()
            })
        })
        .collect();
    let reports = runs.into_iter().map(|run| run.join().unwrap()).collect();
    (backend, reports)
}

/// How many requests each process sent, and how many responses each was told.
fn sent(events: &[Event]) -> [(usize, usize); 2] {
    let mut sent = [(0, 0), (0, 0)];
    for event in events {
        match event {
            Event::Sent(_, process) => sent[*process].0 += 1,
            Event::Answered(_, process, _) => sent[*process].1 += 1,
            Event::Read(_) => {}
        }
    }
    sent
}

#[test]
fn across_two_processes_own_spend_crosses_the_floor_by_at_most_one_estimate_error_each() {
    // 5,000 points, the shipped 20% floor (1,000), 1,050 left, others spending nothing, no
    // seeded peak; the reserve factor 0 so the line is the floor alone. Every request is
    // hinted 16 and costs 17, so a process admitting without counting the other's request in
    // flight would cross the floor by that request's whole estimate.
    let state = tempfile::tempdir().unwrap();
    let settings = GovernorSettings {
        reserve_factor: 0.0,
        min_peak_per_minute: BTreeMap::new(),
        ..sharing(state.path())
    };
    let (backend, reports) = two_update_manies(&settings, 1050, None);

    assert_eq!(*backend.most_in_flight.lock().unwrap(), [1, 1]);
    let events = backend.events();
    assert_eq!(
        sent(&events),
        [(100, 100), (100, 100)],
        "both batches completed"
    );
    let lowest = events
        .iter()
        .filter_map(|event| match event {
            Event::Answered(at, _, left) if *at < instant(RESET) => Some(*left),
            _ => None,
        })
        .min()
        .unwrap();
    assert!(lowest < 1000, "the floor was reached: {events:?}");
    assert!(
        lowest >= 1000 - 2,
        "under the floor by at most one estimate error (1) per process: {lowest}"
    );
    let under = events
        .iter()
        .position(|event| matches!(event, Event::Answered(_, _, left) if *left < 1000))
        .unwrap();
    assert!(
        events[under..].iter().all(|event| match event {
            Event::Sent(at, _) => *at >= instant(RESET),
            _ => true,
        }),
        "nothing was sent after the first response under the floor until the reset's reading: {:?}",
        &events[under..]
    );
    for report in &reports {
        assert!(
            report.waits.iter().any(|wait| wait.reason == Reason::Floor),
            "{report:?}"
        );
    }
}

#[test]
fn another_consumer_taking_remaining_under_the_reserve_stops_both_processes_until_a_reading_admits()
{
    // The shipped reserve factor with a 5-point-a-minute seeded peak: about 297 points at the
    // spike. Another consumer drops remaining to 1,100 at 30 s, which the next response, to
    // whichever process sent first, reports: above the floor, under the floor plus reserve.
    let state = tempfile::tempdir().unwrap();
    let settings = GovernorSettings {
        min_peak_per_minute: BTreeMap::from([("graphql".to_owned(), 5.0)]),
        ..sharing(state.path())
    };
    let (backend, reports) = two_update_manies(&settings, 4900, Some((T0 + 30, 1100)));

    let events = backend.events();
    assert_eq!(
        sent(&events),
        [(100, 100), (100, 100)],
        "both batches completed"
    );
    let reported = events
        .iter()
        .position(|event| matches!(event, Event::Answered(_, _, left) if *left <= 1100))
        .unwrap();
    let Event::Answered(reported_at, _, left) = events[reported] else {
        unreachable!()
    };
    let reserve = 5.0 / 60.0 * (RESET - reported_at.timestamp()) as f64;
    assert!(
        left > 1000 && (left as f64) < 1000.0 + reserve,
        "between the floor and the floor plus the reserve: {left}, reserve {reserve}"
    );
    // After that response, the next request either process sends goes at a fresh reading
    // that admits it, a held refresh or more after.
    let next = events[reported..]
        .iter()
        .position(|event| matches!(event, Event::Sent(..)))
        .map(|at| reported + at)
        .unwrap();
    let Event::Sent(next_at, _) = events[next] else {
        unreachable!()
    };
    assert!(
        next_at >= reported_at + chrono::Duration::seconds(30),
        "held past a refresh: {next_at}"
    );
    assert!(
        events[reported..next]
            .iter()
            .any(|event| matches!(event, Event::Read(at) if *at == next_at)),
        "the request went at a fresh reading: {:?}",
        &events[reported..=next]
    );
    assert!(
        reports.iter().all(|report| report
            .waits
            .iter()
            .any(|wait| wait.reason == Reason::Reserve)),
        "both processes held on the reserve: {reports:?}"
    );
}

#[test]
fn a_fresh_process_starts_from_the_burn_others_spent_as_the_host_state_recorded_it() {
    // A first process reads others spend 300 GraphQL points in 5 minutes, a 60-a-minute peak.
    // A fresh process holds on the reserve that recorded burn sizes; with the state corrupted
    // it has no history and, with no seeded peak, goes at once.
    for corrupt in [false, true] {
        let state = tempfile::tempdir().unwrap();
        let settings = GovernorSettings {
            min_peak_per_minute: BTreeMap::new(),
            ..sharing(state.path())
        };
        let clock = Simulated::at(instant(T0));
        let _taking_part = clock.join();
        let remaining = Arc::new(Mutex::new(4000_u64));
        let script = Arc::clone(&remaining);
        let readings = Arc::new(Scripted(Box::new(move || {
            vec![
                reading("graphql", *script.lock().unwrap(), T0 + 3600),
                reading("core", 5000, T0 + 3600),
            ]
        })));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let ask = |governor: &Governor, resource: &str| {
            runtime.block_on(async {
                governor
                    .admit_call(&["followups".to_owned()], Kind::Read)
                    .await;
                governor
                    .before_request(&Request {
                        resource: resource.to_owned(),
                        cost_hint: Some(1),
                        content_creating: false,
                    })
                    .await;
                governor.after_response(&[]);
                governor.request_ended();
                governor.end_call();
            });
        };
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&notices);
        let first = Governor::new(
            settings.clone(),
            clock.clone(),
            readings.clone(),
            Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_owned())),
        );
        // The first process spends on core alone; each of its reads sees GraphQL fall.
        ask(&first, "core");
        clock.advance(300);
        *remaining.lock().unwrap() = 3700;
        ask(&first, "core");
        drop(first);
        let file = state.path().join("followups.json");
        assert!(file.is_file(), "the state is the host's: {file:?}");
        if corrupt {
            std::fs::write(&file, "{ not json").unwrap();
        }
        // The fresh process, on the same 3,700 left: the floor plus the reserve the recorded
        // burn sizes (60/min × 3,299 s) is 4,299, which holds its GraphQL request.
        clock.advance(1);
        let sink = Arc::clone(&notices);
        let fresh = Governor::new(
            settings.clone(),
            clock.clone(),
            readings.clone(),
            Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_owned())),
        );
        let before = notices.lock().unwrap().len();
        let at = clock.now();
        ask(&fresh, "graphql");
        let report = fresh.report().unwrap();
        if corrupt {
            assert_eq!(report.waits, Vec::new(), "no history: nothing held it");
            assert_eq!(clock.now(), at);
        } else {
            assert_eq!(report.waits.len(), 1, "{report:?}");
            assert_eq!(report.waits[0].reason, Reason::Reserve);
            let line = notices.lock().unwrap()[before].clone();
            assert!(line.contains("60.0/min peak"), "{line}");
        }
    }
}

/// GitHub's rate-limit state as whatever the case's closure says now.
struct Scripted(Box<dyn Fn() -> Vec<Reading> + Send + Sync>);

impl Readings for Scripted {
    fn read<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        let readings = (self.0)();
        Box::pin(async move { Ok(readings) })
    }
}
