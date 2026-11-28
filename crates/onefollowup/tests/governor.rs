//! The quota governor on a virtual clock, GitHub's rate-limit state the only thing doubled.
//!
//! The doubled reading is a script stated in the case. Cases drive the governor through its
//! two seams — the call boundary and the per-request gate — as the store does;
//! `tests/governor_store.rs` drives the store's real chokepoint over local-md, the replay of
//! `spike-quota-burn`'s two real traces included.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use onefollowup::config::GovernorSettings;
use onefollowup::governor::{Clock as _, Governor, Kind, Reading, Reason, Request};

mod support;
use support::{DESTINATION, Scripted, T0, Virtual, instant, reading, shipped};

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
        let readings = Scripted::new(&clock, script);
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
                    content_creating: false,
                })
                .await;
        });
        (self.clock.now() - before).num_milliseconds() as f64 / 1000.0
    }

    /// The request in flight was answered with `readings`, and is over: as a source tells the
    /// gate, its response first, then its end.
    fn respond(&self, readings: &[Reading]) {
        self.governor.after_response(readings);
        self.governor.request_ended();
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
        "onefollowup: holding requests to followups on graphql: remaining 1300 of 5000, floor 1000, reserve 400; this request 1.0 of the window resetting 2026-11-22T00:10:00Z; others burn 0.0/min mean, 40.0/min peak; resuming by 2026-11-22T00:02:31Z"
    );
    // It read at the start, before refusing, then every 30 s while held: the admitting
    // reading is the last.
    let reads = harness.readings.reads();
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
    let reads = harness.readings.reads();
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
fn two_windows_of_one_resource_answering_in_turn_each_decide_only_while_they_answer() {
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
    // At 0 s A answers, at the floor: held. B answers the refresh at 30 s with far above floor
    // plus reserve, and it is B's reading that decides: the request goes then, A still live.
    let waited = harness.request(Kind::Read, "graphql", Some(1));
    assert_eq!(waited, 30.0);
    assert_eq!(harness.waits()[0].reason, Reason::Floor);
    assert!(
        harness.notices()[0].contains("remaining 1000 of 5000")
            && harness.notices()[0].contains("of the window resetting 2026-11-22T00:02:00Z"),
        "{:?}",
        harness.notices()
    );
    // B answered last, so the next request goes at once, whatever A last read.
    harness.respond(&[reading("graphql", 4799, T0 + 3600)]);
    harness.end();
    assert_eq!(harness.request(Kind::Read, "graphql", Some(1)), 0.0);
    assert_eq!(harness.waits().len(), 1);
}

#[test]
fn a_rate_limit_reading_of_another_window_with_more_left_releases_what_the_lower_window_held() {
    // The lower window comes from a response's headers; GET /rate_limit then reports another
    // window with more left, as it did 1 read in 121 and 1 in 23 on the traces. The lower
    // window holds the request while it is the one that answered last; the refresh that
    // answers from the other window releases it, and no reading of one decides the other.
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
    assert_eq!(waited, 30.0, "held on 1050 until the refresh read 4900");
    assert!(harness.notices()[0].contains("remaining 1050 of 5000"));
}

#[test]
fn a_request_goes_at_once_while_the_window_that_answered_admits_it_whatever_the_other_reads() {
    // A low window (1010 left: under floor plus any reserve) and a high one (4800) both live;
    // whichever answered last decides, and only that one.
    let harness = Harness::new(shipped(), instant(T0), |_| {
        vec![
            reading("graphql", 4800, T0 + 3000),
            reading("core", 5000, T0 + 3600),
        ]
    });
    assert_eq!(harness.request(Kind::Read, "graphql", Some(1)), 0.0);
    harness.respond(&[reading("graphql", 1010, T0 + 600)]);
    assert_eq!(
        harness.request(Kind::Read, "graphql", Some(1)),
        30.0,
        "the low window answered last"
    );
    harness.respond(&[reading("graphql", 4790, T0 + 3000)]);
    for _ in 0..5 {
        assert_eq!(
            harness.request(Kind::Read, "graphql", Some(3)),
            0.0,
            "the high window answered last, the low one still live at 1010"
        );
        harness.respond(&[reading("graphql", 4780, T0 + 3000)]);
    }
    harness.end();
    assert_eq!(harness.waits().len(), 1, "{:?}", harness.waits());
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
fn recovery_before_the_reset_is_a_reading_above_the_own_estimate_and_never_above_the_last_reading()
{
    // Reserve off, floor 1000. 1002 read at the first call; a request of 1 is admitted and its
    // response reports nothing, so the governor's own estimate is 1001. A request of 2 would
    // take that to 999: held. Each case's refreshes then read the same window, before its reset.
    let mut settings = shipped();
    settings.reserve_factor = 0.0;
    let held = |refreshed: u64| {
        let reads = Arc::new(Mutex::new(0));
        let counted = Arc::clone(&reads);
        let harness = Harness::new(settings.clone(), instant(T0), move |_| {
            let mut count = counted.lock().unwrap();
            *count += 1;
            let remaining = if *count == 1 { 1002 } else { refreshed };
            vec![
                reading("graphql", remaining, T0 + 600),
                reading("core", 5000, T0 + 3600),
            ]
        });
        assert_eq!(harness.request(Kind::Read, "graphql", Some(1)), 0.0);
        harness.end();
        let waited = harness.request(Kind::Read, "graphql", Some(2));
        (waited, harness.notices())
    };
    // Read again at 1002, the last reading: above the estimate, so the spend it showed was
    // less than estimated — recovery, admitted at the first refresh while held.
    let (waited, notices) = held(1002);
    assert_eq!(waited, 30.0);
    assert!(
        notices[0].contains("remaining 1001 of 5000, floor 1000"),
        "{notices:?}"
    );
    // Read again at 4500, above the last reading before the reset: another window's figures,
    // ignored, so the request waits out the reset.
    let (waited, _) = held(4500);
    assert_eq!(waited, 600.0, "held to the reset");
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
                content_creating: false,
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
    harness.respond(&[reading("graphql", 4999, T0 + 3690)]);
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
        "onefollowup: holding requests to followups on graphql: remaining 1500 of 5000, floor 1000, reserve 600; this request 1.0 of the window resetting 2026-11-22T00:15:00Z; others burn 0.0/min mean, 40.0/min peak; resuming by 2026-11-22T00:02:31Z"
    );
    let core = Harness::new(shipped(), instant(T0), |_| {
        vec![
            reading("graphql", 5000, T0 + 3600),
            reading("core", 1500, T0 + 900),
        ]
    });
    assert!(core.request(Kind::Read, "core", Some(1)) > 0.0);
    assert!(
        core.notices()[0].contains("on core: remaining 1500 of 5000, floor 1000, reserve 900; this request 1.0 of the window resetting 2026-11-22T00:15:00Z; others burn 0.0/min mean, 60.0/min peak"),
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
    assert!(
        1000 - lowest < 3,
        "under the floor by less than the actual cost"
    );
    assert_eq!(harness.waits()[0].reason, Reason::Floor);
    assert!(harness.elapsed(T0) >= 3600.0);
}

#[test]
fn a_local_write_neither_takes_nor_waits_for_the_slot_yet_is_admitted_against_its_reading() {
    // A GraphQL request is in flight, its response not yet told, so it holds the slot. A write
    // a source makes to its own files (`local`) goes meanwhile, without waiting for it; and a
    // `local` window under its floor still holds such a write until a reading admits it.
    let mut settings = shipped();
    settings.reserve_factor = 0.0;
    let harness = Harness::new(settings, instant(T0), |at| {
        let local = if at.timestamp() >= T0 + 60 {
            reading("local", 5000, T0 + 3660)
        } else {
            reading("local", 1000, T0 + 60)
        };
        vec![
            reading("graphql", 5000, T0 + 3600),
            reading("core", 5000, T0 + 3600),
            local,
        ]
    });
    let local = Request {
        resource: "local".to_owned(),
        cost_hint: None,
        content_creating: false,
    };
    let admitted = harness.runtime.block_on(async {
        harness
            .governor
            .admit_call(&[DESTINATION.to_owned()], Kind::Write)
            .await;
        harness
            .governor
            .before_request(&Request {
                resource: "graphql".to_owned(),
                cost_hint: Some(1),
                content_creating: false,
            })
            .await;
        // Real time, not the virtual clock: a write waiting on the slot would never return.
        tokio::time::timeout(
            Duration::from_secs(5),
            harness.governor.before_request(&local),
        )
        .await
    });
    assert!(
        admitted.is_ok(),
        "the local write waited for the slot the GraphQL request holds"
    );
    assert_eq!(
        harness.elapsed(T0),
        60.0,
        "held on its own window's floor until the reading at its reset"
    );
    let waits = harness.waits();
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert_eq!(waits[0].resource, "local");
    assert_eq!(waits[0].reason, Reason::Floor);
    harness.end();
}

#[test]
fn two_requests_sent_at_once_are_never_in_flight_together_and_the_second_waits_for_the_first_s_reading()
 {
    // A GitHub source sends some requests concurrently (a visibility check joins two): the
    // second is not admitted until the first's response is told, and is then admitted against
    // what that response said. The first costs 3 against a hint of 1 and leaves graphql under
    // its floor, so the second waits for the reset's reading.
    let mut settings = shipped();
    settings.reserve_factor = 0.0;
    let harness = Harness::new(settings, instant(T0), |at| {
        let graphql = if at.timestamp() >= T0 + 600 {
            reading("graphql", 5000, T0 + 4200)
        } else {
            reading("graphql", 1001, T0 + 600)
        };
        vec![graphql, reading("core", 5000, T0 + 3600)]
    });
    let log = Mutex::new(Vec::<String>::new());
    let in_flight = Mutex::new(0_u32);
    let most = Mutex::new(0_u32);
    let (log_at, in_flight_at, most_at, governor, clock) =
        (&log, &in_flight, &most, &harness.governor, &harness.clock);
    let send = |name: &'static str, answer: Reading| {
        let (log, in_flight, most) = (log_at, in_flight_at, most_at);
        async move {
            governor
                .before_request(&Request {
                    resource: "graphql".to_owned(),
                    cost_hint: Some(1),
                    content_creating: false,
                })
                .await;
            let now = {
                let mut count = in_flight.lock().unwrap();
                *count += 1;
                let mut top = most.lock().unwrap();
                *top = (*top).max(*count);
                clock.now().timestamp() - T0
            };
            log.lock().unwrap().push(format!("{name} sent at {now}"));
            // The network: the other request is polled meanwhile.
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            *in_flight.lock().unwrap() -= 1;
            log.lock().unwrap().push(format!("{name} answered"));
            governor.after_response(&[answer]);
            governor.request_ended();
        }
    };
    harness.runtime.block_on(async {
        harness
            .governor
            .admit_call(&[DESTINATION.to_owned()], Kind::Read)
            .await;
        tokio::join!(
            send("first", reading("graphql", 998, T0 + 600)),
            send("second", reading("graphql", 4999, T0 + 4200)),
        );
    });
    harness.end();
    assert_eq!(*most.lock().unwrap(), 1, "never two in flight");
    assert_eq!(
        *log.lock().unwrap(),
        [
            "first sent at 0",
            "first answered",
            "second sent at 600",
            "second answered"
        ],
        "the second went only once a reading after the first's response admitted it"
    );
    let waits = harness.waits();
    assert_eq!(waits.len(), 1, "{waits:?}");
    assert_eq!(waits[0].reason, Reason::Floor);
}

#[test]
fn a_request_open_past_30_seconds_keeps_the_next_unsent_until_it_ends_then_admits_on_its_reading() {
    // The first request is still in flight 45 s after it was sent, by the governor's clock and
    // the runtime's timer alike (paused here, so the case costs no real time). Nothing but its
    // end may let the second go: no time bound presumes it over. Its response then shows
    // graphql at 998, under the floor of 1000, and the second is admitted on that reading —
    // held until the reset's reading admits it.
    let mut settings = shipped();
    settings.reserve_factor = 0.0;
    let answered = Arc::new(Mutex::new(false));
    let spent = Arc::clone(&answered);
    let harness = Harness::new(settings, instant(T0), move |at| {
        let graphql = if at.timestamp() >= T0 + 600 {
            reading("graphql", 5000, T0 + 4200)
        } else if *spent.lock().unwrap() {
            reading("graphql", 998, T0 + 600)
        } else {
            reading("graphql", 1001, T0 + 600)
        };
        vec![graphql, reading("core", 5000, T0 + 3600)]
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    let log = Mutex::new(Vec::<String>::new());
    let (log_at, governor, clock) = (&log, &harness.governor, &harness.clock);
    let request = Request {
        resource: "graphql".to_owned(),
        cost_hint: Some(1),
        content_creating: false,
    };
    let at = || clock.now().timestamp() - T0;
    runtime.block_on(async {
        governor
            .admit_call(&[DESTINATION.to_owned()], Kind::Read)
            .await;
        let first = async {
            governor.before_request(&request).await;
            log_at
                .lock()
                .unwrap()
                .push(format!("first sent at {}", at()));
            // Open for 45 s on both clocks.
            tokio::time::sleep(Duration::from_secs(45)).await;
            clock.advance(45.0);
            log_at
                .lock()
                .unwrap()
                .push(format!("first answered at {}", at()));
            *answered.lock().unwrap() = true;
            governor.after_response(&[reading("graphql", 998, T0 + 600)]);
            governor.request_ended();
        };
        let second = async {
            tokio::task::yield_now().await;
            governor.before_request(&request).await;
            log_at
                .lock()
                .unwrap()
                .push(format!("second sent at {}", at()));
        };
        tokio::join!(first, second);
    });
    harness.end();
    let log = log.lock().unwrap().clone();
    assert_eq!(
        log,
        [
            "first sent at 0",
            "first answered at 45",
            "second sent at 600"
        ],
        "the second sent nothing while the first was open, then waited on its reading"
    );
    let notices = harness.notices();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("on graphql: remaining 998 of 5000, floor 1000"),
        "held on the first's response reading: {notices:?}"
    );
    assert_eq!(harness.waits()[0].reason, Reason::Floor);
}

#[test]
fn a_request_whose_response_reports_nothing_holds_the_slot_until_it_ends_and_is_counted() {
    // A request failed in transport tells the gate no reading, only that it ended: a request
    // of the next call goes against the one reading less every estimate admitted since, the
    // silent one included — with
    // headroom for three at a hint of 1, the fourth is held, though no reading said anything
    // new. The backend charged each, as the later readings show.
    let charged = Arc::new(Mutex::new(0_u64));
    let held = Arc::clone(&charged);
    let same = Harness::new(
        GovernorSettings {
            reserve_factor: 0.0,
            ..shipped()
        },
        instant(T0),
        move |at| {
            let graphql = if at.timestamp() >= T0 + 600 {
                reading("graphql", 5000, T0 + 4200)
            } else {
                reading("graphql", 1003 - *held.lock().unwrap(), T0 + 600)
            };
            vec![graphql, reading("core", 5000, T0 + 3600)]
        },
    );
    for _ in 0..3 {
        assert_eq!(same.request(Kind::Read, "graphql", Some(1)), 0.0);
        *charged.lock().unwrap() += 1;
        same.end();
    }
    assert_eq!(same.clock.now(), instant(T0), "three go against 1003");
    assert_eq!(
        same.readings.reads().len(),
        1,
        "no reading said anything new"
    );
    assert_eq!(
        same.request(Kind::Read, "graphql", Some(1)),
        600.0,
        "the fourth waited for a reading that admits it"
    );

    // Within one call, a request after the silent one sends nothing until it ends — an hour
    // later, failed in transport with no reading — and then goes, before the call ends.
    let call = Harness::new(shipped(), instant(T0), |_| {
        vec![
            reading("graphql", 5000, T0 + 3600),
            reading("core", 5000, T0 + 3600),
        ]
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap();
    let log = Mutex::new(Vec::<&str>::new());
    let request = Request {
        resource: "graphql".to_owned(),
        cost_hint: Some(1),
        content_creating: false,
    };
    runtime.block_on(async {
        call.governor
            .admit_call(&[DESTINATION.to_owned()], Kind::Read)
            .await;
        tokio::join!(
            async {
                call.governor.before_request(&request).await;
                log.lock().unwrap().push("silent sent");
                tokio::time::sleep(Duration::from_secs(3600)).await;
                log.lock().unwrap().push("silent ended");
                call.governor.request_ended();
            },
            async {
                tokio::task::yield_now().await;
                call.governor.before_request(&request).await;
                log.lock().unwrap().push("next sent");
                call.governor.request_ended();
            },
        );
        log.lock().unwrap().push("call ended");
        call.governor.end_call();
    });
    assert_eq!(
        *log.lock().unwrap(),
        ["silent sent", "silent ended", "next sent", "call ended"],
        "no time bound presumed the silent request over, and its end let the next go"
    );
}

/// Readings taken as a GitHub source takes them: its own `GET /rate_limit` asks the request
/// gate first (`core`, hinted at 0), then answers.
struct GatedRefresh {
    clock: Arc<support::Virtual>,
    governor: std::sync::OnceLock<std::sync::Weak<Governor>>,
    script: Box<support::Script>,
    asked: Mutex<Vec<(DateTime<Utc>, DateTime<Utc>)>>,
}

impl onefollowup::governor::Readings for GatedRefresh {
    fn read<'a>(
        &'a self,
        _: &'a str,
    ) -> onefollowup::governor::Pending<'a, Result<Vec<Reading>, String>> {
        Box::pin(async move {
            let governor = self
                .governor
                .get()
                .and_then(std::sync::Weak::upgrade)
                .unwrap();
            let before = self.clock.now();
            governor
                .before_request(&Request {
                    resource: "core".to_owned(),
                    cost_hint: Some(0),
                    content_creating: false,
                })
                .await;
            self.asked.lock().unwrap().push((before, self.clock.now()));
            // Answered, and over.
            governor.request_ended();
            Ok((self.script)(self.clock.now()))
        })
    }
}

#[test]
fn a_refresh_s_own_rate_limit_read_passes_the_gate_without_waiting_on_the_admission_it_serves() {
    // Core is under its floor throughout, so a core request taking the normal test would wait
    // an hour; graphql recovers at the window reset at 60 s. Core is seeded nowhere, so the
    // call boundary governs graphql alone and the case reaches the gate.
    let clock = support::Virtual::at(instant(T0));
    let readings = Arc::new(GatedRefresh {
        clock: Arc::clone(&clock),
        governor: std::sync::OnceLock::new(),
        script: Box::new(|at| {
            let graphql = if at < instant(T0 + 60) {
                reading("graphql", 900, T0 + 60)
            } else {
                reading("graphql", 5000, T0 + 3660)
            };
            vec![graphql, reading("core", 10, T0 + 3600)]
        }),
        asked: Mutex::new(Vec::new()),
    });
    let mut settings = shipped();
    settings.min_peak_per_minute.remove("core");
    let governor = Arc::new(Governor::new(
        settings,
        clock.clone(),
        readings.clone(),
        Arc::new(|_: &str| {}),
    ));
    readings.governor.set(Arc::downgrade(&governor)).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        governor
            .admit_call(&[DESTINATION.to_owned()], Kind::Read)
            .await;
        governor
            .before_request(&Request {
                resource: "graphql".to_owned(),
                cost_hint: Some(1),
                content_creating: false,
            })
            .await;
    });
    assert_eq!(
        clock.now(),
        instant(T0 + 60),
        "the graphql request waited for its reset"
    );
    let asked = readings.asked.lock().unwrap().clone();
    assert!(asked.len() >= 3, "every refresh asked the gate: {asked:?}");
    assert!(
        asked.iter().all(|(before, after)| before == after),
        "each refresh's own request went at once, its core window under the floor: {asked:?}"
    );
    let waits = governor.report().unwrap().waits;
    assert!(
        waits.iter().all(|wait| wait.resource == "graphql"),
        "nothing waited on core: {waits:?}"
    );
}

#[test]
fn a_call_after_one_that_cost_more_than_a_window_leaves_above_its_floor_is_admitted_in_the_next_window()
 {
    // Floor 1000 and no reserve, so the line is the floor: a full window leaves 4,000 above it.
    // The first call is one large find: 45 requests of 100 points, 40 of them in the window
    // resetting at T0 + 600 (5,000 down to 1,000), its 41st held to that reset, the last 5 in
    // the next window — 4,500 points in all, more than any window leaves above its floor. A
    // second, one-request call of the same kind is then admitted at once on its own request's
    // estimate, in the window resetting at T0 + 4,200: estimated at the 4,500 the first call
    // spent, it would wait for every reset there is.
    let mut settings = shipped();
    settings.reserve_factor = 0.0;
    let spent: Arc<Mutex<std::collections::BTreeMap<i64, u64>>> = Arc::default();
    let window = |at: i64| if at >= T0 + 600 { T0 + 4200 } else { T0 + 600 };
    let script = Arc::clone(&spent);
    let harness = Harness::new(settings, instant(T0), move |at| {
        let reset = window(at.timestamp());
        let left = 5000 - script.lock().unwrap().get(&reset).copied().unwrap_or(0);
        vec![
            reading("graphql", left, reset),
            reading("core", 5000, T0 + 3600),
        ]
    });
    let (sender, answered) = std::sync::mpsc::channel();
    let clock = Arc::clone(&harness.clock);
    // On a thread of its own, under a deadline: a call held for good fails the case rather
    // than waiting through every reset of the virtual clock.
    std::thread::spawn(move || {
        let destination = [DESTINATION.to_owned()];
        let send = |harness: &Harness| {
            harness
                .runtime
                .block_on(harness.governor.before_request(&Request {
                    resource: "graphql".to_owned(),
                    cost_hint: Some(100),
                    content_creating: false,
                }));
            let reset = window(harness.clock.now().timestamp());
            let left = {
                let mut spent = spent.lock().unwrap();
                let held = spent.entry(reset).or_default();
                *held += 100;
                5000 - *held
            };
            harness.respond(&[reading("graphql", left, reset)]);
        };
        harness
            .runtime
            .block_on(harness.governor.admit_call(&destination, Kind::Read));
        for _ in 0..45 {
            send(&harness);
        }
        harness.end();
        let first_waits = harness.waits();
        let second_at = harness.clock.now();
        harness
            .runtime
            .block_on(harness.governor.admit_call(&destination, Kind::Read));
        send(&harness);
        harness.end();
        let _ = sender.send((first_waits, second_at, harness.clock.now(), harness.waits()));
    });
    let (first_waits, second_at, admitted_at, waits) = answered
        .recv_timeout(Duration::from_secs(60))
        .expect("the second call was admitted rather than held for good");
    assert_eq!(
        first_waits.len(),
        1,
        "the large call's 41st request waited: {first_waits:?}"
    );
    assert_eq!(first_waits[0].reason, Reason::Floor);
    assert!(
        second_at >= instant(T0 + 600),
        "the large call ended in the next window: {second_at} {clock:?}",
        clock = clock.now()
    );
    assert_eq!(admitted_at, second_at, "the second call went at once");
    assert!(admitted_at < instant(T0 + 4200), "within that window");
    assert_eq!(waits, first_waits, "the second call waited for nothing");
}
