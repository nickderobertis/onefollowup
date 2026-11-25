//! `groom check`: hold every shard's verdicts to the rules, and plan what publish sends.
//!
//! Every rule is code. The agent's verdicts are read and refused where a record is malformed,
//! a shard's ticket has no verdict or two, a closing verdict is `low`, or its `deciding_commit`
//! is not an ancestor of the default-branch commit prepare fetched. Duplicate links are unioned into
//! clusters, members closing as `fixed` or `obsolete` leave them, and each cluster's survivor is
//! chosen by a fixed order, whichever ticket the agent named. A Queued, In Progress or In Review
//! ticket is never closed or folded, a Todo ticket never folded and closed only when definitely
//! outdated: such a ticket with a closing or duplicate verdict is planned as left for a person.
//!
//! **A plan is input**: the `plan.json` a batch holds when check runs is what publish sends, so
//! check holds it, as well as the plan it computes, to the protection rules, and refuses one
//! that breaks them without writing over it. A passing check writes its computed plan.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{BATCH_RECORD, Groom, PLAN_RECORD, SNAPSHOT_RECORD, batch_id, git};
use crate::api::{GroomCheckArgs, GroomChecked, GroomProblem};
use crate::config::Settings;
use crate::error::Error;
use crate::formats::groom::{
    Confidence, GroomBatch, GroomPlan, GroomRule, GroomSnapshot, GroomVerdict, PlanEntry,
    PlanRecord, Protection, SCHEMA_VERSION, SelectedTicket, SnapshotTicket, Survivor, SurvivorRule,
    VerdictRecord, is_commit, parse, render,
};
use crate::work;

/// A verdict that broke no rule, with the repository its closing commit was verified in.
struct Judged<'a> {
    verdict: VerdictRecord,
    ticket: &'a SelectedTicket,
    /// For a closing verdict, the repository whose fetched default branch holds its commit.
    verified: Option<(String, String)>,
}

/// What check reads a batch with.
struct Batch<'a> {
    git: git::Git<'a>,
    record: &'a GroomBatch,
    /// Every open ticket prepare listed, selected or not: a cluster's members are ranked by it.
    open: BTreeMap<&'a str, &'a SnapshotTicket>,
    problems: Vec<GroomProblem>,
}

impl Batch<'_> {
    fn problem(&mut self, ticket: Option<&str>, rule: GroomRule, detail: String) {
        self.problems.push(GroomProblem {
            ticket: ticket.map(str::to_owned),
            rule,
            detail,
        });
    }

    fn protection(&self, ticket: &str) -> Option<Protection> {
        self.record.open.get(ticket).copied()
    }

    fn selected(&self, ticket: &str) -> Option<&SelectedTicket> {
        self.record.selected.iter().find(|one| one.ticket == ticket)
    }

    /// The repository of `ticket`'s whose fetched default branch holds `commit`, with that default branch.
    fn verify(
        &self,
        ticket: &SelectedTicket,
        commit: &str,
    ) -> Result<Option<(String, String)>, Error> {
        // Only a full commit id reaches git, so a plan's value is never read as an option.
        if !is_commit(commit) {
            return Ok(None);
        }
        for named in &ticket.repositories {
            if let Some(fetched) = self
                .record
                .repositories
                .iter()
                .find(|one| &one.repository == named)
                && git::is_ancestor(self.git, Path::new(&fetched.clone), commit, &fetched.commit)?
            {
                return Ok(Some((fetched.repository.clone(), fetched.commit.clone())));
            }
        }
        Ok(None)
    }

    fn default_branch_of(&self, ticket: &SelectedTicket) -> (Option<String>, Option<String>) {
        ticket
            .repositories
            .iter()
            .find_map(|named| {
                self.record
                    .repositories
                    .iter()
                    .find(|one| &one.repository == named)
            })
            .map_or((ticket.repositories.first().cloned(), None), |fetched| {
                (
                    Some(fetched.repository.clone()),
                    Some(fetched.commit.clone()),
                )
            })
    }
}

/// Read every shard's verdicts, keeping each that breaks no rule.
fn verdicts<'a>(batch: &mut Batch<'a>) -> Result<Vec<Judged<'a>>, Error> {
    let record = batch.record;
    let mut judged: Vec<Judged<'a>> = Vec::new();
    for shard in &record.shards {
        let path = PathBuf::from(&shard.verdicts);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(Error::Failed(format!(
                    "{}: could not be read: {error}",
                    path.display()
                )));
            }
        };
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        let mut kept: BTreeMap<String, VerdictRecord> = BTreeMap::new();
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let at = format!("{} line {}", shard.shard, index + 1);
            match VerdictRecord::parse_line(line) {
                Ok(verdict) => {
                    let ticket = verdict.ticket.clone();
                    if !shard.tickets.contains(&ticket) {
                        batch.problem(
                            Some(&ticket),
                            GroomRule::UnknownTicket,
                            format!("{at}: `{ticket}` is not a ticket of shard {}", shard.shard),
                        );
                        continue;
                    }
                    *seen.entry(ticket.clone()).or_default() += 1;
                    kept.insert(ticket, verdict);
                }
                Err((ticket, broken)) => {
                    if let Some(ticket) = &ticket {
                        *seen.entry(ticket.clone()).or_default() += 1;
                    }
                    for (rule, detail) in broken {
                        batch.problem(ticket.as_deref(), rule, format!("{at}: {detail}"));
                    }
                }
            }
        }
        for ticket in &shard.tickets {
            match seen.get(ticket).copied().unwrap_or_default() {
                0 => batch.problem(
                    Some(ticket),
                    GroomRule::MissingVerdict,
                    format!(
                        "shard {} lists `{ticket}`, and {} holds no verdict for it",
                        shard.shard, shard.verdicts
                    ),
                ),
                1 => {}
                count => {
                    batch.problem(
                        Some(ticket),
                        GroomRule::DuplicateVerdict,
                        format!(
                            "{} holds {count} verdict records for `{ticket}`; keep exactly one",
                            shard.verdicts
                        ),
                    );
                    kept.remove(ticket);
                }
            }
        }
        for (ticket, verdict) in kept {
            if seen.get(&ticket).copied() != Some(1) {
                continue;
            }
            let Some(selected) = record.selected.iter().find(|one| one.ticket == ticket) else {
                continue;
            };
            if let Some(other) = &verdict.duplicate_of
                && !record.open.contains_key(other)
            {
                batch.problem(
                    Some(&ticket),
                    GroomRule::DuplicateOfUnknown,
                    format!(
                        "`duplicate_of` names `{other}`, which is not an open ticket of the batch's snapshot"
                    ),
                );
                continue;
            }
            let mut verified = None;
            if verdict.verdict.closes()
                && let Some(commit) = verdict.deciding_commit.as_deref()
            {
                verified = batch.verify(selected, commit)?;
                if verified.is_none() {
                    let heads: Vec<String> = selected
                        .repositories
                        .iter()
                        .filter_map(|named| {
                            record
                                .repositories
                                .iter()
                                .find(|one| &one.repository == named)
                                .map(|one| format!("`{}` at {}", one.repository, one.commit))
                        })
                        .collect();
                    batch.problem(
                        Some(&ticket),
                        GroomRule::CommitNotOnMain,
                        format!(
                            "the `{}` verdict's `deciding_commit` {commit} is not an ancestor of the fetched default branch of {}",
                            verdict.verdict.as_str(),
                            if heads.is_empty() {
                                "any repository: the ticket names none".to_owned()
                            } else {
                                heads.join(" or ")
                            }
                        ),
                    );
                    continue;
                }
            }
            judged.push(Judged {
                verdict,
                ticket: selected,
                verified,
            });
        }
    }
    Ok(judged)
}

fn entry(batch: &Batch<'_>, judged: &Judged<'_>, left_for_person: Option<String>) -> PlanEntry {
    let (repository, commit) = judged.verified.clone().map_or_else(
        || batch.default_branch_of(judged.ticket),
        |(repository, commit)| (Some(repository), Some(commit)),
    );
    PlanEntry {
        ticket: judged.verdict.ticket.clone(),
        status: judged.ticket.status.category.clone(),
        verdict: judged.verdict.verdict,
        confidence: judged.verdict.confidence,
        evidence: judged.verdict.evidence.clone(),
        deciding_commit: judged.verdict.deciding_commit.clone(),
        evidence_paths: judged.verdict.evidence_paths.clone(),
        repository,
        commit,
        duplicate_of: None,
        root_cause: judged.verdict.root_cause.clone(),
        left_for_person,
    }
}

/// Union-find's root of `id`.
fn root(parents: &mut BTreeMap<String, String>, id: &str) -> String {
    let mut at = id.to_owned();
    while let Some(parent) = parents.get(&at).filter(|parent| **parent != at) {
        at = parent.clone();
    }
    parents.insert(id.to_owned(), at.clone());
    at
}

/// The survivor of `members` by the fixed order, and the rule that decided it.
fn survivor(
    batch: &Batch<'_>,
    members: &[String],
    judged: &BTreeMap<&str, &Judged<'_>>,
) -> (String, SurvivorRule) {
    let mut seams: BTreeMap<&str, usize> = BTreeMap::new();
    for member in members {
        if let Some(seam) = judged
            .get(member.as_str())
            .and_then(|one| one.verdict.seam_repository.as_deref())
        {
            *seams.entry(seam).or_default() += 1;
        }
    }
    let seam = seams
        .iter()
        .max_by(|one, other| one.1.cmp(other.1).then(other.0.cmp(one.0)))
        .map(|(seam, _)| *seam);
    // A member prepare left unselected is ranked as fully as a selected one: its
    // repositories and creation time come from the snapshot of every open ticket.
    let listed = |member: &str| batch.open.get(member).copied();
    let created = |member: &str| listed(member).and_then(|one| one.created_at.clone());
    // Each member's rank: smaller wins, rule by rule.
    let rank = |member: &String| {
        let protection = batch.protection(member);
        let verdict = judged.get(member.as_str()).map(|one| &one.verdict);
        let in_seam = seam.is_some_and(|seam| {
            listed(member).is_some_and(|one| one.repositories.iter().any(|named| named == seam))
        });
        (
            protection != Some(Protection::Claimed),
            protection != Some(Protection::Todo),
            !in_seam,
            std::cmp::Reverse(verdict.map_or(0, |one| one.evidence_paths.len())),
            std::cmp::Reverse(verdict.map_or(0, |one| one.evidence.trim().len())),
            created(member).is_none(),
            created(member),
            member.clone(),
        )
    };
    let mut ranked: Vec<&String> = members.iter().collect();
    ranked.sort_by_key(|member| rank(member));
    let first = ranked[0];
    // A cluster has at least two members, so there is always a runner-up.
    let second = rank(ranked[1]);
    let one = rank(first);
    let chosen_by = if one.0 != second.0 {
        SurvivorRule::Claimed
    } else if one.1 != second.1 {
        SurvivorRule::Todo
    } else if one.2 != second.2 {
        SurvivorRule::SeamRepository
    } else if one.3 != second.3 || one.4 != second.4 {
        SurvivorRule::MostEvidence
    } else {
        SurvivorRule::Oldest
    };
    (first.clone(), chosen_by)
}

/// Plan the judged verdicts.
fn plan(batch: &Batch<'_>, judged: &[Judged<'_>]) -> GroomPlan {
    let mut plan = GroomPlan::default();
    let by_ticket: BTreeMap<&str, &Judged<'_>> = judged
        .iter()
        .map(|one| (one.verdict.ticket.as_str(), one))
        .collect();
    let mut closed: BTreeSet<&str> = BTreeSet::new();
    let mut left: BTreeMap<&str, String> = BTreeMap::new();
    for one in judged {
        let ticket = one.verdict.ticket.as_str();
        if !one.verdict.verdict.closes() {
            continue;
        }
        match batch.protection(ticket) {
            Some(Protection::Claimed) => {
                left.insert(
                    ticket,
                    "it is claimed (Queued, In Progress or In Review), which grooming never closes"
                        .to_owned(),
                );
            }
            Some(Protection::Todo)
                if one.verdict.confidence != Confidence::High || one.verified.is_none() =>
            {
                left.insert(ticket, "it is Todo, which grooming closes only on a high-confidence verdict whose deciding commit is verified on the default branch".to_owned());
            }
            _ => {
                closed.insert(ticket);
                let entry = entry(batch, one, None);
                if one.verdict.verdict == GroomVerdict::Fixed {
                    plan.fixed.push(entry);
                } else {
                    plan.obsolete.push(entry);
                }
            }
        }
    }

    let mut parents: BTreeMap<String, String> = BTreeMap::new();
    for one in judged {
        if let Some(other) = &one.verdict.duplicate_of {
            let from = root(&mut parents, &one.verdict.ticket);
            let to = root(&mut parents, other);
            if from != to {
                let (low, high) = if from < to { (from, to) } else { (to, from) };
                parents.insert(high, low);
            }
        }
    }
    let ids: Vec<String> = parents.keys().cloned().collect();
    let mut clusters: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in ids {
        let at = root(&mut parents, &id);
        clusters.entry(at).or_default().push(id);
    }
    let mut folded: BTreeMap<&str, (String, Option<String>)> = BTreeMap::new();
    for members in clusters.values() {
        let members: Vec<String> = members
            .iter()
            .filter(|member| !closed.contains(member.as_str()))
            .cloned()
            .collect();
        if members.len() < 2 {
            continue;
        }
        let (survivor, chosen_by) = survivor(batch, &members, &by_ticket);
        let shared = members
            .iter()
            .find_map(|member| by_ticket.get(member.as_str())?.verdict.root_cause.clone());
        for member in &members {
            if *member == survivor || !by_ticket.contains_key(member.as_str()) {
                continue;
            }
            let ticket = by_ticket[member.as_str()].verdict.ticket.as_str();
            match batch.protection(member) {
                Some(Protection::Claimed) => {
                    left.entry(ticket).or_insert_with(|| format!("it is claimed (Queued, In Progress or In Review), which grooming never folds; its cluster's survivor is {survivor}"));
                }
                Some(Protection::Todo) => {
                    left.entry(ticket).or_insert_with(|| format!("it is Todo, which grooming never folds as a duplicate; its cluster's survivor is {survivor}"));
                }
                _ => {
                    let root_cause = by_ticket[member.as_str()]
                        .verdict
                        .root_cause
                        .clone()
                        .or_else(|| shared.clone());
                    folded.insert(ticket, (survivor.clone(), root_cause));
                }
            }
        }
        plan.survivors.push(Survivor {
            ticket: survivor,
            members,
            chosen_by,
        });
    }

    for one in judged {
        let ticket = one.verdict.ticket.as_str();
        if closed.contains(ticket) {
            continue;
        }
        if let Some((survivor, root_cause)) = folded.get(ticket) {
            let mut entry = entry(batch, one, None);
            entry.duplicate_of = Some(survivor.clone());
            entry.root_cause.clone_from(root_cause);
            plan.duplicates.push(entry);
            continue;
        }
        let entry = entry(batch, one, left.get(ticket).cloned());
        if one.verdict.verdict == GroomVerdict::Unresolved {
            plan.unresolved.push(entry.clone());
        }
        plan.kept.push(entry);
    }
    plan
}

/// Hold `plan`, named `what`, to the protection rules.
fn protect(batch: &mut Batch<'_>, plan: &GroomPlan, what: &str) -> Result<(), Error> {
    for entry in plan.fixed.iter().chain(&plan.obsolete) {
        let ticket = entry.ticket.as_str();
        match batch.protection(ticket) {
            None => batch.problem(
                Some(ticket),
                GroomRule::PlanRecord,
                format!("{what} closes `{ticket}`, which the batch did not list as open"),
            ),
            Some(Protection::Claimed) => batch.problem(
                Some(ticket),
                GroomRule::ClaimedClosed,
                format!("{what} closes `{ticket}`, which is Queued, In Progress or In Review: grooming never closes claimed work"),
            ),
            Some(protection) => {
                let verified = match (batch.selected(ticket), entry.deciding_commit.as_deref()) {
                    (Some(selected), Some(commit)) => {
                        let selected = selected.clone();
                        batch.verify(&selected, commit)?.is_some()
                    }
                    _ => false,
                };
                if entry.confidence == Confidence::Low {
                    batch.problem(
                        Some(ticket),
                        GroomRule::LowConfidenceClose,
                        format!("{what} closes `{ticket}` on a `low`-confidence verdict"),
                    );
                } else if protection == Protection::Todo
                    && (entry.confidence != Confidence::High || !verified)
                {
                    batch.problem(
                        Some(ticket),
                        GroomRule::TodoCloseBelowBar,
                        format!(
                            "{what} closes the Todo ticket `{ticket}` on a `{}`-confidence verdict whose deciding commit is {}: a Todo ticket closes only on a `high`-confidence verdict whose `deciding_commit` is verified on the fetched default branch",
                            match entry.confidence {
                                Confidence::High => "high",
                                Confidence::Medium => "medium",
                                Confidence::Low => "low",
                            },
                            if verified { "on the default branch" } else { "not verified on the default branch" }
                        ),
                    );
                } else if !verified {
                    batch.problem(
                        Some(ticket),
                        GroomRule::CommitNotOnMain,
                        format!("{what} closes `{ticket}` on a deciding commit not verified on the fetched default branch"),
                    );
                }
            }
        }
    }
    for entry in &plan.duplicates {
        let ticket = entry.ticket.as_str();
        match batch.protection(ticket) {
            None => batch.problem(
                Some(ticket),
                GroomRule::PlanRecord,
                format!("{what} folds `{ticket}`, which the batch did not list as open"),
            ),
            Some(Protection::Claimed) => batch.problem(
                Some(ticket),
                GroomRule::ClaimedFolded,
                format!("{what} folds `{ticket}` as a duplicate, and it is Queued, In Progress or In Review: grooming never folds claimed work"),
            ),
            Some(Protection::Todo) => batch.problem(
                Some(ticket),
                GroomRule::TodoFolded,
                format!("{what} folds the Todo ticket `{ticket}` as a duplicate, which grooming never does"),
            ),
            Some(Protection::Unprotected) => {}
        }
    }
    Ok(())
}

/// Run `groom check`.
///
/// # Errors
///
/// A batch id that is not one, a batch prepare did not write, a file that cannot be read or
/// written, or — at exit 2, printing the answer — any problem standing.
pub fn run(settings: &Settings, args: GroomCheckArgs) -> Result<GroomChecked, Error> {
    let groom = Groom::new(settings)?;
    let id = batch_id(&args.batch)?;
    let directory = groom.batch(id);
    let record_path = directory.join(BATCH_RECORD);
    let text = std::fs::read_to_string(&record_path).map_err(|_| {
        Error::Refused(format!(
            "{}: no grooming batch {id} was prepared; next: run `onefollowup groom prepare`",
            record_path.display()
        ))
    })?;
    let record: GroomBatch = parse(&text, "grooming batch")?;
    let snapshot_path = directory.join(SNAPSHOT_RECORD);
    let snapshot: GroomSnapshot = parse(
        &std::fs::read_to_string(&snapshot_path).map_err(|error| {
            Error::Failed(format!(
                "{}: could not be read: {error}; next: run `onefollowup groom prepare` again",
                snapshot_path.display()
            ))
        })?,
        "grooming snapshot",
    )?;
    let mut batch = Batch {
        git: groom.git(),
        record: &record,
        open: snapshot
            .tickets
            .iter()
            .map(|ticket| (ticket.ticket.as_str(), ticket))
            .collect(),
        problems: Vec::new(),
    };
    let judged = verdicts(&mut batch)?;
    let planned = plan(&batch, &judged);
    protect(&mut batch, &planned, "the computed plan")?;
    let plan_path = directory.join(PLAN_RECORD);
    match std::fs::read_to_string(&plan_path) {
        Ok(text) => match parse::<PlanRecord>(&text, "grooming plan") {
            Ok(held) if held.batch == id => protect(&mut batch, &held.plan, "plan.json")?,
            Ok(held) => batch.problem(
                None,
                GroomRule::PlanRecord,
                format!(
                    "{} is batch {}'s plan, not {id}'s",
                    plan_path.display(),
                    held.batch
                ),
            ),
            Err(error) => batch.problem(
                None,
                GroomRule::PlanRecord,
                format!(
                    "{}: {error}; next: remove it, and check writes the plan",
                    plan_path.display()
                ),
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(Error::Failed(format!(
                "{}: could not be read: {error}",
                plan_path.display()
            )));
        }
    }
    let checked = GroomChecked {
        batch: id.to_owned(),
        problems: batch.problems,
        plan: planned,
    };
    if !checked.problems.is_empty() {
        return Err(Error::Unpassed {
            message: format!(
                "the grooming batch {id} breaks {} rule(s): {}; next: correct each verdict record named, then run `groom check --batch {id}` again",
                checked.problems.len(),
                checked
                    .problems
                    .iter()
                    .map(|problem| format!(
                        "{} ({})",
                        problem.rule.as_str(),
                        problem.ticket.as_deref().unwrap_or("the batch")
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            answer: Box::new(checked),
        });
    }
    work::write_text(
        &plan_path,
        &render(&PlanRecord {
            schema_version: SCHEMA_VERSION,
            batch: id.to_owned(),
            plan: checked.plan.clone(),
        }),
    )?;
    Ok(checked)
}

#[cfg(test)]
mod tests {
    //! The age rule of the survivor order, which no journey can reach: local-md, the one
    //! source the journeys run on, reports no creation time for a task, so this is the cheapest
    //! honest proof that an unselected member's age, read from the snapshot, decides.

    use std::collections::BTreeMap;
    use std::path::Path;

    use super::{Batch, Judged, survivor};
    use crate::formats::groom::{
        Confidence, GroomBatch, GroomVerdict, HeldStatus, Protection, SelectedTicket,
        SnapshotTicket, SurvivorRule, VerdictRecord,
    };
    use crate::groom::git::Git;

    fn listed(ticket: &str, created_at: &str) -> SnapshotTicket {
        SnapshotTicket {
            ticket: ticket.to_owned(),
            title: format!("widgets: {ticket}"),
            status: HeldStatus {
                category: "backlog".to_owned(),
                name: "Backlog".to_owned(),
            },
            url: None,
            repositories: vec!["github.com/example/widgets".to_owned()],
            created_at: Some(created_at.to_owned()),
            content: String::new(),
        }
    }

    #[test]
    fn an_unselected_member_older_than_the_selected_one_survives_by_age() {
        let fresh = SelectedTicket {
            ticket: "followups:fresh".to_owned(),
            title: "widgets: fresh".to_owned(),
            status: HeldStatus {
                category: "backlog".to_owned(),
                name: "Backlog".to_owned(),
            },
            protection: Protection::Unprotected,
            repositories: vec!["github.com/example/widgets".to_owned()],
            created_at: Some("2026-11-20T00:00:00Z".to_owned()),
            content_digest: "sha256:0".to_owned(),
            overlap_comments: Vec::new(),
            reasons: vec!["it holds no groom record".to_owned()],
        };
        // `settled` sorts after `fresh` by id, so only its age can make it the survivor.
        let snapshot = [
            listed("followups:fresh", "2026-11-20T00:00:00Z"),
            listed("followups:settled", "2026-01-01T00:00:00Z"),
        ];
        let record = GroomBatch {
            schema_version: 1,
            batch: "groom-20261121T000000000Z".to_owned(),
            destination: "followups".to_owned(),
            namespace: "orchestrator".to_owned(),
            verified_at: "2026-11-21T00:00:00Z".to_owned(),
            repositories: Vec::new(),
            open: BTreeMap::from([
                ("followups:fresh".to_owned(), Protection::Unprotected),
                ("followups:settled".to_owned(), Protection::Unprotected),
            ]),
            selected: vec![fresh.clone()],
            shards: Vec::new(),
        };
        let batch = Batch {
            git: Git {
                working_directory: Path::new("."),
                environment: &[],
            },
            record: &record,
            open: snapshot
                .iter()
                .map(|one| (one.ticket.as_str(), one))
                .collect(),
            problems: Vec::new(),
        };
        let judged = Judged {
            verdict: VerdictRecord {
                schema_version: 1,
                ticket: "followups:fresh".to_owned(),
                verdict: GroomVerdict::Keep,
                confidence: Confidence::High,
                // No evidence to outrank the unselected member, which has no verdict here.
                evidence: String::new(),
                deciding_commit: None,
                evidence_paths: Vec::new(),
                duplicate_of: Some("followups:settled".to_owned()),
                root_cause: Some("the save reads the size twice".to_owned()),
                seam_repository: None,
            },
            ticket: &fresh,
            verified: None,
        };
        let by_ticket = BTreeMap::from([("followups:fresh", &judged)]);
        let members = ["followups:fresh".to_owned(), "followups:settled".to_owned()];
        assert_eq!(
            survivor(&batch, &members, &by_ticket),
            ("followups:settled".to_owned(), SurvivorRule::Oldest)
        );
    }
}
