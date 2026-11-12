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
    BoardComment, BoardStatus, Checked, CommentsChecked, CommentsSettled, Drafted, Gathered,
    HookRun, ItemStatus, Launch, Launches, Prepared, Problem, PublishOutcome, Published,
    RoleStatus, SearchHit, SearchResults, Settled, SettledLaunch, Staged, StagedOp, StatusesShown,
    TicketValidated,
};
use onefollowup::cli::Cli;
use onefollowup::config::StatusRole;
use onefollowup::formats::outbox::StagedWrite;
use onefollowup::formats::{HeldPriority, Level};
use serde_json::{Value, to_value};

const README: &str = include_str!("../../../README.md");
const BATCH: &str = "nightly-2026-11-01";

fn status() -> Option<ItemStatus> {
    Some(ItemStatus {
        category: "backlog".to_owned(),
        name: "Proposal".to_owned(),
    })
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
                store_operations: Vec::new(),
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
                }],
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
