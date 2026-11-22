//! The workload `groom-pass-store-operations` and `groom-repeat-pass-store-operations` measure:
//! a local-md destination of 600 open and 1,400 closed tickets across 6 one-commit repositories,
//! 2 comments each, groomed in one full pass through the binary — 12 shards of 50 with scripted
//! verdicts: 520 kept, 50 fixed, 6 obsolete, 4 unresolved and 20 duplicates in 12 clusters, one
//! check and one publish — then a second pass with nothing changed and the first batch's
//! publish run again. Every destination operation is counted at onefollowup's store boundary,
//! as each verb printed it, and recorded; the comparison with each threshold is onebudgetspec's.

use std::collections::BTreeMap;

use onefollowup::formats::groom::record_key;
use serde_json::{Value, json};

use crate::groom::workload::{self, Ticket, Workload};
use crate::groom::world::{NAMESPACE, World, ago, grooming_marker, keep, verdict};
use crate::grooming::Binary;
use crate::telemetry;

/// The full pass's budget.
const FULL: &str = "groom-pass-store-operations";
/// The repeat pass's budget.
const REPEAT: &str = "groom-repeat-pass-store-operations";

/// The open statuses the workload's open tickets cycle through.
pub const OPEN_STATUSES: [&str; 7] = [
    "backlog",
    "todo",
    "backlog",
    "draft",
    "queued",
    "backlog",
    "in-progress",
];

/// Generate `repositories` one-commit repositories as [`workload::generate`] does, then `open` open
/// tickets cycling through [`OPEN_STATUSES`] and `closed` closed ones, round-robin over the
/// repositories, each holding `comments` comments a person made two days ago.
pub fn destination(
    world: &World,
    repositories: usize,
    files: usize,
    open: usize,
    closed: usize,
    comments: usize,
) -> Workload {
    let generated = workload::generate(world, repositories, files, 0);
    let made = ago(2 * 86_400);
    let tickets: Vec<Ticket> = (0..open + closed)
        .map(|index| {
            let repository = generated.repositories[index % repositories]
                .repository
                .clone();
            let path = format!("src/module_{}.rs", index % files.max(1));
            let status = if index < open {
                OPEN_STATUSES[index % OPEN_STATUSES.len()]
            } else if index % 2 == 0 {
                "done"
            } else {
                "cancelled"
            };
            let ticket = Ticket {
                id: format!("ticket-{index:04}"),
                status,
                repository,
                path,
            };
            let mut body = format!(
                "`{}` in `{}` answers a value its callers do not expect, number {index}.\n\n## Comments\n",
                ticket.path, ticket.repository
            );
            for at in 0..comments {
                body.push_str(&format!(
                    "\n<!-- onetaskgraph:comment id=\"c-{index:04}-{at}\" author=\"person\" created_at=\"{made}\" updated_at=\"{made}\" -->\n### person — {made}\n\nSeen again, note {at}.\n\n<!-- /onetaskgraph:comment -->\n"
                ));
            }
            world.ticket(
                &ticket.id,
                &format!(
                    "service: `{}` answers the wrong value ({index})",
                    ticket.path
                ),
                ticket.status,
                &[&ticket.repository],
                &body,
            );
            ticket
        })
        .collect();
    Workload {
        repositories: generated.repositories,
        tickets,
    }
}

/// What the scripted agents answer one ticket with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Role {
    Fixed,
    Obsolete,
    Unresolved,
    /// A duplicate of the survivor named.
    Duplicate(String),
    /// A cluster's survivor, resting on the most evidence.
    Survivor,
    Kept,
}

/// Each open ticket's role: closing verdicts and duplicates only on tickets nobody can claim,
/// so every one is closed; 8 clusters of 3 and 4 of 2.
fn roles(open: &[&Ticket]) -> BTreeMap<String, Role> {
    let unclaimed: Vec<&str> = open
        .iter()
        .filter(|ticket| matches!(ticket.status, "backlog" | "draft"))
        .map(|ticket| ticket.id.as_str())
        .collect();
    assert!(unclaimed.len() >= 92, "enough unclaimed tickets");
    let mut roles: BTreeMap<String, Role> = open
        .iter()
        .map(|ticket| (ticket.id.clone(), Role::Kept))
        .collect();
    let survivors = &unclaimed[60..72];
    for (at, id) in unclaimed[..92].iter().enumerate() {
        let role = match at {
            0..50 => Role::Fixed,
            50..56 => Role::Obsolete,
            56..60 => Role::Unresolved,
            60..72 => Role::Survivor,
            _ => {
                let duplicate = at - 72;
                let cluster = if duplicate < 16 {
                    duplicate / 2
                } else {
                    8 + (duplicate - 16)
                };
                Role::Duplicate(survivors[cluster].to_owned())
            }
        };
        roles.insert((*id).to_owned(), role);
    }
    roles
}

/// The store operations `printed` reports, as `<phase>: <op> on <source> (<items> items)`.
fn operations(phase: &str, printed: &Value) -> Vec<String> {
    printed["store_operations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|one| one["destination"] == true)
        .map(|one| {
            format!(
                "{phase}: {} on {} ({} items)",
                one["op"].as_str().unwrap_or_default(),
                one["source"].as_str().unwrap_or_default(),
                one["items"]
            )
        })
        .collect()
}

/// Every ticket of `ids` as the store reads it: its title, content and every metadata key but
/// its `groom` record, and its comments.
fn kept_state(world: &World, ids: &[String]) -> Vec<Value> {
    let qualified: Vec<String> = ids.iter().map(|id| World::qualified(id)).collect();
    world
        .store()
        .read(&qualified, true)
        .expect("the tickets read")
        .into_iter()
        .map(|read| {
            let item = read.item.expect("the ticket is there");
            let mut metadata = item.metadata.clone();
            metadata.remove(&record_key(NAMESPACE));
            json!({
                "title": item.title,
                "content": item.content,
                "metadata": metadata,
                "comments": read.comments.expect("comments").into_iter().map(|one| one.body).collect::<Vec<_>>(),
            })
        })
        .collect()
}

#[test]
fn a_full_grooming_pass_then_a_repeat_record_their_store_operations() {
    let world = World::new();
    let generated = destination(&world, 6, 4, 600, 1_400, 2);
    let main: BTreeMap<String, String> = generated
        .repositories
        .iter()
        .map(|one| (one.repository.clone(), one.commit.clone()))
        .collect();
    let open = generated.open();
    assert_eq!(open.len(), 600);
    let tickets: BTreeMap<String, &Ticket> = open
        .iter()
        .map(|ticket| (ticket.id.clone(), *ticket))
        .collect();
    let roles = roles(&open);
    let kept: Vec<String> = roles
        .iter()
        .filter(|(_, role)| **role == Role::Kept)
        .map(|(id, _)| id.clone())
        .collect();
    assert_eq!(
        kept.len() + 12,
        520,
        "520 kept, the 12 survivors among them"
    );
    let before = kept_state(&world, &kept);

    let prepared = world.json(&Binary, 0, &["groom", "prepare"]);
    let batch = prepared["batch"].as_str().expect("a batch").to_owned();
    assert_eq!(prepared["selected"], 600);
    assert_eq!(prepared["shards"].as_array().map(Vec::len), Some(12));
    world.verdicts(&batch, |id| {
        let ticket = tickets[id];
        let commit = main[&ticket.repository].as_str();
        let path = ticket.path.as_str();
        match &roles[id] {
            Role::Fixed => verdict(id, "fixed", "high", Some(commit), &[path], None),
            Role::Obsolete => verdict(id, "obsolete", "medium", Some(commit), &[path], None),
            Role::Unresolved => verdict(id, "unresolved", "low", None, &[], None),
            Role::Duplicate(survivor) => verdict(id, "keep", "high", None, &[path], Some(survivor)),
            Role::Survivor => keep(id, &[path, "README.md"]),
            Role::Kept => keep(id, &[path]),
        }
    });
    let checked = world.json(&Binary, 0, &["groom", "check", "--batch", &batch]);
    assert_eq!(checked["problems"], json!([]));
    let published = world.json(&Binary, 0, &["groom", "publish", "--batch", &batch]);
    assert_eq!(published["mismatched"], json!([]), "{published:#}");

    for (id, role) in &roles {
        let record = world
            .groom_record(id)
            .expect("every verified ticket holds a record");
        assert_eq!(
            record["commit"],
            main[&tickets[id.as_str()].repository].as_str(),
            "{id} was verified at its repository's main"
        );
        let (status, mark) = match role {
            Role::Fixed => ("done", "fixed"),
            Role::Obsolete => ("cancelled", "obsolete"),
            Role::Duplicate(_) => ("cancelled", "duplicate"),
            _ => continue,
        };
        let read = world.read(id);
        let item = read.item.expect("the ticket");
        assert_eq!(item.status.category, status, "{id} reads closed");
        let comments = read.comments.expect("comments");
        assert_eq!(comments.len(), 3, "{id}: its two and grooming's");
        assert!(comments[2].body.contains(&grooming_marker(&batch, mark)));
        if let Role::Duplicate(survivor) = role {
            assert_eq!(
                item.duplicate_of(),
                Some(World::qualified(survivor)),
                "{id} is linked to the survivor the priority order picks"
            );
        }
    }
    assert_eq!(
        kept_state(&world, &kept),
        before,
        "every kept ticket: no new comment, its body and other keys unchanged"
    );
    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(published["report_json"].as_str().expect("a path"))
            .expect("report.json"),
    )
    .expect("JSON");
    assert_eq!(report["outcomes"].as_array().map(Vec::len), Some(600));
    assert_eq!(report["survivors"].as_array().map(Vec::len), Some(12));

    let mut full: Vec<String> = operations("prepare", &prepared);
    full.extend(operations("check", &checked));
    full.extend(operations("publish", &published));
    telemetry::record(
        FULL,
        full.len(),
        &format!(
            "{} destination store operations from groom prepare to the end of groom publish: {}",
            full.len(),
            full.join("; ")
        ),
    );

    // Straight after: a pass with nothing changed, then the first batch's publish once more.
    let again = world.json(&Binary, 0, &["groom", "prepare"]);
    assert_eq!(again["selected"], 0, "nothing changed selects nothing");
    assert_eq!(again["shards"], json!([]));
    let republished = world.json(&Binary, 0, &["groom", "publish", "--batch", &batch]);
    assert_eq!(republished["mismatched"], json!([]));
    assert!(
        republished["store_operations"]
            .as_array()
            .expect("operations")
            .iter()
            .all(|one| !["update-many", "comment-add-many"]
                .contains(&one["op"].as_str().unwrap_or_default())),
        "a fully receipted batch sends nothing: {republished:#}"
    );
    let mut repeat: Vec<String> = operations("prepare", &again);
    repeat.extend(operations("publish again", &republished));
    telemetry::record(
        REPEAT,
        repeat.len(),
        &format!(
            "{} destination store operations of a pass with nothing changed and the first batch's publish run again: {}",
            repeat.len(),
            repeat.join("; ")
        ),
    );
}
