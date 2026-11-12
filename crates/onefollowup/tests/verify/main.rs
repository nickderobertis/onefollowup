//! The verify loop through the library, in process: each journey calls the verbs' public
//! functions in `onefollowup::api`, as the command line's dispatch does, over onetaskgraph
//! local-md sources, with the verifying agent played by scripted steps that run the agent's
//! tools. (`crates/onefollowup-e2e` runs the same journeys against the built binary.)

mod adoption;
mod baseline;
mod configuration;
mod duplicates;
mod edges;
mod journey;
mod refusals;
mod safeguards;
mod world;
