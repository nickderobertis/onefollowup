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
//! (`resets_at`), and admission is against the live window that answered last — the latest
//! response's or reading's — alone: a token is answered from two windows of one resource at
//! once, interleaved request by request, so no reading of one window decides anything of the
//! other. A remaining that rises inside one window before its reset is ignored, though the
//! window is still the one that answered. A request of estimated cost `c` is admitted while
//! `remaining - own spend no reading has shown - c >= floor + reserve`, where own spend no
//! reading has shown is every process's requests still in flight and any answered with no
//! reading of the resource, the floor is `floor_percent` of the limit and the reserve is others'
//! peak burn rate in one window times the seconds to the window's reset times `reserve_factor`.
//! Otherwise it waits, refreshing the reading every [`HELD_REFRESH`], until a reading admits it
//! or the window resets. It only ever waits: a batch it paces completes, however long it takes.
//!
//! **Own spend is what responses report.** Each request's own cost is what its response said
//! it cost ([`Governor::cost_reported`]: GitHub's `rateLimit { cost }`), else what it was
//! admitted at; the observed cost of a request of its kind is learned from reported costs
//! alone. Others' burn is a window's fall between two readings less the own cost of the request
//! whose response carried the later one and less own spend no reading showed meanwhile — never
//! a fall booked as onefollowup's, nor onefollowup's spend booked as others' — kept window by
//! window, so the reserve each window keeps is the burn other work really caused in one.
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
//! **One state per host.** What is learned of a backend kind — its windows, everyone's own spend
//! no reading has shown and the cost still in flight, others' burn history, the observed costs and the
//! content-write window — is one state every onefollowup process on the host shares, in
//! `governor.state_dir` (the private `host` module), so admission counts every process's
//! spend and a fresh process starts from the burn estimate others learned. Sources are keyed by
//! their backend kind ([`Governor::set_backends`]), so every destination of one kind shares one
//! state; with no state directory the state is the process's alone. Others' burn history older
//! than one peak window is folded into one interval per window per fifth of a peak window, so the state's
//! size is bounded by `burn_history_seconds` rather than by how many requests earlier windows
//! made, and each request's bookkeeping costs the same however long a host has run. A call is
//! admitted at its boundary against its own next request's estimate, never an earlier whole
//! call's spend, which no window might leave room for. Each process still holds its
//! own slot, so onefollowup's own spend crosses a floor by at most one request's estimate error
//! per process.
//!
//! **Content-creating writes.** A request the gate is told creates content, in a destination
//! call, is admitted only while the backend kind's writes across the host in the last minute
//! and the last hour leave room under `max_content_writes_per_minute` and
//! `max_content_writes_per_hour` for every content item it may carry, and those items are
//! counted as it is admitted; otherwise it waits for the oldest counted writes to leave the
//! window. These are counts because no response reports GitHub's secondary content-creation
//! limit. The gate is told only that a request creates content, never how many contents a
//! source batched into it (GitHub's `comment add-many` sends several `addComment`s in one
//! request), so a call that may create several declares how many
//! ([`Governor::admit_content_call`], at most [`Governor::content_chunk`], which the store splits
//! its batches by): its first content-creating request is admitted only with room for all of
//! them and counts all of them, and each later one re-counts the same items at its own instant,
//! which only ever keeps them in the windows longer — however the source spreads them over its
//! requests, no item is counted earlier than it is sent and none is left uncounted. A call that
//! declares nothing counts one per content-creating request.

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
    /// When `remaining` was read.
    read_at: DateTime<Utc>,
    /// When a response last came from this window, its remaining risen or not: the window
    /// with the latest is the one that answered.
    answered_at: DateTime<Utc>,
    /// [`Metered::unread`] when it was read, so own spend no reading has shown since is the
    /// difference.
    unread_at_read: f64,
}

/// What others spent of one resource between two readings of one window.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Interval {
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    spend: f64,
    /// The window it fell in, by its reset: others' burn is measured window by window.
    window: DateTime<Utc>,
}

/// One resource of one backend kind.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Metered {
    windows: BTreeMap<DateTime<Utc>, Window>,
    /// Every point onefollowup, in any process on the host, spent of this resource in a
    /// request whose response showed no reading of it, cumulative: spend some window holds
    /// that none of its readings has shown.
    unread: f64,
    history: VecDeque<Interval>,
    /// What one request cost, by [`Kind`], as its response reported it.
    request_cost: [Mean; 2],
    /// What one request was admitted at — its hint, or the estimate standing in for one — by
    /// [`Kind`]: what a call's next pending request is estimated at, at the call boundary.
    #[serde(default)]
    admitted_cost: [Mean; 2],
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
    /// How many content items the call may create, when it declared it
    /// ([`Governor::admit_content_call`]).
    contents: Option<usize>,
}

/// A declared call's content items as last counted: when, and how many, so a later
/// content-creating request of the call moves them to its own instant rather than adding to
/// them.
#[derive(Debug, Clone, Copy)]
struct Stamp {
    at: DateTime<Utc>,
    items: usize,
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
    /// The slot's request, until it ends: what it was admitted at, and what its response
    /// read and said it cost, which are taken in together once it ends.
    in_flight: Option<Answer>,
    /// Admitted requests holding no slot — a refresh's own allowance read, a write to a
    /// source's own files, any request while the governor is off — not yet ended, oldest
    /// first, each with what it was admitted at and what its response read and reported.
    loose: VecDeque<Answer>,
    quota_reads: u64,
    calls: u64,
    waits: Vec<Wait>,
}

/// One admitted request, from its admission or its response to its end.
#[derive(Debug, Clone, Default)]
struct Answer {
    /// The resource it was admitted on: `None` for a request admitted holding no slot.
    resource: Option<String>,
    kind: Option<Kind>,
    /// What it was admitted at, and counted in flight at.
    estimate: f64,
    /// Every reading its response carried.
    readings: Vec<Reading>,
    /// What its response said it cost, and of which resource.
    reported: Option<(String, u64)>,
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
    /// What the held request is estimated to cost.
    cost: f64,
}

/// Why the content-write caps hold a write, with what its wait line names.
#[derive(Debug, Clone)]
struct ContentBlocked {
    source: String,
    resource: String,
    written: usize,
    /// How many content items the held request is counted as.
    items: usize,
    cap: u64,
    resume_by: DateTime<Utc>,
    reason: Reason,
}

/// Why [`Governor::admit`] refuses a request now.
enum Refusal {
    /// A window's floor or reserve.
    Quota(Box<Blocked>),
    /// A content-write window's cap.
    Content(Box<ContentBlocked>),
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

/// What [`Governor::on_admitted`] runs.
pub type Admitted = Arc<dyn Fn() + Send + Sync>;

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
    /// Run once each request is admitted, before it is sent: a test's seam.
    admitted: Mutex<Option<Admitted>>,
    /// Woken whenever the slot is given up.
    released: Notify,
    /// The call in progress's declared content items as last counted; held across a
    /// content-creating request's admission, so two of one call never move the same count.
    stamp: tokio::sync::Mutex<Option<Stamp>>,
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
            admitted: Mutex::new(None),
            released: Notify::new(),
            stamp: tokio::sync::Mutex::new(None),
        }
    }

    /// Run `hook` each time a request is admitted, before it is sent: the seam a test holds
    /// two processes' admissions at to see what each counted of the other.
    pub fn on_admitted(&self, hook: Admitted) {
        *self.admitted.lock().unwrap_or_else(PoisonError::into_inner) = Some(hook);
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
        self.admit_declared(sources, kind, None).await;
    }

    /// [`Self::admit_call`] for a call that may create up to `contents` content items (comments,
    /// issues), however its source batches them into requests: every content-creating request
    /// of the call is admitted only while the content-write windows have room for all of them.
    /// `contents` above [`Self::content_chunk`] is admitted only into windows otherwise empty,
    /// so a caller splits its batches by that chunk.
    pub async fn admit_content_call(&self, sources: &[String], kind: Kind, contents: usize) {
        self.admit_declared(sources, kind, Some(contents)).await;
    }

    /// The most content items one call should declare: the lesser cap, or `None` while the
    /// governor is off.
    #[must_use]
    pub fn content_chunk(&self) -> Option<usize> {
        let settings = self.settings();
        settings.enabled.then(|| {
            let cap = settings
                .max_content_writes_per_minute
                .min(settings.max_content_writes_per_hour)
                .max(1);
            usize::try_from(cap).unwrap_or(usize::MAX)
        })
    }

    async fn admit_declared(&self, sources: &[String], kind: Kind, contents: Option<usize>) {
        *self.stamp.lock().await = None;
        let settings = self.settings().clone();
        {
            let mut state = self.state();
            state.calls += 1;
            state.call = Some(Call {
                sources: sources.to_vec(),
                kind,
                contents,
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

    /// Wait until `source` admits a batch together: one read call (when `read`) and `writes`
    /// write requests, at the costs observed of each, against every window the governor holds
    /// of `source`'s backend kind. It reserves nothing: each request is still admitted on its
    /// own when it is sent. A wait is recorded as any other.
    pub async fn admit_batch(&self, source: &str, read: bool, writes: usize) {
        let settings = self.settings().clone();
        if !settings.enabled {
            return;
        }
        let sources = [source.to_owned()];
        let backend = self.backend(source);
        let mut refreshed = self.refresh_if_stale(source, &settings).await;
        loop {
            let mut governed: BTreeSet<String> =
                self.shared(std::slice::from_ref(&backend), |shared| {
                    shared
                        .get(&backend)
                        .map(|held| held.resources.keys().cloned().collect())
                        .unwrap_or_default()
                });
            governed.extend(self.state().requested.iter().cloned());
            governed.extend(settings.min_peak_per_minute.keys().cloned());
            let mut blocked = None;
            for resource in &governed {
                let reading = if read {
                    self.call_estimate(source, resource, Kind::Read)
                } else {
                    0.0
                };
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "a batch's write count is far under 2^53"
                )]
                let cost = reading
                    + writes as f64 * self.request_estimate(&sources, resource, Kind::Write);
                if let Some(refusal) = self.decide(source, resource, cost, &settings) {
                    blocked = Some((refusal, cost));
                    break;
                }
            }
            match blocked {
                None => return,
                Some(_) if !refreshed => {
                    self.refresh(source).await;
                    refreshed = true;
                }
                Some((refusal, cost)) => self.hold(refusal, cost, &settings).await,
            }
        }
    }

    /// The call in progress has answered: every request of it is over.
    pub fn end_call(&self) {
        if let Ok(mut stamp) = self.stamp.try_lock() {
            *stamp = None;
        }
        let answers: Vec<Answer> = {
            let mut state = self.state();
            // Every request of the call is over, answered or not: what their responses said is
            // taken in, against the call's sources, before the call is let go.
            let answers = state
                .in_flight
                .take()
                .into_iter()
                .chain(state.loose.drain(..))
                .collect();
            self.release(&mut state);
            answers
        };
        for answer in answers {
            self.settle(answer);
        }
        let Some(call) = self.state().call.take() else {
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
                state.loose.push_back(Answer::default());
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
            // Paced by nothing, but what it spends is still onefollowup's own, never others'.
            self.state().loose.push_back(Answer {
                resource: Some(request.resource.clone()),
                kind: Some(kind),
                estimate: request.cost_hint.map_or(0.0, points),
                ..Answer::default()
            });
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
        let content_creating = request.content_creating && in_call;
        loop {
            for source in &sources {
                self.refresh_if_stale(source, &settings).await;
            }
            // Checked and counted in one transaction of the host state (`admit`). A request
            // refused on the estimate is held at once: the call was admitted against a fresh
            // reading, every response since refreshed it, and the hold's own refreshes are what
            // find the headroom come back — the estimate less than it cost — before a reset.
            // A content-creating write is counted at the instant the quota admits it, so it is
            // never counted earlier than it is sent.
            match self
                .admit(
                    &sources,
                    &request.resource,
                    (kind, cost),
                    content_creating,
                    &settings,
                )
                .await
            {
                Ok(()) => break,
                Err(Refusal::Quota(blocked)) => self.hold(*blocked, cost, &settings).await,
                Err(Refusal::Content(blocked)) => self.hold_content(*blocked).await,
            }
        }
        let hook = self
            .admitted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook();
        }
        let mut state = self.state();
        if slotted {
            state.in_flight = Some(Answer {
                resource: Some(request.resource.clone()),
                kind: Some(kind),
                estimate: cost,
                ..Answer::default()
            });
            state.slot = Some(Slot { sent: true });
        } else {
            state.loose.push_back(Answer {
                resource: Some(request.resource.clone()),
                kind: Some(kind),
                estimate: cost,
                ..Answer::default()
            });
        }
    }

    /// Admit one request of `cost` on `resource` to `sources` now, or say why not: every live
    /// window of every source checked against floor plus reserve and, for a content-creating
    /// request, the host's content-write windows against their caps — and, once admitted, its
    /// cost and its place in flight counted and the content write recorded — all in one
    /// transaction of the host state, so no other process admits against the same headroom
    /// between this one's check and its count.
    ///
    /// A content-creating request of a call that declared its contents is counted as all of
    /// them, the count its call's earlier requests made moved to now rather than added to.
    async fn admit(
        &self,
        sources: &[String],
        resource: &str,
        (kind, cost): (Kind, f64),
        content_creating: bool,
        settings: &GovernorSettings,
    ) -> Result<(), Refusal> {
        let declared = self.state().call.as_ref().and_then(|call| call.contents);
        let mut stamp = self.stamp.lock().await;
        let items = declared.unwrap_or(1).max(1);
        let previous = (*stamp).filter(|_| declared.is_some());
        let now = self.clock.now();
        let backends = self.backends_of(sources);
        let of: Vec<(String, String)> = sources
            .iter()
            .map(|source| (source.clone(), self.backend(source)))
            .collect();
        self.shared(&backends, |shared| {
            for (source, backend) in &of {
                if let Some(held) = shared.get_mut(backend)
                    && let Some(blocked) = decide_in(held, source, resource, cost, settings, now)
                {
                    return Err(Refusal::Quota(Box::new(blocked)));
                }
            }
            if content_creating {
                let source = sources.first().cloned().unwrap_or_default();
                for backend in &backends {
                    let held = shared.entry(backend.clone()).or_default();
                    if let Some(blocked) =
                        content_in(held, &source, resource, items, previous, settings, now)
                    {
                        return Err(Refusal::Content(Box::new(blocked)));
                    }
                }
            }
            for backend in &backends {
                let held = shared.entry(backend.clone()).or_default();
                if content_creating {
                    if let Some(previous) = previous {
                        unstamp(held, previous);
                    }
                    held.content_writes.extend(std::iter::repeat_n(now, items));
                }
                if let Some(metered) = held.resources.get_mut(resource)
                    && !metered.windows.is_empty()
                {
                    metered.admitted_cost[kind.index()].push(cost);
                    held.in_flight.push(Flight {
                        process: self.process.clone(),
                        resource: resource.to_owned(),
                        estimate: cost,
                    });
                }
            }
            Ok(())
        })?;
        if content_creating && declared.is_some() {
            *stamp = Some(Stamp { at: now, items });
        }
        Ok(())
    }

    /// Wait out a content-write window until the write it holds has room: one line on
    /// standard error, and the wait recorded.
    async fn hold_content(&self, blocked: ContentBlocked) {
        let started = self.clock.now();
        let (window, cap_key) = match blocked.reason {
            Reason::ContentPerHour => ("hour", "max_content_writes_per_hour"),
            _ => ("minute", "max_content_writes_per_minute"),
        };
        let adding = if blocked.items > 1 {
            format!(" and {} to add", blocked.items)
        } else {
            String::new()
        };
        (self.notices)(&format!(
            "onefollowup: holding content-creating writes to {} on {}: {} in the last {window} across this host{adding}, {cap_key} {}; resuming by {}",
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
    /// What its response read and reported is taken in now, once it can no longer change.
    pub fn request_ended(&self) {
        let answer = {
            let mut state = self.state();
            if !state.loose.is_empty() {
                state.loose.pop_front()
            } else if state.slot.is_some_and(|slot| slot.sent) {
                let answer = state.in_flight.take();
                self.release(&mut state);
                answer
            } else {
                None
            }
        };
        if let Some(answer) = answer {
            self.settle(answer);
        }
    }

    /// What the response to the request in flight said is left: each reading refreshes its
    /// window at no quota cost once the request ends, with what the response said it cost.
    pub fn after_response(&self, readings: &[Reading]) {
        // A response reporting nothing teaches nothing, and leaves the slot's request to learn
        // from its own response.
        if readings.is_empty() {
            return;
        }
        let mut state = self.state();
        match state.in_flight.as_mut() {
            Some(answer) if answer.readings.is_empty() => answer.readings = readings.to_vec(),
            _ => match state.loose.back_mut() {
                Some(answer) => answer.readings = readings.to_vec(),
                None => state.loose.push_back(Answer {
                    readings: readings.to_vec(),
                    ..Answer::default()
                }),
            },
        }
    }

    /// What the response to the request in flight said that very request cost of `resource`:
    /// its own spend, which is what the governor counts as onefollowup's, never a window's
    /// fall. Told at most once per request, after its readings and before it ends.
    pub fn cost_reported(&self, resource: &str, cost: u64) {
        let mut state = self.state();
        let reported = Some((resource.to_owned(), cost));
        match state.in_flight.as_mut() {
            Some(answer) => answer.reported = reported,
            None => match state.loose.back_mut() {
                Some(answer) => answer.reported = reported,
                None => state.loose.push_back(Answer {
                    reported,
                    ..Answer::default()
                }),
            },
        }
    }

    /// Take one ended request's answer into the host state: its cost in flight dropped, its
    /// own spend — what its response reported, else what it was admitted at — kept apart from
    /// every window's fall, its reported cost learned, and each reading refreshing its window.
    fn settle(&self, answer: Answer) {
        if answer.readings.is_empty() && answer.reported.is_none() && answer.resource.is_none() {
            return;
        }
        let now = self.clock.now();
        let history = Kept::of(&self.settings());
        let sources: Vec<String> = {
            let state = self.state();
            match &state.call {
                Some(call) => call.sources.clone(),
                None => state.read_at.keys().cloned().collect(),
            }
        };
        // What it spent, and of which resource: what its response reported, else — for a
        // metered request — the estimate it was admitted at.
        let spent: Option<(String, f64)> = match (&answer.reported, &answer.resource) {
            (Some((resource, cost)), _) => Some((resource.clone(), points(*cost))),
            (None, Some(resource)) => Some((resource.clone(), answer.estimate)),
            (None, None) => None,
        };
        let backends = self.backends_of(&sources);
        self.shared(&backends, |shared| {
            for backend in &backends {
                let held = shared.entry(backend.clone()).or_default();
                if let Some(resource) = &answer.resource
                    && let Some(at) = held.in_flight.iter().position(|flight| {
                        flight.process == self.process && flight.resource == *resource
                    })
                {
                    held.in_flight.remove(at);
                }
                if let Some((resource, spent)) = &spent
                    && let Some(metered) = held.resources.get_mut(resource)
                {
                    if let (Some(kind), Some((_, cost))) = (answer.kind, &answer.reported) {
                        metered.request_cost[kind.index()].push(points(*cost));
                    }
                    // Spend no reading of its resource showed is held by some window all the
                    // same, so it is counted against every one until each is read again.
                    let shown = answer
                        .readings
                        .iter()
                        .any(|reading| reading.resource == *resource);
                    if !shown && !metered.windows.is_empty() {
                        metered.unread += spent;
                    }
                }
                for reading in &answer.readings {
                    let own = spent
                        .as_ref()
                        .filter(|(resource, _)| *resource == reading.resource)
                        .map_or(0.0, |(_, spent)| *spent);
                    observe(held, reading, now, own, history);
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
        let history = Kept::of(&self.settings());
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
                    observe(held, reading, now, 0.0, history);
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

    /// What a call of `kind` to `source` is admitted against at its boundary: its own next
    /// pending request — the one it sends before the gate admits each later one on its own —
    /// at what a request of its kind has been admitted at, else the observed cost of one, else
    /// [`UNKNOWN_COST`]. Never a whole earlier call's spend: a call costing more than a window
    /// leaves above its floor would then hold every later call of its kind for good, however
    /// small.
    fn call_estimate(&self, source: &str, resource: &str, kind: Kind) -> f64 {
        let backend = self.backend(source);
        self.shared(std::slice::from_ref(&backend), |shared| {
            shared
                .get(&backend)
                .and_then(|held| held.resources.get(resource))
                .and_then(|metered| {
                    metered.admitted_cost[kind.index()]
                        .get()
                        .or_else(|| metered.request_cost[kind.index()].get())
                })
                .unwrap_or(UNKNOWN_COST)
        })
    }

    /// Why the live window of `resource` at `source` that answered last refuses a cost of
    /// `cost`, or `None` when it admits it (or none is held: a destination reporting nothing).
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
            decide_in(
                shared.get_mut(&backend)?,
                source,
                resource,
                cost,
                settings,
                now,
            )
        })
    }

    /// Wait out `blocked` until a fresh reading admits the request or its window resets:
    /// one line on standard error, a refresh every [`HELD_REFRESH`], and the wait recorded.
    async fn hold(&self, blocked: Blocked, cost: f64, settings: &GovernorSettings) {
        let started = self.clock.now();
        (self.notices)(&format!(
            "onefollowup: holding requests to {} on {}: remaining {:.0} of {}, floor {:.0}, reserve {:.0}; this request {:.1} of the window resetting {}; others burn {:.1}/min mean, {:.1}/min peak; resuming by {}",
            blocked.source,
            blocked.resource,
            blocked.remaining,
            blocked.limit,
            blocked.floor,
            blocked.reserve,
            blocked.cost,
            blocked.resets_at.to_rfc3339_opts(SecondsFormat::Secs, true),
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

/// Why the live window of `resource` in `held` that answered last refuses a cost of `cost`
/// now, or `None` when it admits it (or none is held). Only that window decides: one token is
/// answered from two windows at once, and no reading of one says anything of the other.
fn decide_in(
    held: &mut Shared,
    source: &str,
    resource: &str,
    cost: f64,
    settings: &GovernorSettings,
    now: DateTime<Utc>,
) -> Option<Blocked> {
    let unanswered = held.unanswered(resource);
    let metered = held.resources.get_mut(resource)?;
    metered.windows.retain(|resets_at, _| *resets_at > now);
    let (mean, peak) = burn(metered, settings.peak_window_seconds);
    let seeded = settings
        .min_peak_per_minute
        .get(resource)
        .copied()
        .unwrap_or(0.0);
    let peak = peak.max(seeded);
    // The window that answered last; of two answering at one instant, the lower.
    let (resets_at, window) = metered.windows.iter().max_by(|(_, one), (_, other)| {
        one.answered_at
            .cmp(&other.answered_at)
            .then(other.remaining.cmp(&one.remaining))
    })?;
    // Own spend no reading of it has shown yet: in flight in any process, or answered with no
    // reading of this resource since it was read.
    let remaining =
        points(window.remaining) - (metered.unread - window.unread_at_read) - unanswered;
    let floor = settings.floor_percent / 100.0 * points(window.limit);
    let to_reset = seconds(now, *resets_at).max(0.0);
    let reserve = peak / 60.0 * to_reset * settings.reserve_factor;
    if remaining - cost - floor - reserve >= 0.0 {
        return None;
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
    Some(Blocked {
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
        cost,
    })
}

/// Take the content items `stamp` counted out of `held`'s window: as many writes of its
/// instant, which are indistinguishable from any other process's of the same instant.
fn unstamp(held: &mut Shared, stamp: Stamp) {
    let mut left = stamp.items;
    held.content_writes.retain(|written| {
        if left > 0 && *written == stamp.at {
            left -= 1;
            false
        } else {
            true
        }
    });
}

/// Why `held`'s content-write windows hold a write of `items` content items now, or `None` when
/// both have room for all of them. The items `previous` counted for the same call are not
/// counted against it: this write moves them rather than adding to them.
fn content_in(
    held: &mut Shared,
    source: &str,
    resource: &str,
    items: usize,
    previous: Option<Stamp>,
    settings: &GovernorSettings,
    now: DateTime<Utc>,
) -> Option<ContentBlocked> {
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
        let mut moved = previous.map_or(0, |stamp| stamp.items);
        let within: Vec<DateTime<Utc>> = held
            .content_writes
            .iter()
            .copied()
            .filter(|written| *written > since)
            .filter(|written| {
                let ours = moved > 0 && previous.is_some_and(|stamp| stamp.at == *written);
                if ours {
                    moved -= 1;
                }
                !ours
            })
            .collect();
        let cap_places = usize::try_from(cap).unwrap_or(usize::MAX);
        // More items than the cap are admitted only into a window otherwise empty.
        let needed = items.min(cap_places);
        if within.len() + needed > cap_places {
            // The window has room once all but `cap - needed` of its writes left it.
            let leaving = within[within.len() + needed - cap_places - 1];
            return Some(ContentBlocked {
                source: source.to_owned(),
                resource: resource.to_owned(),
                written: within.len(),
                items,
                cap,
                resume_by: leaving + chrono::Duration::seconds(span),
                reason,
            });
        }
    }
    None
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
/// remaining in a known window turned into others' spend since its last reading — its fall
/// less `own`, what the response carrying it said onefollowup's own request cost of it, and
/// less own spend no reading showed since — and a higher one before the reset ignored.
fn observe(held: &mut Shared, reading: &Reading, now: DateTime<Utc>, own: f64, history: Kept) {
    let metered = held.resources.entry(reading.resource.clone()).or_default();
    metered.windows.retain(|resets_at, _| *resets_at > now);
    if reading.resets_at <= now {
        return;
    }
    let unread = metered.unread;
    match metered.windows.get_mut(&reading.resets_at) {
        Some(window) if reading.remaining > window.remaining => window.answered_at = now,
        Some(window) => {
            let fall = points(window.remaining - reading.remaining);
            let unread_since = unread - window.unread_at_read;
            if now > window.read_at {
                metered.history.push_back(Interval {
                    from: window.read_at,
                    to: now,
                    spend: (fall - own - unread_since).max(0.0),
                    window: reading.resets_at,
                });
            }
            *window = Window {
                limit: reading.limit,
                remaining: reading.remaining,
                read_at: now,
                answered_at: now,
                unread_at_read: unread,
            };
        }
        None => {
            metered.windows.insert(
                reading.resets_at,
                Window {
                    limit: reading.limit,
                    remaining: reading.remaining,
                    read_at: now,
                    answered_at: now,
                    unread_at_read: unread,
                },
            );
        }
    }
    let horizon =
        now - chrono::Duration::seconds(i64::try_from(history.seconds).unwrap_or(i64::MAX / 1000));
    while metered
        .history
        .front()
        .is_some_and(|interval| interval.to < horizon)
    {
        metered.history.pop_front();
    }
    fold(&mut metered.history, now, history.peak_window_seconds);
}

/// What of others' burn history is kept: how long it is remembered, and the span its peak is
/// measured over.
#[derive(Debug, Clone, Copy)]
struct Kept {
    seconds: u64,
    peak_window_seconds: u64,
}

impl Kept {
    fn of(settings: &GovernorSettings) -> Self {
        Self {
            seconds: settings.burn_history_seconds,
            peak_window_seconds: settings.peak_window_seconds,
        }
    }
}

/// How many buckets one peak window's span of expired history is folded into.
const FOLDS_PER_PEAK_WINDOW: i64 = 5;

/// Fold the intervals of `history` that ended more than one peak window before `now` — which
/// no reading's live window reaches any more — into one interval per window per bucket of a
/// fifth of a peak window, by when each ended: the first start to the last end of the bucket's
/// intervals of that window, their spend summed. So what is kept of earlier windows is at most
/// one interval per window a bucket of `burn_history_seconds` answered from, however many
/// requests they made, and each request's bookkeeping costs the same on a host that has carried
/// a day of history as on a fresh one. Spend is never lost; within a folded bucket it is spread
/// pro rata, as the peak reads any interval.
fn fold(history: &mut VecDeque<Interval>, now: DateTime<Utc>, peak_window_seconds: u64) {
    let window = i64::try_from(peak_window_seconds).unwrap_or(i64::MAX / 1000);
    let bucket_millis = (window * 1000 / FOLDS_PER_PEAK_WINDOW).max(1);
    let expired = now - chrono::Duration::seconds(window);
    let bucket = |interval: &Interval| interval.to.timestamp_millis().div_euclid(bucket_millis);
    // Nothing to fold until two expired intervals of one window share a bucket.
    let mut seen: Vec<(i64, DateTime<Utc>)> = Vec::new();
    let foldable = history
        .iter()
        .take_while(|interval| interval.to < expired)
        .any(|interval| {
            let key = (bucket(interval), interval.window);
            if seen.contains(&key) {
                return true;
            }
            seen.retain(|(held, _)| *held == key.0);
            seen.push(key);
            false
        });
    if !foldable {
        return;
    }
    let mut folded: VecDeque<Interval> = VecDeque::with_capacity(history.len());
    // The folded interval of each window in the bucket being folded, by its place in `folded`.
    let mut open: Vec<(i64, DateTime<Utc>, usize)> = Vec::new();
    for interval in history.drain(..) {
        if interval.to < expired {
            let at = bucket(&interval);
            if let Some(&(_, _, place)) = open
                .iter()
                .find(|(held, window, _)| *held == at && *window == interval.window)
            {
                let last = &mut folded[place];
                last.from = last.from.min(interval.from);
                last.to = last.to.max(interval.to);
                last.spend += interval.spend;
                continue;
            }
            open.retain(|(held, _, _)| *held == at);
            open.push((at, interval.window, folded.len()));
        }
        folded.push_back(interval);
    }
    *history = folded;
}

/// Others' mean and peak burn of one resource in one window, in points a minute, the most of
/// any window `metered` remembers: each window's peak the most others spent of it over any
/// `window_seconds` ending at one of its readings, pro rata within each interval — a history
/// shorter than one span counts what it did not see as nothing, which the seeded peak stands
/// in for. A token answered from two windows at once spends each at its own rate, and each
/// window keeps its own reserve, so their falls are never added together.
fn burn(metered: &Metered, window_seconds: u64) -> (f64, f64) {
    let mut windows: BTreeMap<DateTime<Utc>, Vec<Interval>> = BTreeMap::new();
    for interval in &metered.history {
        windows.entry(interval.window).or_default().push(*interval);
    }
    windows
        .values()
        .map(|intervals| burn_of(intervals, window_seconds))
        .fold((0.0, 0.0), |(mean, peak), (one, two)| {
            (f64::max(mean, one), f64::max(peak, two))
        })
}

/// Others' mean and peak burn over `history`, the intervals of one window, in points a minute.
fn burn_of(history: &[Interval], window_seconds: u64) -> (f64, f64) {
    let (Some(first), Some(last)) = (history.first(), history.last()) else {
        return (0.0, 0.0);
    };
    let span = seconds(first.from, last.to);
    let total: f64 = history.iter().map(|interval| interval.spend).sum();
    let mean = if span > 0.0 { total / span * 60.0 } else { 0.0 };
    let width = points(window_seconds);
    let width_nanos = whole_millis(width) * 1_000_000;
    // Each interval in nanoseconds since the epoch, in the order kept: by when it ended, since
    // every interval ends at the reading that recorded it.
    let nanos = |at: DateTime<Utc>| at.timestamp_nanos_opt().unwrap_or(i64::MAX);
    let held: Vec<(i64, i64, f64)> = history
        .iter()
        .map(|interval| (nanos(interval.from), nanos(interval.to), interval.spend))
        .collect();
    // In that order the intervals overlapping a span are one run of them, so only that run is
    // read: none ending by its start, and none starting at or after its end, which every one
    // ending `longest` past it does. Out of order — never written so — every one is read.
    let ordered = held.is_sorted_by_key(|(_, to, _)| *to);
    let longest = held
        .iter()
        .map(|(from, to, _)| to - from)
        .max()
        .unwrap_or(0);
    // A span in seconds, at the millisecond precision `seconds` states it in.
    #[allow(
        clippy::cast_precision_loss,
        reason = "a span of under 2^53 milliseconds converts exactly"
    )]
    let span_of = |from: i64, to: i64| ((to - from) / 1_000_000) as f64 / 1000.0;
    let mut peak: f64 = 0.0;
    for &(_, end, _) in &held {
        let start = end - width_nanos;
        let first = if ordered {
            held.partition_point(|(_, to, _)| *to <= start)
        } else {
            0
        };
        let mut spent = 0.0;
        for &(from, to, spend) in &held[first..] {
            if ordered && to - longest >= end {
                break;
            }
            let length = span_of(from, to);
            let overlap = span_of(from.max(start), to.min(end));
            if length > 0.0 && overlap > 0.0 {
                spent += spend * overlap / length;
            }
        }
        peak = peak.max(spent / width * 60.0);
    }
    (mean, peak)
}
