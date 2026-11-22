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
//! reports the other's figures. Recovery before a reset is therefore a refreshed reading above
//! the governor's own estimate (the last reading less own spend since) and not above that last
//! reading: own spend estimated higher than it cost. A request of estimated cost `c` is admitted while
//! `remaining - own spend since that reading - c >= floor + reserve`, where the floor is
//! `floor_percent` of the limit and the reserve is others' peak burn rate times the seconds to
//! the window's reset times `reserve_factor`. Otherwise it waits, refreshing the reading every
//! [`HELD_REFRESH`], until a reading admits it or the window resets. It only ever waits: a
//! batch it paces completes, however long it takes.
//!
//! **One request in flight.** A request on a metered resource holds the process's one slot
//! from admission until onetaskgraph tells the gate it ended ([`Governor::request_ended`], once
//! for every admitted request however it ended: answered with or without a reading, failed in
//! transport, or dropped) or its store call ends — nothing else, and no time bound, presumes it
//! over — so requests a destination sends at once (a GitHub source joins some) go one after
//! another, each against the reading the one before it refreshed. The signal names no request,
//! so an ending is put to an admitted request holding no slot first, while one is open: that
//! can only keep the slot a little longer, never give it up while its holder is in flight. The
//! slot is only for requests to a backend whose responses report readings, decided by the
//! resource the request names (manager ruling `a6`): onetaskgraph's request-gate contract
//! reserves `local` for a source's write to its own files (local-md's), which neither takes nor
//! waits for the slot, though it is still admitted against any reading of `local`; every other
//! resource, `stdio` included, takes it. Nothing here names a plugin.
//!
//! **One state per host.** What is learned of a backend kind — its windows, everyone's admitted
//! spend and the cost still in flight, others' burn history, the observed costs and the
//! content-write window — is one state every onefollowup process on the host shares, in
//! `governor.state_dir` ([`host`]), so admission counts every process's spend and a fresh
//! process starts from the burn estimate others learned. Sources are keyed by their backend
//! kind ([`Governor::set_backends`]), so every destination of one kind shares one state; with no
//! state directory the state is the process's alone. Each process still holds its own slot, so
//! onefollowup's own spend crosses a floor by at most one request's estimate error per process.
//!
//! **Content-creating writes.** A request the gate is told creates content, in a destination
//! call, is admitted only while the backend kind's writes across the host in the last minute
//! and the last hour are under `max_content_writes_per_minute` and
//! `max_content_writes_per_hour`, and is counted as it is admitted; otherwise it waits for the
//! oldest counted write to leave the window. These are counts because no response reports
//! GitHub's secondary content-creation limit. Each request counts one, however many contents a
//! source batches into it, since the gate is told no more.

mod host;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use tokio::sync::Notify;

use crate::config::GovernorSettings;

/// How often the reading of a held request is refreshed: one fixed internal interval (manager
/// ruling repair-20 as amended by ruling `a7`), whatever `sample_seconds` says.
pub const HELD_REFRESH: Duration = Duration::from_secs(30);

/// The resource a source names a write to its own files by (onetaskgraph's `GatedRequest`
/// vocabulary): no response to it reports a reading.
const OWN_FILES: &str = "local";

/// What one request is estimated to cost when nothing better is known: no hint was given and
/// no response has shown what a request of its kind costs.
const UNKNOWN_COST: f64 = 1.0;

/// The content-write windows: a minute and an hour, in seconds.
const MINUTE: i64 = 60;
const HOUR: i64 = 3600;

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
    /// Whether it creates content (an issue, a comment), which the content-write caps count.
    pub content_creating: bool,
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
    /// The host's content-creating writes in the last minute were at the per-minute cap.
    ContentPerMinute,
    /// The host's content-creating writes in the last hour were at the per-hour cap.
    ContentPerHour,
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
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Window {
    limit: u64,
    remaining: u64,
    read_at: DateTime<Utc>,
    /// [`Metered::own`] when it was read, so own spend since is the difference.
    own_at_read: f64,
}

/// What others spent of one resource between two readings of one window.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Interval {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    spend: f64,
}

/// One resource of one backend kind.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Metered {
    windows: BTreeMap<DateTime<Utc>, Window>,
    /// Every point onefollowup, in any process on the host, was admitted to spend, cumulative,
    /// by estimate.
    own: f64,
    history: VecDeque<Interval>,
    /// The observed cost of one request, by [`Kind`].
    request_cost: [Mean; 2],
    /// The observed own spend of one whole call, by [`Kind`].
    call_cost: [Mean; 2],
}

/// Admitted spend of one request no response has reported yet, which a reading taken
/// meanwhile cannot show: dropped by its response, by its call's end, or once its process is
/// gone.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Flight {
    process: String,
    resource: String,
    estimate: f64,
}

/// What every process on the host shares of one backend kind.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Shared {
    resources: BTreeMap<String, Metered>,
    in_flight: Vec<Flight>,
    /// When each content-creating write of the last hour was admitted, oldest first.
    content_writes: VecDeque<DateTime<Utc>>,
}

impl Shared {
    /// What of `resource`'s admitted spend no response has reported yet, in every process.
    fn unanswered(&self, resource: &str) -> f64 {
        self.in_flight
            .iter()
            .filter(|flight| flight.resource == resource)
            .map(|flight| flight.estimate)
            .sum()
    }
}

/// The destination call in progress.
#[derive(Debug)]
struct Call {
    sources: Vec<String>,
    kind: Kind,
    /// Each (backend, resource)'s [`Metered::own`] when the call began.
    own_at_start: BTreeMap<(String, String), f64>,
}

#[derive(Default)]
struct State {
    /// When each destination's quota was last read.
    read_at: BTreeMap<String, DateTime<Utc>>,
    /// Resources a request has drawn on, governed at the call boundary from then on.
    requested: BTreeSet<String>,
    /// The resources each destination's calls of each kind drew on, which are what such a
    /// call is admitted against once known.
    drawn: BTreeMap<(String, usize), BTreeSet<String>>,
    call: Option<Call>,
    /// A refresh's own allowance read is due: the one request admitted untested, which the
    /// first request asking the gate under the mark takes.
    refreshing: bool,
    /// The one request of this process admitted against a reading, from the moment it starts
    /// being admitted until its response is told ([`Slot`]); every other metered request
    /// waits for it.
    slot: Option<Slot>,
    /// The slot's request and its estimate, until its response is told or it ends.
    in_flight: Option<(String, Kind, f64)>,
    /// Admitted requests holding no slot — a refresh's own allowance read, a write to a
    /// source's own files, any request while the governor is off — not yet ended.
    unslotted: u32,
    quota_reads: u64,
    calls: u64,
    waits: Vec<Wait>,
}

/// The one metered request of this process in flight, or being admitted.
#[derive(Debug, Clone, Copy)]
struct Slot {
    /// Whether it was sent; not while it is still being admitted (refreshing or held).
    sent: bool,
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

/// Why the content-write caps hold a write, with what its wait line names.
#[derive(Debug, Clone)]
struct ContentBlocked {
    source: String,
    resource: String,
    written: usize,
    cap: u64,
    resume_by: DateTime<Utc>,
    reason: Reason,
}

/// Where the shared state is kept: the host state directory once opened, or the process.
enum Keeper {
    /// No state directory, or not opened yet.
    Process,
    /// The host state directory.
    Host(host::Host),
    /// The state directory that could not be opened, which the process governs without.
    Unusable(PathBuf),
}

/// The quota governor of one store.
pub struct Governor {
    settings: Mutex<GovernorSettings>,
    clock: Arc<dyn Clock>,
    readings: Arc<dyn Readings>,
    notices: Notices,
    state: Mutex<State>,
    /// Each source's backend kind, which is what its state is shared by.
    backends: Mutex<BTreeMap<String, String>>,
    /// The shared state, as last read from the host or as the process alone holds it.
    shared: Mutex<BTreeMap<String, Shared>>,
    keeper: Mutex<Keeper>,
    /// This process's name in the host state.
    process: String,
    /// Woken whenever the slot is given up.
    released: Notify,
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
            backends: Mutex::new(BTreeMap::new()),
            shared: Mutex::new(BTreeMap::new()),
            keeper: Mutex::new(Keeper::Process),
            process: host::process_name(),
            released: Notify::new(),
        }
    }

    /// Key each source's state by its backend kind (`source → kind`): every source of one kind
    /// shares one state. A source not named is a kind of its own, keyed by its name.
    pub fn set_backends(&self, backends: BTreeMap<String, String>) {
        *self.backends.lock().unwrap_or_else(PoisonError::into_inner) = backends;
    }

    fn backend(&self, source: &str) -> String {
        self.backends
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(source)
            .cloned()
            .unwrap_or_else(|| source.to_owned())
    }

    /// Each backend kind of `sources`, once, in name order.
    fn backends_of(&self, sources: &[String]) -> Vec<String> {
        let kinds: BTreeSet<String> = sources.iter().map(|source| self.backend(source)).collect();
        kinds.into_iter().collect()
    }

    /// Run `change` over the shared state of `backends`, read afresh from the host state
    /// directory and written back to it when there is one; never refused, since a state that
    /// cannot be kept is the process's alone.
    fn shared<R>(
        &self,
        backends: &[String],
        change: impl FnOnce(&mut BTreeMap<String, Shared>) -> R,
    ) -> R {
        let directory = self.settings().state_dir.clone();
        let mut keeper = self.keeper.lock().unwrap_or_else(PoisonError::into_inner);
        let reopen = match (&*keeper, &directory) {
            (Keeper::Process, Some(_)) => true,
            (Keeper::Host(host), Some(wanted)) => host.directory() != wanted.as_path(),
            (Keeper::Unusable(failed), Some(wanted)) => failed != wanted,
            (_, None) => {
                *keeper = Keeper::Process;
                false
            }
        };
        if let (true, Some(directory)) = (reopen, &directory) {
            *keeper = match host::Host::open(directory, &self.process) {
                Ok(host) => Keeper::Host(host),
                Err(error) => {
                    (self.notices)(&format!(
                        "onefollowup: the governor's state directory {} cannot be used ({error}); governing this process on its own",
                        directory.display()
                    ));
                    Keeper::Unusable(directory.clone())
                }
            };
        }
        let mut shared = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if let Keeper::Host(host) = &*keeper {
            match host.lock(backends) {
                Ok(locked) => {
                    for backend in backends {
                        let mut read: Shared = locked.read(backend).unwrap_or_default();
                        read.in_flight.retain(|flight| host.alive(&flight.process));
                        shared.insert(backend.clone(), read);
                    }
                    let answer = change(&mut shared);
                    let written = backends.iter().try_for_each(|backend| {
                        shared
                            .get(backend)
                            .map_or(Ok(()), |state| locked.write(backend, state))
                    });
                    drop(locked);
                    if let Err(error) = written {
                        self.unusable(&mut keeper, &error);
                    }
                    return answer;
                }
                Err(error) => self.unusable(&mut keeper, &error),
            }
        }
        for backend in backends {
            shared.entry(backend.clone()).or_default();
        }
        change(&mut shared)
    }

    /// The host state directory failed: say so once, and govern this process on its own.
    fn unusable(&self, keeper: &mut Keeper, error: &std::io::Error) {
        if let Keeper::Host(host) = &*keeper {
            (self.notices)(&format!(
                "onefollowup: the governor's state in {} cannot be kept ({error}); governing this process on its own",
                host.directory().display()
            ));
            *keeper = Keeper::Unusable(host.directory().to_path_buf());
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
        let backends = self.backends_of(sources);
        let own_at_start = self.shared(&backends, |shared| {
            backends
                .iter()
                .flat_map(|backend| {
                    shared.get(backend).into_iter().flat_map(move |held| {
                        held.resources.iter().map(move |(resource, metered)| {
                            ((backend.clone(), resource.clone()), metered.own)
                        })
                    })
                })
                .collect()
        });
        {
            let mut state = self.state();
            state.calls += 1;
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
        let call = {
            let mut state = self.state();
            // Every request of the call is over, answered or not.
            state.in_flight = None;
            state.unslotted = 0;
            self.release(&mut state);
            state.call.take()
        };
        let Some(call) = call else {
            return;
        };
        let backends = self.backends_of(&call.sources);
        self.shared(&backends, |shared| {
            for backend in &backends {
                let Some(held) = shared.get_mut(backend) else {
                    continue;
                };
                held.in_flight
                    .retain(|flight| flight.process != self.process);
                for (resource, metered) in &mut held.resources {
                    let before = call
                        .own_at_start
                        .get(&(backend.clone(), resource.clone()))
                        .copied()
                        .unwrap_or(0.0);
                    let spent = metered.own - before;
                    if spent > 0.0 {
                        metered.call_cost[call.kind.index()].push(spent);
                    }
                }
            }
        });
    }

    /// Admit one request of the call in progress: at once while its estimated cost leaves the
    /// least window of its resource at or above floor plus reserve and, for a content-creating
    /// request of a destination call, the host's content-write windows are under their caps;
    /// otherwise once a fresh reading and the windows do, or the window resets. Never refuses.
    ///
    /// A request on a resource a reading meters waits first for this process's one slot: no
    /// two such requests are ever in flight together, however many the destination sends at
    /// once, so each is admitted against a reading every earlier one's response has refreshed.
    pub async fn before_request(&self, request: &Request) {
        let (sources, kind, in_call) = {
            let mut state = self.state();
            if std::mem::take(&mut state.refreshing) {
                // The slot's holder is refreshing, so the one request under the mark is the
                // refresh's own allowance read, which must not wait on the admission it
                // serves; any other request arriving meanwhile takes the slot's wait below.
                state.unslotted += 1;
                return;
            }
            state.requested.insert(request.resource.clone());
            let (sources, kind, in_call) = match &state.call {
                Some(call) => (call.sources.clone(), call.kind, true),
                None => (state.read_at.keys().cloned().collect(), Kind::Read, false),
            };
            for source in &sources {
                state
                    .drawn
                    .entry((source.clone(), kind.index()))
                    .or_default()
                    .insert(request.resource.clone());
            }
            (sources, kind, in_call)
        };
        let settings = self.settings().clone();
        if !settings.enabled {
            self.state().unslotted += 1;
            return;
        }
        // A source's write to its own files (local-md) is answered by nothing that reports a
        // reading, so it neither takes nor waits for the slot; it is admitted all the same.
        let slotted = request.resource != OWN_FILES;
        if slotted {
            self.take_slot().await;
        }
        let cost = request.cost_hint.map_or_else(
            || self.request_estimate(&sources, &request.resource, kind),
            points,
        );
        let backends = self.backends_of(&sources);
        loop {
            for source in &sources {
                self.refresh_if_stale(source, &settings).await;
            }
            // A request refused on the estimate is held at once: the call was admitted against
            // a fresh reading, every response since refreshed it, and the hold's own refreshes
            // are what find the headroom come back — the estimate less than it cost — before a
            // reset.
            while let Some(refusal) = sources
                .iter()
                .find_map(|source| self.decide(source, &request.resource, cost, &settings))
            {
                self.hold(refusal, cost, &settings).await;
            }
            if !(request.content_creating && in_call) {
                break;
            }
            // Counted as admitted, at the instant the quota admitted it, so a write is never
            // counted earlier than it is sent.
            match self.count_content(&sources, &request.resource, &settings) {
                None => break,
                Some(blocked) => self.hold_content(blocked).await,
            }
        }
        self.shared(&backends, |shared| {
            for backend in &backends {
                if let Some(held) = shared.get_mut(backend)
                    && let Some(metered) = held.resources.get_mut(&request.resource)
                    && !metered.windows.is_empty()
                {
                    metered.own += cost;
                    held.in_flight.push(Flight {
                        process: self.process.clone(),
                        resource: request.resource.clone(),
                        estimate: cost,
                    });
                }
            }
        });
        let mut state = self.state();
        if slotted {
            state.in_flight = Some((request.resource.clone(), kind, cost));
            state.slot = Some(Slot { sent: true });
        } else {
            state.unslotted += 1;
        }
    }

    /// Count one content-creating write to `sources` now, or say why the host's content-write
    /// windows hold it: checked and counted under one lock, so two processes never both take
    /// a window's last place.
    fn count_content(
        &self,
        sources: &[String],
        resource: &str,
        settings: &GovernorSettings,
    ) -> Option<ContentBlocked> {
        let now = self.clock.now();
        let backends = self.backends_of(sources);
        let source = sources.first().cloned().unwrap_or_default();
        self.shared(&backends, |shared| {
            for backend in &backends {
                let held = shared.entry(backend.clone()).or_default();
                let hour_ago = now - chrono::Duration::seconds(HOUR);
                while held
                    .content_writes
                    .front()
                    .is_some_and(|written| *written <= hour_ago)
                {
                    held.content_writes.pop_front();
                }
                for (span, cap, reason) in [
                    (
                        HOUR,
                        settings.max_content_writes_per_hour,
                        Reason::ContentPerHour,
                    ),
                    (
                        MINUTE,
                        settings.max_content_writes_per_minute,
                        Reason::ContentPerMinute,
                    ),
                ] {
                    let since = now - chrono::Duration::seconds(span);
                    let within: Vec<DateTime<Utc>> = held
                        .content_writes
                        .iter()
                        .copied()
                        .filter(|written| *written > since)
                        .collect();
                    let cap_places = usize::try_from(cap).unwrap_or(usize::MAX);
                    if within.len() >= cap_places {
                        // The window has room once all but `cap - 1` of its writes left it.
                        let leaving = within[within.len() - cap_places];
                        return Some(ContentBlocked {
                            source: source.clone(),
                            resource: resource.to_owned(),
                            written: within.len(),
                            cap,
                            resume_by: leaving + chrono::Duration::seconds(span),
                            reason,
                        });
                    }
                }
            }
            for backend in &backends {
                shared
                    .entry(backend.clone())
                    .or_default()
                    .content_writes
                    .push_back(now);
            }
            None
        })
    }

    /// Wait out a content-write window until the write it holds has room: one line on
    /// standard error, and the wait recorded.
    async fn hold_content(&self, blocked: ContentBlocked) {
        let started = self.clock.now();
        let (window, cap_key) = match blocked.reason {
            Reason::ContentPerHour => ("hour", "max_content_writes_per_hour"),
            _ => ("minute", "max_content_writes_per_minute"),
        };
        (self.notices)(&format!(
            "onefollowup: holding content-creating writes to {} on {}: {} in the last {window} across this host, {cap_key} {}; resuming by {}",
            blocked.source,
            blocked.resource,
            blocked.written,
            blocked.cap,
            blocked.resume_by.to_rfc3339_opts(SecondsFormat::Secs, true),
        ));
        let pause = (blocked.resume_by - started)
            .to_std()
            .unwrap_or(Duration::ZERO)
            .max(Duration::from_millis(1));
        self.clock.sleep(pause).await;
        let waited = seconds(started, self.clock.now());
        self.state().waits.push(Wait {
            source: blocked.source,
            resource: blocked.resource,
            seconds: whole_seconds(waited),
            reason: blocked.reason,
        });
    }

    /// Wait for this process's one slot and take it: once its holder ends or its call ends,
    /// which are the only proofs the gate is given that it is over.
    async fn take_slot(&self) {
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            {
                let mut state = self.state();
                if state.slot.is_none() {
                    state.slot = Some(Slot { sent: false });
                    return;
                }
            }
            released.await;
        }
    }

    fn release(&self, state: &mut State) {
        state.slot = None;
        self.released.notify_waiters();
    }

    /// One admitted request is over, however it ended: onetaskgraph tells this exactly once for
    /// every request the gate admitted. An admitted request holding no slot is taken to be the
    /// one ended while any is open; otherwise it is the slot's holder, which gives the slot up.
    pub fn request_ended(&self) {
        let mut state = self.state();
        if state.unslotted > 0 {
            state.unslotted -= 1;
        } else if state.slot.is_some_and(|slot| slot.sent) {
            state.in_flight = None;
            self.release(&mut state);
        }
    }

    /// What the response to the request in flight said is left: each reading refreshes its
    /// window at no quota cost, and teaches the governor what a request of its kind costs.
    pub fn after_response(&self, readings: &[Reading]) {
        let now = self.clock.now();
        let history = self.settings().burn_history_seconds;
        let (in_flight, sources) = {
            let mut state = self.state();
            // A response reporting nothing teaches nothing, and leaves the slot's request to
            // learn from its own response; the slot is given up only when that request ends.
            let in_flight = if readings.is_empty() {
                None
            } else {
                state.in_flight.take()
            };
            let sources: Vec<String> = match &state.call {
                Some(call) => call.sources.clone(),
                None => state.read_at.keys().cloned().collect(),
            };
            (in_flight, sources)
        };
        let backends = self.backends_of(&sources);
        self.shared(&backends, |shared| {
            for reading in readings {
                let learn = in_flight
                    .as_ref()
                    .filter(|(resource, _, _)| *resource == reading.resource)
                    .map(|(_, kind, estimate)| (*kind, *estimate));
                for backend in &backends {
                    let held = shared.entry(backend.clone()).or_default();
                    if learn.is_some()
                        && let Some(at) = held.in_flight.iter().position(|flight| {
                            flight.process == self.process && flight.resource == reading.resource
                        })
                    {
                        held.in_flight.remove(at);
                    }
                    observe(held, reading, now, learn, history);
                }
            }
        });
    }

    async fn refresh_if_stale(&self, source: &str, settings: &GovernorSettings) -> bool {
        let now = self.clock.now();
        let stale = self
            .state()
            .read_at
            .get(source)
            .is_none_or(|read_at| seconds(*read_at, now) >= points(settings.sample_seconds));
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
        {
            let mut state = self.state();
            state.refreshing = false;
            state.quota_reads += 1;
            state.read_at.insert(source.to_owned(), now);
        }
        // A reading that could not be taken teaches nothing; the windows already held stand
        // until their resets.
        if let Ok(readings) = read {
            let backend = self.backend(source);
            self.shared(std::slice::from_ref(&backend), |shared| {
                let held = shared.entry(backend.clone()).or_default();
                for reading in &readings {
                    observe(held, reading, now, None, history);
                }
            });
        }
    }

    fn request_estimate(&self, sources: &[String], resource: &str, kind: Kind) -> f64 {
        let backends = self.backends_of(sources);
        self.shared(&backends, |shared| {
            backends
                .iter()
                .filter_map(|backend| {
                    shared.get(backend)?.resources.get(resource)?.request_cost[kind.index()].get()
                })
                .fold(None, |most: Option<f64>, cost| {
                    Some(most.map_or(cost, |most| most.max(cost)))
                })
                .unwrap_or(UNKNOWN_COST)
        })
    }

    fn call_estimate(&self, source: &str, resource: &str, kind: Kind) -> f64 {
        let backend = self.backend(source);
        self.shared(std::slice::from_ref(&backend), |shared| {
            shared
                .get(&backend)
                .and_then(|held| held.resources.get(resource))
                .and_then(|metered| {
                    metered.call_cost[kind.index()]
                        .get()
                        .or_else(|| metered.request_cost[kind.index()].get())
                })
                .unwrap_or(UNKNOWN_COST)
        })
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
        let backend = self.backend(source);
        self.shared(std::slice::from_ref(&backend), |shared| {
            let metered = shared.get_mut(&backend)?.resources.get_mut(resource)?;
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
        })
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

/// Take `reading` into what the governor holds of its backend kind: a new window kept, a lower
/// remaining in a known window turned into others' spend since its last reading, a higher one
/// before the reset ignored.
fn observe(
    held: &mut Shared,
    reading: &Reading,
    now: DateTime<Utc>,
    learn: Option<(Kind, f64)>,
    history_seconds: u64,
) {
    // What the reading can show of own spend: everything admitted but what is unanswered.
    let unanswered = held.unanswered(&reading.resource);
    let metered = held.resources.entry(reading.resource.clone()).or_default();
    metered.windows.retain(|resets_at, _| *resets_at > now);
    if reading.resets_at <= now {
        return;
    }
    let own = metered.own - unanswered;
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
