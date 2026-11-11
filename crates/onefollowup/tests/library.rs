//! The library gives what the command line prints: the configuration's resolution and a
//! verb's function called directly agree with the built binary run over the same settings.

use std::path::Path;
use std::process::{Command, Output};

use onefollowup::api::{self, ConfigShowArgs, PrepareArgs};
use onefollowup::config::{self, Inputs};
use onefollowup::{Error, cli};

fn run(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_onefollowup"))
        .args(arguments)
        .current_dir(directory)
        .env_clear()
        .output()
        .expect("the binary runs")
}

fn settings(directory: &Path, set: &[&str]) -> onefollowup::Settings {
    let inputs = Inputs {
        working_directory: directory.to_path_buf(),
        set: set.iter().map(|one| (*one).to_owned()).collect(),
        ..Inputs::default()
    };
    config::resolve(&inputs).expect("the configuration resolves")
}

/// A temporary directory named by its real path, as the binary names its working
/// directory: on macOS the temporary root under `/var` is a link to `/private/var`. Windows
/// keeps the root as given, since its real path carries a `\\?\` prefix nothing prints.
fn tempdir() -> tempfile::TempDir {
    let root = std::env::temp_dir();
    #[cfg(not(windows))]
    let root = root.canonicalize().expect("the temporary root resolves");
    tempfile::tempdir_in(root).expect("a temporary directory")
}

#[test]
fn config_show_through_the_library_is_what_the_cli_prints() {
    let directory = tempdir();
    std::fs::write(
        directory.path().join("onefollowup.yaml"),
        "destination: bugs\nnamespace: acme\n",
    )
    .expect("written");
    for (arguments, set, sources) in [
        (vec!["config", "show"], vec![], false),
        (
            vec![
                "--set",
                "estimate.raise_at=9",
                "config",
                "show",
                "--sources",
            ],
            vec!["estimate.raise_at=9"],
            true,
        ),
    ] {
        let output = run(directory.path(), &arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let shown = api::config_show(
            &settings(directory.path(), &set),
            ConfigShowArgs { sources },
        )
        .expect("config show");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("{}\n", cli::print(&shown))
        );
    }
    let kinds = api::kinds(&settings(directory.path(), &[]), api::NoArgs {}).expect("kinds");
    let output = run(directory.path(), &["kinds"]);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{}\n", cli::print(&kinds))
    );
}

#[test]
fn a_stubbed_verb_refuses_through_the_library_as_the_cli_does() {
    let directory = tempdir();
    let arguments = PrepareArgs {
        batch: "nightly-1".to_owned(),
        aliases: vec![],
        unanswered: false,
    };
    let error =
        api::prepare(&settings(directory.path(), &[]), arguments).expect_err("not implemented");
    assert!(matches!(error, Error::NotImplemented { verb: "prepare" }));
    let output = run(directory.path(), &["prepare", "--batch", "nightly-1"]);
    assert_eq!(output.status.code(), Some(i32::from(error.exit_code())));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!("onefollowup {error}\n")
    );
    assert!(output.stdout.is_empty());
}
