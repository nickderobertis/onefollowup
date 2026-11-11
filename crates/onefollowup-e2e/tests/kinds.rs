//! `onefollowup kinds` through the built binary: exactly what the configuration states,
//! each kind with its allowed dispositions resolved.

mod support;

use serde_json::json;
use support::{json, onefollowup};

const TRIAGE: &str = r#"
fields:
  error_code: {description: "The error's code, exactly as logged."}
  client: {description: The client's account id.}
kinds:
  - name: root-cause
    description: One broken invariant.
  - name: error
    description: One error code.
    duplicates: {same: [error_code]}
    priority: {fixed: high}
    initial_status: accepted
    destination: errors
    dispositions: [filed, wont-fix]
  - name: client-error
    description: One client's error.
    ticket_template: templates/client-error.md.j2
    duplicates: {same: [error_code, client]}
default_kind: root-cause
relations:
  - {name: of-error, from: client-error, to: error, cardinality: many-to-one, decided_by: {match: [error_code]}, store: parent}
  - {name: caused-by, from: client-error, to: root-cause, cardinality: many-to-many, decided_by: agent, store: link, rollup: max}
dispositions:
  - {name: filed, description: Filed as or on a ticket., requires: [ticket]}
  - {name: wont-fix, description: Not worth fixing., requires: [reason]}
  - {name: fixed-in-change, description: A change already fixed it., requires: [change, reason]}
default_dispositions: [filed, fixed-in-change]
routes:
  - name: on-call
    match: {kind: [client-error], relations: {of-error: {min: 1}}}
    set: {priority: urgent, labels: [on-call], assignee: alice}
    command: [rotation, today]
"#;

#[test]
fn kinds_prints_exactly_what_the_configuration_states() {
    let directory = support::tempdir();
    std::fs::write(directory.path().join("onefollowup.yaml"), TRIAGE).expect("written");
    let shown = json(
        &onefollowup(directory.path())
            .arg("kinds")
            .output()
            .expect("it runs"),
    );
    let expected = json!({
        "schema_version": 1,
        "fields": {
            "error_code": {"description": "The error's code, exactly as logged."},
            "client": {"description": "The client's account id."}
        },
        "kinds": [
            {
                "name": "root-cause",
                "description": "One broken invariant.",
                "ticket_template": "builtin:ticket-template.md.j2",
                "duplicates": "agent",
                "priority": {"model": "estimate"},
                "initial_status": "proposal",
                "destination": "followups",
                "dispositions": ["filed", "fixed-in-change"],
                "dispositions_from": "default_dispositions"
            },
            {
                "name": "error",
                "description": "One error code.",
                "ticket_template": "builtin:ticket-template.md.j2",
                "duplicates": {"same": ["error_code"]},
                "priority": {"fixed": "high"},
                "initial_status": "accepted",
                "destination": "errors",
                "dispositions": ["filed", "wont-fix"],
                "dispositions_from": "kind"
            },
            {
                "name": "client-error",
                "description": "One client's error.",
                "ticket_template": "templates/client-error.md.j2",
                "duplicates": {"same": ["error_code", "client"]},
                "priority": {"model": "estimate"},
                "initial_status": "proposal",
                "destination": "followups",
                "dispositions": ["filed", "fixed-in-change"],
                "dispositions_from": "default_dispositions"
            }
        ],
        "default_kind": "root-cause",
        "relations": [
            {"name": "of-error", "from": "client-error", "to": "error", "cardinality": "many-to-one", "decided_by": {"match": ["error_code"]}, "store": "parent", "rollup": "none"},
            {"name": "caused-by", "from": "client-error", "to": "root-cause", "cardinality": "many-to-many", "decided_by": "agent", "store": "link", "rollup": "max"}
        ],
        "dispositions": [
            {"name": "filed", "description": "Filed as or on a ticket.", "requires": ["ticket"]},
            {"name": "wont-fix", "description": "Not worth fixing.", "requires": ["reason"]},
            {"name": "fixed-in-change", "description": "A change already fixed it.", "requires": ["change", "reason"]}
        ],
        "default_dispositions": ["filed", "fixed-in-change"],
        "routes": [
            {
                "name": "on-call",
                "match": {"kind": ["client-error"], "relations": {"of-error": {"min": 1}}},
                "set": {"priority": "urgent", "labels": ["on-call"], "assignee": "alice"},
                "command": ["rotation", "today"]
            }
        ]
    });
    assert_eq!(shown, expected);
}

#[test]
fn without_default_dispositions_a_kind_allows_every_catalogued_one() {
    let directory = support::tempdir();
    let without = TRIAGE.replace("default_dispositions: [filed, fixed-in-change]\n", "");
    std::fs::write(directory.path().join("onefollowup.yaml"), without).expect("written");
    let shown = json(
        &onefollowup(directory.path())
            .arg("kinds")
            .output()
            .expect("it runs"),
    );
    assert_eq!(shown["default_dispositions"], serde_json::Value::Null);
    assert_eq!(
        shown["kinds"][0]["dispositions"],
        json!(["filed", "wont-fix", "fixed-in-change"])
    );
    assert_eq!(shown["kinds"][0]["dispositions_from"], "catalogue");
    assert_eq!(
        shown["kinds"][1]["dispositions"],
        json!(["filed", "wont-fix"]),
        "a kind's own list stands"
    );

    let empty = directory.path().join("empty.yaml");
    std::fs::write(&empty, "").expect("written");
    let defaults = json(
        &onefollowup(directory.path())
            .args(["--config", empty.to_str().expect("UTF-8"), "kinds"])
            .output()
            .expect("it runs"),
    );
    assert_eq!(
        defaults["kinds"][0]["dispositions"],
        json!([
            "filed",
            "not-reproducible",
            "already-fixed",
            "too-low-impact"
        ])
    );
}

#[test]
fn a_refused_configuration_is_refused_by_kinds_too() {
    let directory = support::tempdir();
    std::fs::write(
        directory.path().join("onefollowup.yaml"),
        TRIAGE.replace("default_kind: root-cause", "default_kind: nothing"),
    )
    .expect("written");
    let output = onefollowup(directory.path())
        .arg("kinds")
        .output()
        .expect("it runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("`default_kind` from the file"));
}
