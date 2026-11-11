//! What the batch's receipt journal says: which card each ticket is bound to, the latest
//! version a receipt recorded for each card, and the receipts the loop appends.

use std::collections::BTreeMap;

use crate::error::Error;
use crate::formats::receipts::{
    CopyAction, CopyOutcome, Outcome, Receipt, ReceiptError, ReceiptJournal,
};
use crate::verify::context::Batch;
use crate::verify::decide::Bound;
use crate::work;

/// The batch's journal.
///
/// # Errors
///
/// A journal that cannot be read.
pub fn read(batch: &Batch<'_>) -> Result<ReceiptJournal, Error> {
    Ok(work::journal(&batch.work.receipts(&batch.batch))?)
}

/// Whether a copy receipt landed its card.
#[must_use]
pub fn landed(copy: &CopyOutcome) -> bool {
    copy.error.is_none()
        && matches!(
            copy.action,
            CopyAction::Created | CopyAction::Updated | CopyAction::Unchanged
        )
}

/// The card each ticket's latest landed copy wrote, by the ticket's drafts-source item.
#[must_use]
pub fn bound(journal: &ReceiptJournal) -> Bound {
    let mut bound = Bound::new();
    for receipt in &journal.receipts {
        if let (Some(ticket), Some(card), Outcome::Copy(copy)) =
            (&receipt.ticket, &receipt.card, &receipt.outcome)
            && landed(copy)
        {
            let version = copy
                .result
                .as_ref()
                .and_then(|result| result.version.clone());
            bound.insert(ticket.clone(), (card.clone(), version));
        }
    }
    bound
}

/// The latest version any receipt recorded for each card.
#[must_use]
pub fn latest(journal: &ReceiptJournal) -> BTreeMap<String, String> {
    let mut latest = BTreeMap::new();
    for receipt in &journal.receipts {
        let Some(card) = &receipt.card else {
            continue;
        };
        let version = match &receipt.outcome {
            Outcome::Copy(copy) => copy
                .result
                .as_ref()
                .and_then(|result| result.version.clone()),
            Outcome::ReEstimate(estimate) => estimate.writes.iter().rev().find_map(|write| {
                write
                    .result
                    .as_ref()
                    .and_then(|result| result.version.clone())
            }),
            Outcome::Comment(_) => None,
        };
        if let Some(version) = version {
            latest.insert(card.clone(), version);
        }
    }
    latest
}

/// The line (1-based) and digest of the latest landed copy of the ticket stored as `item`.
#[must_use]
pub fn last_landed(journal: &ReceiptJournal, item: &str) -> Option<(u64, Option<String>)> {
    journal
        .receipts
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, receipt)| match &receipt.outcome {
            Outcome::Copy(copy) if receipt.ticket.as_deref() == Some(item) && landed(copy) => {
                Some(((at + 1) as u64, copy.digest.clone()))
            }
            _ => None,
        })
}

/// A copy refused before it reached a card, as its receipt records it.
#[must_use]
pub fn refused_copy(
    batch: &str,
    ticket: &str,
    card: Option<String>,
    kind: &str,
    message: &str,
) -> Receipt {
    Receipt {
        batch: batch.to_owned(),
        ticket: Some(ticket.to_owned()),
        card,
        outcome: Outcome::Copy(CopyOutcome {
            action: CopyAction::Failed,
            result: None,
            status_condition: None,
            priority_condition: None,
            error: Some(ReceiptError {
                kind: kind.to_owned(),
                message: message.to_owned(),
            }),
            held: None,
            digest: None,
            unchanged_since: None,
        }),
    }
}

/// Append `receipts` to the batch's journal.
///
/// # Errors
///
/// A journal that cannot be appended to.
pub fn append(batch: &Batch<'_>, receipts: &[Receipt]) -> Result<(), Error> {
    if receipts.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = receipts.iter().map(Receipt::render_line).collect();
    Ok(work::append(&batch.work.receipts(&batch.batch), &lines)?)
}
