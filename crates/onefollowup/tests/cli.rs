//! The command line's dispatch, in process: every verb's arguments reach its library
//! function through `cli::dispatch`. (`crates/onefollowup-e2e` drives the built binary.)

use clap::Parser as _;
use onefollowup::Error;
use onefollowup::cli::{self, Cli};

fn dispatched(arguments: &[&str]) -> Result<String, Error> {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let file = directory.path().join("onefollowup.yaml");
    std::fs::write(&file, "destination: bugs\n").expect("written");
    let file = file.display().to_string();
    let cli = Cli::try_parse_from(
        ["onefollowup", "--config", file.as_str()]
            .iter()
            .chain(arguments.iter()),
    )
    .unwrap_or_else(|error| panic!("{arguments:?}: {error}"));
    cli::dispatch(cli.command, &cli.config)
}

#[test]
fn every_stubbed_verb_reaches_its_function_and_refuses() {
    let verbs: &[&[&str]] = &[&["statuses"]];
    for arguments in verbs {
        let error = dispatched(arguments).expect_err("not implemented");
        let verb = arguments
            .iter()
            .take_while(|word| !word.starts_with("--"))
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            matches!(&error, Error::NotImplemented { verb: named } if *named == verb),
            "{arguments:?}: {error}"
        );
        assert_eq!(
            error.to_string(),
            format!("{verb}: not implemented in this release")
        );
        assert_eq!(error.exit_code(), 2);
    }
}

#[test]
fn the_implemented_verbs_print_json() {
    let shown: serde_json::Value =
        serde_json::from_str(&dispatched(&["config", "show"]).expect("config show")).expect("JSON");
    assert_eq!(shown["config"]["destination"], "bugs");
    let kinds: serde_json::Value =
        serde_json::from_str(&dispatched(&["kinds"]).expect("kinds")).expect("JSON");
    assert_eq!(kinds["kinds"][0]["destination"], "bugs");
}

#[test]
fn a_refused_configuration_and_a_bad_key_value_are_refused() {
    let error = dispatched(&["--set", "destinaton=x", "config", "show"]).expect_err("refused");
    assert!(matches!(error, Error::Config(_)));
    assert_eq!(error.exit_code(), 2);
    assert!(
        Cli::try_parse_from([
            "onefollowup",
            "draft",
            "--batch",
            "b",
            "--title",
            "t",
            "--repository",
            "r",
            "--stamp",
            "=v"
        ])
        .is_err()
    );
    let store = onefollowup::store::Store::open(
        Some(std::path::Path::new("/nonexistent")),
        std::path::Path::new("."),
        &[],
    );
    let Err(refusal) = store else {
        panic!("a store at a missing path is refused")
    };
    let error = Error::from(refusal);
    assert_eq!(error.exit_code(), 1);
    let format =
        <onefollowup::formats::ticket::TicketRecord as onefollowup::formats::Versioned>::parse(
            "[]",
        )
        .expect_err("not a record");
    assert_eq!(Error::from(format).exit_code(), 1);
}

#[test]
fn every_verify_verb_reaches_its_function_and_refuses_a_batch_id_that_is_none() {
    let verbs: &[&[&str]] = &[
        &[
            "draft",
            "--batch",
            "not a batch",
            "--title",
            "t",
            "--repository",
            "github.com/a/b",
        ],
        &["prepare", "--batch", "not a batch"],
        &["ticket", "validate", "--batch", "not a batch", "t"],
        &["ticket", "board-status", "--batch", "not a batch", "s:i"],
        &[
            "ticket",
            "search",
            "--batch",
            "not a batch",
            "--ticket",
            "t",
            "words",
        ],
        &["ticket", "copy", "--batch", "not a batch", "t"],
        &[
            "ticket",
            "comment",
            "--batch",
            "not a batch",
            "--item",
            "s:i",
            "--root-cause",
            "r",
        ],
        &["ticket", "re-estimate", "--batch", "not a batch", "s:i"],
        &[
            "ticket",
            "relate",
            "--batch",
            "not a batch",
            "--relation",
            "r",
            "--from",
            "a",
            "--to",
            "b",
        ],
        &["check", "--batch", "not a batch"],
        &["publish", "--batch", "not a batch"],
        &["settle", "--batch", "not a batch"],
    ];
    for arguments in verbs {
        let error = dispatched(arguments).expect_err("refused");
        assert!(
            matches!(&error, Error::Refused(message) if message.contains("`not a batch` is not a batch id")),
            "{arguments:?}: {error}"
        );
        assert_eq!(error.exit_code(), 2);
    }
}
