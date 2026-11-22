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

mod support;

/// `groom publish` over a batch planning several closes, on a virtual clock, its doubled
/// reading metering local-md's record writes (`local`) with headroom for one request but not
/// the status read and the closes together, until its window resets at 60 s; nobody else
/// spends. The batch waits once, before its status read — nothing is written at any reading
/// taken while held — and goes through with no further wait.
#[test]
fn publish_waits_before_its_status_read_until_the_read_and_its_closes_are_admitted_together() {
    use std::sync::{Arc, Mutex};

    use groom::world::verdict;
    use onefollowup::governor::{Clock as _, Parts, Reading, Reason};
    use support::{Scripted, T0, Virtual, instant, reading};

    const WIDGETS: &str = "github.com/example/widgets";
    let world = World::new();
    world.repository(WIDGETS, &[("src/a.rs", "a\n")]);
    let fix = world.commit(WIDGETS, &[("src/a.rs", "a fixed\n")], "fix the save");
    let tickets = ["fixed-1", "fixed-2", "fixed-3", "fixed-4"];
    for id in tickets {
        world.ticket(
            id,
            &format!("widgets: ticket {id}"),
            "backlog",
            &[WIDGETS],
            &format!("The body of {id}."),
        );
    }
    let batch = world
        .checked(&InProcess, |id| {
            verdict(id, "fixed", "high", Some(&fix), &["src/a.rs"], None)
        })
        .expect("a batch");

    let before: Vec<String> = tickets.iter().map(|id| world.ticket_text(id)).collect();
    let clock = Virtual::at(instant(T0));
    let readings = Scripted::new(&clock, |at| {
        let local = if at < instant(T0 + 60) {
            Reading {
                limit: 1000,
                ..reading("local", 201, T0 + 60)
            }
        } else {
            Reading {
                limit: 1000,
                ..reading("local", 1000, T0 + 3660)
            }
        };
        vec![
            local,
            reading("graphql", 5000, T0 + 3600),
            reading("core", 5000, T0 + 3600),
        ]
    });
    let unchanged = Arc::new(Mutex::new(Vec::new()));
    let (seen, root) = (Arc::clone(&unchanged), world.root.clone());
    readings.on_read(move |at| {
        let now: Vec<String> = tickets
            .iter()
            .map(|id| {
                std::fs::read_to_string(root.join(format!("followups-root/tasks/{id}.md")))
                    .unwrap_or_default()
            })
            .collect();
        seen.lock().unwrap().push((at, now));
    });
    let parts = Parts {
        clock: clock.clone(),
        readings: Some(readings.clone()),
        notices: Arc::new(|line: &str| eprintln!("{line}")),
    };
    let published = onefollowup::governor::with_parts(parts, || {
        world.json(&InProcess, 0, &["groom", "publish", "--batch", &batch])
    });

    for id in tickets {
        assert_eq!(world.status(id), "done", "{id} closed");
    }
    let waits = published["governor"]["waits"].as_array().expect("waits");
    assert_eq!(waits.len(), 1, "one wait, the batch's: {published:#}");
    assert_eq!(waits[0]["resource"], "local");
    assert_eq!(waits[0]["seconds"], 60);
    assert_eq!(
        serde_json::from_value::<Reason>(waits[0]["reason"].clone()).unwrap(),
        Reason::Floor
    );
    assert_eq!(
        clock.now(),
        instant(T0 + 60),
        "nothing waited after the batch was admitted"
    );
    let held: Vec<_> = unchanged
        .lock()
        .unwrap()
        .iter()
        .filter(|(at, _)| *at < instant(T0 + 60))
        .cloned()
        .collect();
    assert!(held.len() >= 2, "read while held: {held:?}");
    assert!(
        held.iter().all(|(_, now)| *now == before),
        "no close was written before the batch was admitted"
    );
}
