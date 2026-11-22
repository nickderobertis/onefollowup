//! `snapshot`: prepare's input-set step alone, telling its caller whether the batch holds
//! anything to verify. It reads no destination and renders no task. A non-empty first pass
//! is saved as the batch's state, which a later `prepare` continues; an empty batch leaves no
//! working file, so nothing it did is seen later.

use crate::api::{SCHEMA_VERSION, SnapshotArgs, Snapshotted};
use crate::config::Settings;
use crate::error::Error;
use crate::verify::context::Batch;
use crate::verify::prepare::{self, Pass};

/// Snapshot the batch's input set and say whether it is empty.
///
/// # Errors
///
/// A refused batch id or alias, a failing hook (nothing written), or the store's refusal.
pub fn run(settings: &Settings, args: SnapshotArgs) -> Result<Snapshotted, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let store = batch.store()?;
    let (state, pass) = prepare::input_set(&batch, &store, &args.aliases)?;
    let drafts = state
        .inputs
        .iter()
        .filter(|input| input.source == "draft")
        .count();
    let empty = state.inputs.is_empty();
    // Only a first pass is saved: a continuing one is already the state, and one after a
    // settled pass cannot be marked open in the batch-state format, so `prepare` opens it.
    if !empty && pass == Pass::First {
        batch.save_state(&state)?;
    }
    Ok(Snapshotted {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch.clone(),
        drafts,
        inputs: state.inputs.len() - drafts,
        empty,
        store_operations: store.operations(),
        governed: crate::api::Governed::of(&store),
    })
}
