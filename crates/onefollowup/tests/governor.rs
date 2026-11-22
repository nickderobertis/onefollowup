//! The quota governor on a virtual clock, GitHub's rate-limit state the only thing doubled.
//!
//! The doubled reading replays `spike-quota-burn`'s two real traces (`fixtures/quota-burn/`,
//! committed as that spike's branch holds them) or a script stated in the case. Cases drive
//! the governor through its two seams — the call boundary and the per-request gate — as the
//! store does; `tests/governor_store.rs` drives the store's real chokepoint over local-md.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use onefollowup::config::GovernorSettings;
use onefollowup::governor::{Clock, Governor, Kind, Pending, Reading, Readings, Reason, Request};

const DESTINATION: &str = "followups";

/// A clock that moves only when told to or waited on: a wait passes at once, its length added.
struct Virtual {
    now: Mutex<DateTime<Utc>>,
    waited: Mutex<Option<DateTime<Utc>>>,
    /// Past this a wait fails the case rather than running on.
    limit: Mutex<Option<DateTime<Utc>>>,
}

impl Virtual {
    fn at(start: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(start),
            waited: Mutex::new(None),
            limit: Mutex::new(Some(start + chrono::Duration::days(2))),
        })
    }

    fn advance(&self, seconds: f64) {
        let mut now = self.now.lock().unwrap();
        *now += chrono::Duration::milliseconds((seconds * 1000.0).round() as i64);
    }
}

impl Clock for Virtual {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap()
    }

    fn sleep(&self, duration: Duration) -> Pending<'_, ()> {
        self.advance(duration.as_secs_f64());
        let now = self.now();
        assert!(
            self.waited
                .lock()
                .unwrap()
                .replace(now)
                .is_none_or(|last| now > last),
            "a wait moves the clock"
        );
        if let Some(limit) = *self.limit.lock().unwrap() {
            assert!(now < limit, "the case waited past {limit}");
        }
        Box::pin(async {})
    }
}

type Script = dyn Fn(DateTime<Utc>) -> Vec<Reading> + Send + Sync;

/// GitHub's rate-limit state as a script of virtual time, recording when it was read.
struct Scripted {
    clock: Arc<Virtual>,
    script: Box<Script>,
    reads: Mutex<Vec<DateTime<Utc>>>,
}

impl Readings for Scripted {
    fn read<'a>(&'a self, source: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        assert_eq!(source, DESTINATION, "only the destination is read");
        let now = self.clock.now();
        self.reads.lock().unwrap().push(now);
        let readings = (self.script)(now);
        Box::pin(async move { Ok(readings) })
    }
}

fn instant(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).unwrap()
}

/// 2026-11-22T00:00:00Z: the start of every scripted case.
const T0: i64 = 1_795_305_600;

fn reading(resource: &str, remaining: u64, resets_at: i64) -> Reading {
    Reading {
        resource: resource.to_owned(),
        limit: 5000,
        remaining,
        resets_at: instant(resets_at),
    }
}

/// The shipped defaults, read from the shipped file.
fn shipped() -> GovernorSettings {
    onefollowup::config::defaults("test-host").governor
}

struct Harness {
    clock: Arc<Virtual>,
    readings: Arc<Scripted>,
    notices: Arc<Mutex<Vec<String>>>,
    governor: Governor,
    runtime: tokio::runtime::Runtime,
}

impl Harness {
    fn new(
        settings: GovernorSettings,
        start: DateTime<Utc>,
        script: impl Fn(DateTime<Utc>) -> Vec<Reading> + Send + Sync + 'static,
    ) -> Self {
        let clock = Virtual::at(start);
        let readings = Arc::new(Scripted {
            clock: Arc::clone(&clock),
            script: Box::new(script),
            reads: Mutex::new(Vec::new()),
        });
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&notices);
        let governor = Governor::new(
            settings,
            clock.clone(),
            readings.clone(),
            Arc::new(move |line: &str| {
                eprintln!("{line}");
                sink.lock().unwrap().push(line.to_owned());
            }),
        );
        Self {
            clock,
            readings,
            notices,
            governor,
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        }
    }

    /// One destination call of `kind`, admitted at the boundary, then one request on
    /// `resource` costing `hint`: the virtual seconds admission took.
    fn request(&self, kind: Kind, resource: &str, hint: Option<u64>) -> f64 {
        let before = self.clock.now();
        self.runtime.block_on(async {
            self.governor
                .admit_call(&[DESTINATION.to_owned()], kind)
                .await;
            self.governor
                .before_request(&Request {
                    resource: resource.to_owned(),
                    cost_hint: hint,
                })
                .await;
        });
        (self.clock.now() - before).num_milliseconds() as f64 / 1000.0
    }

    fn respond(&self, readings: &[Reading]) {
        self.governor.after_response(readings);
    }

    fn end(&self) {
        self.governor.end_call();
    }

    fn waits(&self) -> Vec<onefollowup::governor::Wait> {
        self.governor
            .report()
            .map(|report| report.waits)
            .unwrap_or_default()
    }

    fn notices(&self) -> Vec<String> {
        self.notices.lock().unwrap().clone()
    }

    fn elapsed(&self, from: i64) -> f64 {
        (self.clock.now() - instant(from)).num_milliseconds() as f64 / 1000.0
    }
}

/// One reading of a committed trace, as `spike-quota-burn`'s `sample.py` and
/// `convert_apply_logs.py` wrote it.
#[derive(Debug, Clone)]
struct Row {
    t: f64,
    value: serde_json::Value,
}

impl Row {
    fn window(&self, resource: &str) -> (f64, f64, f64) {
        let window = &self.value[resource];
        (
            window["limit"].as_f64().unwrap(),
            window["remaining"].as_f64().unwrap(),
            window["reset"].as_f64().unwrap(),
        )
    }

    fn own_writes(&self) -> f64 {
        self.value["own_writes"].as_f64().unwrap_or(0.0)
    }
}

fn trace(name: &str) -> Vec<Row> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/quota-burn/{name}"));
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            Row {
                t: value["t"].as_f64().unwrap(),
                value,
            }
        })
        .collect()
}

const TRACES: [&str; 2] = [
    "2026-11-21-sample-60m.jsonl",
    "2026-11-20-grooming-pass.jsonl",
];

/// Others' spend of one resource over a trace, derived as the spike's `replay.py` derives it:
/// readings marked excluded, or another window's (a reset both agreeing neighbours disagree
/// with), left out; others' spend over an interval the fall in remaining less the pass's own
/// recorded spend (its writes at the trace's smallest fall per write where it recorded none);
/// across a reset, what was spent after it plus the interval before at the last rate.
struct Others {
    limit: f64,
    start: f64,
    remaining: f64,
    /// Others' cumulative spend at each kept reading.
    spent: Vec<(f64, f64)>,
    resets: Vec<f64>,
}

impl Others {
    fn of(rows: &[Row], resource: &str) -> Self {
        let foreign = |k: usize| {
            k > 0
                && k + 1 < rows.len()
                && ["graphql", "core"].iter().any(|r| {
                    let (before, here, after) = (
                        rows[k - 1].window(r).2,
                        rows[k].window(r).2,
                        rows[k + 1].window(r).2,
                    );
                    before == after && after != here
                })
        };
        let mut kept: Vec<Row> = Vec::new();
        let mut carried = 0.0;
        for (k, row) in rows.iter().enumerate() {
            if row.value.get("excluded").is_some() || foreign(k) {
                carried += row.own_writes();
                continue;
            }
            let mut row = row.clone();
            row.value["own_writes"] = (row.own_writes() + carried).into();
            carried = 0.0;
            kept.push(row);
        }
        let per_write = kept
            .windows(2)
            .filter_map(|pair| {
                let (a, b) = (pair[0].window(resource), pair[1].window(resource));
                let writes = pair[1].own_writes();
                (writes > 0.0 && a.1 >= b.1 && a.2 == b.2).then(|| (a.1 - b.1) / writes)
            })
            .fold(None, |least: Option<f64>, cost| {
                Some(least.map_or(cost, |l| l.min(cost)))
            })
            .unwrap_or(0.0);
        let (limit, remaining, first_reset) = kept[0].window(resource);
        let mut spent = vec![(kept[0].t, 0.0)];
        let mut resets = Vec::new();
        let mut last_rate = 0.0;
        for pair in kept.windows(2) {
            let (t0, t1) = (pair[0].t, pair[1].t);
            let (a, b) = (pair[0].window(resource), pair[1].window(resource));
            let own = pair[1].value["own"][resource]
                .as_f64()
                .unwrap_or_else(|| pair[1].own_writes() * per_write);
            let reset = b.1 > a.1 || (a.2 != b.2 && a.2 <= t1);
            let spend = if reset {
                let tau = if t0 < a.2 && a.2 <= t1 {
                    a.2
                } else {
                    (t0 + t1) / 2.0
                };
                resets.push(tau);
                let after = (t1 - tau) / (t1 - t0);
                (limit - b.1 - own * after).max(0.0) + last_rate * (tau - t0)
            } else {
                (a.1 - b.1 - own).max(0.0)
            };
            last_rate = spend / (t1 - t0);
            let total = spent.last().unwrap().1 + spend;
            spent.push((t1, total));
        }
        if first_reset > kept[0].t && resets.first().is_none_or(|first| first_reset < *first) {
            resets.insert(0, first_reset);
        }
        Self {
            limit,
            start: kept[0].t,
            remaining,
            spent,
            resets,
        }
    }

    /// Others' cumulative spend at `t`, linear between readings, repeating past the end.
    fn at(&self, t: f64) -> f64 {
        let (end, total) = *self.spent.last().unwrap();
        let span = end - self.start;
        let repeat = ((t - self.start) / span).floor();
        let t = t - repeat * span;
        let i = self
            .spent
            .windows(2)
            .position(|pair| pair[0].0 <= t && t <= pair[1].0)
            .unwrap_or(self.spent.len() - 2);
        let ((t0, c0), (t1, c1)) = (self.spent[i], self.spent[i + 1]);
        repeat * total + c0 + (t - t0) / (t1 - t0) * (c1 - c0)
    }

    /// The window `t` falls in: the reset before it, if any, and the one after it — the
    /// trace's own resets, then hourly after the last.
    fn window(&self, t: f64) -> (Option<f64>, f64) {
        let mut resets = self.resets.clone();
        while resets.last().is_none_or(|last| *last <= t) {
            let next = resets
                .last()
                .map_or(self.start + 3600.0, |last| last + 3600.0);
            resets.push(next);
        }
        let next = resets.iter().copied().find(|reset| *reset > t).unwrap();
        let previous = resets.iter().copied().rfind(|reset| *reset <= t);
        (previous, next)
    }
}

/// A trace as GitHub's rate-limit state: at a virtual instant, each resource's window as
/// others' spend in the trace leaves it, less what this pass has spent of that window.
struct Replay {
    start: f64,
    others: BTreeMap<String, Others>,
    own: Mutex<BTreeMap<(String, i64), f64>>,
}

impl Replay {
    fn of(name: &str) -> Self {
        let rows = trace(name);
        Self {
            start: rows[0].t,
            others: ["graphql", "core"]
                .into_iter()
                .map(|resource| (resource.to_owned(), Others::of(&rows, resource)))
                .collect(),
            own: Mutex::new(BTreeMap::new()),
        }
    }

    fn start(&self) -> DateTime<Utc> {
        instant(self.start.ceil() as i64)
    }

    fn readings(&self, at: DateTime<Utc>) -> Vec<Reading> {
        let t = at.timestamp() as f64;
        let own = self.own.lock().unwrap();
        self.others
            .iter()
            .map(|(resource, others)| {
                let (previous, next) = others.window(t);
                let left = match previous {
                    Some(reset) => others.limit - (others.at(t) - others.at(reset)),
                    None => others.remaining - (others.at(t) - others.at(others.start)),
                };
                let mine = own
                    .get(&(resource.clone(), next as i64))
                    .copied()
                    .unwrap_or(0.0);
                Reading {
                    resource: resource.clone(),
                    limit: others.limit as u64,
                    remaining: (left - mine).max(0.0).floor() as u64,
                    resets_at: instant(next as i64),
                }
            })
            .collect()
    }

    fn spend(&self, resource: &str, points: u64, at: DateTime<Utc>) {
        let (_, next) = self.others[resource].window(at.timestamp() as f64);
        *self
            .own
            .lock()
            .unwrap()
            .entry((resource.to_owned(), next as i64))
            .or_default() += points as f64;
    }
}

/// A pass of `points` on `resource`, spent at 25 points a minute in 12.5-point batches (12
/// and 13 alternately, a request's hint being whole points), every one an admitted call.
fn pass(trace_name: &str, resource: &str, points: u64) -> (Harness, f64) {
    let replay = Arc::new(Replay::of(trace_name));
    let start = replay.start();
    let script = Arc::clone(&replay);
    let harness = Harness::new(shipped(), start, move |at| script.readings(at));
    // Any wait fails the case, at its first pause rather than after it.
    *harness.clock.limit.lock().unwrap() = Some(start);
    let mut spent = 0;
    let mut admission = 0.0;
    let mut batch = 0;
    while spent < points {
        let cost = (if batch % 2 == 0 { 12 } else { 13 }).min(points - spent);
        let waited = harness.request(Kind::Write, resource, Some(cost));
        assert_eq!(
            waited,
            0.0,
            "{trace_name} {resource} {points}: batch {batch} waited: {:?}",
            harness.notices()
        );
        admission += waited;
        replay.spend(resource, cost, harness.clock.now());
        harness.end();
        harness.clock.advance(30.0);
        spent += cost;
        batch += 1;
    }
    (harness, admission)
}

#[test]
fn with_quota_ample_both_traces_admit_every_request_of_a_full_and_a_weekly_pass_without_waiting() {
    for trace_name in TRACES {
        for resource in ["graphql", "core"] {
            for points in [1500, 150] {
                let (harness, admission) = pass(trace_name, resource, points);
                assert!(
                    harness.waits().is_empty(),
                    "{trace_name} {resource} {points}: {:?} {:?}",
                    harness.waits(),
                    harness.notices()
                );
                assert_eq!(
                    admission, 0.0,
                    "{trace_name} {resource} {points}: no virtual time passes in admission"
                );
                assert!(harness.notices().is_empty());
                assert_eq!(
                    harness.governor.report().map(|report| report.waits.len()),
                    Some(0),
                    "the governor object is reported, with no wait"
                );
                assert!(
                    !harness.readings.reads.lock().unwrap().is_empty(),
                    "the doubled reading was read"
                );
            }
        }
    }
}

#[test]
fn when_the_reserve_is_not_covered_a_request_waits_says_so_and_resumes_at_the_reading_that_admits_it()
 {
    // Floor 1000; seeded GraphQL peak 40/min; 600 s to the reset: reserve 400. Remaining 1300
    // less a request of 1 leaves 299 over the floor, which the reserve covers once
    // 40/60 * to_reset <= 299, 448.5 s before the reset: 151.5 s in, so the first held refresh
    // that admits it is at 180 s.
    let harness = Harness::new(shipped(), instant(T0), |_| {
        vec![
            reading("graphql", 1300, T0 + 600),
            reading("core", 5000, T0 + 3600),
        ]
    });
    let waited = harness.request(Kind::Read, "graphql", Some(1));
    assert_eq!(waited, 180.0);
    let waits = harness.waits();
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert_eq!(waits[0].resource, "graphql");
    assert_eq!(waits[0].reason, Reason::Reserve);
    assert_eq!(waits[0].seconds, 180);
    let notices = harness.notices();
    assert_eq!(notices.len(), 1, "one line per wait: {notices:?}");
    assert_eq!(
        notices[0],
        "onefollowup: holding requests to followups on graphql: remaining 1300 of 5000, floor 1000, reserve 400; others burn 0.0/min mean, 40.0/min peak; resuming by 2026-11-22T00:02:31Z"
    );
    // It read at the start, before refusing, then every 30 s while held: the admitting
    // reading is the last.
    let reads = harness.readings.reads.lock().unwrap().clone();
    assert_eq!(reads.last().copied(), Some(instant(T0 + 180)));
    assert_eq!(
        reads.iter().filter(|read| **read > instant(T0)).count(),
        6,
        "{reads:?}"
    );
}

#[test]
fn a_spike_in_others_spend_under_the_floor_stops_every_request_until_a_reading_shows_recovery() {
    // 1100 left, resetting at 120 s: a request goes. By the next reading others have taken it
    // to 900, under the floor; the window resets at 120 s and the new one is full.
    let harness = Harness::new(shipped(), instant(T0), |at| {
        let elapsed = at.timestamp() - T0;
        let graphql = match elapsed {
            ..60 => reading("graphql", 1100, T0 + 120),
            60..120 => reading("graphql", 900, T0 + 120),
            _ => reading("graphql", 5000, T0 + 3720),
        };
        vec![graphql, reading("core", 5000, T0 + 3600)]
    });
    assert_eq!(harness.request(Kind::Write, "graphql", Some(1)), 0.0);
    harness.end();
    harness.clock.advance(60.0);
    let waited = harness.request(Kind::Write, "graphql", Some(1));
    assert_eq!(
        waited,
        60.0,
        "held until the reading after the reset: {:?} {:?}",
        harness.notices(),
        harness.waits()
    );
    let waits = harness.waits();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].reason, Reason::Floor);
    let reads = harness.readings.reads.lock().unwrap().clone();
    assert_eq!(
        reads.last().copied(),
        Some(instant(T0 + 120)),
        "admitted by the recovering reading and nothing before it: {reads:?}"
    );
    assert!(harness.notices()[0].contains("remaining 900 of 5000, floor 1000"));
}

#[test]
fn the_reserve_is_sized_from_others_measured_burn_and_the_first_reading_showing_it_breached_stops_admission()
 {
    // Others burn 100 points a minute — over the seeded 40 — for 300 s, then 1500 in the next
    // 30 s. The window resets at 1200 s.
    let reset = T0 + 1200;
    let harness = Harness::new(shipped(), instant(T0), move |at| {
        let elapsed = (at.timestamp() - T0) as u64;
        let others = if elapsed <= 300 {
            elapsed * 100 / 60
        } else {
            2000
        };
        vec![
            reading("graphql", 4900 - others, reset),
            reading("core", 5000, T0 + 3600),
        ]
    });
    for _ in 0..=10 {
        assert_eq!(
            harness.request(Kind::Read, "graphql", Some(1)),
            0.0,
            "ample at the measured burn"
        );
        harness.end();
        harness.clock.advance(30.0);
    }
    assert!(harness.waits().is_empty());
    // 330 s: the first fresh reading shows 2900 left. At the seeded 40/min the reserve over the
    // 870 s left would be 580, which 2900 covers; at the peak just measured it is not covered.
    let waited = harness.request(Kind::Read, "graphql", Some(1));
    assert!(waited > 0.0, "admission stopped");
    let line = &harness.notices()[0];
    assert!(line.contains("remaining 2900 of 5000"), "{line}");
    let figure = |after: &str, before: &str| -> f64 {
        line.split(after)
            .nth(1)
            .and_then(|rest| rest.split(before).next())
            .unwrap()
            .parse()
            .unwrap()
    };
    let peak = figure("/min mean, ", "/min peak");
    let reserve = figure("reserve ", ";");
    assert!(peak > 100.0, "the measured peak, not the seeded 40: {line}");
    assert!(
        reserve > 2900.0 - 1000.0,
        "the measured reserve is what 2900 does not cover, unlike the seeded 580: {line}"
    );
}

#[test]
fn as_the_reset_nears_the_reserve_shrinks_and_spending_is_allowed_down_to_the_floor() {
    // 1010 remaining, floor 1000. With an hour to go the reserve (2400) holds a request; with
    // 3 s to go (reserve 2) every request is admitted until remaining reaches the floor plus
    // what is left of the reserve.
    let reset = T0 + 3600;
    let spent = Arc::new(Mutex::new(0_u64));
    let held = Arc::clone(&spent);
    let harness = Harness::new(shipped(), instant(reset - 3), move |_| {
        vec![
            reading("graphql", 1010 - *held.lock().unwrap(), reset),
            reading("core", 5000, reset),
        ]
    });
    let mut admitted = 0;
    loop {
        let waited = harness.request(Kind::Write, "graphql", Some(1));
        harness.end();
        if waited > 0.0 {
            break;
        }
        *spent.lock().unwrap() += 1;
        admitted += 1;
    }
    assert_eq!(
        admitted, 8,
        "1010 down to 1002: floor 1000 plus a reserve of 2"
    );
    // The same reading an hour from its reset admits nothing.
    let far = Harness::new(shipped(), instant(T0), move |_| {
        vec![
            reading("graphql", 1010, reset),
            reading("core", 5000, reset),
        ]
    });
    assert!(far.request(Kind::Write, "graphql", Some(1)) > 0.0);
    assert_eq!(far.waits()[0].reason, Reason::Reserve);
}

#[test]
fn two_windows_of_one_resource_answering_in_turn_admit_only_against_the_lower_until_its_reset() {
    // Window A (at the floor, resets at 120 s) and window B (4800 left, resets in an hour)
    // answer in turn, as GitHub was measured answering one token.
    let harness = Harness::new(shipped(), instant(T0), |at| {
        let turn = (at.timestamp() - T0) / 30;
        let graphql = if turn % 2 == 0 && at.timestamp() < T0 + 120 {
            reading("graphql", 1000, T0 + 120)
        } else {
            reading("graphql", 4800, T0 + 3600)
        };
        vec![graphql, reading("core", 5000, T0 + 3600)]
    });
    // At 0 s A answers; B's reading at 30 s, with far more left, admits nothing A refuses:
    // held until A resets at 120 s.
    let waited = harness.request(Kind::Read, "graphql", Some(1));
    assert_eq!(waited, 120.0);
    assert_eq!(harness.waits()[0].reason, Reason::Floor);
    // After A's reset only B is live, and admits at once.
    harness.end();
    assert_eq!(harness.request(Kind::Read, "graphql", Some(1)), 0.0);
}

#[test]
fn a_rate_limit_reading_of_another_window_with_more_left_admits_nothing_the_lower_window_refuses() {
    // The lower window comes from a response's headers; GET /rate_limit then reports another
    // window with more left, as it did 1 read in 121 and 1 in 23 on the traces.
    let harness = Harness::new(shipped(), instant(T0), |_| {
        vec![
            reading("graphql", 4900, T0 + 3000),
            reading("core", 5000, T0 + 3600),
        ]
    });
    assert_eq!(harness.request(Kind::Read, "graphql", Some(1)), 0.0);
    harness.respond(&[reading("graphql", 1050, T0 + 300)]);
    harness.end();
    let waited = harness.request(Kind::Read, "graphql", Some(1));
    assert!(
        waited >= 30.0,
        "held, though every refresh reads 4900: {waited}"
    );
    assert!(harness.notices()[0].contains("remaining 1050 of 5000"));
}

#[test]
fn a_remaining_that_rises_within_one_window_before_its_reset_is_ignored() {
    // 1100 left at 240 s to the reset: held (reserve 160, 1100 - 1 < 1160) until the reserve
    // shrinks enough, 148.5 s before the reset — the refresh at 120 s.
    let steady = Harness::new(shipped(), instant(T0), |_| {
        vec![
            reading("graphql", 1100, T0 + 240),
            reading("core", 5000, T0 + 3600),
        ]
    });
    assert_eq!(steady.request(Kind::Read, "graphql", Some(1)), 120.0);
    // The same window reading 4500 at every refresh after the first, before its reset: the
    // wait is the same, so the risen remaining admitted nothing.
    let reads = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&reads);
    let rising = Harness::new(shipped(), instant(T0), move |_| {
        let mut count = counted.lock().unwrap();
        *count += 1;
        let remaining = if *count == 1 { 1100 } else { 4500 };
        vec![
            reading("graphql", remaining, T0 + 240),
            reading("core", 5000, T0 + 3600),
        ]
    });
    assert_eq!(rising.request(Kind::Read, "graphql", Some(1)), 120.0);
    assert!(*reads.lock().unwrap() > 2, "it was read, risen, while held");
}

#[test]
fn graphql_and_core_reset_apart_and_each_is_admitted_on_its_own_window() {
    // Both ample when the call is admitted; then a GraphQL response shows its window at 1010
    // until its reset at 90 s, while core, resetting 150 s later, still has plenty.
    let harness = Harness::new(shipped(), instant(T0), |at| {
        let graphql = if at.timestamp() < T0 + 90 {
            reading("graphql", 4000, T0 + 90)
        } else {
            reading("graphql", 5000, T0 + 3690)
        };
        vec![graphql, reading("core", 4000, T0 + 240)]
    });
    let request = |resource: &str| {
        let before = harness.clock.now();
        harness
            .runtime
            .block_on(harness.governor.before_request(&Request {
                resource: resource.to_owned(),
                cost_hint: Some(1),
            }));
        (harness.clock.now() - before).num_seconds()
    };
    harness.runtime.block_on(
        harness
            .governor
            .admit_call(&[DESTINATION.to_owned()], Kind::Read),
    );
    assert_eq!(request("graphql"), 0);
    harness.respond(&[reading("graphql", 1010, T0 + 90)]);
    assert_eq!(request("core"), 0, "core goes on its own window");
    harness.respond(&[reading("core", 3999, T0 + 240)]);
    assert_eq!(request("graphql"), 90, "graphql waits for its own reset");
    assert_eq!(request("core"), 0);
    let waits = harness.waits();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].resource, "graphql");
}

#[test]
fn a_fresh_process_with_no_history_reserves_from_the_seeded_peaks() {
    // Seeded 40/min GraphQL and 60/min core over 900 s: reserves of 600 and 900, each process
    // holding on the one resource short of it.
    let graphql = Harness::new(shipped(), instant(T0), |_| {
        vec![
            reading("graphql", 1500, T0 + 900),
            reading("core", 5000, T0 + 3600),
        ]
    });
    assert!(graphql.request(Kind::Read, "graphql", Some(1)) > 0.0);
    assert_eq!(
        graphql.notices()[0],
        "onefollowup: holding requests to followups on graphql: remaining 1500 of 5000, floor 1000, reserve 600; others burn 0.0/min mean, 40.0/min peak; resuming by 2026-11-22T00:02:31Z"
    );
    let core = Harness::new(shipped(), instant(T0), |_| {
        vec![
            reading("graphql", 5000, T0 + 3600),
            reading("core", 1500, T0 + 900),
        ]
    });
    assert!(core.request(Kind::Read, "core", Some(1)) > 0.0);
    assert!(
        core.notices()[0].contains("on core: remaining 1500 of 5000, floor 1000, reserve 900; others burn 0.0/min mean, 60.0/min peak"),
        "{:?}",
        core.notices()
    );
}

#[test]
fn within_one_process_own_spend_crosses_the_floor_by_at_most_one_requests_estimate_error() {
    // Reserve off (factor 0) so the line is the floor alone; every response reports an actual
    // cost of 3 against a hint of 1.
    let mut settings = shipped();
    settings.reserve_factor = 0.0;
    let remaining = Arc::new(Mutex::new(1020_u64));
    let held = Arc::clone(&remaining);
    let harness = Harness::new(settings, instant(T0), move |at| {
        let left = if at.timestamp() >= T0 + 3600 {
            5000
        } else {
            *held.lock().unwrap()
        };
        let reset = if at.timestamp() >= T0 + 3600 {
            T0 + 7200
        } else {
            T0 + 3600
        };
        vec![
            reading("graphql", left, reset),
            reading("core", 5000, T0 + 3600),
        ]
    });
    let mut lowest = u64::MAX;
    let mut sent = 0;
    loop {
        let waited = harness.request(Kind::Write, "graphql", Some(1));
        if waited > 0.0 {
            break;
        }
        sent += 1;
        let mut left = remaining.lock().unwrap();
        *left -= 3;
        lowest = lowest.min(*left);
        harness.respond(&[reading("graphql", *left, T0 + 3600)]);
        harness.end();
    }
    // 1020 → 1017 … 1002 → 999: the last request admitted went at 1002 (1002 - 1 >= 1000) and
    // cost 3, so remaining fell to 999, under the floor by 1 = one request's estimate error
    // (3 - 1) at most; after that response, nothing went until the reset's reading.
    assert_eq!(sent, 7);
    assert_eq!(lowest, 999);
    assert!(1000 - lowest < 3, "under the floor by less than the actual cost");
    assert_eq!(harness.waits()[0].reason, Reason::Floor);
    assert!(harness.elapsed(T0) >= 3600.0);

    // Two requests in flight against one reading: the second is admitted only against what
    // the first's estimate leaves, so with headroom for one, the second waits.
    let concurrent = Harness::new(
        GovernorSettings {
            reserve_factor: 0.0,
            ..shipped()
        },
        instant(T0),
        |at| {
            let (left, reset) = if at.timestamp() >= T0 + 600 {
                (5000, T0 + 4200)
            } else {
                (1001, T0 + 600)
            };
            vec![
                reading("graphql", left, reset),
                reading("core", 5000, T0 + 3600),
            ]
        },
    );
    concurrent.runtime.block_on(async {
        concurrent
            .governor
            .admit_call(&[DESTINATION.to_owned()], Kind::Read)
            .await;
        let request = Request {
            resource: "graphql".to_owned(),
            cost_hint: Some(1),
        };
        let start = concurrent.clock.now();
        concurrent.governor.before_request(&request).await;
        assert_eq!(concurrent.clock.now(), start, "the first goes at once");
        // No response yet: the first is in flight.
        concurrent.governor.before_request(&request).await;
        assert_eq!(
            concurrent.clock.now(),
            instant(T0 + 600),
            "the second waited for a reading that admits it"
        );
    });
}
