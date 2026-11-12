//! The verify loop and the comment pass through the library, in process: each journey calls
//! the verbs' public functions in `onefollowup::api`, as the command line's dispatch does, over
//! onetaskgraph local-md sources, with the verifying and answering agents played by scripted
//! steps. (`crates/onefollowup-e2e` runs the same journeys against the built binary.)

mod adoption;
mod answering;
mod assignees;
mod baseline;
mod comment_adoption;
mod comment_refusals;
mod comments;
mod configuration;
mod duplicates;
mod edges;
mod journey;
mod outstanding;
mod pending_recovery;
mod refusals;
mod refused_accounts;
mod relations;
mod rollups;
mod safeguards;
mod snapshots;
mod world;
