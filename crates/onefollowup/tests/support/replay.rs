//! `spike-quota-burn`'s two real traces (`fixtures/quota-burn/`, committed as that spike's
//! branch holds them) replayed as GitHub's rate-limit state on a virtual clock.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use onefollowup::governor::Reading;

use super::instant;

/// What onetaskgraph's request gate contract names a source's write to its own files.
const LOCAL: &str = "local";

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

pub const TRACES: [&str; 2] = [
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
pub struct Replay {
    start: f64,
    others: BTreeMap<String, Others>,
    own: Mutex<BTreeMap<(String, i64), f64>>,
}

impl Replay {
    pub fn of(name: &str) -> Self {
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

    pub fn start(&self) -> DateTime<Utc> {
        instant(self.start.ceil() as i64)
    }

    /// Every window at `at`: each traced resource as others left it, less what this pass spent
    /// of it, and `metered` — one of them — again under the name `local`, the resource a
    /// local-md destination names each record write by, stated in units of `unit` points, so
    /// that one record a pass writes over local-md draws `unit` points of that traced window.
    pub fn readings(&self, at: DateTime<Utc>, metered: &str, unit: f64) -> Vec<Reading> {
        let t = at.timestamp() as f64;
        let own = self.own.lock().unwrap();
        let left = |resource: &str, others: &Others, unit: f64| {
            let (previous, next) = others.window(t);
            let left = match previous {
                Some(reset) => others.limit - (others.at(t) - others.at(reset)),
                None => others.remaining - (others.at(t) - others.at(others.start)),
            };
            let mine = own
                .get(&(resource.to_owned(), next as i64))
                .copied()
                .unwrap_or(0.0);
            Reading {
                resource: resource.to_owned(),
                limit: (others.limit / unit) as u64,
                remaining: ((left - mine) / unit).max(0.0).floor() as u64,
                resets_at: instant(next as i64),
            }
        };
        let mut readings: Vec<Reading> = self
            .others
            .iter()
            .map(|(resource, others)| left(resource, others, 1.0))
            .collect();
        readings.push(left(LOCAL, &self.others[metered], unit));
        readings
    }

    /// The pass spent `points` of `metered`'s window at `at`, which its `local` view shows.
    pub fn spend(&self, metered: &str, points: f64, at: DateTime<Utc>) {
        let (_, next) = self.others[metered].window(at.timestamp() as f64);
        *self
            .own
            .lock()
            .unwrap()
            .entry((LOCAL.to_owned(), next as i64))
            .or_default() += points;
    }
}
