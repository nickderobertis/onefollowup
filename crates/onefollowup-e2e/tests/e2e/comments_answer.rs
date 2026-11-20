//! `comments answer`: the comment pass in one verb. Two batches' tickets carry a person's
//! comment each; `comments.launch_command` is a scratch `answer-batch` script that records its
//! arguments and, playing the answering agent, posts the reply its batch's feedback file shapes
//! and writes the account. Both batches succeeding settle as `comments settle` does; one
//! failing settles nothing, and a re-run gathers afresh and settles; `--dry-run` is
//! `comments gather --dry-run`; and a configuration without the key is refused before anything
//! is gathered.

use std::path::Path;

use serde_json::Value;

use crate::answering::{DESTINATION, comment, file, pending, replies_to, watermark};
use crate::world::World;

const A: &str = "nightly-2026-11-01";
const B: &str = "nightly-2026-11-02";

/// The configuration naming the scratch script, each placeholder its own argument.
const CONFIG: &str = "host_label: build-host-1\ncomments:\n  launch_command: [answer-batch, '{batch}', '{feedback}', '{snapshot}']\n";

/// One call the script recorded: its batch, feedback file and snapshot, and whether the
/// feedback file was there when it ran.
#[derive(Debug, PartialEq, Eq)]
struct Call {
    batch: String,
    feedback: String,
    snapshot: String,
    feedback_present: bool,
}

/// A world configured by `config`, with `answer-batch` on its `PATH`, and one ticket each of
/// `A` and `B` carrying one person's comment: `(world, [(item, comment id)])`.
fn answering_world(config: &str) -> (World, Vec<(String, String)>) {
    let mut world = World::new(&[DESTINATION], config);
    let root = world.root.display().to_string();
    // The answering agent, as a shell script: it records the call, fails where `fail-<batch>`
    // exists, and otherwise answers the one comment its feedback file quotes as that file says.
    world.script(
        "answer-batch",
        &format!(
            r#"present=absent; [ -f "$2" ] && present=present
printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$present" >> {root}/calls.log
if [ -f "{root}/fail-$1" ]; then echo "the answering dispatch of $1 failed" >&2; exit 3; fi
issue=$(sed -n 's/^<!-- orchestrator:follow-up-quoted issue="\([^"]*\)".*/\1/p' "$2" | head -n 1)
id=$(sed -n 's/^<!-- orchestrator:follow-up-quoted issue="[^"]*" comment="\([^"]*\)".*/\1/p' "$2" | head -n 1)
opening=$(sed -n 's/^- Reply opening: `\(.*\)`$/\1/p' "$2" | head -n 1)
marker=$(sed -n 's/^- Marker if it does not: `\(.*\)`$/\1/p' "$2" | head -n 1)
at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
printf '\n<!-- onetaskgraph:comment id="%s-reply" author="follow-up-agent" created_at="%s" updated_at="%s" -->\n### follow-up-agent — %s\n\n%s\n\nThe ticket stands as written.\n\n%s\n\n<!-- /onetaskgraph:comment -->\n' "$id" "$at" "$at" "$at" "$opening" "$marker" >> "{root}/{DESTINATION}-root/tasks/${{issue#*:}}.md"
stem=$(basename "$2" .md)
printf '{{"schema": 3, "run": "%s", "feedback": "%s.md", "responses": [{{"comment": "%s", "issue": "%s", "action": "Read the ticket against the comment.", "reply": "%s-reply", "reply_url": null, "reply_fallback": null, "verdict": "does-not-confirm"}}]}}\n' "$1" "$stem" "$id" "$issue" "$id" > "$(dirname "$2")/$stem.responses.json""#
        ),
    );
    let a = file(&mut world, A, &["layout-save-reads-size-twice"]);
    let b = file(&mut world, B, &["export-drops-unicode"]);
    let commented = vec![
        (a[0].clone(), "20261101T100000Z-1".to_owned()),
        (b[0].clone(), "20261101T102000Z-1".to_owned()),
    ];
    comment(
        &world,
        &commented[0].0,
        &commented[0].1,
        "alice",
        "2026-11-01T10:00:00Z",
        "Is this still happening after the 2.3 release?",
    );
    comment(
        &world,
        &commented[1].0,
        &commented[1].1,
        "carol",
        "2026-11-01T10:20:00Z",
        "Does this drop every non-ASCII character, or only emoji?",
    );
    world.reset_phases();
    (world, commented)
}

/// Every call the script recorded, in order.
fn calls(root: &Path) -> Vec<Call> {
    std::fs::read_to_string(root.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            assert_eq!(parts.len(), 4, "{line}");
            Call {
                batch: parts[0].to_owned(),
                feedback: parts[1].to_owned(),
                snapshot: parts[2].to_owned(),
                feedback_present: parts[3] == "present",
            }
        })
        .collect()
}

/// The batches a snapshot selected comments for, in the order `comments launches` plans them.
fn launch_order(snapshot: &str) -> Vec<String> {
    let record: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{snapshot}/snapshot.json")).expect("the record"),
    )
    .expect("JSON");
    record["selection"]
        .as_object()
        .expect("a selection")
        .keys()
        .cloned()
        .collect()
}

/// The snapshot directories gathered so far.
fn snapshots(world: &World) -> usize {
    std::fs::read_dir(world.work().join("feedback/.snapshots"))
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0)
}

#[test]
fn every_batch_answering_settles_the_pass_as_comments_settle_does() {
    let (mut world, commented) = answering_world(CONFIG);

    let answered = world.ok("answer", &["comments", "answer"], "");
    let snapshot = answered["snapshot"]
        .as_str()
        .expect("a snapshot")
        .to_owned();
    let order = launch_order(&snapshot);
    assert_eq!(order.len(), 2, "both batches were selected: {answered}");
    let recorded = calls(&world.root);
    let expected: Vec<Call> = order
        .iter()
        .map(|batch| Call {
            batch: batch.clone(),
            feedback: format!(
                "{}/feedback/{batch}/{}.md",
                world.work().display(),
                Path::new(&snapshot)
                    .file_name()
                    .expect("a stamp")
                    .to_string_lossy()
            ),
            snapshot: snapshot.clone(),
            feedback_present: true,
        })
        .collect();
    assert_eq!(
        recorded, expected,
        "once per batch, in launch order, each with its own placeholders and feedback file"
    );
    let launches: Vec<(&str, &str, i64)> = answered["launches"]
        .as_array()
        .expect("launches")
        .iter()
        .map(|one| {
            (
                one["batch"].as_str().expect("a batch"),
                one["feedback"].as_str().expect("a feedback file"),
                one["exit"].as_i64().expect("an exit"),
            )
        })
        .collect();
    assert_eq!(
        launches,
        expected
            .iter()
            .map(|call| (call.batch.as_str(), call.feedback.as_str(), 0))
            .collect::<Vec<_>>(),
        "{answered}"
    );

    let settled = &answered["settled"];
    assert_eq!(settled["snapshot"], snapshot.as_str(), "{answered}");
    let verified: Vec<(&str, i64)> = settled["settled"]
        .as_array()
        .expect("settled")
        .iter()
        .map(|one| {
            (
                one["batch"].as_str().expect("a batch"),
                one["verified"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    assert_eq!(verified, [(A, 1), (B, 1)], "{answered}");
    assert_eq!(settled["needs_attention"], Value::Null, "{answered}");
    assert!(pending(&world).is_empty());
    for (item, id) in &commented {
        assert_eq!(replies_to(&world, item, id), 1, "{id} has one reply");
    }
    let moved = watermark(&world, DESTINATION).expect("the watermark moved");
    assert_eq!(settled["watermark"], moved["since"], "{answered}");
    let operations: Vec<&str> = answered["store_operations"]
        .as_array()
        .expect("operations")
        .iter()
        .filter(|one| one["destination"] == true)
        .map(|one| one["op"].as_str().expect("an op"))
        .collect();
    assert_eq!(
        operations.first(),
        Some(&"comments-since"),
        "the gathering's listing, then the settle's: {answered}"
    );
    assert!(operations.contains(&"read"), "{answered}");

    // The next pass finds nothing to answer: its empty launch list runs nothing and settles.
    let again = world.ok("answer", &["comments", "answer"], "");
    assert_eq!(again["launches"], serde_json::json!([]), "{again}");
    assert_eq!(calls(&world.root).len(), 2, "no batch was launched again");
}

#[test]
fn a_failed_batch_settles_nothing_and_a_re_run_gathers_afresh() {
    let (mut world, commented) = answering_world(CONFIG);
    std::fs::write(world.root.join(format!("fail-{B}")), "").expect("the failure");

    let refused = world.fails(1, &["comments", "answer"], "");
    assert!(
        refused.contains(&format!(
            "failed for batch `{B}` (exit 3: the answering dispatch of {B} failed)"
        )),
        "{refused}"
    );
    assert!(
        !refused.contains(&format!("`{A}`")),
        "only the failed batch is named: {refused}"
    );
    let batches: Vec<String> = calls(&world.root)
        .into_iter()
        .map(|call| call.batch)
        .collect();
    assert_eq!(batches.len(), 2, "the other batch's command still ran");
    assert!(batches.contains(&A.to_owned()) && batches.contains(&B.to_owned()));
    assert!(
        watermark(&world, DESTINATION).is_none(),
        "nothing was settled, so the watermark is unmoved"
    );
    assert!(pending(&world).is_empty(), "nothing was settled");

    // The re-run gathers afresh: A's posted reply answers its comment, so only B is launched.
    std::fs::remove_file(world.root.join(format!("fail-{B}"))).expect("the fix");
    let answered = world.ok("answer", &["comments", "answer"], "");
    let relaunched: Vec<String> = calls(&world.root)[2..]
        .iter()
        .map(|call| call.batch.clone())
        .collect();
    assert_eq!(relaunched, [B], "{answered}");
    assert_ne!(
        answered["snapshot"],
        calls(&world.root)[0].snapshot.as_str(),
        "a fresh snapshot"
    );
    for (item, id) in &commented {
        assert_eq!(replies_to(&world, item, id), 1, "{id} has one reply");
    }
    let moved = watermark(&world, DESTINATION).expect("the watermark moved");
    assert_eq!(
        answered["settled"]["watermark"], moved["since"],
        "{answered}"
    );
}

#[test]
fn a_dry_run_prints_what_comments_gather_dry_run_prints_and_launches_nothing() {
    let (mut world, _) = answering_world(CONFIG);
    let gathered = world.ok("gather", &["comments", "gather", "--dry-run"], "");
    let answered = world.ok("answer", &["comments", "answer", "--dry-run"], "");
    // Each dry run writes its own snapshot, named by the instant it was taken.
    let theirs = gathered["snapshot"].as_str().expect("a snapshot");
    let ours = answered["snapshot"].as_str().expect("a snapshot");
    let normalized = |value: &Value, snapshot: &str| -> Value {
        serde_json::from_str(&value.to_string().replace(snapshot, "<snapshot>")).expect("JSON")
    };
    assert_eq!(
        normalized(&answered, ours),
        normalized(&gathered, theirs),
        "{answered}"
    );
    assert_eq!(answered["dry_run"], true, "{answered}");
    assert!(calls(&world.root).is_empty(), "nothing was launched");
    assert!(
        !Path::new(ours).join("launched.json").exists(),
        "the snapshot was not claimed"
    );
    assert!(
        watermark(&world, DESTINATION).is_none(),
        "nothing was settled"
    );
}

#[test]
fn without_a_launch_command_it_is_refused_before_anything_is_gathered() {
    let (world, _) = answering_world("host_label: build-host-1\n");
    for args in [
        &["comments", "answer"][..],
        &["comments", "answer", "--dry-run"],
    ] {
        let refused = world.fails(2, args, "");
        assert!(
            refused.contains("`comments.launch_command` is not configured"),
            "{refused}"
        );
        assert!(refused.contains("nothing was gathered"), "{refused}");
    }
    assert_eq!(snapshots(&world), 0, "no snapshot was taken");
    assert!(calls(&world.root).is_empty());
}
