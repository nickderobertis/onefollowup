//! The verify loop's journeys, each driving the built `onefollowup` binary as a subprocess
//! over onetaskgraph local-md sources, with the verifying agent played by scripted steps that
//! run the agent's tools; and the budget runner every `budgets.yaml` entry names.

mod adoption;
mod answering;
mod board_record;
mod budget;
mod budget_runner;
mod comment_adoption;
mod comment_budgets;
mod comment_forms;
mod comment_refusals;
mod comments;
mod comments_answer;
mod configuration;
mod default_shape;
mod duplicates;
mod examples;
mod hook_context;
mod journey;
mod outstanding;
mod pending_recovery;
mod private_drafts;
mod refusals;
mod refused_accounts;
mod renamed_shape;
mod rollups;
mod safeguards;
mod snapshot;
mod snapshots;
mod telemetry;
mod template_data;
mod template_refusals;
mod world;
