//! What the governor's tests share: a virtual clock and GitHub's rate-limit state as a script
//! of it — the one thing a case doubles.

#![allow(
    dead_code,
    reason = "each test crate including this module uses its own part of it"
)]

pub mod replay;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use onefollowup::config::GovernorSettings;
use onefollowup::governor::{Clock, Pending, Reading, Readings};

/// The destination every case governs.
pub const DESTINATION: &str = "followups";

/// A clock that moves only when told to or waited on: a wait passes at once, its length added.
pub struct Virtual {
    pub now: Mutex<DateTime<Utc>>,
    pub waited: Mutex<Option<DateTime<Utc>>>,
    /// Past this a wait fails the case rather than running on.
    pub limit: Mutex<Option<DateTime<Utc>>>,
}

impl Virtual {
    pub fn at(start: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self {
            now: Mutex::new(start),
            waited: Mutex::new(None),
            limit: Mutex::new(Some(start + chrono::Duration::days(2))),
        })
    }

    pub fn advance(&self, seconds: f64) {
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

pub type Script = dyn Fn(DateTime<Utc>) -> Vec<Reading> + Send + Sync;

/// What a case runs at each reading, before it is answered: an assertion about the
/// destination while held, or a change a person makes to it.
pub type Hook = dyn Fn(DateTime<Utc>) + Send + Sync;

/// GitHub's rate-limit state as a script of virtual time, recording when it was read.
pub struct Scripted {
    pub clock: Arc<Virtual>,
    pub script: Box<Script>,
    pub reads: Mutex<Vec<DateTime<Utc>>>,
    pub hook: Mutex<Option<Box<Hook>>>,
}

impl Scripted {
    pub fn new(
        clock: &Arc<Virtual>,
        script: impl Fn(DateTime<Utc>) -> Vec<Reading> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            clock: Arc::clone(clock),
            script: Box::new(script),
            reads: Mutex::new(Vec::new()),
            hook: Mutex::new(None),
        })
    }

    pub fn on_read(&self, hook: impl Fn(DateTime<Utc>) + Send + Sync + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(hook));
    }

    pub fn reads(&self) -> Vec<DateTime<Utc>> {
        self.reads.lock().unwrap().clone()
    }
}

impl Readings for Scripted {
    fn read<'a>(&'a self, source: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        assert_eq!(source, DESTINATION, "only the destination is read");
        let now = self.clock.now();
        self.reads.lock().unwrap().push(now);
        if let Some(hook) = self.hook.lock().unwrap().as_ref() {
            hook(now);
        }
        let readings = (self.script)(now);
        Box::pin(async move { Ok(readings) })
    }
}

pub fn instant(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).unwrap()
}

/// 2026-11-22T00:00:00Z: the start of every scripted case.
pub const T0: i64 = 1_795_305_600;

pub fn reading(resource: &str, remaining: u64, resets_at: i64) -> Reading {
    Reading {
        resource: resource.to_owned(),
        limit: 5000,
        remaining,
        resets_at: instant(resets_at),
    }
}

/// The shipped defaults, read from the shipped file.
pub fn shipped() -> GovernorSettings {
    onefollowup::config::defaults("test-host").governor
}

/// What a [`Simulated`] clock runs each time it is about to move: the instant it leaves.
pub type Advance = dyn Fn(DateTime<Utc>) + Send + Sync;

/// Why a participant's wait ended in a panic: the case killed it, as `SIGKILL` would.
pub struct Killed;

struct Simulation {
    now: DateTime<Utc>,
    /// Threads running a process on this clock; time moves only once every one waits.
    participants: usize,
    /// Each waiting participant's ticket, with when it wakes and its thread.
    sleepers: std::collections::BTreeMap<u64, (DateTime<Utc>, std::thread::ThreadId)>,
    tickets: u64,
    /// Participants to kill at an instant, each woken then with [`Killed`].
    kill: Vec<(std::thread::ThreadId, DateTime<Utc>)>,
    killed: Option<u64>,
}

/// A clock several processes, each on a thread of its own, share: a wait blocks until every
/// participant waits, then time jumps to the earliest wake and only that waiter goes on. Work
/// between waits takes no time, so what one process does at an instant every other sees
/// before time moves.
pub struct Simulated {
    simulation: Mutex<Simulation>,
    moved: std::sync::Condvar,
    on_advance: Mutex<Option<Box<Advance>>>,
    limit: DateTime<Utc>,
}

/// One thread's part in a [`Simulated`] clock, given up when dropped (a kill's unwinding
/// included).
pub struct Participant(Arc<Simulated>);

impl Drop for Participant {
    fn drop(&mut self) {
        let mut simulation = self.0.lock();
        simulation.participants -= 1;
        self.0.moved.notify_all();
    }
}

impl Simulated {
    pub fn at(start: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self {
            simulation: Mutex::new(Simulation {
                now: start,
                participants: 0,
                sleepers: std::collections::BTreeMap::new(),
                tickets: 0,
                kill: Vec::new(),
                killed: None,
            }),
            moved: std::sync::Condvar::new(),
            on_advance: Mutex::new(None),
            limit: start + chrono::Duration::days(2),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Simulation> {
        self.simulation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Take part from this thread until the answer is dropped.
    pub fn join(self: &Arc<Self>) -> Participant {
        self.lock().participants += 1;
        Participant(Arc::clone(self))
    }

    /// Run `hook` with the instant the clock leaves, every time it is about to move.
    pub fn on_advance(&self, hook: impl Fn(DateTime<Utc>) + Send + Sync + 'static) {
        *self.on_advance.lock().unwrap() = Some(Box::new(hook));
    }

    /// Kill the participant on `thread` at `at`: its wait spanning that instant ends there in
    /// a [`Killed`] panic. A participant that waits across no such instant is left alone.
    pub fn kill(&self, thread: std::thread::ThreadId, at: DateTime<Utc>) {
        self.lock().kill.push((thread, at));
    }

    /// How many participants wait on the clock now.
    pub fn waiting(&self) -> usize {
        self.lock().sleepers.len()
    }

    /// Move the clock by `seconds` while nobody waits: the case's own passage of time.
    pub fn advance(&self, seconds: i64) {
        let mut simulation = self.lock();
        assert!(simulation.sleepers.is_empty(), "nobody waits");
        let from = simulation.now;
        if let Some(hook) = self.on_advance.lock().unwrap().as_ref() {
            hook(from);
        }
        simulation.now = from + chrono::Duration::seconds(seconds);
    }
}

impl Clock for Simulated {
    fn now(&self) -> DateTime<Utc> {
        self.lock().now
    }

    fn sleep(&self, duration: Duration) -> Pending<'_, ()> {
        let me = std::thread::current().id();
        let mut simulation = self.lock();
        let ticket = simulation.tickets;
        simulation.tickets += 1;
        let wake = simulation.now
            + chrono::Duration::milliseconds(i64::try_from(duration.as_millis()).unwrap().max(1));
        simulation.sleepers.insert(ticket, (wake, me));
        loop {
            if simulation.killed == Some(ticket) {
                simulation.killed = None;
                self.moved.notify_all();
                drop(simulation);
                std::panic::panic_any(Killed);
            }
            if !simulation.sleepers.contains_key(&ticket) {
                return Box::pin(async {});
            }
            if simulation.sleepers.len() == simulation.participants {
                let (&first, &(at, _)) = simulation
                    .sleepers
                    .iter()
                    .min_by_key(|(ticket, (at, _))| (*at, **ticket))
                    .unwrap();
                let victim = simulation
                    .kill
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, when))| *when < at)
                    .find_map(|(entry, (victim, when))| {
                        let (ticket, _) = simulation
                            .sleepers
                            .iter()
                            .find(|(_, (_, sleeper))| sleeper == victim)?;
                        Some((entry, *ticket, *when))
                    });
                let (chosen, next) = victim.map_or((first, at), |(_, ticket, when)| (ticket, when));
                if let Some((entry, ticket, _)) = victim {
                    simulation.kill.remove(entry);
                    simulation.killed = Some(ticket);
                }
                if next > simulation.now {
                    if let Some(hook) = self.on_advance.lock().unwrap().as_ref() {
                        hook(simulation.now);
                    }
                    assert!(next < self.limit, "the case waited past {}", self.limit);
                    simulation.now = next;
                }
                simulation.sleepers.remove(&chosen);
                self.moved.notify_all();
                continue;
            }
            simulation = self
                .moved
                .wait(simulation)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}
