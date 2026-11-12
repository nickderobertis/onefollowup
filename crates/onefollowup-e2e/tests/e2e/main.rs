//! The verify loop's journeys, each driving the built `onefollowup` binary as a subprocess
//! over onetaskgraph local-md sources, with the verifying agent played by scripted steps that
//! run the agent's tools; and the budget runner every `budgets.yaml` entry names.

mod adoption;
mod budget;
mod budget_runner;
mod configuration;
mod duplicates;
mod examples;
mod journey;
mod refusals;
mod safeguards;
mod telemetry;
mod world;
