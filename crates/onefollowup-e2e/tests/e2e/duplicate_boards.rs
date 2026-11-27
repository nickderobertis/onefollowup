//! The destinations the filing-duplicates journeys file against, generated rather than
//! committed: a check may not read the real `followups` board, and a snapshot of it would be
//! thousands of tickets to keep current.
//!
//! `fixtures/filing-duplicates.json` holds 13 of the merged clusters `spike-filing-duplicates`
//! replayed whose later members its terms-plus-file search caught, worded as filed: each later
//! member's draft (its title as the draft carried it, its root cause as the body, its failing
//! file) and each earlier member's title, root cause and suggested fix, with the status the
//! journeys give it — `open`, closed as `fixed`, or closed as a `duplicate` of a survivor.
//! Their origins are `github.com/example/<repository>`. [`crowd`] adds the unrelated tickets.

use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::world::{COMMIT, HOST, World};

/// One merged cluster: its later member's draft and its earlier members.
#[derive(Debug, Clone, Deserialize)]
pub struct Cluster {
    /// The cluster's surviving ticket.
    pub cluster: String,
    /// The later member, whose filing should have found an earlier one.
    pub later: String,
    /// The repository the later member is about.
    pub repository: String,
    /// The later member's draft.
    pub draft: DraftText,
    /// The earlier members.
    pub earlier: Vec<Earlier>,
}

/// A draft, as its batch recorded it.
#[derive(Debug, Clone, Deserialize)]
pub struct DraftText {
    /// Its title.
    pub title: String,
    /// Its body.
    pub body: String,
    /// The files it names.
    pub paths: Vec<String>,
}

/// An earlier member of a cluster.
#[derive(Debug, Clone, Deserialize)]
pub struct Earlier {
    /// Its key on the board.
    pub key: String,
    /// The repository it is filed in.
    pub repository: String,
    /// Its title.
    pub title: String,
    /// `open`, `fixed` or `duplicate`.
    pub status: String,
    /// Its root cause and suggested fix.
    pub body: String,
}

#[derive(Deserialize)]
struct Fixture {
    clusters: Vec<Cluster>,
}

static CLUSTERS: LazyLock<Vec<Cluster>> = LazyLock::new(|| {
    serde_json::from_str::<Fixture>(include_str!("fixtures/filing-duplicates.json"))
        .expect("the clusters fixture is JSON of its shape")
        .clusters
});

/// Every cluster of the fixture.
pub fn clusters() -> Vec<Cluster> {
    CLUSTERS.clone()
}

/// The repositories the boards file in.
pub const REPOSITORIES: [&str; 6] = [
    "onejudge",
    "onepipeline",
    "onepipeline-ui",
    "ai-orchestrator",
    "onevcs",
    "onebudgetspec",
];

/// A repository's origin.
pub fn origin(repository: &str) -> String {
    format!("github.com/example/{repository}")
}

/// The local-md id an earlier member is filed under.
pub fn item_id(key: &str) -> String {
    key.replace('#', "-")
}

/// The root-cause slug an earlier member's record names: its title's first words.
pub fn slug_of(title: &str) -> String {
    let invariant = title.split_once(": ").map_or(title, |(_, rest)| rest);
    invariant
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .take(7)
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

/// The local-md status a fixture status is held at.
fn status_of(status: &str) -> &'static str {
    match status {
        "open" => "backlog",
        "fixed" => "done",
        "duplicate" => "cancelled",
        other => panic!("the fixture status {other:?} is open, fixed or duplicate"),
    }
}

/// Write `destination`'s item `id`: a follow-up ticket `batch` filed in `repository`.
pub fn ticket(
    world: &World,
    destination: &str,
    id: &str,
    (repository, title, root_cause): (&str, &str, &str),
    batch: &str,
    status: &str,
    body: &str,
) {
    let origin = origin(repository);
    let record = record(&origin, root_cause, batch);
    world.item(
        destination,
        id,
        &format!(
            "title: {}\nstatus: {}\npriority: medium\nmetadata:\n  orchestrator.follow-up: {record}",
            Value::from(title),
            status_of(status)
        ),
        &format!("{body}\n"),
    );
}

/// The follow-up record `batch` left on a ticket of root cause `root_cause` about `origin`.
pub fn record(origin: &str, root_cause: &str, batch: &str) -> Value {
    json!({
        "schema": 14,
        "root_cause": root_cause,
        "repository": origin,
        "created_by_run": batch,
        "owning_runs": [batch],
        "drafts": [format!("drafts:{batch}/drafts/x")],
        "basis": {origin: COMMIT},
        "verified_at": "2026-10-01T11:30:00Z",
        "host": HOST,
        "priority_estimate": "medium",
        "board_estimate": "medium",
        "frequency": "intermittent",
        "impact": {
            "Severity": "high",
            "Workaround": "re-run the failed step",
            "Severity with the workaround": "medium",
        },
    })
}

/// File `earlier` on `destination` as another batch left it, answering its id and root cause.
pub fn place(world: &World, destination: &str, earlier: &Earlier) -> (String, String) {
    let id = item_id(&earlier.key);
    let root_cause = slug_of(&earlier.title);
    ticket(
        world,
        destination,
        &id,
        (&earlier.repository, &earlier.title, &root_cause),
        &format!("groomed-{id}"),
        &earlier.status,
        &earlier.body,
    );
    (format!("{destination}:{id}"), root_cause)
}

/// Neutral words an unrelated ticket is written from: none is a word any cluster's draft
/// searches.
const FILLER: [&str; 24] = [
    "gizmo",
    "calibration",
    "sprocket",
    "widget",
    "lantern",
    "harbour",
    "meadow",
    "pebble",
    "quartz",
    "saffron",
    "tundra",
    "velvet",
    "walnut",
    "zephyr",
    "orchard",
    "granite",
    "juniper",
    "kettle",
    "lagoon",
    "marble",
    "nectar",
    "oyster",
    "parsley",
    "thistle",
];

/// One unrelated ticket's words beyond its filler: each word given is written into it.
pub struct Crowd<'a> {
    /// How many unrelated tickets to file.
    pub count: usize,
    /// How many of them are open; the rest are closed, as fixed.
    pub open: usize,
    /// Each word, and how many of the unrelated tickets carry it.
    pub words: &'a [(&'a str, usize)],
}

/// One unrelated ticket of a crowd, as another batch left it.
pub struct CrowdTicket {
    /// Its id on the board.
    pub id: String,
    /// The repository it is filed in, by name.
    pub repository: String,
    pub title: String,
    pub root_cause: String,
    /// The batch that filed it.
    pub batch: String,
    /// `open`, or `fixed`.
    pub status: &'static str,
    pub body: String,
}

/// `crowd`'s unrelated tickets, spread across `repositories`: each word goes on its own run
/// of tickets, offset per word, so a ticket carries a word from several lists only where
/// their runs overlap.
pub fn crowd_tickets(repositories: &[&str], crowd: &Crowd<'_>) -> Vec<CrowdTicket> {
    let mut offset = 0;
    let mut carried: Vec<Vec<&str>> = vec![Vec::new(); crowd.count];
    for (word, many) in crowd.words {
        assert!(
            *many <= crowd.count,
            "{word} cannot be on {many} of {} tickets",
            crowd.count
        );
        for at in 0..*many {
            carried[(offset + at * 7) % crowd.count].push(word);
        }
        offset += 13;
    }
    carried
        .iter()
        .enumerate()
        .map(|(at, words)| {
            let repository = repositories[at % repositories.len()];
            let one = FILLER[at % FILLER.len()];
            let two = FILLER[(at / FILLER.len() + 7) % FILLER.len()];
            let extra = words.join(" and ");
            CrowdTicket {
                id: format!("crowd-{at:04}"),
                repository: repository.to_owned(),
                title: format!("{repository}: the {one} {two} drifts by {at}"),
                root_cause: format!("{one}-{two}-drifts-{at}"),
                batch: format!("nightly-crowd-{}", at % 40),
                status: if at < crowd.open { "open" } else { "fixed" },
                body: format!(
                    "## Root cause\n\nThe {one} {two} {extra} drifts by {at}.\n\n## Suggested fix\n\n- {repository} `src/{one}_{two}.rs`: recompute the {two}."
                ),
            }
        })
        .collect()
}

/// File `crowd`'s unrelated tickets on `destination`, spread across the six repositories,
/// as other batches left them.
pub fn crowd(world: &World, destination: &str, crowd: &Crowd<'_>) {
    for ticket in crowd_tickets(&REPOSITORIES, crowd) {
        self::ticket(
            world,
            destination,
            &ticket.id,
            (&ticket.repository, &ticket.title, &ticket.root_cause),
            &ticket.batch,
            ticket.status,
            &ticket.body,
        );
    }
}
