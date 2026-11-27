//! The verify loop's journeys, each driving the built `onefollowup` binary as a subprocess
//! over onetaskgraph local-md sources, with the verifying agent played by scripted steps that
//! run the agent's tools; and the budget runner every `budgets.yaml` entry names.

mod adoption;
mod answering;
mod board_record;
mod bounded_prepare;
mod budget;
mod budget_runner;
mod comment_adoption;
mod comment_budgets;
mod comment_forms;
mod comment_refusals;
mod comments;
mod comments_answer;
mod configuration;
mod crowded_filing;
mod default_shape;
mod duplicate_boards;
mod duplicates;
mod examples;
mod filing_duplicates;
mod governed_publish;
mod graphql_cost;
mod hook_context;
mod host_governor;
#[path = "../isolated/mod.rs"]
mod isolated;
mod isolated_environment;
mod journey;
mod loopback_boards;
mod loopback_github;
mod outstanding;
mod pending_recovery;
mod prepare_cost;
mod private_drafts;
mod real_board_prices;
mod refusals;
mod refused_accounts;
mod renamed_shape;
mod rollups;
mod routed_duplicates;
mod safeguards;
mod snapshot;
mod snapshots;
mod telemetry;
mod template_data;
mod template_refusals;
mod world;

#[path = "../groom/mod.rs"]
mod groom;
mod groom_budget;
mod groom_comments;
mod groom_interrupted;
mod grooming;
