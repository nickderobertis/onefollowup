//! `groom check` over one prepared batch whose verdicts exercise every rule: each refusal
//! named by rule and ticket at exit 2 with no plan written, and once corrected a plan that keeps
//! each cluster's survivor by the priority order, drops closing members from clusters, leaves
//! protected tickets' closing and duplicate verdicts for a person, and closes a
//! definitely-outdated Todo ticket. A `plan.json` breaking the protection rules is refused and
//! kept as it is until it is put right.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::world::{Runner, World};

const WIDGETS: &str = "github.com/example/widgets";
const GADGETS: &str = "github.com/example/gadgets";

/// The prepared batch, its repositories' commits, and every ticket's good verdict.
struct Batch {
    world: World,
    id: String,
    /// Each shard's verdicts path and its tickets.
    shards: Vec<(String, Vec<String>)>,
    first: String,
    fix: String,
    side: String,
    good: BTreeMap<String, Value>,
}

/// One refusal: what it is, the ticket and rule check names, the verdicts and the further lines.
type Case<'a> = (
    &'a str,
    &'a str,
    &'a str,
    BTreeMap<String, Value>,
    Vec<(&'a str, String)>,
);

fn t(id: &str) -> String {
    format!("followups:{id}")
}

/// A verdict record.
#[allow(
    clippy::too_many_arguments,
    reason = "one argument per member of the record it builds"
)]
fn verdict(
    ticket: &str,
    verdict: &str,
    confidence: &str,
    commit: Option<&str>,
    paths: &[&str],
    duplicate_of: Option<&str>,
    seam: Option<&str>,
) -> Value {
    json!({
        "schema_version": 1,
        "ticket": t(ticket),
        "verdict": verdict,
        "confidence": confidence,
        "evidence": format!("{ticket}: read what {} rests on at main", paths.join(", ")),
        "deciding_commit": commit,
        "evidence_paths": paths,
        "duplicate_of": duplicate_of.map(t),
        "root_cause": duplicate_of.map(|_| "the layout's save reads the window size twice"),
        "seam_repository": seam,
    })
}

impl Batch {
    fn new(runner: &dyn Runner) -> Self {
        let world = World::new();
        let first = world.repository(WIDGETS, &[("src/a.rs", "a\n"), ("src/b.rs", "b\n")]);
        let fix = world.commit(WIDGETS, &[("src/a.rs", "a fixed\n")], "fix the save");
        let side = world.side_commit(WIDGETS, "side", &[("src/c.rs", "c\n")]);
        world.repository(GADGETS, &[("lib/x.rs", "x\n")]);
        let tickets: [(&str, &str, &str); 17] = [
            ("o1", "backlog", WIDGETS),
            ("o2", "backlog", WIDGETS),
            ("o3", "backlog", WIDGETS),
            ("o4", "backlog", WIDGETS),
            ("o5", "backlog", WIDGETS),
            ("o6", "backlog", WIDGETS),
            ("o7", "backlog", WIDGETS),
            ("o8", "backlog", WIDGETS),
            ("o9", "backlog", WIDGETS),
            ("g1", "backlog", GADGETS),
            ("q1", "queued", WIDGETS),
            ("q2", "queued", WIDGETS),
            ("p1", "in-progress", WIDGETS),
            ("t1", "todo", WIDGETS),
            ("t2", "todo", WIDGETS),
            ("t4", "todo", WIDGETS),
            ("u1", "backlog", WIDGETS),
        ];
        for (id, status, repository) in tickets {
            world.ticket(
                id,
                &format!("widgets: ticket {id}"),
                status,
                &[repository],
                &format!("The body of {id}."),
            );
        }
        let printed = world.json(runner, 0, &["groom", "prepare", "--shard-size", "9"]);
        let id = printed["batch"].as_str().expect("a batch").to_owned();
        let record = world.batch_record(&id);
        let shards: Vec<(String, Vec<String>)> = record["shards"]
            .as_array()
            .expect("shards")
            .iter()
            .map(|shard| {
                (
                    shard["verdicts"].as_str().expect("a path").to_owned(),
                    shard["tickets"]
                        .as_array()
                        .expect("tickets")
                        .iter()
                        .map(|one| one.as_str().expect("an id").to_owned())
                        .collect(),
                )
            })
            .collect();
        assert_eq!(shards.len(), 2);
        let (f, s) = (fix.as_str(), first.as_str());
        let good = [
            verdict("o1", "fixed", "high", Some(f), &["src/a.rs"], None, None),
            verdict("o2", "obsolete", "medium", Some(s), &[], None, None),
            verdict("o3", "keep", "high", None, &["src/b.rs"], Some("q1"), None),
            verdict("o4", "keep", "high", None, &["src/b.rs"], Some("q2"), None),
            verdict(
                "o5",
                "keep",
                "high",
                None,
                &["src/a.rs", "src/b.rs"],
                None,
                Some(GADGETS),
            ),
            verdict(
                "o6",
                "keep",
                "medium",
                None,
                &["src/a.rs"],
                Some("o7"),
                None,
            ),
            verdict(
                "o7",
                "keep",
                "high",
                None,
                &["src/a.rs", "src/b.rs"],
                None,
                None,
            ),
            verdict(
                "o8",
                "fixed",
                "high",
                Some(f),
                &["src/a.rs"],
                Some("o9"),
                None,
            ),
            verdict("o9", "keep", "high", None, &["src/a.rs"], None, None),
            verdict(
                "g1",
                "keep",
                "high",
                None,
                &["lib/x.rs"],
                Some("o5"),
                Some(GADGETS),
            ),
            verdict("q1", "fixed", "high", Some(f), &["src/a.rs"], None, None),
            verdict("q2", "keep", "high", None, &["src/b.rs"], Some("p1"), None),
            verdict("p1", "keep", "high", None, &["src/b.rs"], None, None),
            verdict("t1", "fixed", "high", Some(f), &["src/a.rs"], None, None),
            verdict(
                "t2",
                "obsolete",
                "medium",
                Some(f),
                &["src/a.rs"],
                None,
                None,
            ),
            verdict("t4", "keep", "high", None, &["src/b.rs"], Some("p1"), None),
            verdict("u1", "unresolved", "low", None, &[], None, None),
        ]
        .into_iter()
        .map(|one| (one["ticket"].as_str().expect("an id").to_owned(), one))
        .collect();
        Self {
            world,
            id,
            shards,
            first,
            fix,
            side,
            good,
        }
    }

    /// Write every shard's verdicts from `verdicts`, then each of `extra` as a further line.
    fn write(&self, verdicts: &BTreeMap<String, Value>, extra: &[(&str, String)]) {
        for (path, tickets) in &self.shards {
            let mut text = String::new();
            for ticket in tickets {
                if let Some(one) = verdicts.get(ticket) {
                    text.push_str(&one.to_string());
                    text.push('\n');
                }
                for (on, line) in extra {
                    if t(on) == *ticket {
                        text.push_str(line);
                        text.push('\n');
                    }
                }
            }
            std::fs::write(path, text).expect("verdicts written");
        }
    }

    fn check(&self, runner: &dyn Runner, code: i32) -> Value {
        self.world
            .json(runner, code, &["groom", "check", "--batch", &self.id])
    }

    fn plan_path(&self) -> std::path::PathBuf {
        self.world.batch(&self.id).join("plan.json")
    }

    /// Check refuses at exit 2 naming `rule` for `ticket`, and writes no plan over what stands.
    fn refuses(&self, runner: &dyn Runner, ticket: &str, rule: &str, case: &str) {
        let before = std::fs::read_to_string(self.plan_path()).ok();
        let printed = self.check(runner, 2);
        let problems = printed["problems"].as_array().expect("problems");
        assert!(
            problems
                .iter()
                .any(|one| one["ticket"] == t(ticket) && one["rule"] == rule),
            "{case}: check names `{rule}` for {ticket}: {problems:#?}"
        );
        assert!(
            problems.iter().all(|one| one["detail"]
                .as_str()
                .is_some_and(|detail| !detail.is_empty())),
            "{case}: every problem says what is wrong"
        );
        assert_eq!(
            std::fs::read_to_string(self.plan_path()).ok(),
            before,
            "{case}: a refused check writes no plan"
        );
    }
}

fn ids(entries: &Value) -> Vec<String> {
    entries
        .as_array()
        .expect("a list")
        .iter()
        .map(|one| {
            one["ticket"]
                .as_str()
                .expect("an id")
                .trim_start_matches("followups:")
                .to_owned()
        })
        .collect()
}

fn entry<'a>(plan: &'a Value, list: &str, ticket: &str) -> &'a Value {
    plan[list]
        .as_array()
        .expect("a list")
        .iter()
        .find(|one| one["ticket"] == t(ticket))
        .unwrap_or_else(|| panic!("{ticket} is in `{list}`: {plan:#}"))
}

pub fn verdicts_are_held_to_the_rules_and_planned(runner: &dyn Runner) {
    let batch = Batch::new(runner);
    let good = &batch.good;
    let side = batch.side.as_str();
    let absent = "0123456789abcdef0123456789abcdef01234567";

    let cases: Vec<Case<'_>> = {
        let with = |ticket: &str, key: &str, value: Value| {
            let mut changed = good.clone();
            changed
                .get_mut(&t(ticket))
                .expect("a verdict")
                .as_object_mut()
                .expect("an object")
                .insert(key.to_owned(), value);
            changed
        };
        let without = |ticket: &str| {
            let mut changed = good.clone();
            changed.remove(&t(ticket));
            changed
        };
        vec![
            (
                "a fixed verdict off main",
                "o1",
                "commit-not-on-main",
                with("o1", "deciding_commit", json!(side)),
                vec![],
            ),
            (
                "an obsolete verdict off main",
                "o2",
                "commit-not-on-main",
                with("o2", "deciding_commit", json!(side)),
                vec![],
            ),
            (
                "a closing commit no clone holds",
                "o2",
                "commit-not-on-main",
                with("o2", "deciding_commit", json!(absent)),
                vec![],
            ),
            (
                "a low-confidence close",
                "o1",
                "low-confidence-close",
                with("o1", "confidence", json!("low")),
                vec![],
            ),
            (
                "a shard missing a verdict",
                "u1",
                "missing-verdict",
                without("u1"),
                vec![],
            ),
            (
                "two verdicts for one ticket",
                "o3",
                "duplicate-verdict",
                good.clone(),
                vec![("o3", good[&t("o3")].to_string())],
            ),
        ]
    };
    for (case, ticket, rule, verdicts, extra) in cases {
        batch.write(&verdicts, &extra);
        batch.refuses(runner, ticket, rule, case);
        assert!(!batch.plan_path().exists(), "{case}: no plan is written");
    }

    batch.write(good, &[]);
    let printed = batch.check(runner, 0);
    assert_eq!(printed["batch"], batch.id);
    assert_eq!(printed["problems"], json!([]));
    let plan = &printed["plan"];
    let written: Value = serde_json::from_str(
        &std::fs::read_to_string(batch.plan_path()).expect("a passing check writes the plan"),
    )
    .expect("JSON");
    assert_eq!(written["schema_version"], 1);
    assert_eq!(
        &written["plan"], plan,
        "plan.json holds the plan check answered"
    );

    assert_eq!(ids(&plan["fixed"]), ["o1", "o8", "t1"], "{plan:#}");
    assert_eq!(ids(&plan["obsolete"]), ["o2"]);
    let t1 = entry(plan, "fixed", "t1");
    assert_eq!(
        t1["status"], "todo",
        "a definitely-outdated Todo ticket is closed"
    );
    assert_eq!(t1["deciding_commit"], batch.fix);
    assert_eq!(t1["repository"], WIDGETS);
    assert_eq!(
        entry(plan, "obsolete", "o2")["deciding_commit"],
        batch.first
    );

    let duplicates: Vec<(String, String)> = plan["duplicates"]
        .as_array()
        .expect("duplicates")
        .iter()
        .map(|one| {
            (
                one["ticket"].as_str().expect("an id").to_owned(),
                one["duplicate_of"].as_str().expect("a survivor").to_owned(),
            )
        })
        .collect();
    assert_eq!(
        duplicates,
        [
            (t("o3"), t("q1")),
            (t("o4"), t("p1")),
            (t("o5"), t("g1")),
            (t("o6"), t("o7")),
        ],
        "each folded member names its cluster's survivor"
    );
    assert!(
        entry(plan, "duplicates", "o3")["root_cause"]
            .as_str()
            .is_some_and(|cause| !cause.is_empty())
    );
    let survivors: Vec<(String, Vec<String>, String)> = plan["survivors"]
        .as_array()
        .expect("survivors")
        .iter()
        .map(|one| {
            (
                one["ticket"].as_str().expect("an id").to_owned(),
                one["members"]
                    .as_array()
                    .expect("members")
                    .iter()
                    .map(|member| member.as_str().expect("an id").to_owned())
                    .collect(),
                one["chosen_by"].as_str().expect("a rule").to_owned(),
            )
        })
        .collect();
    let survivor = |ticket: &str| {
        survivors
            .iter()
            .find(|(one, _, _)| *one == t(ticket))
            .unwrap_or_else(|| panic!("{ticket} survives its cluster: {survivors:?}"))
            .clone()
    };
    assert_eq!(
        survivor("q1"),
        (t("q1"), vec![t("o3"), t("q1")], "claimed".to_owned()),
        "a claimed ticket survives, its closing verdict left for a person"
    );
    assert_eq!(
        survivor("p1"),
        (
            t("p1"),
            vec![t("o4"), t("p1"), t("q2"), t("t4")],
            "oldest".to_owned()
        ),
        "two claimed members tie on every rule before the last"
    );
    assert_eq!(
        survivor("g1"),
        (
            t("g1"),
            vec![t("g1"), t("o5")],
            "seam-repository".to_owned()
        ),
        "the member in the repository its members name as the seam's wins over more evidence"
    );
    assert_eq!(
        survivor("o7"),
        (t("o7"), vec![t("o6"), t("o7")], "most-evidence".to_owned())
    );
    assert_eq!(
        survivors.len(),
        4,
        "a cluster its closing member left is no cluster: {survivors:?}"
    );

    assert_eq!(
        ids(&plan["kept"]),
        ["g1", "o7", "o9", "p1", "q1", "q2", "t2", "t4", "u1"],
        "everything not closed or folded is kept"
    );
    for (ticket, why) in [
        ("q1", "never closes"),
        ("q2", "never folds"),
        ("t2", "closes only on a high-confidence verdict"),
        ("t4", "never folds as a duplicate"),
    ] {
        let kept = entry(plan, "kept", ticket);
        assert!(
            kept["left_for_person"]
                .as_str()
                .is_some_and(|reason| reason.contains(why)),
            "{ticket} is left for a person because it {why}: {kept:#}"
        );
        assert!(
            kept["evidence"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }
    assert_eq!(entry(plan, "kept", "q1")["verdict"], "fixed");
    for ticket in ["g1", "o7", "o9", "p1", "u1"] {
        assert_eq!(entry(plan, "kept", ticket)["left_for_person"], Value::Null);
    }
    assert_eq!(ids(&plan["unresolved"]), ["u1"]);
}

pub fn malformed_verdict_records_are_refused_by_rule(runner: &dyn Runner) {
    let batch = Batch::new(runner);
    let good = &batch.good;
    let with = |ticket: &str, key: &str, value: Option<Value>| {
        let mut changed = good.clone();
        let object = changed
            .get_mut(&t(ticket))
            .expect("a verdict")
            .as_object_mut()
            .expect("an object");
        match value {
            Some(value) => {
                object.insert(key.to_owned(), value);
            }
            None => {
                object.shift_remove(key);
            }
        }
        changed
    };
    let cases = [
        (
            "a fixed verdict with empty evidence",
            "o1",
            "evidence-required",
            with("o1", "evidence", Some(json!(""))),
        ),
        (
            "an obsolete verdict with empty evidence",
            "o2",
            "evidence-required",
            with("o2", "evidence", Some(json!("  "))),
        ),
        (
            "a record missing a field",
            "o9",
            "missing-field",
            with("o9", "confidence", None),
        ),
        (
            "an unknown verdict",
            "o9",
            "invalid-value",
            with("o9", "verdict", Some(json!("maybe"))),
        ),
        (
            "an unknown confidence",
            "o9",
            "invalid-value",
            with("o9", "confidence", Some(json!("certain"))),
        ),
        (
            "a deciding commit that is not 40 hex",
            "o1",
            "invalid-value",
            with("o1", "deciding_commit", Some(json!("abc123"))),
        ),
        (
            "another schema",
            "o9",
            "schema-version",
            with("o9", "schema_version", Some(json!(2))),
        ),
        (
            "an unknown key",
            "o9",
            "unknown-key",
            with("o9", "severity", Some(json!("high"))),
        ),
        (
            "a duplicate with no root cause",
            "o6",
            "root-cause-required",
            with("o6", "root_cause", Some(Value::Null)),
        ),
        (
            "a closing verdict without its commit",
            "o1",
            "deciding-commit",
            with("o1", "deciding_commit", Some(Value::Null)),
        ),
        (
            "a ticket that is not text",
            "o9",
            "missing-verdict",
            with("o9", "ticket", Some(json!(9))),
        ),
        (
            "evidence that is not text",
            "o9",
            "invalid-value",
            with("o9", "evidence", Some(json!(3))),
        ),
        (
            "an evidence path leaving the repository",
            "o9",
            "invalid-value",
            with("o9", "evidence_paths", Some(json!(["../secrets"]))),
        ),
        (
            "an empty duplicate",
            "o9",
            "invalid-value",
            with("o9", "duplicate_of", Some(json!(""))),
        ),
        (
            "a seam that is no repository",
            "o9",
            "invalid-value",
            with("o9", "seam_repository", Some(json!("widgets"))),
        ),
        (
            "a kept verdict naming a deciding commit",
            "o9",
            "deciding-commit",
            with("o9", "deciding_commit", Some(json!(batch.fix))),
        ),
        (
            "a duplicate of itself",
            "o6",
            "duplicate-of-self",
            with("o6", "duplicate_of", Some(json!(t("o6")))),
        ),
        (
            "a duplicate of a ticket nobody holds open",
            "o9",
            "duplicate-of-unknown",
            {
                let mut changed = with("o9", "duplicate_of", Some(json!("followups:nowhere")));
                changed
                    .get_mut(&t("o9"))
                    .and_then(Value::as_object_mut)
                    .expect("an object")
                    .insert("root_cause".to_owned(), json!("a cause"));
                changed
            },
        ),
    ];
    for (case, ticket, rule, verdicts) in cases {
        batch.write(&verdicts, &[]);
        batch.refuses(runner, ticket, rule, case);
        batch.write(good, &[]);
        let printed = batch.check(runner, 0);
        assert_eq!(
            printed["problems"],
            json!([]),
            "{case}: corrected, the check passes"
        );
    }

    batch.write(
        good,
        &[("o9", "not json".to_owned()), ("o9", "[1]".to_owned())],
    );
    let printed = batch.check(runner, 2);
    let rules: Vec<&str> = printed["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|one| one["rule"].as_str().expect("a rule"))
        .collect();
    assert_eq!(
        rules,
        ["verdict-record", "verdict-record"],
        "a line that is no object names no ticket"
    );
    let mut stranger = good.clone();
    stranger.insert(t("zz"), {
        let mut one = good[&t("o9")].clone();
        one["ticket"] = json!(t("zz"));
        one
    });
    batch.write(&stranger, &[("o9", stranger[&t("zz")].to_string())]);
    batch.refuses(
        runner,
        "zz",
        "unknown-ticket",
        "a verdict for a ticket of no shard",
    );
}

pub fn a_plan_breaking_protection_is_refused_and_kept(runner: &dyn Runner) {
    let batch = Batch::new(runner);
    batch.write(&batch.good, &[]);
    let passed = batch.check(runner, 0);
    let saved = std::fs::read_to_string(batch.plan_path()).expect("the plan");
    let held: Value = serde_json::from_str(&saved).expect("JSON");
    let kept = |ticket: &str| entry(&held["plan"], "kept", ticket).clone();

    let edit = |change: &dyn Fn(&mut Value)| {
        let mut plan = held.clone();
        change(&mut plan["plan"]);
        serde_json::to_string_pretty(&plan).expect("JSON")
    };
    let push = |list: &'static str, entry: Value| {
        move |plan: &mut Value| {
            plan[list]
                .as_array_mut()
                .expect("a list")
                .push(entry.clone());
        }
    };
    let mut closing_q1 = kept("q1");
    closing_q1["left_for_person"] = Value::Null;
    let mut folding_p1 = kept("p1");
    folding_p1["duplicate_of"] = json!(t("q1"));
    let mut folding_t4 = kept("t4");
    folding_t4["duplicate_of"] = json!(t("p1"));
    folding_t4["left_for_person"] = Value::Null;
    let mut closing_t2 = kept("t2");
    closing_t2["left_for_person"] = Value::Null;
    let side = batch.side.clone();
    let cases: Vec<(&str, &str, &str, String)> = vec![
        (
            "a plan closing a Queued ticket",
            "q1",
            "claimed-closed",
            edit(&push("fixed", closing_q1)),
        ),
        (
            "a plan folding an In Progress ticket",
            "p1",
            "claimed-folded",
            edit(&push("duplicates", folding_p1)),
        ),
        (
            "a plan folding a Todo ticket",
            "t4",
            "todo-folded",
            edit(&push("duplicates", folding_t4)),
        ),
        (
            "a plan closing a Todo ticket on a medium verdict",
            "t2",
            "todo-close-below-bar",
            edit(&push("obsolete", closing_t2)),
        ),
        (
            "a plan closing a Todo ticket on a commit off main",
            "t1",
            "todo-close-below-bar",
            edit(&|plan: &mut Value| {
                let t1 = plan["fixed"]
                    .as_array_mut()
                    .expect("fixed")
                    .iter_mut()
                    .find(|one| one["ticket"] == t("t1"))
                    .expect("t1 is closed");
                t1["deciding_commit"] = json!(side);
            }),
        ),
    ];
    for (case, ticket, rule, plan) in cases {
        std::fs::write(batch.plan_path(), &plan).expect("the plan edited");
        batch.refuses(runner, ticket, rule, case);
        assert_eq!(
            std::fs::read_to_string(batch.plan_path()).expect("the plan"),
            plan,
            "{case}: nothing is written over the plan"
        );
        std::fs::write(batch.plan_path(), &saved).expect("the plan corrected");
        assert_eq!(
            batch.check(runner, 0),
            passed,
            "{case}: corrected, the check passes"
        );
    }

    let mut closing_o9 = kept("o9");
    closing_o9["verdict"] = json!("fixed");
    closing_o9["deciding_commit"] = json!(batch.side.clone());
    let mut low_o9 = closing_o9.clone();
    low_o9["confidence"] = json!("low");
    low_o9["deciding_commit"] = json!(batch.fix.clone());
    let mut stranger = kept("o9");
    stranger["ticket"] = json!(t("zz"));
    let more: Vec<(&str, Option<&str>, &str, String)> = vec![
        (
            "a plan closing on a commit off main",
            Some("o9"),
            "commit-not-on-main",
            edit(&push("fixed", closing_o9)),
        ),
        (
            "a plan closing on a low verdict",
            Some("o9"),
            "low-confidence-close",
            edit(&push("obsolete", low_o9)),
        ),
        (
            "a plan closing a ticket the batch never listed",
            Some("zz"),
            "plan-record",
            edit(&push("fixed", stranger.clone())),
        ),
        (
            "a plan folding a ticket the batch never listed",
            Some("zz"),
            "plan-record",
            edit(&push("duplicates", stranger)),
        ),
        ("another batch's plan", None, "plan-record", {
            let mut plan = held.clone();
            plan["batch"] = json!("groom-20260101T000000000Z");
            serde_json::to_string_pretty(&plan).expect("JSON")
        }),
    ];
    for (case, ticket, rule, plan) in more {
        std::fs::write(batch.plan_path(), &plan).expect("the plan edited");
        let printed = batch.check(runner, 2);
        assert!(
            printed["problems"]
                .as_array()
                .expect("problems")
                .iter()
                .any(|one| {
                    one["rule"] == rule
                        && one["ticket"] == ticket.map_or(Value::Null, |id| json!(t(id)))
                }),
            "{case}: check names `{rule}`: {printed:#}"
        );
        std::fs::write(batch.plan_path(), &saved).expect("the plan corrected");
        assert_eq!(
            batch.check(runner, 0),
            passed,
            "{case}: corrected, the check passes"
        );
    }

    std::fs::write(batch.plan_path(), "{\"schema_version\": 2}").expect("a stranger plan");
    let printed = batch.check(runner, 2);
    assert_eq!(printed["problems"][0]["rule"], "plan-record");
    assert_eq!(printed["problems"][0]["ticket"], Value::Null);
    std::fs::remove_file(batch.plan_path()).expect("removed");
    assert_eq!(
        batch.check(runner, 0),
        passed,
        "removed, the check writes its plan"
    );
    assert_eq!(
        std::fs::read_to_string(batch.plan_path()).expect("the plan"),
        saved
    );
}

pub fn a_batch_not_prepared_is_refused(runner: &dyn Runner) {
    let world = World::new();
    let ran = runner.run(
        &world,
        &["groom", "check", "--batch", "groom-20260101T000000000Z"],
    );
    assert_eq!(ran.code, 2, "{}", ran.stderr);
    assert!(
        ran.stderr
            .contains("no grooming batch groom-20260101T000000000Z was prepared"),
        "{}",
        ran.stderr
    );
    let ran = runner.run(&world, &["groom", "check", "--batch", "../elsewhere"]);
    assert_eq!(ran.code, 2, "{}", ran.stderr);
    assert!(
        ran.stderr.contains("is not a grooming batch id"),
        "{}",
        ran.stderr
    );
}
