//! What a child started through `isolated::command` receives: the variables it was given and
//! the host's own (`SystemRoot` on Windows) — nothing else of this process's environment, the
//! variables the platform sets in every process (`isolated::PLATFORM_SET`) aside — and
//! a `PATH` from `isolated::search_path` on which git runs. The child is this test binary run
//! again as [`probe`], which prints the environment it received.

use std::collections::{BTreeMap, BTreeSet};

use crate::isolated;

/// Set only in the child: tells [`probe`] to print its environment.
const PROBE: &str = "ONEFOLLOWUP_E2E_ENVIRONMENT_PROBE";
/// Where the probe's environment starts and ends in its output, between libtest's own lines.
const START: &str = "<<environment>>";
const END: &str = "<</environment>>";

/// Not a case of its own: [`a_cleared_child_receives_only_its_variables_and_the_hosts`] runs it
/// in a child, with `PROBE` set, to print the environment the child received as JSON. Run in
/// the suite, without `PROBE`, it does nothing.
#[test]
fn probe() {
    if std::env::var_os(PROBE).is_none() {
        return;
    }
    let received: BTreeMap<String, String> = std::env::vars().collect();
    println!(
        "{START}{}{END}",
        serde_json::to_string(&received).expect("the environment serializes")
    );
}

#[test]
fn a_cleared_child_receives_only_its_variables_and_the_hosts() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = isolated::search_path(None, isolated::UNIX_TOOLS);
    let output = isolated::command(
        std::env::current_exe().expect("this test binary"),
        directory.path(),
        [(PROBE, "1"), ("PATH", &*path)],
    )
    // libtest names a test by its path within the crate.
    .args(["isolated_environment::probe", "--exact", "--nocapture"])
    .output()
    .expect("the probe runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "the probe failed: {output:?}");
    let printed = stdout
        .split_once(START)
        .and_then(|(_, rest)| rest.split_once(END))
        .map(|(printed, _)| printed)
        .unwrap_or_else(|| panic!("the probe printed no environment: {stdout}"));
    let mut received: BTreeMap<String, String> =
        serde_json::from_str(printed).expect("the probe printed JSON");
    // What the platform set in the child itself, not what reached it from this process: its
    // value is the platform's own format, and it is no variable the helper owes the child.
    for name in isolated::PLATFORM_SET {
        if let Some(value) = received.remove(*name) {
            assert!(
                platform_format(name, &value),
                "{name} is not in the format the platform sets it in"
            );
        }
    }

    let expected: BTreeSet<String> = [PROBE, "PATH"]
        .into_iter()
        .chain(isolated::HOST.iter().copied())
        .map(str::to_owned)
        .collect();
    // Names alone: a leaked variable's value may be a secret of the host's.
    assert_eq!(
        received.keys().cloned().collect::<BTreeSet<_>>(),
        expected,
        "the variables the child received"
    );
    assert_eq!(received[PROBE], "1");
    assert_eq!(received["PATH"], path);
    for name in isolated::HOST {
        assert_eq!(
            Some(&received[*name]),
            std::env::var(name).ok().as_ref(),
            "{name} is this process's own"
        );
    }
    if cfg!(windows) {
        assert!(
            received.contains_key("SystemRoot"),
            "a Windows child opens no socket without SystemRoot"
        );
    }

    let git = isolated::command("git", directory.path(), [("PATH", &*path)])
        .arg("--version")
        .output()
        .expect("git starts on the helper's PATH");
    assert!(git.status.success(), "git --version: {git:?}");
    assert!(
        String::from_utf8_lossy(&git.stdout).starts_with("git version"),
        "{git:?}"
    );
}

/// Whether `value` is in the format the platform itself sets the [`isolated::PLATFORM_SET`]
/// variable `name` in. CoreFoundation's `__CF_USER_TEXT_ENCODING` is `0x<hex uid>:<n>:<n>`.
fn platform_format(name: &str, value: &str) -> bool {
    match name {
        "__CF_USER_TEXT_ENCODING" => {
            let mut fields = value.split(':');
            let uid = fields.next().and_then(|uid| uid.strip_prefix("0x"));
            let numbers: Vec<&str> = fields.collect();
            uid.is_some_and(|uid| !uid.is_empty() && uid.chars().all(|c| c.is_ascii_hexdigit()))
                && numbers.len() == 2
                && numbers
                    .iter()
                    .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        }
        _ => false,
    }
}
