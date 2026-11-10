//! The `onefollowup` command line: a thin layer over [`crate::api`].
//!
//! Every command's handler is one arm of [`dispatch`], and every arm is
//! `handle(config, args, api::<verb>)`: resolve the settings, call the library function,
//! print its result. Nothing else happens here, and `tests/parity.rs` fails when an arm does
//! more, or when a command has no library function behind it.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::api;
use crate::config::{self, Inputs, Settings};
use crate::error::{EXIT_FAILED, EXIT_OK, Error};

/// The follow-up loop for any orchestrator repository: record drafts, verify them, file one
/// ticket per root cause, and answer people's comments on those tickets.
///
/// Every verb prints a JSON document on success. Exit codes: 0 success; 1 failed while
/// running; 2 refused (a usage error, a refused configuration, or a verb not implemented in
/// this release).
#[derive(Debug, Parser)]
#[command(name = "onefollowup", version, propagate_version = true)]
pub struct Cli {
    /// Where the configuration comes from.
    #[command(flatten)]
    pub config: ConfigArgs,
    /// The verb.
    #[command(subcommand)]
    pub command: Command,
}

/// The configuration flags every verb takes: a flag beats an `ONEFOLLOWUP_` variable,
/// which beats the configuration file, which beats the default.
#[derive(Debug, Clone, Default, Args)]
pub struct ConfigArgs {
    /// The configuration file, over `ONEFOLLOWUP_CONFIG` and `./onefollowup.yaml`.
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Set any configuration key (a dotted path for a nested one); the value is YAML.
    #[arg(long = "set", global = true, value_name = "KEY=VALUE")]
    pub set: Vec<String>,
    /// The `destination` key, over any `--set destination=`.
    #[arg(long, global = true, value_name = "SOURCE")]
    pub destination: Option<String>,
    /// The `drafts_source` key, over any `--set drafts_source=`.
    #[arg(long, global = true, value_name = "SOURCE")]
    pub drafts_source: Option<String>,
    /// The `store` key, over any `--set store=`.
    #[arg(long, global = true, value_name = "PATH")]
    pub store: Option<String>,
}

impl ConfigArgs {
    /// The settings these flags, this process's environment and the configuration file
    /// resolve to.
    ///
    /// # Errors
    ///
    /// A refused configuration.
    pub fn resolve(&self) -> Result<Settings, Error> {
        let inputs = Inputs {
            config_file: self.config.clone(),
            set: self.set.clone(),
            destination: self.destination.clone(),
            drafts_source: self.drafts_source.clone(),
            store: self.store.clone(),
            ..Inputs::from_process()?
        };
        Ok(config::resolve(&inputs)?)
    }
}

/// Every verb.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Record one draft; its body is read from stdin.
    Draft(api::DraftArgs),
    /// Snapshot a batch's inputs, make its destination reads, and write the verifying
    /// agent's task.
    Prepare(api::PrepareArgs),
    /// The tools the verifying agent runs.
    #[command(subcommand)]
    Ticket(TicketCommand),
    /// Hold a batch's dispositions, tickets and staged writes to the rules.
    Check(api::BatchArgs),
    /// Send a batch's staged writes and receipt every outcome.
    Publish(api::BatchArgs),
    /// Close out a published batch.
    Settle(api::BatchArgs),
    /// The comment-answering pass.
    #[command(subcommand)]
    Comments(CommentsCommand),
    /// Read the destination's statuses against the configured status roles.
    Statuses(api::NoArgs),
    /// Print the effective kinds, relations, dispositions and routes.
    Kinds(api::NoArgs),
    /// The configuration.
    #[command(subcommand)]
    Config(ConfigCommand),
}

/// `onefollowup ticket …`.
#[derive(Debug, Subcommand)]
pub enum TicketCommand {
    /// Hold one ticket to the rules.
    Validate(api::TicketValidateArgs),
    /// Read one destination item's state and comments.
    BoardStatus(api::TicketBoardStatusArgs),
    /// Search the destination for a ticket's duplicates.
    Search(api::TicketSearchArgs),
    /// Stage a ticket's copy to its destination.
    Copy(api::TicketCopyArgs),
    /// Stage the batch's evidence comment on an item; the evidence is read from stdin.
    Comment(api::TicketCommentArgs),
    /// Stage an item's re-estimate.
    ReEstimate(api::TicketReEstimateArgs),
    /// Stage an edge of a configured relation.
    Relate(api::TicketRelateArgs),
}

/// `onefollowup comments …`.
#[derive(Debug, Subcommand)]
pub enum CommentsCommand {
    /// Gather people's comments to answer.
    Gather(api::CommentsGatherArgs),
    /// Plan the answering dispatches a snapshot launches.
    Launches(api::CommentsLaunchesArgs),
    /// Hold a gathering's responses to the rules.
    Check(api::CommentsCheckArgs),
    /// Settle a snapshot's answers.
    Settle(api::CommentsSettleArgs),
}

/// `onefollowup config …`.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the effective configuration as JSON.
    Show(api::ConfigShowArgs),
}

/// Run the command line over this process's arguments.
#[must_use]
pub fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli.command, &cli.config) {
        Ok(printed) => {
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{printed}")
                .and_then(|()| out.flush())
                .is_err()
            {
                return ExitCode::from(EXIT_FAILED);
            }
            ExitCode::from(EXIT_OK)
        }
        Err(error) => {
            match error {
                Error::NotImplemented { .. } => eprintln!("onefollowup {error}"),
                _ => eprintln!("onefollowup: {error}"),
            }
            ExitCode::from(error.exit_code())
        }
    }
}

/// Run one verb: every arm resolves the settings, calls the verb's library function, and
/// prints its result — and does nothing else.
///
/// # Errors
///
/// What the verb, or the resolution of its settings, refused with.
pub fn dispatch(command: Command, config: &ConfigArgs) -> Result<String, Error> {
    match command {
        Command::Draft(args) => handle(config, args, api::draft),
        Command::Prepare(args) => handle(config, args, api::prepare),
        Command::Ticket(TicketCommand::Validate(args)) => {
            handle(config, args, api::ticket_validate)
        }
        Command::Ticket(TicketCommand::BoardStatus(args)) => {
            handle(config, args, api::ticket_board_status)
        }
        Command::Ticket(TicketCommand::Search(args)) => handle(config, args, api::ticket_search),
        Command::Ticket(TicketCommand::Copy(args)) => handle(config, args, api::ticket_copy),
        Command::Ticket(TicketCommand::Comment(args)) => handle(config, args, api::ticket_comment),
        Command::Ticket(TicketCommand::ReEstimate(args)) => {
            handle(config, args, api::ticket_re_estimate)
        }
        Command::Ticket(TicketCommand::Relate(args)) => handle(config, args, api::ticket_relate),
        Command::Check(args) => handle(config, args, api::check),
        Command::Publish(args) => handle(config, args, api::publish),
        Command::Settle(args) => handle(config, args, api::settle),
        Command::Comments(CommentsCommand::Gather(args)) => {
            handle(config, args, api::comments_gather)
        }
        Command::Comments(CommentsCommand::Launches(args)) => {
            handle(config, args, api::comments_launches)
        }
        Command::Comments(CommentsCommand::Check(args)) => {
            handle(config, args, api::comments_check)
        }
        Command::Comments(CommentsCommand::Settle(args)) => {
            handle(config, args, api::comments_settle)
        }
        Command::Statuses(args) => handle(config, args, api::statuses),
        Command::Kinds(args) => handle(config, args, api::kinds),
        Command::Config(ConfigCommand::Show(args)) => handle(config, args, api::config_show),
    }
}

/// Resolve the settings, call `verb`, and print its result.
fn handle<A, R: Serialize>(
    config: &ConfigArgs,
    args: A,
    verb: fn(&Settings, A) -> Result<R, Error>,
) -> Result<String, Error> {
    let settings = config.resolve()?;
    let result = verb(&settings, args)?;
    Ok(print(&result))
}

/// A verb's result as the command line prints it: indented JSON.
#[must_use]
pub fn print<R: Serialize>(result: &R) -> String {
    serde_json::to_string_pretty(result)
        .unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"))
}
