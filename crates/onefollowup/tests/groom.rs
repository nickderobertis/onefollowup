//! The grooming journeys (`onefollowup-e2e`'s `tests/groom/`), each running the library in
//! process: the command line parses the arguments, the settings resolve from the world's own
//! environment, and the verb's library function runs.

#[path = "../../onefollowup-e2e/tests/groom/mod.rs"]
mod groom;

use clap::Parser as _;
use groom::world::{Ran, Runner, World};
use onefollowup::cli::{Cli, Command, GroomCommand, print};
use onefollowup::config::{self, Inputs};
use onefollowup::{Error, api};

/// `onefollowup`'s library, run in process in the world's root with the world's environment.
struct InProcess;

impl Runner for InProcess {
    fn run(&self, world: &World, args: &[&str]) -> Ran {
        let cli = Cli::try_parse_from(std::iter::once("onefollowup").chain(args.iter().copied()))
            .expect("the arguments parse");
        let inputs = Inputs {
            config_file: cli.config.config,
            set: cli.config.set,
            destination: cli.config.destination,
            drafts_source: cli.config.drafts_source,
            store: cli.config.store,
            environment: world.environment(),
            working_directory: world.root.clone(),
            hostname: None,
        };
        let ran = config::resolve(&inputs)
            .map_err(Error::from)
            .and_then(|settings| match cli.command {
                Command::Groom(GroomCommand::Prepare(args)) => {
                    api::groom_prepare(&settings, args).map(|result| print(&result))
                }
                Command::Groom(GroomCommand::Check(args)) => {
                    api::groom_check(&settings, args).map(|result| print(&result))
                }
                Command::Groom(GroomCommand::Publish(args)) => {
                    api::groom_publish(&settings, args).map(|result| print(&result))
                }
                other => panic!("a grooming journey runs only `groom` verbs, not {other:?}"),
            });
        match ran {
            Ok(stdout) => Ran {
                code: 0,
                stdout,
                stderr: String::new(),
            },
            Err(error) => Ran {
                code: error.exit_code().into(),
                stdout: error.document().unwrap_or_default(),
                stderr: format!("onefollowup: {error}"),
            },
        }
    }
}

groom::groom_journeys!(InProcess);
