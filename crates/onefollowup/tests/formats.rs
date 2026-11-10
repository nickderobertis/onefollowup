//! Every record format against records as they stand on boards and disks today.
//!
//! The fixtures under `tests/fixtures/` were written once by the Python writers of the
//! repository these formats are adopted from (at its commit `cf73059f`, with synthetic
//! values), and committed: nothing here reads or runs anything of that repository. Each is
//! read and re-rendered byte for byte; a record one schema above — and, where one exists,
//! one below — the version read is refused naming the format, the schema found and the
//! schema read.

use onefollowup::formats::dispositions::{CarriedDispositions, Dispositions};
use onefollowup::formats::draft::{DraftRecord, DraftsProjectRecord};
use onefollowup::formats::existing_tickets::ExistingTickets;
use onefollowup::formats::markers::{
    CommentMarker, EvidenceComment, FeedbackBoundary, FeedbackSnapshot, IssueLine, QuotedAnchor,
    Reply, ReplyTo, evidence_opening, reply_opening,
};
use onefollowup::formats::outbox::{Outbox, StagedWrite};
use onefollowup::formats::pending::Pending;
use onefollowup::formats::receipts::{Outcome, ReceiptJournal};
use onefollowup::formats::responses::Responses;
use onefollowup::formats::snapshot::{Launched, Snapshot};
use onefollowup::formats::ticket::TicketRecord;
use onefollowup::formats::watermark::Watermark;
use onefollowup::formats::{FormatError, Level, Verdict, Versioned};

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
        full.budget.as_ref().map(|budget| budget.threshold),
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

    let evidence_text = fixture("comment-markers/evidence-comment.md");
    let evidence = EvidenceComment::parse(&evidence_text, NAMESPACE).expect("an evidence comment");
    assert_eq!(evidence.render(NAMESPACE), evidence_text);
    assert!(
        EvidenceComment::parse(&fixture("comment-markers/reply-attributed.md"), NAMESPACE).is_err()
    );
    assert!(EvidenceComment::parse("no newline", NAMESPACE).is_err());
    assert!(EvidenceComment::parse("one line\n", NAMESPACE).is_err());
    assert!(EvidenceComment::parse("a\n\nb\n", NAMESPACE).is_err());
    for path in ["reply-attributed.md", "reply-unattributed.md"] {
        let text = fixture(&format!("comment-markers/{path}"));
        let reply =
            Reply::parse(&text, NAMESPACE).unwrap_or_else(|error| panic!("{path}: {error}"));
        assert_eq!(reply.render(NAMESPACE), text, "{path}");
    }
    assert!(Reply::parse(&evidence_text, NAMESPACE).is_err());
    let foreign = fixture("comment-markers/reply-attributed.md").replacen(
        "run `nightly-2026-11-01`",
        "run `other`",
        1,
    );
    assert!(Reply::parse(&foreign, NAMESPACE).is_err());
    let unnamed = fixture("comment-markers/reply-attributed.md").replacen(
        "to @alice's comment",
        "to alice",
        1,
    );
    assert!(Reply::parse(&unnamed, NAMESPACE).is_err());
    let built = Reply {
        batch: batch.to_owned(),
        root_cause: "x".to_owned(),
        to: ReplyTo {
            answers: "c".to_owned(),
            verdict: Some(Verdict::Confirms),
        },
        url: url.to_owned(),
        author: None,
        response: "Thanks.".to_owned(),
    };
    assert_eq!(Reply::parse(&built.render(NAMESPACE), NAMESPACE), Ok(built));

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

#[test]
fn dispositions_round_trip_with_and_without_kind() {
    let plain: Dispositions = round_trips("dispositions/dispositions.json");
    assert!(plain.dispositions.iter().all(|entry| entry.kind.is_none()));
    let kinded: Dispositions = round_trips("dispositions/dispositions-with-kind.json");
    assert_eq!(kinded.dispositions[1].kind.as_deref(), Some("error"));
    round_trips::<Dispositions>("dispositions/skeleton-first-open.json");
    let carried: CarriedDispositions = round_trips("dispositions/dispositions.carried.json");
    assert!(
        carried
            .dispositions
            .iter()
            .all(|entry| entry.kind.is_none())
    );
    let carried: CarriedDispositions =
        round_trips("dispositions/dispositions.carried-with-kind.json");
    assert!(
        carried
            .dispositions
            .iter()
            .all(|entry| entry.kind.is_some())
    );
    refused::<Dispositions>(
        "dispositions/dispositions-schema2.json",
        "dispositions",
        "2",
        1,
    );
    refused::<CarriedDispositions>(
        "dispositions/dispositions.carried-schema2.json",
        "carried dispositions",
        "2",
        1,
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
    let evidence = EvidenceComment {
        batch: batch.to_owned(),
        root_cause: "export-dialog-drops-unicode".to_owned(),
        evidence: "Seen again — the title café lost its accent.".to_owned(),
    };
    let outbox = Outbox {
        batch: batch.to_owned(),
        writes: vec![
            StagedWrite::Copy {
                ticket: format!("drafts:{batch}/tickets/layout-save-reads-size-twice"),
                destination: "followups".to_owned(),
            },
            StagedWrite::Comment {
                item: "followups:item-13".to_owned(),
                root_cause: evidence.root_cause.clone(),
                body: evidence.render(NAMESPACE),
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
