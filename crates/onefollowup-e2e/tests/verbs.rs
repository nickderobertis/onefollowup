//! Every verb of the v1 surface but `config show` and `kinds`, driven through the built
//! binary: each parses its arguments — a malformed invocation is a usage error. A verb the
//! verify loop implements then needs its store, and fails naming the onetaskgraph
//! configuration it did not find; every other verb refuses with exit 2 and `not implemented in
//! this release`.

mod support;

use support::onefollowup;

/// The verbs the verify loop implements.
const IMPLEMENTED: &[&str] = &[
    "draft",
    "prepare",
    "ticket validate",
    "ticket board-status",
    "ticket search",
    "ticket copy",
    "ticket comment",
    "ticket re-estimate",
    "check",
    "publish",
    "settle",
];

/// Each verb: a well-formed invocation, and a malformed one.
const VERBS: &[(&[&str], &[&str])] = &[
    (
        &[
            "draft",
            "--batch",
            "b1",
            "--title",
            "T",
            "--repository",
            "github.com/example/widgets",
            "--path",
            "src/a.rs",
            "--path",
            "src/b.rs",
            "--author",
            "worker",
            "--stamp",
            "node=n1",
            "--stamp",
            "session=s",
        ],
        &[
            "draft",
            "--batch",
            "b1",
            "--title",
            "T",
            "--repository",
            "r",
            "--stamp",
            "no-equals",
        ],
    ),
    (
        &[
            "prepare",
            "--batch",
            "b1",
            "--alias",
            "b0",
            "--alias",
            "b00",
            "--unanswered",
        ],
        &["prepare"],
    ),
    (
        &["ticket", "validate", "--batch", "b1", "a-root-cause"],
        &["ticket", "validate", "--batch", "b1"],
    ),
    (
        &["ticket", "board-status", "--batch", "b1", "followups:42"],
        &["ticket", "board-status", "followups:42"],
    ),
    (
        &[
            "ticket",
            "search",
            "--batch",
            "b1",
            "--ticket",
            "a-root-cause",
            "resize during save",
        ],
        &["ticket", "search", "--batch", "b1", "resize"],
    ),
    (
        &["ticket", "copy", "--batch", "b1", "a-root-cause"],
        &["ticket", "copy", "--batch", "b1", "a", "b"],
    ),
    (
        &[
            "ticket",
            "comment",
            "--batch",
            "b1",
            "--item",
            "followups:42",
            "--root-cause",
            "a-root-cause",
        ],
        &[
            "ticket",
            "comment",
            "--batch",
            "b1",
            "--item",
            "followups:42",
        ],
    ),
    (
        &["ticket", "re-estimate", "--batch", "b1", "followups:42"],
        &["ticket", "re-estimate", "followups:42", "--bogus"],
    ),
    (
        &[
            "ticket",
            "relate",
            "--batch",
            "b1",
            "--relation",
            "of-error",
            "--from",
            "a",
            "--to",
            "followups:7",
        ],
        &[
            "ticket",
            "relate",
            "--batch",
            "b1",
            "--relation",
            "of-error",
            "--from",
            "a",
        ],
    ),
    (&["check", "--batch", "b1"], &["check"]),
    (&["publish", "--batch", "b1"], &["publish", "--batch"]),
    (&["settle", "--batch", "b1"], &["settle", "b1"]),
    (
        &["comments", "gather", "--batch", "b1", "--dry-run"],
        &[
            "comments",
            "gather",
            "--batch",
            "b1",
            "--item",
            "followups:42",
        ],
    ),
    (
        &["comments", "gather", "--item", "followups:42"],
        &["comments", "gather", "--dry-run", "yes"],
    ),
    (
        &["comments", "gather"],
        &["comments", "gather", "--since", "1h"],
    ),
    (
        &["comments", "launches", "--snapshot", "s"],
        &["comments", "launches"],
    ),
    (
        &["comments", "check", "--feedback", "f.md"],
        &["comments", "check", "--snapshot", "s"],
    ),
    (
        &[
            "comments",
            "settle",
            "--snapshot",
            "s",
            "--launched",
            "b1=f1.md",
            "--launched",
            "b2=f2.md",
        ],
        &["comments", "settle", "--snapshot", "s", "--launched", "b1"],
    ),
    (&["statuses"], &["statuses", "--bogus"]),
];

#[test]
fn every_verb_parses_its_arguments_then_needs_its_store_or_refuses_as_not_implemented() {
    let directory = support::tempdir();
    let mut verbs = std::collections::BTreeSet::new();
    for (well_formed, malformed) in VERBS {
        let verb = well_formed
            .iter()
            .take_while(|word| !word.starts_with("--"))
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        verbs.insert(verb.clone());

        let output = onefollowup(directory.path())
            .args(*well_formed)
            .output()
            .expect("it runs");
        let stderr = String::from_utf8_lossy(&output.stderr);
        if IMPLEMENTED.contains(&verb.as_str()) {
            assert_eq!(output.status.code(), Some(1), "{well_formed:?}: {stderr}");
            assert!(
                stderr.contains("no onetaskgraph.yaml was found"),
                "{well_formed:?}: {stderr}"
            );
        } else {
            assert_eq!(output.status.code(), Some(2), "{well_formed:?}: {stderr}");
            assert_eq!(
                stderr,
                format!("onefollowup {verb}: not implemented in this release\n"),
                "{well_formed:?}"
            );
        }
        assert!(output.stdout.is_empty(), "{well_formed:?}");

        let output = onefollowup(directory.path())
            .args(*malformed)
            .output()
            .expect("it runs");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{malformed:?}: {stderr}");
        assert!(
            stderr.starts_with("error: ") && stderr.contains("try '--help'"),
            "{malformed:?} is a usage error: {stderr}"
        );
        assert!(
            !stderr.contains("not implemented"),
            "{malformed:?} never reaches the verb: {stderr}"
        );
    }
    assert_eq!(
        verbs.len(),
        17,
        "every verb but `config show` and `kinds`: {verbs:?}"
    );
}

#[test]
fn an_unknown_verb_and_a_bare_invocation_are_usage_errors_and_help_lists_the_surface() {
    let directory = support::tempdir();
    for arguments in [
        &["re-key"][..],
        &[],
        &["ticket"],
        &["comments"],
        &["config"],
    ] {
        let output = onefollowup(directory.path())
            .args(arguments)
            .output()
            .expect("it runs");
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Usage: onefollowup"),
            "{arguments:?}"
        );
    }
    let output = onefollowup(directory.path())
        .arg("--help")
        .output()
        .expect("it runs");
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    for verb in [
        "draft", "prepare", "ticket", "check", "publish", "settle", "comments", "statuses",
        "kinds", "config",
    ] {
        assert!(help.contains(verb), "--help names {verb}");
    }
    assert!(!help.contains("re-key"));
    let output = onefollowup(directory.path())
        .arg("--version")
        .output()
        .expect("it runs");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("onefollowup {}\n", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn a_refused_configuration_stops_a_verb_before_it_runs() {
    let directory = support::tempdir();
    std::fs::write(
        directory.path().join("onefollowup.yaml"),
        "destinaton: typo\n",
    )
    .expect("written");
    let output = onefollowup(directory.path())
        .args(["prepare", "--batch", "b1"])
        .output()
        .expect("it runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr.starts_with("onefollowup: configuration key `destinaton` from the file "),
        "{stderr}"
    );
    assert!(!stderr.contains("not implemented"));
}
