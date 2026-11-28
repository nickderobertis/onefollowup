//! Filing-time duplicates worded differently from the tickets they repeat: the later members
//! of 13 merged clusters `spike-filing-duplicates` replayed, worded as filed, against a
//! destination holding their earlier members open and closed. `prepare`'s term and file
//! searches render each earlier member as a candidate, the scripted agent relates each draft
//! to it by an evidence comment instead of filing it again, and `check` refuses an answer
//! that leaves a candidate neither related nor dismissed.

use serde_json::{Value, json};

use onefollowup::verify::candidates;

use crate::duplicate_boards::{self, Crowd, clusters, origin};
use crate::journey::{comments, front, record};
use crate::telemetry;
use crate::world::{Ticket, World, answer};

/// The batch every cluster draft is recorded under.
pub const BATCH: &str = "nightly-2026-11-21";

/// What the scripted agent did with each draft.
pub enum Fate {
    /// Related to the candidate `item`, of root cause `root_cause`, by an evidence comment.
    Related {
        /// The candidate.
        item: String,
        /// Its root cause.
        root_cause: String,
        /// The other earlier members rendered, closed as its duplicates: dismissed as such.
        merged: Vec<String>,
    },
    /// Filed as the new ticket `slug`.
    Filed {
        /// The ticket's root cause.
        slug: String,
    },
}

/// Stage each draft's fate as the verifying agent would, answer every draft, and check,
/// publish and settle the batch.
pub fn file(world: &mut World, batch: &str, fates: &[(String, Fate)]) {
    let mut answers = Vec::new();
    for (draft, fate) in fates {
        match fate {
            Fate::Related {
                item,
                root_cause,
                merged,
            } => {
                world.ok(
                    "agent",
                    &[
                        "ticket",
                        "comment",
                        "--batch",
                        batch,
                        "--item",
                        item,
                        "--root-cause",
                        root_cause,
                    ],
                    "Seen again on build-host-1: the same guarantee broke, reported in other words.\n",
                );
                world.ok(
                    "agent",
                    &["ticket", "re-estimate", "--batch", batch, item],
                    "",
                );
                let mut answered = answer(
                    draft,
                    "root-cause",
                    "filed",
                    &[root_cause],
                    "The candidate states this root cause; the evidence goes there.",
                );
                answered["dismissed"] = world
                    .candidates(batch, draft)
                    .into_iter()
                    .filter(|candidate| candidate["item"] != item.as_str())
                    .map(|candidate| {
                        let reason = if merged.iter().any(|one| candidate["item"] == one.as_str()) {
                            format!("Another report of this root cause, closed as a duplicate of {item}, where this evidence goes.")
                        } else {
                            "Its root cause is a different invariant.".to_owned()
                        };
                        json!({"item": candidate["item"], "reason": reason})
                    })
                    .collect();
                answers.push(answered);
            }
            Fate::Filed { slug } => {
                world.ticket(
                    batch,
                    &Ticket::new(slug, &format!("the {slug} is new"), &[draft]),
                );
                world.ok("agent", &["ticket", "copy", "--batch", batch, slug], "");
                answers.push(answer(draft, "root-cause", "filed", &[slug], "Verified."));
            }
        }
    }
    world.answer(batch, &answers);
    let checked = world.ok("check", &["check", "--batch", batch], "");
    assert_eq!(checked["sound"], true, "{checked}");
    world.ok("publish", &["publish", "--batch", batch], "");
    world.ok("settle", &["settle", "--batch", batch], "");
}

/// The candidate the scripted agent relates a duplicate to: the first open earlier member it
/// was shown, else the first closed one.
pub fn target(candidates: &[Value], earlier: &[(String, String, String)]) -> Fate {
    let shown: Vec<&(String, String, String)> = earlier
        .iter()
        .filter(|(item, _, _)| candidates.iter().any(|one| one["item"] == item.as_str()))
        .collect();
    let chosen = shown
        .iter()
        .find(|(_, _, status)| status == "open")
        .or_else(|| shown.first())
        .expect("an earlier member is rendered");
    Fate::Related {
        item: chosen.0.clone(),
        root_cause: chosen.1.clone(),
        merged: shown
            .iter()
            .filter(|(item, _, _)| *item != chosen.0)
            .map(|(item, _, _)| item.clone())
            .collect(),
    }
}

/// Assert `world`'s batch ended as `fates` say: one evidence comment of `batch` on each
/// related item, at the status it held, and a new ticket for each filed one alone.
pub fn assert_fates(world: &World, destination: &str, batch: &str, fates: &[(String, Fate)]) {
    let filed: Vec<String> = world
        .items(destination)
        .into_iter()
        .filter(|(_, text)| record(text)["created_by_run"] == batch)
        .map(|(_, text)| {
            record(&text)["root_cause"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    let mut new = Vec::new();
    for (draft, fate) in fates {
        match fate {
            Fate::Related {
                item, root_cause, ..
            } => {
                let id = item.split_once(':').expect("a qualified id").1;
                let text = world.read_item(destination, id);
                let marker = format!("run=\"{batch}\" root_cause=\"{root_cause}\"");
                assert_eq!(
                    comments(&text).matches(&marker).count(),
                    1,
                    "{draft} is related to {item} by one evidence comment: {text}"
                );
                assert!(
                    !filed.contains(root_cause),
                    "{draft} was not filed again as {root_cause}"
                );
            }
            Fate::Filed { slug } => new.push(slug.clone()),
        }
    }
    let mut filed = filed;
    filed.sort();
    new.sort();
    assert_eq!(filed, new, "only the drafts with no duplicate were filed");
}

/// One cluster's earlier members as placed on the destination: `(item, root cause, status)`.
pub type Placed = Vec<(String, String, String)>;

/// The clusters' world: a destination holding a crowd of 300 unrelated tickets (70 open) and
/// every cluster's earlier members, open and closed, and the batch's drafts — each cluster's
/// later member, then one fresh draft. Answers the world, each cluster draft's id, the
/// earlier members placed for each cluster (`(item, root cause, status)`), and the fresh
/// draft's id.
pub fn clusters_world() -> (World, Vec<String>, Vec<Placed>, String) {
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    duplicate_boards::crowd(
        &world,
        "followups",
        &Crowd {
            count: 300,
            open: 70,
            words: &[],
        },
    );
    let clusters = clusters();
    let mut earlier_of = Vec::new();
    for cluster in &clusters {
        let placed: Vec<(String, String, String)> = cluster
            .earlier
            .iter()
            .map(|earlier| {
                let (item, root_cause) = duplicate_boards::place(&world, "followups", earlier);
                (item, root_cause, earlier.status.clone())
            })
            .collect();
        earlier_of.push(placed);
    }
    let closed_only = earlier_of
        .iter()
        .filter(|placed| placed.iter().all(|(_, _, status)| status != "open"))
        .count();
    assert!(
        closed_only >= 3,
        "{closed_only} clusters' earlier members are all closed"
    );

    let mut drafts = Vec::new();
    for cluster in &clusters {
        let paths: Vec<&str> = cluster.draft.paths.iter().map(String::as_str).collect();
        drafts.push(world.draft_in(
            BATCH,
            &origin(&cluster.repository),
            &cluster.draft.title,
            &paths,
            &format!("{}\n", cluster.draft.body),
        ));
    }
    let fresh = world.draft_in(
        BATCH,
        &origin("onevcs"),
        "a branch name ending in a dot truncates the publish summary",
        &["crates/onevcs/src/summary.rs"],
        "Publishing a branch whose name ends in a dot prints a summary that cuts the name short, so a reviewer cannot find the change from it.\n",
    );
    (world, drafts, earlier_of, fresh)
}

#[test]
fn differently_worded_duplicates_are_rendered_and_related_to_their_earlier_tickets_open_or_closed()
{
    let (mut world, drafts, earlier_of, fresh) = clusters_world();
    let clusters = clusters();
    let prepared = world.ok("prepare", &["prepare", "--batch", BATCH], "");

    let mut fates = Vec::new();
    let mut recorded = Vec::new();
    for ((cluster, draft), earlier) in clusters.iter().zip(&drafts).zip(&earlier_of) {
        let candidates = world.candidates(BATCH, draft);
        for (item, _, _) in earlier {
            assert!(
                candidates.iter().any(|one| one["item"] == item.as_str()),
                "{} ({}): its earlier member {item} is rendered: {candidates:?}",
                cluster.later,
                cluster.cluster
            );
        }
        let fate = target(&candidates, earlier);
        let Fate::Related { item, .. } = &fate else {
            unreachable!("a duplicate is related");
        };
        let search = prepared["duplicate_searches"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|search| search["input"] == draft.as_str())
            .cloned()
            .unwrap_or_default();
        recorded.push(json!({
            "cluster": cluster.cluster,
            "later": cluster.later,
            "draft": draft,
            "related_to": item,
            "stoplisted": search["stoplisted"],
            "queries": search["queries"],
            "candidates": candidates,
        }));
        fates.push((draft.clone(), fate));
    }
    let noise = world.candidates(BATCH, &fresh);
    assert!(
        earlier_of.iter().flatten().all(|(item, _, _)| !noise
            .iter()
            .any(|one| one["item"] == item.as_str())
            || noise.len() <= candidates::BOUND),
        "the draft with no duplicate is shown at most the bound: {noise:?}"
    );
    fates.push((
        fresh,
        Fate::Filed {
            slug: "publish-summary-keeps-branch-names".to_owned(),
        },
    ));
    let task = std::fs::read_to_string(prepared["task_template"].as_str().expect("a task"))
        .expect("the task is written");
    for (item, _, _) in earlier_of.iter().flatten() {
        assert!(
            task.contains(&format!("`{item}`")),
            "the task renders {item}"
        );
    }
    telemetry::record_journey(
        "filing-duplicates-replay",
        &json!({"batch": BATCH, "drafts": recorded}),
    );

    file(&mut world, BATCH, &fates);
    assert_fates(&world, "followups", BATCH, &fates);
    for placed in &earlier_of {
        for (item, _, status) in placed {
            let id = item.split_once(':').expect("a qualified id").1;
            let held = front(&world.read_item("followups", id))["status"].clone();
            let expected = match status.as_str() {
                "open" => "backlog",
                "fixed" => "done",
                _ => "cancelled",
            };
            assert_eq!(held, expected, "{item} keeps the status a person gave it");
        }
    }
}

#[test]
fn check_refuses_a_candidate_neither_related_nor_dismissed_and_accepts_it_dismissed_with_a_reason()
{
    let mut world = World::new(&["followups"], "host_label: build-host-1\n");
    let cluster = &clusters()[0];
    let earlier = &cluster.earlier[0];
    let (item, _) = duplicate_boards::place(&world, "followups", earlier);
    let paths: Vec<&str> = cluster.draft.paths.iter().map(String::as_str).collect();
    let draft = world.draft_in(
        BATCH,
        &origin(&cluster.repository),
        &cluster.draft.title,
        &paths,
        &format!("{}\n", cluster.draft.body),
    );
    world.ok("prepare", &["prepare", "--batch", BATCH], "");
    assert!(
        world
            .candidates(BATCH, &draft)
            .iter()
            .any(|one| one["item"] == item.as_str()),
        "{item} is rendered"
    );
    let slug = "redirected-turns-keep-their-events";
    world.ticket(
        BATCH,
        &Ticket::new(slug, "redirected turns keep their events", &[&draft]),
    );
    world.ok("agent", &["ticket", "copy", "--batch", BATCH, slug], "");

    // Filed with the candidate left unaccounted for.
    world.answer_as_given(
        BATCH,
        &[answer(&draft, "root-cause", "filed", &[slug], "Verified.")],
    );
    let refused = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(refused["sound"], false, "{refused}");
    let message = refused["problems"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(refused["problems"][0]["subject"], draft.as_str());
    assert!(
        message.contains(&format!("`{item}`")) && message.contains("neither related nor dismissed"),
        "{message}"
    );

    // Dismissed without a reason: still refused.
    let mut dismissed = answer(&draft, "root-cause", "filed", &[slug], "Verified.");
    dismissed["dismissed"] = json!([{"item": item, "reason": " "}]);
    world.answer_as_given(BATCH, &[dismissed.clone()]);
    let refused = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(refused["sound"], false, "{refused}");
    let message = refused["problems"][0]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(message.contains("without a reason"), "{refused}");

    // Dismissed with one: sound, and the batch files its own ticket.
    dismissed["dismissed"] = json!([{
        "item": item,
        "reason": "It records a second live note crashing a turn; this draft's turns were never redirected."
    }]);
    world.answer_as_given(BATCH, &[dismissed]);
    let checked = world.ok("check", &["check", "--batch", BATCH], "");
    assert_eq!(checked["sound"], true, "{checked}");
    let written: Value = serde_json::from_str(
        &std::fs::read_to_string(world.work().join(format!("dispositions/{BATCH}.json")))
            .expect("the answers"),
    )
    .expect("JSON");
    assert_eq!(written["schema"], 2, "{written}");
}
