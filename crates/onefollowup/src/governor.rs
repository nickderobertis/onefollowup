//! The quota governor: paces every destination call onefollowup makes, and every request
//! inside one, so a pass never spends a token it shares below what others need, and never
//! waits while quota is ample (README.md, "The quota governor").
//!
//! It is asked at two seams, both owned by [`crate::store`]: once per destination call, before
//! the call is sent ([`Governor::admit_call`]), and once per backend request the call sends
//! ([`Governor::before_request`], told each response's reading through
//! [`Governor::after_response`]). Nothing else calls it, so a store call any verb adds is
//! governed without the verb knowing.
//!
//! **The rule.** Per destination and resource, readings are keyed by the window they name
//! (`resets_at`): admission is against every window not yet past its reset, the least of them
//! deciding, and a remaining that rises inside one window before its reset is ignored — a token
//! is answered from two windows of one resource at once, and `GET /rate_limit` now and then
//! reports the other's figures. A request of estimated cost `c` is admitted while
//! `remaining - own spend since that reading - c >= floor + reserve`, where the floor is
//! `floor_percent` of the limit and the reserve is others' peak burn rate times the seconds to
//! the window's reset times `reserve_factor`. Otherwise it waits, refreshing the reading every
//! [`HELD_REFRESH`], until a reading admits it or the window resets. It only ever waits: a
//! batch it paces completes, however long it takes.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::config::GovernorSettings;

/// How often the reading of a held request is refreshed: one fixed internal interval (manager
/// ruling repair-20 as amended by ruling `a7`), whatever `sample_seconds` says.
pub const HELD_REFRESH: Duration = Duration::from_secs(30);

/// What one request is estimated to cost when nothing better is known: no hint was given and
/// no response has shown what a request of its kind costs.
const UNKNOWN_COST: f64 = 1.0;

/// A boxed future, which is what the governor's injected clock and reader hand back.
pub type Pending<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a destination's backend said is left of one rate-limit window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// The backend's name for what is metered.
    pub resource: String,
    /// The window's whole allowance.
    pub limit: u64,
    /// What was left of it.
    pub remaining: u64,
    /// When the window resets.
    pub resets_at: DateTime<Utc>,
}

/// Whether a destination call reads or writes, which is whose observed cost estimates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A call that only reads.
    Read,
    /// A call that writes.
    Write,
}

impl Kind {
    const fn index(self) -> usize {
        match self {
            Self::Read => 0,
            Self::Write => 1,
        }
    }
}

/// One backend request a destination is about to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The resource it draws on, in [`Reading::resource`]'s vocabulary.
    pub resource: String,
    /// What the destination expects it to cost, when it can say.
    pub cost_hint: Option<u64>,
}

/// The time the governor reads and waits on: the system's, or a test's virtual clock.
pub trait Clock: Send + Sync {
    /// Now.
    fn now(&self) -> DateTime<Utc>;
    /// Wait `duration`.
    fn sleep(&self, duration: Duration) -> Pending<'_, ()>;
}

/// The system clock, waiting on the runtime's timer.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn sleep(&self, duration: Duration) -> Pending<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// Where the governor takes a fresh reading of one destination's quota from.
pub trait Readings: Send + Sync {
    /// Every window the destination `source` reports, asked afresh; none for a destination
    /// that meters nothing, and the reason when the reading could not be taken.
    fn read<'a>(&'a self, source: &'a str) -> Pending<'a, Result<Vec<Reading>, String>>;
}

/// Where the governor's one line per wait goes: standard error, unless a test captures it.
pub type Notices = Arc<dyn Fn(&str) + Send + Sync>;

/// What a governor is built from besides its settings. A store builds the system's
/// ([`Parts::system`]) unless [`with_parts`] installed others for the calling thread.
#[derive(Clone)]
pub struct Parts {
    /// The clock it reads and waits on.
    pub clock: Arc<dyn Clock>,
    /// Where it reads quota from; `None` is the store's own engine.
    pub readings: Option<Arc<dyn Readings>>,
    /// Where its wait lines go.
    pub notices: Notices,
}

impl Parts {
    /// The system clock, the store's own readings, and standard error.
    #[must_use]
    pub fn system() -> Self {
        Self {
            clock: Arc::new(SystemClock),
            readings: None,
            notices: Arc::new(|line: &str| eprintln!("{line}")),
        }
    }
}

thread_local! {
    static INSTALLED: RefCell<Option<Parts>> = const { RefCell::new(None) };
}

/// Run `run` with every store opened on this thread governed through `parts` — the seam a test
/// replaces GitHub's rate-limit state and the clock through, and nothing else.
pub fn with_parts<T>(parts: Parts, run: impl FnOnce() -> T) -> T {
    let previous = INSTALLED.with(|installed| installed.replace(Some(parts)));
    struct Restore(Option<Parts>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            INSTALLED.with(|installed| *installed.borrow_mut() = previous);
        }
    }
    let _restore = Restore(previous);
    run()
}

/// The parts [`with_parts`] installed on this thread, or the system's.
#[must_use]
pub fn installed() -> Parts {
    INSTALLED
        .with(|installed| installed.borrow().clone())
        .unwrap_or_else(Parts::system)
}

/// Why a request was held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// Its cost would have taken remaining under the floor.
    Floor,
    /// Its cost would have taken remaining under the floor plus the reserve.
    Reserve,
}

/// One wait the governor made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    /// The destination.
    pub source: String,
    /// The resource whose window held it.
    pub resource: String,
    /// How long it waited, in whole seconds rounded up.
    pub seconds: u64,
    /// Why.
    pub reason: Reason,
}

/// What a verb reports of the governor beside its store operations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// Every wait it made, in order; empty when quota was ample throughout.
    pub waits: Vec<Wait>,
}

/// A running mean.
#[derive(Debug, Clone, Copy, Default)]
struct Mean {
    sum: f64,
    count: u32,
}

impl Mean {
    fn push(&mut self, value: f64) {
        self.sum += value;
        self.count = self.count.saturating_add(1);
    }

    fn get(self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / f64::from(self.count))
    }
}

/// The latest reading of one window.
#[derive(Debug, Clone, Copy)]
struct Window {
    limit: u64,
    remaining: u64,
    read_at: DateTime<Utc>,
    /// [`Metered::own`] when it was read, so own spend since is the difference.
    own_at_read: f64,
}

/// What others spent of one resource between two readings of one window.
#[derive(Debug, Clone, Copy)]
struct Interval {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    spend: f64,
}

/// One resource of one destination.
#[derive(Debug, Default)]
struct Metered {
    windows: BTreeMap<DateTime<Utc>, Window>,
    /// Every point onefollowup was admitted to spend, cumulative, by estimate.
    own: f64,
    /// What of [`Self::own`] is in the one request in flight, which a reading taken meanwhile
    /// cannot show yet: cleared by its response, by the next request (a destination sends
    /// one request after another, so the next means this one was answered) and by its call's
    /// end.
    unanswered: f64,
    history: VecDeque<Interval>,
    /// The observed cost of one request, by [`Kind`].
    request_cost: [Mean; 2],
    /// The observed own spend of one whole call, by [`Kind`].
    call_cost: [Mean; 2],
}

/// One destination.
#[derive(Debug, Default)]
struct Destination {
    read_at: Option<DateTime<Utc>>,
    resources: BTreeMap<String, Metered>,
}

/// The destination call in progress.
#[derive(Debug)]
struct Call {
    sources: Vec<String>,
    kind: Kind,
    /// Each (source, resource)'s [`Metered::own`] when the call began.
    own_at_start: BTreeMap<(String, String), f64>,
}

#[derive(Default)]
struct State {
    destinations: BTreeMap<String, Destination>,
    /// Resources a request has drawn on, governed at the call boundary from then on.
    requested: BTreeSet<String>,
    /// The resources each destination's calls of each kind drew on, which are what such a
    /// call is admitted against once known.
    drawn: BTreeMap<(String, usize), BTreeSet<String>>,
    call: Option<Call>,
    /// A refresh's own allowance read is in flight: the one request admitted untested.
    refreshing: bool,
    /// The request in flight and its estimate, until its response is told.
    in_flight: Option<(String, Kind, f64)>,
    quota_reads: u64,
    calls: u64,
    waits: Vec<Wait>,
}

/// Why one window refuses a request, with what its wait line names.
#[derive(Debug, Clone)]
struct Blocked {
    source: String,
    resource: String,
    remaining: f64,
    limit: u64,
    floor: f64,
    reserve: f64,
    mean_per_minute: f64,
    peak_per_minute: f64,
    resets_at: DateTime<Utc>,
    resume_by: DateTime<Utc>,
    reason: Reason,
}

/// The quota governor of one store.
pub struct Governor {
    settings: Mutex<GovernorSettings>,
    clock: Arc<dyn Clock>,
    readings: Arc<dyn Readings>,
    notices: Notices,
    state: Mutex<State>,
}

fn seconds(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    // Millisecond precision is all a reset or a wait is ever stated in.
    #[allow(
        clippy::cast_precision_loss,
        reason = "a span of under 2^53 milliseconds converts exactly"
    )]
    let millis = (to - from).num_milliseconds() as f64;
    millis / 1000.0
}

fn whole_seconds(span: f64) -> u64 {
    // A wait's length rounded up to whole seconds, never negative.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0 first, and a wait is far under u64::MAX seconds"
    )]
    let whole = span.max(0.0).ceil() as u64;
    whole
}

#[allow(
    clippy::cast_precision_loss,
    reason = "quota figures are far under 2^53, which converts exactly"
)]
const fn points(value: u64) -> f64 {
    value as f64
}

impl Governor {
    /// A governor over `settings`, reading quota through `readings` and time through `clock`.
    #[must_use]
    pub fn new(
        settings: GovernorSettings,
        clock: Arc<dyn Clock>,
        readings: Arc<dyn Readings>,
        notices: Notices,
    ) -> Self {
        Self {
            settings: Mutex::new(settings),
            clock,
            readings,
            notices,
            state: Mutex::new(State::default()),
        }
    }

    /// Replace the settings, keeping everything learned.
    pub fn configure(&self, settings: GovernorSettings) {
        *self.settings() = settings;
    }

    fn settings(&self) -> MutexGuard<'_, GovernorSettings> {
        self.settings.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many quota readings it took: one per destination asked.
    #[must_use]
    pub fn quota_reads(&self) -> u64 {
        self.state().quota_reads
    }

    /// What a verb reports of it: `None` before it governed any destination call.
    #[must_use]
    pub fn report(&self) -> Option<Report> {
        let state = self.state();
        (state.calls > 0).then(|| Report {
            waits: state.waits.clone(),
        })
    }

    /// Admit one destination call to `sources`, waiting while any resource they report would
    /// be taken under its admission line by the call's estimated cost, and keep them as the
    /// call in progress, whose requests [`Self::before_request`] admits one by one.
    pub async fn admit_call(&self, sources: &[String], kind: Kind) {
        let settings = self.settings().clone();
        {
            let mut state = self.state();
            state.calls += 1;
            let own_at_start = sources
                .iter()
                .flat_map(|source| {
                    state
                        .destinations
                        .get(source)
                        .into_iter()
                        .flat_map(move |held| {
                            held.resources.iter().map(move |(resource, metered)| {
                                ((source.clone(), resource.clone()), metered.own)
                            })
                        })
                })
                .collect();
            state.call = Some(Call {
                sources: sources.to_vec(),
                kind,
                own_at_start,
            });
        }
        if !settings.enabled {
            return;
        }
        for source in sources {
            let mut refreshed = self.refresh_if_stale(source, &settings).await;
            loop {
                let governed = {
                    let state = self.state();
                    match state.drawn.get(&(source.clone(), kind.index())) {
                        Some(drawn) if !drawn.is_empty() => drawn.clone(),
                        // Before a call of this kind has drawn on anything — a fresh process,
                        // or a destination whose requests never ask the gate — every resource
                        // the governor knows of is governed.
                        _ => {
                            let mut governed = state.requested.clone();
                            governed.extend(settings.min_peak_per_minute.keys().cloned());
                            governed
                        }
                    }
                };
                let mut blocked = None;
                for resource in &governed {
                    let cost = self.call_estimate(source, resource, kind);
                    if let Some(refusal) = self.decide(source, resource, cost, &settings) {
                        blocked = Some((refusal, cost));
                        break;
                    }
                }
                match blocked {
                    None => break,
                    Some(_) if !refreshed => {
                        // Read again before a call the estimate says would cross the line.
                        self.refresh(source).await;
                        refreshed = true;
                    }
                    Some((refusal, cost)) => self.hold(refusal, cost, &settings).await,
                }
            }
        }
    }

    /// The call in progress has answered: learn what it spent.
    pub fn end_call(&self) {
        let mut state = self.state();
        let Some(call) = state.call.take() else {
            return;
        };
        for source in &call.sources {
            for (resource, metered) in state
                .destinations
                .get_mut(source)
                .into_iter()
                .flat_map(|held| held.resources.iter_mut())
            {
                metered.unanswered = 0.0;
                let before = call
                    .own_at_start
                    .get(&(source.clone(), resource.clone()))
                    .copied()
                    .unwrap_or(0.0);
                let spent = metered.own - before;
                if spent > 0.0 {
                    metered.call_cost[call.kind.index()].push(spent);
                }
            }
        }
    }

    /// Admit one request of the call in progress: at once while its estimated cost leaves the
    /// least window of its resource at or above floor plus reserve, otherwise once a fresh
    /// reading does, or the window resets. Never refuses.
    pub async fn before_request(&self, request: &Request) {
        let (sources, kind) = {
            let mut state = self.state();
            if state.refreshing {
                // By the one-in-flight rule the only request under a refresh is the refresh's
                // own allowance read, which must not wait on the admission it serves.
                state.in_flight = None;
                return;
            }
            state.requested.insert(request.resource.clone());
            let (sources, kind) = match &state.call {
                Some(call) => (call.sources.clone(), call.kind),
                None => (state.destinations.keys().cloned().collect(), Kind::Read),
            };
            // The request before this one has been answered: what it spent is the
            // destination's to show from here on.
            for metered in state
                .destinations
                .values_mut()
                .flat_map(|held| held.resources.values_mut())
            {
                metered.unanswered = 0.0;
            }
            for source in &sources {
                state
                    .drawn
                    .entry((source.clone(), kind.index()))
                    .or_default()
                    .insert(request.resource.clone());
            }
            (sources, kind)
        };
        let settings = self.settings().clone();
        if !settings.enabled {
            return;
        }
        let cost = request.cost_hint.map_or_else(
            || self.request_estimate(&sources, &request.resource, kind),
            points,
        );
        for source in &sources {
            self.refresh_if_stale(source, &settings).await;
        }
        // A request refused on the estimate is held at once: the call was admitted against a
        // fresh reading, every response since refreshed it, and the hold's own refreshes are
        // what find the headroom come back — the estimate less than it cost — before a reset.
        while let Some(refusal) = sources
            .iter()
            .find_map(|source| self.decide(source, &request.resource, cost, &settings))
        {
            self.hold(refusal, cost, &settings).await;
        }
        let mut state = self.state();
        for source in &sources {
            if let Some(metered) = state
                .destinations
                .get_mut(source)
                .and_then(|held| held.resources.get_mut(&request.resource))
                && !metered.windows.is_empty()
            {
                metered.own += cost;
                metered.unanswered = cost;
            }
        }
        state.in_flight = Some((request.resource.clone(), kind, cost));
    }

    /// What the response to the request in flight said is left: each reading refreshes its
    /// window at no quota cost, and teaches the governor what a request of its kind costs.
    pub fn after_response(&self, readings: &[Reading]) {
        let now = self.clock.now();
        let history = self.settings().burn_history_seconds;
        let mut state = self.state();
        let in_flight = state.in_flight.take();
        let sources: Vec<String> = match &state.call {
            Some(call) => call.sources.clone(),
            None => state.destinations.keys().cloned().collect(),
        };
        for reading in readings {
            let learn = in_flight
                .as_ref()
                .filter(|(resource, _, _)| *resource == reading.resource)
                .map(|(_, kind, estimate)| (*kind, *estimate));
            for source in &sources {
                if let Some((_, estimate)) = learn
                    && let Some(metered) = state
                        .destinations
                        .get_mut(source)
                        .and_then(|held| held.resources.get_mut(&reading.resource))
                {
                    metered.unanswered = (metered.unanswered - estimate).max(0.0);
                }
                observe(&mut state, source, reading, now, learn, history);
            }
        }
    }

    async fn refresh_if_stale(&self, source: &str, settings: &GovernorSettings) -> bool {
        let now = self.clock.now();
        let stale = self
            .state()
            .destinations
            .get(source)
            .and_then(|held| held.read_at)
            .is_none_or(|read_at| seconds(read_at, now) >= points(settings.sample_seconds));
        if stale {
            self.refresh(source).await;
        }
        stale
    }

    /// Take a fresh reading of `source`, its own allowance read admitted untested.
    async fn refresh(&self, source: &str) {
        self.state().refreshing = true;
        let read = self.readings.read(source).await;
        let now = self.clock.now();
        let history = self.settings().burn_history_seconds;
        let mut state = self.state();
        state.refreshing = false;
        state.quota_reads += 1;
        state
            .destinations
            .entry(source.to_owned())
            .or_default()
            .read_at = Some(now);
        // A reading that could not be taken teaches nothing; the windows already held stand
        // until their resets.
        if let Ok(readings) = read {
            for reading in &readings {
                observe(&mut state, source, reading, now, None, history);
            }
        }
    }

    fn request_estimate(&self, sources: &[String], resource: &str, kind: Kind) -> f64 {
        let state = self.state();
        sources
            .iter()
            .filter_map(|source| {
                state
                    .destinations
                    .get(source)?
                    .resources
                    .get(resource)?
                    .request_cost[kind.index()]
                .get()
            })
            .fold(None, |most: Option<f64>, cost| {
                Some(most.map_or(cost, |most| most.max(cost)))
            })
            .unwrap_or(UNKNOWN_COST)
    }

    fn call_estimate(&self, source: &str, resource: &str, kind: Kind) -> f64 {
        let state = self.state();
        let metered = state
            .destinations
            .get(source)
            .and_then(|held| held.resources.get(resource));
        metered
            .and_then(|metered| {
                metered.call_cost[kind.index()]
                    .get()
                    .or_else(|| metered.request_cost[kind.index()].get())
            })
            .unwrap_or(UNKNOWN_COST)
    }

    /// Why the least live window of `resource` at `source` refuses a cost of `cost`, or `None`
    /// when every one admits it (or none is held: a destination reporting nothing).
    fn decide(
        &self,
        source: &str,
        resource: &str,
        cost: f64,
        settings: &GovernorSettings,
    ) -> Option<Blocked> {
        let now = self.clock.now();
        let mut state = self.state();
        let metered = state
            .destinations
            .get_mut(source)?
            .resources
            .get_mut(resource)?;
        metered.windows.retain(|resets_at, _| *resets_at > now);
        let (mean, peak) = burn(metered, settings.peak_window_seconds);
        let seeded = settings
            .min_peak_per_minute
            .get(resource)
            .copied()
            .unwrap_or(0.0);
        let peak = peak.max(seeded);
        let mut worst: Option<(f64, Blocked)> = None;
        for (resets_at, window) in &metered.windows {
            let remaining = points(window.remaining) - (metered.own - window.own_at_read);
            let floor = settings.floor_percent / 100.0 * points(window.limit);
            let to_reset = seconds(now, *resets_at).max(0.0);
            let reserve = peak / 60.0 * to_reset * settings.reserve_factor;
            let headroom = remaining - cost - floor - reserve;
            if headroom >= 0.0 || worst.as_ref().is_some_and(|(least, _)| *least <= headroom) {
                continue;
            }
            let (reason, resume_by) = if remaining - cost < floor {
                (Reason::Floor, *resets_at)
            } else {
                // The reserve shrinks with the time to the reset; it admits the cost once
                // `peak * factor * to_reset <= remaining - cost - floor`.
                let rate = peak / 60.0 * settings.reserve_factor;
                let lead = (remaining - cost - floor) / rate;
                let lead = chrono::Duration::milliseconds(whole_millis(lead));
                (Reason::Reserve, (*resets_at - lead).max(now))
            };
            worst = Some((
                headroom,
                Blocked {
                    source: source.to_owned(),
                    resource: resource.to_owned(),
                    remaining,
                    limit: window.limit,
                    floor,
                    reserve,
                    mean_per_minute: mean,
                    peak_per_minute: peak,
                    resets_at: *resets_at,
                    resume_by,
                    reason,
                },
            ));
        }
        worst.map(|(_, blocked)| blocked)
    }

    /// Wait out `blocked` until a fresh reading admits the request or its window resets:
    /// one line on standard error, a refresh every [`HELD_REFRESH`], and the wait recorded.
    async fn hold(&self, blocked: Blocked, cost: f64, settings: &GovernorSettings) {
        let started = self.clock.now();
        (self.notices)(&format!(
            "onefollowup: holding requests to {} on {}: remaining {:.0} of {}, floor {:.0}, reserve {:.0}; others burn {:.1}/min mean, {:.1}/min peak; resuming by {}",
            blocked.source,
            blocked.resource,
            blocked.remaining,
            blocked.limit,
            blocked.floor,
            blocked.reserve,
            blocked.mean_per_minute,
            blocked.peak_per_minute,
            blocked.resume_by.to_rfc3339_opts(SecondsFormat::Secs, true),
        ));
        let mut cost_left = true;
        while cost_left {
            let now = self.clock.now();
            let to_reset = seconds(now, blocked.resets_at);
            let pause = if to_reset > 0.0 && to_reset < HELD_REFRESH.as_secs_f64() {
                Duration::from_secs_f64(to_reset)
            } else {
                HELD_REFRESH
            };
            self.clock.sleep(pause).await;
            self.refresh(&blocked.source).await;
            cost_left = self
                .decide(&blocked.source, &blocked.resource, cost, settings)
                .is_some_and(|still| still.resets_at == blocked.resets_at);
        }
        let waited = seconds(started, self.clock.now());
        self.state().waits.push(Wait {
            source: blocked.source,
            resource: blocked.resource,
            seconds: whole_seconds(waited),
            reason: blocked.reason,
        });
    }
}

fn whole_millis(span_seconds: f64) -> i64 {
    // A lead of at most a window's length, in milliseconds.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clamped to a day of milliseconds, far inside i64"
    )]
    let millis = (span_seconds.clamp(0.0, 86_400.0) * 1000.0) as i64;
    millis
}

/// Take `reading` of `source` into what the governor holds: a new window kept, a lower
/// remaining in a known window turned into others' spend since its last reading, a higher one
/// before the reset ignored.
fn observe(
    state: &mut State,
    source: &str,
    reading: &Reading,
    now: DateTime<Utc>,
    learn: Option<(Kind, f64)>,
    history_seconds: u64,
) {
    let metered = state
        .destinations
        .entry(source.to_owned())
        .or_default()
        .resources
        .entry(reading.resource.clone())
        .or_default();
    metered.windows.retain(|resets_at, _| *resets_at > now);
    if reading.resets_at <= now {
        return;
    }
    // What the reading can show of own spend: everything admitted but what is unanswered.
    let own = metered.own - metered.unanswered;
    match metered.windows.get_mut(&reading.resets_at) {
        Some(window) if reading.remaining > window.remaining => {}
        Some(window) => {
            let fall = points(window.remaining - reading.remaining);
            let own_since = own - window.own_at_read;
            if let Some((kind, estimate)) = learn {
                // This response's fall, less what earlier admissions since the reading
                // account for, is what the request cost (others' spend included, so a bound
                // from above).
                metered.request_cost[kind.index()].push((fall - (own_since - estimate)).max(0.0));
            }
            if now > window.read_at {
                metered.history.push_back(Interval {
                    from: window.read_at,
                    to: now,
                    spend: (fall - own_since).max(0.0),
                });
            }
            *window = Window {
                limit: reading.limit,
                remaining: reading.remaining,
                read_at: now,
                own_at_read: own,
            };
        }
        None => {
            metered.windows.insert(
                reading.resets_at,
                Window {
                    limit: reading.limit,
                    remaining: reading.remaining,
                    read_at: now,
                    own_at_read: own,
                },
            );
        }
    }
    let horizon =
        now - chrono::Duration::seconds(i64::try_from(history_seconds).unwrap_or(i64::MAX / 1000));
    while metered
        .history
        .front()
        .is_some_and(|interval| interval.to < horizon)
    {
        metered.history.pop_front();
    }
}

/// Others' mean and peak burn of one resource, in points a minute: the peak the most they
/// spent over any `window_seconds` ending at a reading, pro rata within each interval, over
/// the whole window — a history shorter than one window counts what it did not see as
/// nothing, which the seeded peak stands in for.
fn burn(metered: &Metered, window_seconds: u64) -> (f64, f64) {
    let (Some(first), Some(last)) = (metered.history.front(), metered.history.back()) else {
        return (0.0, 0.0);
    };
    let span = seconds(first.from, last.to);
    let total: f64 = metered.history.iter().map(|interval| interval.spend).sum();
    let mean = if span > 0.0 { total / span * 60.0 } else { 0.0 };
    let width = points(window_seconds);
    let mut peak: f64 = 0.0;
    for end in metered.history.iter().map(|interval| interval.to) {
        let start = end - chrono::Duration::milliseconds(whole_millis(width));
        let mut spent = 0.0;
        for interval in &metered.history {
            let length = seconds(interval.from, interval.to);
            let overlap = seconds(interval.from.max(start), interval.to.min(end));
            if length > 0.0 && overlap > 0.0 {
                spent += interval.spend * overlap / length;
            }
        }
        peak = peak.max(spent / width * 60.0);
    }
    (mean, peak)
}
