//! Library parity: every verb the CLI defines has a public library function behind it, and
//! the CLI's handler for each does no more than resolve its settings, call that function
//! and print its result.
//!
//! The verbs are enumerated from the CLI's own command definition; the handlers are read
//! from `src/cli.rs`'s `dispatch`, every arm of which must be exactly
//! `handle(config, args, api::<verb>)`, and `handle` itself must be exactly
//! resolve-call-print.

use std::collections::BTreeSet;

use clap::CommandFactory as _;
use onefollowup::cli::Cli;
use regex::Regex;

const CLI_SOURCE: &str = include_str!("../src/cli.rs");
const API_SOURCE: &str = include_str!("../src/api.rs");

/// Every leaf command's path, e.g. `["ticket", "board-status"]`.
fn leaves(command: &clap::Command, path: &mut Vec<String>, into: &mut Vec<Vec<String>>) {
    let subcommands: Vec<&clap::Command> = command
        .get_subcommands()
        .filter(|one| one.get_name() != "help")
        .collect();
    if subcommands.is_empty() {
        into.push(path.clone());
        return;
    }
    for subcommand in subcommands {
        path.push(subcommand.get_name().to_owned());
        leaves(subcommand, path, into);
        path.pop();
    }
}

fn function_for(path: &[String]) -> String {
    path.join("_").replace('-', "_")
}

/// The text of the function `name` in `source`, whitespace collapsed to single spaces.
fn function_body(source: &str, signature: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` is in src/cli.rs"));
    let end = source[start..]
        .find("\n}\n")
        .map(|at| start + at + 2)
        .expect("the function ends");
    source[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn every_verb_has_a_library_function_and_a_thin_handler() {
    let mut paths = Vec::new();
    leaves(&Cli::command(), &mut Vec::new(), &mut paths);
    let verbs: BTreeSet<String> = paths.iter().map(|path| function_for(path)).collect();
    assert_eq!(verbs.len(), 19, "the v1 surface's verbs: {verbs:?}");

    let dispatch = function_body(CLI_SOURCE, "pub fn dispatch(");
    let arm = Regex::new(
        r"Command::\w+\((?:\w+Command::\w+\()?args\)?\) => (?:\{ handle\(config, args, api::(\w+)\) \}|handle\(config, args, api::(\w+)\),) ?",
    )
    .expect("a valid pattern");
    let handled: BTreeSet<String> = arm
        .captures_iter(&dispatch)
        .map(|captures| {
            captures
                .get(1)
                .or_else(|| captures.get(2))
                .expect("a verb")
                .as_str()
                .to_owned()
        })
        .collect();
    let rest = arm.replace_all(&dispatch, "");
    assert_eq!(
        rest.trim(),
        "pub fn dispatch(command: Command, config: &ConfigArgs) -> Result<String, Error> { match command { } }",
        "every arm of `dispatch` is `handle(config, args, api::<verb>)` and nothing else"
    );
    assert_eq!(
        handled, verbs,
        "each verb is handled once, by the library function named for it"
    );

    for verb in &verbs {
        assert!(
            API_SOURCE.contains(&format!("pub fn {verb}(")),
            "`onefollowup::api::{verb}` is a public library function"
        );
    }

    assert_eq!(
        function_body(CLI_SOURCE, "fn handle<"),
        "fn handle<A, R: Serialize>( config: &ConfigArgs, args: A, verb: fn(&Settings, A) -> Result<R, Error>, ) -> Result<String, Error> { let settings = config.resolve()?; let result = verb(&settings, args)?; Ok(print(&result)) }",
        "a handler resolves the settings, calls the verb and prints its result — nothing else"
    );
}

#[test]
fn the_surface_is_the_v1_surface_and_has_no_re_key() {
    let mut paths = Vec::new();
    leaves(&Cli::command(), &mut Vec::new(), &mut paths);
    let names: BTreeSet<String> = paths.iter().map(|path| path.join(" ")).collect();
    let expected: BTreeSet<String> = [
        "draft",
        "prepare",
        "ticket validate",
        "ticket board-status",
        "ticket search",
        "ticket copy",
        "ticket comment",
        "ticket re-estimate",
        "ticket relate",
        "check",
        "publish",
        "settle",
        "comments gather",
        "comments launches",
        "comments check",
        "comments settle",
        "statuses",
        "kinds",
        "config show",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(names, expected);
    assert!(names.iter().all(|name| !name.contains("re-key")));
}
