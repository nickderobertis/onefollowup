//! Every record format against records as they stand on boards and disks today.
//!
//! The fixtures under `tests/fixtures/` were written once by the Python writers of the
//! repository these formats are adopted from (at its commit `cf73059f`, with synthetic
//! values), and committed: nothing here reads or runs anything of that repository. Each is
//! read and re-rendered byte for byte; a record one schema above — and, where one exists,
//! one below — the version read is refused naming the format, the schema found and the
//! schema read. The gathering (`gathering/`) and the outbox are onefollowup's own formats,
//! with no earlier writer; their fixtures were written for these tests.

use onefollowup::formats::batch::BatchState;
use onefollowup::formats::dispositions::{CarriedDispositions, Dispositions};
use onefollowup::formats::draft::{DraftRecord, DraftsProjectRecord};
use onefollowup::formats::existing_tickets::ExistingTickets;
use onefollowup::formats::gathering::Gathering;
use onefollowup::formats::markers::{
    self, CommentMarker, FeedbackBoundary, FeedbackSnapshot, IssueLine, QuotedAnchor, ReplyTo,
    evidence_opening, reply_opening,
};
use onefollowup::formats::outbox::{Outbox, StagedWrite};
use onefollowup::formats::pending::Pending;
use onefollowup::formats::receipts::{
    AssignAction, AssignOutcome, EdgeAction, EdgeStore, Outcome, ReceiptJournal, RelateOutcome,
};
use onefollowup::formats::responses::Responses;
use onefollowup::formats::snapshot::{Launched, Snapshot};
use onefollowup::formats::ticket::TicketRecord;
use onefollowup::formats::watermark::Watermark;
use onefollowup::formats::{FormatError, Level, Verdict, Versioned};
use onefollowup::verify::render::{self, Common};
use onefollowup::verify::template::Located;
use serde_json::{Value, json};

const NAMESPACE: &str = "orchestrator";

fn fixture(path: &str) -> String {
    let full = format!("{}/tests/fixtures/{path}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&full).unwrap_or_else(|error| panic!("{full}: {error}"))
}

fn round_trips<T: Versioned + std::fmt::Debug>(path: &str) -> T {
    let text = fixture(path);
    let parsed = T::parse(&text).unwrap_or_else(|error| panic!("{path}: {error}"));
    assert_eq!(parsed.render(), text, "{path} re-rendered");
    parsed
}

fn refused<T: Versioned + std::fmt::Debug>(path: &str, format: &str, found: &str, reads: u64) {
    let error = T::parse(&fixture(path)).expect_err(path);
    assert_eq!(
        error,
        FormatError::Schema {
            format: leak(format),
            found: found.to_owned(),
            reads
        },
        "{path}"
    );
    let message = error.to_string();
    for part in [
        format,
        &format!("schema {found}"),
        &format!("reads schema {reads}"),
    ] {
        assert!(message.contains(part), "{message} names {part}");
    }
}

/// A comment rendered through a shipped comment template under the shipped configuration.
fn shipped(template: &str, comment: &Value, marker: &CommentMarker) -> String {
    let config = onefollowup::config::defaults("build-host-1");
    render::comment(
        &Common {
            config: &config,
            kind: &config.kinds[0],
            batch: &marker.batch,
            destination: "followups",
        },
        &Located::Shipped(template.to_owned()),
        comment,
        marker,
    )
    .expect("the shipped template renders")
}

fn leak(text: &str) -> &'static str {
    Box::leak(text.to_owned().into_boxed_str())
}

#[test]
fn ticket_records_round_trip() {
    let minimal: TicketRecord = round_trips("ticket-record/record-minimal.json");
    assert_eq!(minimal.priority_estimate, Level::Low);
    assert_eq!(
        (minimal.kind.as_ref(), minimal.fields.as_ref()),
        (None, None)
    );
    let full: TicketRecord = round_trips("ticket-record/record-full.json");
    assert_eq!(
        full.budget
            .as_ref()
            .and_then(|budget| budget.threshold.as_f64()),
        Some(300.0)
    );
    round_trips::<TicketRecord>("ticket-record/record-budget-fractional-threshold.json");

    let extended: TicketRecord = round_trips("ticket-record/record-extended.json");
    assert_eq!(extended.kind.as_deref(), Some("client-error"));
    assert_eq!(extended.board_assignee.as_deref(), Some("alice"));
    assert_eq!(extended.edges.as_ref().map(Vec::len), Some(1));
    assert_eq!(
        extended
            .fields
            .as_ref()
            .and_then(|fields| fields.get("client"))
            .map(String::as_str),
        Some("acme — west")
    );

    // The same record without the four keys onefollowup adds is today's record, byte for byte.
    let plain = TicketRecord {
        kind: None,
        fields: None,
        edges: None,
        board_assignee: None,
        ..extended
    };
    assert_eq!(plain.render(), fixture("ticket-record/record-full.json"));

    // The record at ai-orchestrator 8bf72a30's `tests/fixtures/cycle-time/ticket-v15.json`, its
    // `schema` set to 14 and spelled as a record is stored (one line, as `json.dumps` writes it):
    // its `draft_sources`, `transcripts` and `witnesses` read in the schema-15 order, and it
    // re-renders byte for byte, its integer threshold `123` still `123`.
    let board: TicketRecord = round_trips("ticket-record/record-board-fields.json");
    assert_eq!(
        board
            .budget
            .as_ref()
            .map(|budget| budget.threshold.to_string()),
        Some("123".to_owned())
    );
    let draft = "drafts:listing-run/drafts/20260101T000000Z-cursor";
    assert_eq!(
        board
            .draft_sources
            .as_ref()
            .and_then(|sources| sources.get(draft))
            .map(String::as_str),
        Some("author node; node verify-cursor")
    );
    assert_eq!(
        board
            .transcripts
            .as_ref()
            .and_then(|transcripts| transcripts.get(draft))
            .map(String::as_str),
        Some("onepipeline transcript listing-run verify-cursor")
    );
    assert_eq!(board.witnesses.as_ref().map(Vec::len), Some(1));
    let keys: Vec<String> = serde_json::from_str::<serde_json::Map<String, Value>>(&board.render())
        .expect("an object")
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        keys,
        [
            "schema",
            "root_cause",
            "repository",
            "created_by_run",
            "owning_runs",
            "drafts",
            "draft_sources",
            "transcripts",
            "basis",
            "witnesses",
            "verified_at",
            "host",
            "priority_estimate",
            "frequency",
            "budget",
            "board_estimate",
        ],
        "the schema-15 records' key order"
    );

    // `impact` and `projected` follow the keys before them, each omitted when absent.
    let mut impact = indexmap::IndexMap::new();
    impact.insert("Severity".to_owned(), "high".to_owned());
    let mut projected = indexmap::IndexMap::new();
    projected.insert("root_cause".to_owned(), "A cursor skips.".to_owned());
    let more = TicketRecord {
        impact: Some(impact),
        projected: Some(projected),
        ..board
    };
    let text = more.render();
    assert!(
        text.ends_with(r#""board_estimate": "medium", "impact": {"Severity": "high"}, "projected": {"root_cause": "A cursor skips."}}"#),
        "{text}"
    );
    assert_eq!(TicketRecord::parse(&text), Ok(more));
}

#[test]
fn ticket_records_of_another_schema_are_refused() {
    refused::<TicketRecord>(
        "ticket-record/record-minimal-schema13.json",
        "ticket record",
        "13",
        14,
    );
    refused::<TicketRecord>(
        "ticket-record/record-board-fields-schema15.json",
        "ticket record",
        "15",
        14,
    );
    refused::<TicketRecord>(
        "ticket-record/record-minimal-schema15.json",
        "ticket record",
        "15",
        14,
    );
    let text = fixture("ticket-record/record-minimal.json");
    for (replaced, found) in [
        ("\"schema\": 14", "true"),
        ("\"schema\": 14", "14.0"),
        ("\"schema\": 14", "\"14\""),
    ] {
        let error =
            TicketRecord::parse(&text.replacen(replaced, &format!("\"schema\": {found}"), 1))
                .expect_err(found);
        assert!(
            matches!(error, FormatError::Schema { .. }),
            "{found}: {error}"
        );
    }
    let unknown = text.replacen("\"host\"", "\"run\": \"x\", \"host\"", 1);
    assert!(matches!(
        TicketRecord::parse(&unknown),
        Err(FormatError::Malformed { .. })
    ));
    let unschemed = text.replacen("\"schema\": 14, ", "", 1);
    assert!(
        TicketRecord::parse(&unschemed)
            .expect_err("no schema")
            .to_string()
            .contains("states no `schema`")
    );
    assert!(matches!(
        TicketRecord::parse("[]"),
        Err(FormatError::Malformed { .. })
    ));
    assert!(matches!(
        TicketRecord::parse("{"),
        Err(FormatError::Malformed { .. })
    ));
}

#[test]
fn draft_records_round_trip() {
    for variant in ["node", "monitor-engine-run", "manager", "node-unresolved"] {
        round_trips::<DraftRecord>(&format!("draft/record-{variant}.json"));
    }
    refused::<DraftRecord>("draft/record-node-schema2.json", "draft record", "2", 1);
    let project = DraftsProjectRecord {
        batch: "nightly-2026-11-01".to_owned(),
        run_id: None,
    };
    assert_eq!(
        project.render(),
        r#"{"schema": 1, "run": "nightly-2026-11-01"}"#
    );
    assert_eq!(DraftsProjectRecord::parse(&project.render()), Ok(project));
}

#[test]
fn comment_markers_round_trip() {
    for (path, reply) in [
        ("evidence-marker.txt", None),
        ("reply-marker-confirms.txt", Some(Some(Verdict::Confirms))),
        (
            "reply-marker-does-not-confirm.txt",
            Some(Some(Verdict::DoesNotConfirm)),
        ),
        ("reply-marker-legacy-no-verdict.txt", Some(None)),
    ] {
        let line = fixture(&format!("comment-markers/{path}"));
        let marker = CommentMarker::parse(&line, NAMESPACE)
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        assert_eq!(marker.reply.as_ref().map(|to| to.verdict), reply, "{path}");
        assert_eq!(marker.render(NAMESPACE), line, "{path}");
    }
    let marker = CommentMarker::parse(&fixture("comment-markers/evidence-marker.txt"), NAMESPACE)
        .expect("read");
    assert_eq!(
        marker.render("acme"),
        r#"<!-- acme:follow-up-comment run="nightly-2026-11-01" root_cause="layout-save-reads-size-twice" -->"#
    );
    for line in [
        "<!-- orchestrator:follow-up-comment run=\"a\" -->",
        "<!-- orchestrator:follow-up-comment run=\"a\" root_cause=\"b\" kind=\"evidence\" answers=\"c\" -->",
        "<!-- orchestrator:follow-up-comment run=\"a\" root_cause=\"b\" kind=\"reply\" answers=\"c d\" -->",
        "<!-- orchestrator:follow-up-comment run=\"a\" root_cause=\"b\" kind=\"reply\" answers=\"c\" verdict=\"maybe\" -->",
    ] {
        assert!(CommentMarker::parse(line, NAMESPACE).is_err(), "{line}");
    }
    assert!(CommentMarker::parse(&fixture("comment-markers/evidence-marker.txt"), "acme").is_err());

    let batch = "nightly-2026-11-01";
    assert_eq!(
        evidence_opening(batch),
        fixture("comment-markers/evidence-opening.txt")
    );
    let url = "https://github.com/example/widgets/issues/12#issuecomment-1001";
    assert_eq!(
        reply_opening(batch, url, Some("alice")),
        fixture("comment-markers/reply-opening-attributed.txt")
    );
    assert_eq!(
        reply_opening(batch, url, None),
        fixture("comment-markers/reply-opening-unattributed.txt")
    );

    // The shipped comment templates write 0.5.0's comments byte for byte.
    let evidence_text = fixture("comment-markers/evidence-comment.md");
    let root_cause = "layout-save-reads-size-twice";
    assert_eq!(
        shipped(
            "comment-evidence.md.j2",
            &json!({"batch": batch, "root_cause": root_cause,
                    "evidence": "Seen again — a resize during save on build-host-1.\n",
                    "bears_on": null, "question": null, "sources": null}),
            &CommentMarker {
                batch: batch.to_owned(),
                root_cause: root_cause.to_owned(),
                reply: None
            },
        ),
        evidence_text
    );
    for (path, author, answers, verdict, response) in [
        (
            "reply-attributed.md",
            Some("alice"),
            "IC_kwDOA1001",
            Verdict::Confirms,
            "The ticket now names the resize — thanks.",
        ),
        (
            "reply-unattributed.md",
            None,
            "IC_kwDOA1002",
            Verdict::DoesNotConfirm,
            "This is a different cause; the ticket stands.",
        ),
    ] {
        let to = ReplyTo {
            answers: answers.to_owned(),
            verdict: Some(verdict),
        };
        assert_eq!(
            shipped(
                "comment-reply.md.j2",
                &json!({"batch": batch, "root_cause": root_cause, "answers": answers,
                        "verdict": verdict.as_str(), "url": url, "author": author,
                        "response": response}),
                &CommentMarker {
                    batch: batch.to_owned(),
                    root_cause: root_cause.to_owned(),
                    reply: Some(to.clone())
                },
            ),
            fixture(&format!("comment-markers/{path}")),
            "{path}"
        );
        let owner = markers::owner(&fixture(&format!("comment-markers/{path}")), NAMESPACE)
            .expect("a reply is owned by its marker");
        assert_eq!(owner.reply, Some(to), "{path}");
    }

    // A comment is owned by its marker alone, whatever comes before it.
    let owner = markers::owner(&evidence_text, NAMESPACE).expect("owned");
    assert_eq!((owner.batch.as_str(), owner.reply), (batch, None));
    let reopened = evidence_text.replacen("run `nightly-2026-11-01`", "run `someone-else`", 1);
    assert_eq!(
        markers::owner(&reopened, NAMESPACE).map(|owner| owner.batch),
        Some(batch.to_owned()),
        "the opening line names nothing the marker does not"
    );
    let blocked = format!(
        "<details>\n<summary>Additional evidence from follow-up run {batch}.</summary>\n\n- Host: build-host-1\n\n</details>\n\nSeen again.\n\n{}\n",
        CommentMarker {
            batch: batch.to_owned(),
            root_cause: root_cause.to_owned(),
            reply: None
        }
        .render(NAMESPACE)
    );
    assert_eq!(
        markers::owner(&blocked, NAMESPACE).map(|owner| owner.root_cause),
        Some(root_cause.to_owned())
    );
    for unowned in [
        "no newline",
        "one line\n",
        "a\n\nb\n",
        &evidence_text.replace(NAMESPACE, "acme"),
    ] {
        assert!(markers::owner(unowned, NAMESPACE).is_none(), "{unowned}");
    }

    let anchor_line = fixture("comment-markers/quoted-comment-anchor.txt");
    let anchor = QuotedAnchor::parse(&anchor_line, NAMESPACE).expect("an anchor");
    assert_eq!(anchor.render(NAMESPACE), anchor_line);
    assert!(
        QuotedAnchor::parse(
            "<!-- orchestrator:follow-up-quoted issue=\"a b\" comment=\"c\" -->",
            NAMESPACE
        )
        .is_err()
    );
    assert!(
        QuotedAnchor::parse(
            "<!-- orchestrator:follow-up-quoted issue=\"a\" -->",
            NAMESPACE
        )
        .is_err()
    );
    for path in ["feedback-boundary-time.txt", "feedback-boundary-none.txt"] {
        let line = fixture(&format!("comment-markers/{path}"));
        assert_eq!(
            FeedbackBoundary::parse(&line, NAMESPACE)
                .expect(path)
                .render(NAMESPACE),
            line
        );
    }
    assert_eq!(
        FeedbackBoundary::parse(
            &fixture("comment-markers/feedback-boundary-none.txt"),
            NAMESPACE
        ),
        Ok(FeedbackBoundary(None))
    );
    assert!(FeedbackBoundary::parse(&anchor_line, NAMESPACE).is_err());
    let snapshot_line = fixture("comment-markers/feedback-snapshot.txt");
    assert_eq!(
        FeedbackSnapshot::parse(&snapshot_line, NAMESPACE)
            .expect("an anchor")
            .render(NAMESPACE),
        snapshot_line
    );
    assert!(FeedbackSnapshot::parse(&anchor_line, NAMESPACE).is_err());
    for path in ["issue-line-version.txt", "issue-line-none.txt"] {
        let line = fixture(&format!("comment-markers/{path}"));
        assert_eq!(IssueLine::parse(&line).expect(path).render(), line);
    }
    assert!(IssueLine::parse("followups:item-42 at version none").is_err());
}

#[test]
fn the_receipt_journal_round_trips() {
    let text = fixture("receipts/journal-schema3.jsonl");
    let journal = ReceiptJournal::parse(&text).expect("a schema-3 journal");
    assert_eq!(journal.receipts.len(), 11);
    assert!(
        journal
            .receipts
            .iter()
            .any(|receipt| matches!(receipt.outcome, Outcome::ReEstimate(_)))
    );
    assert!(
        journal
            .receipts
            .iter()
            .any(|receipt| matches!(receipt.outcome, Outcome::Comment(_)))
    );
    assert_eq!(journal.render(), text);
}

#[test]
fn an_edges_and_an_assignees_receipt_lines_round_trip_within_schema_3() {
    let text = fixture("receipts/journal-relations-schema3.jsonl");
    let journal = ReceiptJournal::parse(&text).expect("a schema-3 journal");
    let edges: Vec<&RelateOutcome> = journal
        .receipts
        .iter()
        .filter_map(|receipt| match &receipt.outcome {
            Outcome::Relate(edge) => Some(edge),
            _ => None,
        })
        .collect();
    assert_eq!(
        edges.iter().map(|edge| edge.action).collect::<Vec<_>>(),
        [EdgeAction::Written, EdgeAction::Exists, EdgeAction::Failed]
    );
    assert!(edges[0].derived && edges[0].store == EdgeStore::Parent);
    assert!(
        journal.receipts[2].card.is_none(),
        "an unwritten edge names no card"
    );
    let assigned: Vec<&AssignOutcome> = journal
        .receipts
        .iter()
        .filter_map(|receipt| match &receipt.outcome {
            Outcome::Assign(assign) => Some(assign),
            _ => None,
        })
        .collect();
    assert_eq!(assigned[0].action, AssignAction::Written);
    assert_eq!(assigned[0].when, ["alice"]);
    assert_eq!(assigned[1].action, AssignAction::Held);
    assert!(
        assigned[1].when.is_empty(),
        "never assigned: held to nobody"
    );
    assert_eq!(
        assigned[1].error.as_ref().map(|error| error.kind.as_str()),
        Some("assignee-held")
    );
    assert_eq!(journal.render(), text);
    let assign = text.lines().nth(3).expect("an assign line");
    assert!(
        matches!(
            ReceiptJournal::parse(&assign.replacen(
                "\"card\": \"bugs:item-3\"",
                "\"card\": null",
                1
            )),
            Err(FormatError::Malformed { .. })
        ),
        "an assignee's receipt names its card"
    );
}

#[test]
fn a_receipt_journal_with_a_line_of_another_schema_is_refused() {
    for (path, found) in [
        (
            "receipts/journal-with-schema2-copy-line.jsonl",
            "2 (line 3)",
        ),
        ("receipts/journal-with-schema4-line.jsonl", "4 (line 3)"),
    ] {
        let error = ReceiptJournal::parse(&fixture(path)).expect_err(path);
        assert_eq!(
            error,
            FormatError::Schema {
                format: "receipt journal",
                found: found.to_owned(),
                reads: 3
            },
            "{path}"
        );
    }
    let line = fixture("receipts/journal-schema3.jsonl")
        .lines()
        .next()
        .expect("a line")
        .to_owned();
    for broken in [
        "{".to_owned(),
        line.replacen("\"command\": \"copy\"", "\"command\": \"delete\"", 1),
        line.replacen("\"action\": \"created\"", "\"action\": \"exploded\"", 1),
        line.replacen("\"command\": \"copy\"", "\"command\": \"comment\"", 1),
        line.replacen("\"card\": \"followups:item-42\"", "\"card\": null", 1)
            .replacen("\"command\": \"copy\"", "\"command\": \"re-estimate\"", 1),
    ] {
        assert!(
            matches!(
                ReceiptJournal::parse(&broken),
                Err(FormatError::Malformed { .. })
            ),
            "{broken}"
        );
    }
}

/// A schema-1 record, read as schema 2 and written as it: byte for byte the same but for the
/// schema it states, since schema 2 only adds an optional member.
fn reads_schema_one<T: Versioned + std::fmt::Debug>(path: &str) -> T {
    let text = fixture(path);
    assert!(text.contains("\"schema\": 1,"), "{path} is schema 1");
    let parsed = T::parse(&text).unwrap_or_else(|error| panic!("{path}: {error}"));
    assert_eq!(
        parsed.render(),
        text.replacen("\"schema\": 1,", "\"schema\": 2,", 1),
        "{path} re-rendered as schema 2"
    );
    parsed
}

#[test]
fn dispositions_round_trip_with_dismissals_and_still_read_schema_one() {
    let plain: Dispositions = reads_schema_one("dispositions/dispositions.json");
    assert!(plain.dispositions.iter().all(|entry| entry.kind.is_none()));
    let kinded: Dispositions = reads_schema_one("dispositions/dispositions-with-kind.json");
    assert_eq!(kinded.dispositions[1].kind.as_deref(), Some("error"));
    assert!(
        kinded
            .dispositions
            .iter()
            .all(|entry| entry.dismissed.is_empty())
    );
    reads_schema_one::<Dispositions>("dispositions/skeleton-first-open.json");
    let dismissing: Dispositions = round_trips("dispositions/dispositions-with-dismissals.json");
    assert_eq!(
        dismissing.dispositions[0].dismissed[0].item,
        "followups:item-17"
    );
    assert!(dismissing.dispositions[1].dismissed.is_empty());
    let carried: CarriedDispositions = reads_schema_one("dispositions/dispositions.carried.json");
    assert!(
        carried
            .dispositions
            .iter()
            .all(|entry| entry.kind.is_none())
    );
    let carried: CarriedDispositions =
        reads_schema_one("dispositions/dispositions.carried-with-kind.json");
    assert!(
        carried
            .dispositions
            .iter()
            .all(|entry| entry.kind.is_some())
    );
    let carried: CarriedDispositions =
        round_trips("dispositions/dispositions.carried-with-dismissals.json");
    assert_eq!(carried.dispositions[0].dismissed.len(), 1);
    refused::<Dispositions>(
        "dispositions/dispositions-schema3.json",
        "dispositions",
        "3",
        2,
    );
    refused::<CarriedDispositions>(
        "dispositions/dispositions.carried-schema3.json",
        "carried dispositions",
        "3",
        2,
    );
}

#[test]
fn responses_round_trip() {
    round_trips::<Responses>("responses/20261101T120000Z.responses.json");
    refused::<Responses>(
        "responses/20261101T120000Z.responses-schema2.json",
        "responses",
        "2",
        3,
    );
    refused::<Responses>(
        "responses/20261101T120000Z.responses-schema4.json",
        "responses",
        "4",
        3,
    );
}

#[test]
fn snapshots_and_their_launch_round_trip() {
    round_trips::<Snapshot>("snapshot/snapshot.json");
    refused::<Snapshot>("snapshot/snapshot-schema1.json", "snapshot", "1", 2);
    refused::<Snapshot>("snapshot/snapshot-schema3.json", "snapshot", "3", 2);
    round_trips::<Launched>("snapshot/launched.json");
    let gathering = round_trips::<Gathering>("gathering/gathering.json");
    assert_eq!(gathering.listings["followups"].comments.len(), 2);
    assert_eq!(gathering.last_written[0].item, "followups:item-42");
    refused::<Gathering>("gathering/gathering-schema2.json", "gathering", "2", 1);
    refused::<Launched>("snapshot/launched-schema2.json", "launched record", "2", 1);
}

#[test]
fn watermarks_pending_writes_and_existing_tickets_round_trip() {
    round_trips::<Watermark>("watermark/watermark-followups.json");
    refused::<Watermark>(
        "watermark/watermark-followups-schema2.json",
        "watermark",
        "2",
        1,
    );
    round_trips::<Pending>("pending/pending.json");
    refused::<Pending>("pending/pending-schema2.json", "pending writes", "2", 1);
    round_trips::<ExistingTickets>("existing-tickets/existing-tickets.json");
    round_trips::<ExistingTickets>("existing-tickets/existing-tickets-empty.json");
    refused::<ExistingTickets>(
        "existing-tickets/existing-tickets-schema2.json",
        "existing tickets",
        "2",
        1,
    );
}

#[test]
fn an_outbox_written_by_the_library_round_trips() {
    let batch = "nightly-2026-11-01";
    let marker = CommentMarker {
        batch: batch.to_owned(),
        root_cause: "export-dialog-drops-unicode".to_owned(),
        reply: None,
    };
    let body = shipped(
        "comment-evidence.md.j2",
        &json!({"batch": batch, "root_cause": marker.root_cause,
                "evidence": "Seen again — the title café lost its accent.",
                "bears_on": null, "question": null, "sources": null}),
        &marker,
    );
    let outbox = Outbox {
        batch: batch.to_owned(),
        writes: vec![
            StagedWrite::Copy {
                ticket: format!("drafts:{batch}/tickets/layout-save-reads-size-twice"),
                destination: "followups".to_owned(),
            },
            StagedWrite::Comment {
                item: "followups:item-13".to_owned(),
                root_cause: marker.root_cause.clone(),
                body,
            },
            StagedWrite::ReEstimate {
                item: "followups:item-13".to_owned(),
                priority_estimate: Level::High,
            },
            StagedWrite::Relate {
                relation: "of-error".to_owned(),
                from: format!("drafts:{batch}/tickets/client-acme-e1042"),
                to: "followups:item-7".to_owned(),
            },
        ],
    };
    let written = fixture("outbox/outbox.json");
    assert_eq!(outbox.render(), written);
    assert_eq!(Outbox::parse(&written), Ok(outbox));
    refused::<Outbox>("outbox/outbox-schema2.json", "outbox", "2", 1);
    let below = written.replacen("\"schema\": 1,", "\"schema\": 0,", 1);
    assert!(matches!(
        Outbox::parse(&below),
        Err(FormatError::Schema { .. })
    ));
    let unknown_op = written.replacen("\"op\": \"copy\"", "\"op\": \"delete\"", 1);
    assert!(matches!(
        Outbox::parse(&unknown_op),
        Err(FormatError::Malformed { .. })
    ));
}

#[test]
fn a_batch_state_written_before_relations_still_reads_and_one_carrying_them_round_trips() {
    // Written by this crate at the base of the relations change, carrying no `relations`.
    let before = fixture("batch-state/state-before-relations.json");
    let state = BatchState::parse(&before).expect("a schema-1 state from before relations");
    assert!(state.relations.is_empty());
    assert_eq!(
        state.render(),
        before,
        "a state with no relations writes as it always did"
    );
    let with = fixture("batch-state/state-with-relations.json");
    let state = BatchState::parse(&with).expect("a schema-1 state carrying relations");
    assert_eq!(state.relations.len(), 1);
    assert_eq!(
        state.relations[0].links[0].other_end,
        onefollowup::store::LinkEnd::Missing
    );
    assert_eq!(state.render(), with);
}
