//! The verbs' result documents, held to the contract document.
//!
//! `README.md`'s "Results" section states each verb's result once, as an example JSON
//! document under a `#### \`<verb>\`` heading. This builds the same example from the
//! library's result types and requires the two to be equal, and requires the section to
//! name exactly the verbs the CLI defines (but `kinds` and `config show`, whose documents
//! the configuration section states). A field renamed, added or removed on either side
//! fails here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::CommandFactory as _;
use indexmap::IndexMap;
use onefollowup::api::{
    AnsweredLaunch, AnsweredPass, BoardComment, BoardStatus, Checked, CommentsAnswered,
    CommentsChecked, CommentsSettled, Drafted, DuplicateCandidate, DuplicateQuery, DuplicateSearch,
    Gathered, GroomChecked, GroomPrepared, GroomProblem, GroomShard, HookRun, ItemStatus, Launch,
    Launches, Prepared, Problem, PublishOutcome, Published, QueryKind, RoleStatus, SearchHit,
    SearchResults, Settled, SettledLaunch, Snapshotted, Staged, StagedOp, StatusesShown,
    TicketValidated,
};
use onefollowup::cli::Cli;
use onefollowup::config::StatusRole;
use onefollowup::formats::groom::{Confidence, GroomPlan, GroomVerdict, PlanEntry, Survivor};
use onefollowup::formats::outbox::StagedWrite;
use onefollowup::formats::{HeldPriority, Level};
use onefollowup::store::StoreOperation;
use serde_json::{Value, to_value};

const README: &str = include_str!("../../../README.md");
const BATCH: &str = "nightly-2026-11-01";

fn status() -> Option<ItemStatus> {
    Some(ItemStatus {
        category: "backlog".to_owned(),
        name: "Proposal".to_owned(),
    })
}

const GROOM: &str = "groom-20261121T090000000Z";

fn groomed(ticket: &str, survivor: Option<&str>, left: Option<&str>) -> PlanEntry {
    let duplicate = survivor.is_some();
    PlanEntry {
        ticket: ticket.to_owned(),
        status: if duplicate { "backlog" } else { "in-progress" }.to_owned(),
        verdict: if duplicate {
            GroomVerdict::Keep
        } else {
            GroomVerdict::Fixed
        },
        confidence: Confidence::High,
        evidence: "src/layout.rs reads the size once since the fix".to_owned(),
        deciding_commit: (!duplicate)
            .then(|| "0123456789abcdef0123456789abcdef01234567".to_owned()),
        evidence_paths: vec!["src/layout.rs".to_owned()],
        repository: Some("github.com/example/widgets".to_owned()),
        commit: Some("89abcdef0123456789abcdef0123456789abcdef".to_owned()),
        duplicate_of: survivor.map(str::to_owned),
        root_cause: duplicate.then(|| "the layout's save reads the window size twice".to_owned()),
        left_for_person: left.map(str::to_owned),
    }
}

fn staged(write: StagedWrite, index: usize) -> Value {
    to_value(Staged {
        schema_version: 1,
        batch: BATCH.to_owned(),
        index,
        write,
        matched: None,
        store_operations: Vec::new(),
    })
    .expect("JSON")
}

/// Every stubbed verb's result, as the library's types build it.
fn examples() -> BTreeMap<&'static str, Value> {
    let ticket = format!("drafts:{BATCH}/tickets/layout-save-reads-size-twice");
    let snapshot = PathBuf::from("feedback/.snapshots/20261101T120000Z");
    let feedback = PathBuf::from(format!("feedback/{BATCH}/20261101T120000Z.md"));
    let json = |value: Result<Value, serde_json::Error>| value.expect("JSON");
    BTreeMap::from([
        (
            "draft",
            json(to_value(Drafted {
                schema_version: 1,
                batch: BATCH.to_owned(),
                draft: format!("drafts:{BATCH}/drafts/20261101T101500Z-layout-save-loses-resize"),
                path: PathBuf::from(format!(
                    ".follow-ups/tasks/{BATCH}/drafts/20261101T101500Z-layout-save-loses-resize.md"
                )),
            })),
        ),
        (
            "prepare",
            json(to_value(Prepared {
                schema_version: 1,
                batch: BATCH.to_owned(),
                drafts: 3,
                inputs: 1,
                answers: PathBuf::from(format!("dispositions/{BATCH}.json")),
                task_template: PathBuf::from(format!("tasks/{BATCH}/task.md")),
                absent_targets: Vec::new(),
                missing_links: Vec::new(),
                duplicate_searches: vec![DuplicateSearch {
                    input: format!(
                        "drafts:{BATCH}/drafts/20261101T101500Z-layout-save-loses-resize"
                    ),
                    stoplisted: vec!["after".to_owned()],
                    queries: vec![
                        DuplicateQuery {
                            text: "layout save loses its size after a resize".to_owned(),
                            kind: QueryKind::Title,
                            hits: 0,
                            retained: 0,
                            capped: false,
                        },
                        DuplicateQuery {
                            text: "layout".to_owned(),
                            kind: QueryKind::Term,
                            hits: 31,
                            retained: 21,
                            capped: true,
                        },
                        DuplicateQuery {
                            text: "resize".to_owned(),
                            kind: QueryKind::Term,
                            hits: 2,
                            retained: 2,
                            capped: false,
                        },
                        DuplicateQuery {
                            text: "layout.rs".to_owned(),
                            kind: QueryKind::File,
                            hits: 1,
                            retained: 1,
                            capped: false,
                        },
                    ],
                    matching: 1,
                    candidates: vec![DuplicateCandidate {
                        item: "followups:item-17".to_owned(),
                        title: "widgets: a resize must keep the saved layout".to_owned(),
                        status: onefollowup::store::ItemStatus {
                            category: "done".to_owned(),
                            name: "Done".to_owned(),
                        },
                        url: None,
                        root_cause: "resize-keeps-layout".to_owned(),
                        shared: vec!["resize".to_owned(), "layout.rs".to_owned()],
                        capped: vec!["layout".to_owned()],
                    }],
                }],
                store_operations: Vec::new(),
            })),
        ),
        (
            "snapshot",
            json(to_value(Snapshotted {
                schema_version: 1,
                batch: BATCH.to_owned(),
                drafts: 3,
                inputs: 1,
                empty: false,
                store_operations: vec![StoreOperation {
                    op: "find".to_owned(),
                    source: "drafts".to_owned(),
                    destination: false,
                    items: 3,
                    searches: Some(1),
                }],
            })),
        ),
        (
            "ticket validate",
            json(to_value(TicketValidated {
                schema_version: 1,
                batch: BATCH.to_owned(),
                ticket: "layout-save-reads-size-twice".to_owned(),
                sound: false,
                problems: vec![
                    "the ticket records the field `client`, which `fields` does not declare"
                        .to_owned(),
                ],
                absent_targets: Vec::new(),
                store_operations: Vec::new(),
            })),
        ),
        (
            "ticket board-status",
            json(to_value(BoardStatus {
                schema_version: 1,
                batch: BATCH.to_owned(),
                item: "followups:item-42".to_owned(),
                title: "widgets: a layout save reads one window size".to_owned(),
                status: status(),
                priority: HeldPriority::High,
                version: Some("2026-11-01T11:59:58Z".to_owned()),
                url: Some("https://github.com/example/widgets/issues/42".to_owned()),
                comments: vec![BoardComment {
                    id: "IC_kwDOA1001".to_owned(),
                    author: Some("alice".to_owned()),
                    url: Some(
                        "https://github.com/example/widgets/issues/42#issuecomment-1001".to_owned(),
                    ),
                    last_changed: "2026-11-01T11:00:00Z".to_owned(),
                    body: "Seen again after a resize.".to_owned(),
                }],
                status_role: None,
                priority_estimate: None,
                estimate_line: None,
            })),
        ),
        (
            "ticket search",
            json(to_value(SearchResults {
                schema_version: 1,
                batch: BATCH.to_owned(),
                ticket: "layout-save-reads-size-twice".to_owned(),
                query: "layout save window size".to_owned(),
                hits: vec![SearchHit {
                    item: "followups:item-42".to_owned(),
                    title: "widgets: a layout save reads one window size".to_owned(),
                    status: status(),
                    url: Some("https://github.com/example/widgets/issues/42".to_owned()),
                }],
                store_operations: Vec::new(),
            })),
        ),
        (
            "ticket copy",
            staged(
                StagedWrite::Copy {
                    ticket: ticket.clone(),
                    destination: "followups".to_owned(),
                },
                0,
            ),
        ),
        (
            "ticket comment",
            staged(
                StagedWrite::Comment {
                    item: "followups:item-13".to_owned(),
                    root_cause: "export-dialog-drops-unicode".to_owned(),
                    body: format!(
                        "Additional evidence from follow-up run `{BATCH}`.\n\nSeen again.\n\n<!-- orchestrator:follow-up-comment run=\"{BATCH}\" root_cause=\"export-dialog-drops-unicode\" -->\n"
                    ),
                },
                1,
            ),
        ),
        (
            "ticket re-estimate",
            staged(
                StagedWrite::ReEstimate {
                    item: "followups:item-13".to_owned(),
                    priority_estimate: Level::High,
                },
                2,
            ),
        ),
        (
            "ticket relate",
            staged(
                StagedWrite::Relate {
                    relation: "of-error".to_owned(),
                    from: format!("drafts:{BATCH}/tickets/client-acme-e1042"),
                    to: "followups:item-7".to_owned(),
                },
                3,
            ),
        ),
        (
            "check",
            json(to_value(Checked {
                schema_version: 1,
                batch: BATCH.to_owned(),
                sound: false,
                problems: vec![Problem {
                    subject: format!("drafts:{BATCH}/drafts/20261101T102500Z-flaky-resize-test"),
                    message: "the input has no disposition; give it exactly one".to_owned(),
                }],
            })),
        ),
        (
            "publish",
            json(to_value(Published {
                schema_version: 1,
                batch: BATCH.to_owned(),
                receipts: PathBuf::from(format!("receipts/{BATCH}.jsonl")),
                outcomes: vec![PublishOutcome {
                    index: 0,
                    op: StagedOp::Copy,
                    item: Some("followups:item-42".to_owned()),
                    outcome: "created".to_owned(),
                    error: None,
                }],
                store_operations: Vec::new(),
            })),
        ),
        (
            "settle",
            json(to_value(Settled {
                schema_version: 1,
                batch: BATCH.to_owned(),
                read_back: 1,
                confirmed: 1,
                hook: Some(HookRun {
                    argv: vec!["notify".to_owned(), "--batch-settled".to_owned()],
                    exit: 0,
                }),
                retired: vec![PathBuf::from(format!("outbox/{BATCH}.json"))],
                pending: 0,
                needs_attention: false,
                store_operations: Vec::new(),
            })),
        ),
        (
            "comments gather",
            json(to_value(Gathered {
                schema_version: 1,
                snapshot: Some(snapshot.clone()),
                dry_run: false,
                items: 3,
                selected: IndexMap::from([(BATCH.to_owned(), 2)]),
                left: 1,
                launch_command: None,
                store_operations: Vec::new(),
            })),
        ),
        (
            "comments launches",
            json(to_value(Launches {
                schema_version: 1,
                snapshot: snapshot.clone(),
                launches: vec![Launch {
                    batch: BATCH.to_owned(),
                    feedback: feedback.clone(),
                }],
            })),
        ),
        (
            "comments answer",
            json(to_value(CommentsAnswered::Pass(AnsweredPass {
                schema_version: 1,
                snapshot: snapshot.clone(),
                launches: vec![AnsweredLaunch {
                    batch: BATCH.to_owned(),
                    feedback: feedback.clone(),
                    exit: Some(0),
                    error: None,
                }],
                failed: Vec::new(),
                settled: Some(CommentsSettled {
                    schema_version: 1,
                    snapshot: snapshot.clone(),
                    settled: vec![SettledLaunch {
                        batch: BATCH.to_owned(),
                        feedback: feedback.clone(),
                        writes: 2,
                        pending: 0,
                        verified: 2,
                        outstanding: 0,
                        problems: Vec::new(),
                    }],
                    pending: 0,
                    needs_attention: false,
                    watermark: Some("2026-11-01T12:00:00Z".to_owned()),
                    store_operations: Vec::new(),
                }),
                store_operations: Vec::new(),
            }))),
        ),
        (
            "comments check",
            json(to_value(CommentsChecked {
                schema_version: 1,
                feedback: feedback.clone(),
                sound: true,
                problems: vec![],
            })),
        ),
        (
            "comments settle",
            json(to_value(CommentsSettled {
                schema_version: 1,
                snapshot,
                settled: vec![SettledLaunch {
                    batch: BATCH.to_owned(),
                    feedback,
                    writes: 2,
                    pending: 0,
                    verified: 2,
                    outstanding: 0,
                    problems: Vec::new(),
                }],
                pending: 0,
                needs_attention: false,
                watermark: Some("2026-11-01T12:00:00Z".to_owned()),
                store_operations: Vec::new(),
            })),
        ),
        (
            "groom prepare",
            json(to_value(GroomPrepared {
                schema_version: 1,
                batch: GROOM.to_owned(),
                destination: "followups".to_owned(),
                selected: 2,
                unchanged: 47,
                protected: 1,
                shards: vec![GroomShard {
                    shard: format!("{GROOM}-1"),
                    task: PathBuf::from(format!(
                        "/work/.onefollowup/groom/{GROOM}/{GROOM}-1/task.md"
                    )),
                    verdicts: PathBuf::from(format!(
                        "/work/.onefollowup/groom/{GROOM}/{GROOM}-1/verdicts.jsonl"
                    )),
                    tickets: 2,
                }],
                store_operations: ["list", "comments-since", "read"]
                    .into_iter()
                    .zip([49, 3, 2])
                    .map(|(op, items)| StoreOperation {
                        op: op.to_owned(),
                        source: "followups".to_owned(),
                        destination: true,
                        items,
                        searches: None,
                    })
                    .collect(),
            })),
        ),
        (
            "groom check",
            json(to_value(GroomChecked {
                batch: GROOM.to_owned(),
                problems: vec![GroomProblem {
                    ticket: Some("followups:item-7".to_owned()),
                    rule: "low-confidence-close".to_owned(),
                    detail: format!(
                        "{GROOM}-1 line 2: a `fixed` verdict closes the ticket, which `low` confidence never does: raise the evidence to `medium` or `high`, or answer `keep` or `unresolved`"
                    ),
                }],
                plan: GroomPlan {
                    fixed: Vec::new(),
                    obsolete: Vec::new(),
                    duplicates: vec![groomed("followups:item-9", Some("followups:item-3"), None)],
                    survivors: vec![Survivor {
                        ticket: "followups:item-3".to_owned(),
                        members: vec!["followups:item-3".to_owned(), "followups:item-9".to_owned()],
                        chosen_by: "claimed".to_owned(),
                    }],
                    kept: vec![groomed(
                        "followups:item-3",
                        None,
                        Some("it is claimed (Queued or In Progress), which grooming never closes"),
                    )],
                    unresolved: Vec::new(),
                },
            })),
        ),
        (
            "statuses",
            json(to_value(StatusesShown {
                schema_version: 1,
                destination: "followups".to_owned(),
                roles: vec![RoleStatus {
                    role: StatusRole::Proposal,
                    category: "backlog".to_owned(),
                    destination_names: vec!["Proposal".to_owned()],
                }],
            })),
        ),
    ])
}

/// The example under each `#### \`<verb>\`` heading of the README's "Results" section.
fn documented() -> BTreeMap<String, Value> {
    let start = README
        .find("\n### Results\n")
        .expect("README.md has a Results section");
    let section = &README[start + 1..];
    let section = &section[..section[1..]
        .find("\n## ")
        .map_or(section.len(), |end| end + 1)];
    let mut found = BTreeMap::new();
    for part in section.split("\n#### `").skip(1) {
        let verb = part.split('`').next().expect("a heading").to_owned();
        let block = part
            .split("```json\n")
            .nth(1)
            .and_then(|rest| rest.split("\n```").next());
        let block = block.unwrap_or_else(|| panic!("`{verb}` has a ```json example"));
        let value = serde_json::from_str(block)
            .unwrap_or_else(|error| panic!("`{verb}`'s example: {error}"));
        assert!(
            found.insert(verb.clone(), value).is_none(),
            "`{verb}` is documented twice"
        );
    }
    found
}

fn verbs(command: &clap::Command, path: &mut Vec<String>, into: &mut Vec<String>) {
    let subcommands: Vec<&clap::Command> = command
        .get_subcommands()
        .filter(|one| one.get_name() != "help")
        .collect();
    if subcommands.is_empty() {
        into.push(path.join(" "));
    }
    for subcommand in subcommands {
        path.push(subcommand.get_name().to_owned());
        verbs(subcommand, path, into);
        path.pop();
    }
}

#[test]
fn the_contract_document_states_every_verbs_result_as_the_library_types_it() {
    let mut defined = Vec::new();
    verbs(&Cli::command(), &mut Vec::new(), &mut defined);
    let mut expected: Vec<String> = defined
        .into_iter()
        .filter(|verb| verb != "kinds" && verb != "config show")
        .collect();
    expected.sort();
    let examples = examples();
    assert_eq!(
        examples
            .keys()
            .map(|one| (*one).to_owned())
            .collect::<Vec<_>>(),
        expected
    );

    let documented = documented();
    assert_eq!(
        documented.keys().cloned().collect::<Vec<_>>(),
        expected,
        "README.md documents every verb's result once"
    );
    for (verb, example) in &examples {
        assert_eq!(
            &documented[*verb],
            example,
            "README.md's `{verb}` result drifted from the library's type; it reads:\n{}",
            serde_json::to_string_pretty(example).expect("JSON")
        );
    }
}
