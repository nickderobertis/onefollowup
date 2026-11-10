//! `onefollowup config show` through the built binary: the shipped defaults, where the
//! configuration file is found, every key's precedence — flag, then `ONEFOLLOWUP_`
//! variable, then file, then default — with `--sources` naming each layer, and every
//! refusal naming its key and its layer.

mod support;

use std::path::{Path, PathBuf};

use onefollowup::config::KEYS;
use serde_json::{Value, json};
use support::{json, onefollowup};

fn show(directory: &Path, environment: &[(&str, &str)], arguments: &[&str]) -> Value {
    let mut command = onefollowup(directory);
    command
        .envs(environment.iter().copied())
        .args(arguments)
        .args(["config", "show", "--sources"]);
    json(&command.output().expect("it runs"))
}

fn refusal(directory: &Path, environment: &[(&str, &str)], arguments: &[&str]) -> String {
    let output = onefollowup(directory)
        .envs(environment.iter().copied())
        .args(arguments)
        .args(["config", "show"])
        .output()
        .expect("it runs");
    assert_eq!(output.status.code(), Some(2), "{arguments:?} is refused");
    assert!(output.stdout.is_empty());
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn at<'a>(value: &'a Value, pointer: &str) -> &'a Value {
    value
        .pointer(pointer)
        .unwrap_or_else(|| panic!("{pointer} is in {value}"))
}

#[test]
fn with_nothing_configured_it_prints_every_default() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let shown = show(directory.path(), &[], &[]);
    assert_eq!(shown["schema_version"], 1);
    let config = &shown["config"];
    let keys: Vec<&str> = config
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, KEYS, "every key, in order");
    let hostname = gethostname::gethostname().to_string_lossy().into_owned();
    for (key, expected) in [
        ("store", Value::Null),
        ("drafts_source", json!("drafts")),
        ("destination", json!("followups")),
        ("namespace", json!("orchestrator")),
        ("host_label", json!(hostname)),
        ("batch_id_pattern", json!("[\\w.@+-]*[\\w@+-][\\w.@+-]*")),
        ("estimate", json!({"raise_at": 3, "raise_cap": "high"})),
        ("task_template", json!("builtin:task-template.md.j2")),
        ("persona", json!("builtin:persona.yaml")),
        ("fields", json!({})),
        ("default_kind", json!("root-cause")),
        ("relations", json!([])),
        ("default_dispositions", Value::Null),
        ("routes", json!([])),
        ("hooks", json!({})),
        ("snapshot", json!({"launchable_for": "24h"})),
    ] {
        assert_eq!(config[key], expected, "{key}");
    }
    let kinds = config["kinds"].as_array().expect("a list");
    assert_eq!(kinds.len(), 1);
    assert_eq!(kinds[0]["name"], "root-cause");
    assert_eq!(kinds[0]["duplicates"], "agent");
    assert_eq!(kinds[0]["priority"], json!({"model": "estimate"}));
    assert_eq!(kinds[0]["initial_status"], "proposal");
    assert!(kinds[0].get("dispositions").is_none());
    let dispositions: Vec<(&Value, &Value)> = config["dispositions"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|one| (&one["name"], &one["requires"]))
        .collect();
    assert_eq!(
        dispositions,
        [
            (&json!("filed"), &json!(["ticket"])),
            (&json!("not-reproducible"), &json!(["reason"])),
            (&json!("already-fixed"), &json!(["reason"])),
            (&json!("too-low-impact"), &json!(["reason"])),
        ]
    );
    let categories: Vec<&Value> = [
        "proposal",
        "accepted",
        "deferred",
        "queued",
        "in-progress",
        "done",
        "cancelled",
    ]
    .iter()
    .map(|role| &config["statuses"][role]["category"])
    .collect();
    assert_eq!(
        categories,
        [
            &json!("backlog"),
            &json!("todo"),
            &json!("draft"),
            &json!("queued"),
            &json!("in-progress"),
            &json!("done"),
            &json!("cancelled")
        ]
    );
    let levels: Vec<&Value> = config["rubric"]["levels"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|one| &one["name"])
        .collect();
    assert_eq!(
        levels,
        [
            &json!("critical"),
            &json!("high"),
            &json!("medium"),
            &json!("low")
        ]
    );
    for key in KEYS {
        assert_eq!(shown["sources"][key], json!({"layer": "default"}), "{key}");
    }

    let plain = json(
        &onefollowup(directory.path())
            .args(["config", "show"])
            .output()
            .expect("it runs"),
    );
    assert!(
        plain.get("sources").is_none(),
        "--sources alone adds the sources"
    );
    assert_eq!(plain["config"], shown["config"]);
}

#[test]
fn a_file_overrides_its_keys_and_is_found_by_flag_then_variable_then_working_directory() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let write = |name: &str, text: &str| -> PathBuf {
        let path = directory.path().join(name);
        std::fs::write(&path, text).expect("written");
        path
    };
    let here = write("onefollowup.yaml", "destination: here\nnamespace: acme\n");
    let shown = show(directory.path(), &[], &[]);
    assert_eq!(shown["config"]["destination"], "here");
    assert_eq!(shown["config"]["namespace"], "acme");
    assert_eq!(
        shown["config"]["drafts_source"], "drafts",
        "the rest stay defaulted"
    );
    assert_eq!(
        shown["sources"]["destination"],
        json!({"layer": "file", "name": here})
    );
    assert_eq!(
        shown["sources"]["drafts_source"],
        json!({"layer": "default"})
    );

    let variable = write("variable.yaml", "destination: variable\n");
    let variable_text = variable.display().to_string();
    let shown = show(
        directory.path(),
        &[("ONEFOLLOWUP_CONFIG", &variable_text)],
        &[],
    );
    assert_eq!(
        shown["config"]["destination"], "variable",
        "ONEFOLLOWUP_CONFIG beats the working directory's"
    );
    assert_eq!(
        shown["config"]["namespace"], "orchestrator",
        "the working directory's file is not read"
    );

    let flag = write("flag.yaml", "destination: flag\n");
    let flag_text = flag.display().to_string();
    let shown = show(
        directory.path(),
        &[("ONEFOLLOWUP_CONFIG", &variable_text)],
        &["--config", &flag_text],
    );
    assert_eq!(
        shown["config"]["destination"], "flag",
        "--config beats both"
    );
    assert_eq!(
        shown["sources"]["destination"],
        json!({"layer": "file", "name": flag})
    );

    let missing = directory.path().join("missing.yaml").display().to_string();
    let stderr = refusal(directory.path(), &[], &["--config", &missing]);
    assert!(
        stderr.contains(&format!("from the file {missing}: could not be read")),
        "{stderr}"
    );
}

/// One key's precedence case: what a file, a variable and a `--set` each state for it, where
/// in the configuration the effect shows, and what that is under each layer. `base` is
/// whatever else the file needs for the value to be valid.
struct Case {
    base: &'static str,
    file: &'static str,
    variable: (&'static str, &'static str),
    set: &'static str,
    pointer: &'static str,
    values: [Value; 3],
}

fn case(key: &str) -> Option<Case> {
    let simple = |file, variable, set, pointer, values| Case {
        base: "",
        file,
        variable,
        set,
        pointer,
        values,
    };
    Some(match key {
        "store" => simple(
            "store: file.yaml",
            ("ONEFOLLOWUP_STORE", "env.yaml"),
            "store=set.yaml",
            "/store",
            [json!("file.yaml"), json!("env.yaml"), json!("set.yaml")],
        ),
        "drafts_source" => simple(
            "drafts_source: f",
            ("ONEFOLLOWUP_DRAFTS_SOURCE", "e"),
            "drafts_source=s",
            "/drafts_source",
            [json!("f"), json!("e"), json!("s")],
        ),
        "destination" => simple(
            "destination: f",
            ("ONEFOLLOWUP_DESTINATION", "e"),
            "destination=s",
            "/destination",
            [json!("f"), json!("e"), json!("s")],
        ),
        "namespace" => simple(
            "namespace: f",
            ("ONEFOLLOWUP_NAMESPACE", "e"),
            "namespace=s",
            "/namespace",
            [json!("f"), json!("e"), json!("s")],
        ),
        "host_label" => simple(
            "host_label: f",
            ("ONEFOLLOWUP_HOST_LABEL", "e"),
            "host_label=s",
            "/host_label",
            [json!("f"), json!("e"), json!("s")],
        ),
        "batch_id_pattern" => simple(
            "batch_id_pattern: 'f+'",
            ("ONEFOLLOWUP_BATCH_ID_PATTERN", "e+"),
            "batch_id_pattern=s+",
            "/batch_id_pattern",
            [json!("f+"), json!("e+"), json!("s+")],
        ),
        "statuses" => simple(
            "statuses: {done: {description: f}}",
            ("ONEFOLLOWUP_STATUSES__DONE__DESCRIPTION", "e"),
            "statuses.done.description=s",
            "/statuses/done/description",
            [json!("f"), json!("e"), json!("s")],
        ),
        "rubric" => simple(
            "rubric: {no_workaround: f}",
            ("ONEFOLLOWUP_RUBRIC__NO_WORKAROUND", "e"),
            "rubric.no_workaround=s",
            "/rubric/no_workaround",
            [json!("f"), json!("e"), json!("s")],
        ),
        "estimate" => simple(
            "estimate: {raise_at: 4}",
            ("ONEFOLLOWUP_ESTIMATE__RAISE_AT", "5"),
            "estimate.raise_at=6",
            "/estimate/raise_at",
            [json!(4), json!(5), json!(6)],
        ),
        "task_template" => simple(
            "task_template: f.j2",
            ("ONEFOLLOWUP_TASK_TEMPLATE", "e.j2"),
            "task_template=s.j2",
            "/task_template",
            [json!("f.j2"), json!("e.j2"), json!("s.j2")],
        ),
        "persona" => simple(
            "persona: f.yaml",
            ("ONEFOLLOWUP_PERSONA", "e.yaml"),
            "persona=s.yaml",
            "/persona",
            [json!("f.yaml"), json!("e.yaml"), json!("s.yaml")],
        ),
        "fields" => simple(
            "fields: {code: {description: f}}",
            ("ONEFOLLOWUP_FIELDS__CODE__DESCRIPTION", "e"),
            "fields.code.description=s",
            "/fields/code/description",
            [json!("f"), json!("e"), json!("s")],
        ),
        "kinds" => simple(
            "kinds: [{name: root-cause, description: f}]",
            ("ONEFOLLOWUP_KINDS", "[{name: root-cause, description: e}]"),
            "kinds=[{name: root-cause, description: s}]",
            "/kinds/0/description",
            [json!("f"), json!("e"), json!("s")],
        ),
        "default_kind" => Case {
            base: "kinds: [{name: root-cause, description: R}, {name: f, description: F}, {name: e, description: E}, {name: s, description: S}]\n",
            file: "default_kind: f",
            variable: ("ONEFOLLOWUP_DEFAULT_KIND", "e"),
            set: "default_kind=s",
            pointer: "/default_kind",
            values: [json!("f"), json!("e"), json!("s")],
        },
        "relations" => simple(
            "relations: [{name: f, from: root-cause, to: root-cause, cardinality: many-to-one, decided_by: agent, store: link}]",
            (
                "ONEFOLLOWUP_RELATIONS",
                "[{name: e, from: root-cause, to: root-cause, cardinality: many-to-one, decided_by: agent, store: link}]",
            ),
            "relations=[{name: s, from: root-cause, to: root-cause, cardinality: many-to-one, decided_by: agent, store: link}]",
            "/relations/0/name",
            [json!("f"), json!("e"), json!("s")],
        ),
        "dispositions" => simple(
            "dispositions: [{name: f, description: F, requires: [ticket]}]",
            (
                "ONEFOLLOWUP_DISPOSITIONS",
                "[{name: e, description: E, requires: [reason]}]",
            ),
            "dispositions=[{name: s, description: S, requires: [change]}]",
            "/dispositions/0/name",
            [json!("f"), json!("e"), json!("s")],
        ),
        "default_dispositions" => simple(
            "default_dispositions: [filed]",
            ("ONEFOLLOWUP_DEFAULT_DISPOSITIONS", "[not-reproducible]"),
            "default_dispositions=[already-fixed]",
            "/default_dispositions",
            [
                json!(["filed"]),
                json!(["not-reproducible"]),
                json!(["already-fixed"]),
            ],
        ),
        "routes" => simple(
            "routes: [{name: f}]",
            ("ONEFOLLOWUP_ROUTES", "[{name: e}]"),
            "routes=[{name: s}]",
            "/routes/0/name",
            [json!("f"), json!("e"), json!("s")],
        ),
        "hooks" => simple(
            "hooks: {batch_settled: [f]}",
            ("ONEFOLLOWUP_HOOKS__BATCH_SETTLED", "[e]"),
            "hooks.batch_settled=[s]",
            "/hooks/batch_settled",
            [json!(["f"]), json!(["e"]), json!(["s"])],
        ),
        "snapshot" => simple(
            "snapshot: {launchable_for: 1h}",
            ("ONEFOLLOWUP_SNAPSHOT__LAUNCHABLE_FOR", "2h"),
            "snapshot.launchable_for=3h",
            "/snapshot/launchable_for",
            [json!("1h"), json!("2h"), json!("3h")],
        ),
        _ => return None,
    })
}

#[test]
fn every_key_resolves_flag_then_variable_then_file_then_default() {
    let missing: Vec<&str> = KEYS
        .iter()
        .copied()
        .filter(|key| case(key).is_none())
        .collect();
    assert!(
        missing.is_empty(),
        "every key the configuration declares has a precedence case; none for {missing:?}"
    );
    for key in KEYS {
        let case = case(key).expect("checked above");
        let directory = tempfile::tempdir().expect("a temporary directory");
        let file = directory.path().join("onefollowup.yaml");
        std::fs::write(&file, format!("{}{}\n", case.base, case.file)).expect("written");
        let pointer = format!("/config{}", case.pointer);
        let source = format!("/sources/{key}");

        let shown = show(directory.path(), &[], &[]);
        assert_eq!(
            at(&shown, &pointer),
            &case.values[0],
            "{key}: the file beats the default"
        );
        assert_eq!(
            at(&shown, &source),
            &json!({"layer": "file", "name": file}),
            "{key}"
        );

        let variables = [case.variable];
        let shown = show(directory.path(), &variables, &[]);
        assert_eq!(
            at(&shown, &pointer),
            &case.values[1],
            "{key}: the variable beats the file"
        );
        assert_eq!(
            at(&shown, &source),
            &json!({"layer": "env", "name": case.variable.0}),
            "{key}"
        );

        let shown = show(directory.path(), &variables, &["--set", case.set]);
        assert_eq!(
            at(&shown, &pointer),
            &case.values[2],
            "{key}: --set beats the variable"
        );
        assert_eq!(
            at(&shown, &source),
            &json!({"layer": "flag", "name": "--set"}),
            "{key}"
        );
    }
}

#[test]
fn the_dedicated_flags_beat_set_for_their_keys() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    for (key, flag) in [
        ("destination", "--destination"),
        ("drafts_source", "--drafts-source"),
        ("store", "--store"),
    ] {
        let assignment = format!("{key}=from-set");
        let shown = show(
            directory.path(),
            &[(&format!("ONEFOLLOWUP_{}", key.to_uppercase()), "from-env")],
            &["--set", &assignment, flag, "from-flag"],
        );
        assert_eq!(shown["config"][key], "from-flag", "{flag} beats --set");
        assert_eq!(
            shown["sources"][key],
            json!({"layer": "flag", "name": flag})
        );
    }
}

#[test]
fn an_unknown_key_or_a_wrongly_typed_value_is_refused_naming_the_key_and_the_layer() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let file = directory.path().join("onefollowup.yaml");
    let file_layer = format!("from the file {}", file.display());
    for (text, key) in [
        ("destinaton: typo\n", "destinaton"),
        ("estimate: {raise_soon: 1}\n", "estimate.raise_soon"),
        ("estimate: {raise_at: many}\n", "estimate.raise_at"),
        ("kinds: {name: root-cause}\n", "kinds"),
        ("hooks: {batch_settled: notify}\n", "hooks.batch_settled"),
    ] {
        std::fs::write(&file, text).expect("written");
        let stderr = refusal(directory.path(), &[], &[]);
        assert!(
            stderr.contains(&format!("`{key}` {file_layer}")),
            "{text}: {stderr}"
        );
    }
    std::fs::remove_file(&file).expect("removed");
    for (variable, value, key) in [
        ("ONEFOLLOWUP_DESTINATON", "typo", "destinaton"),
        (
            "ONEFOLLOWUP_ESTIMATE__RAISE_AT",
            "many",
            "estimate.raise_at",
        ),
        ("ONEFOLLOWUP_KINDS", "7", "kinds"),
    ] {
        let stderr = refusal(directory.path(), &[(variable, value)], &[]);
        assert!(
            stderr.contains(&format!("`{key}` from the environment variable {variable}")),
            "{variable}: {stderr}"
        );
    }
    for (assignment, key) in [
        ("destinaton=typo", "destinaton"),
        ("estimate.raise_cap=soon", "estimate.raise_cap"),
        ("snapshot=1h", "snapshot"),
    ] {
        let stderr = refusal(directory.path(), &[], &["--set", assignment]);
        assert!(
            stderr.contains(&format!("`{key}` from the flag --set")),
            "{assignment}: {stderr}"
        );
    }
}

/// The refusal cases the library's own tests read too.
#[derive(serde::Deserialize)]
struct Refusals {
    base: String,
    cases: Vec<Refusal>,
}

#[derive(serde::Deserialize)]
struct Refusal {
    #[serde(default)]
    over_base: bool,
    document: String,
    key: String,
    message: String,
}

#[test]
fn every_rule_refuses_naming_its_key_and_the_file() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../onefollowup/tests/fixtures/config/refusals.yaml");
    let refusals: Refusals =
        serde_saphyr::from_str(&std::fs::read_to_string(path).expect("the cases")).expect("YAML");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let file = directory.path().join("onefollowup.yaml");
    for case in &refusals.cases {
        let document = if case.over_base {
            format!("{}{}", refusals.base, case.document)
        } else {
            case.document.clone()
        };
        std::fs::write(&file, &document).expect("written");
        let stderr = refusal(directory.path(), &[], &[]);
        assert!(
            stderr.starts_with(&format!(
                "onefollowup: configuration key `{}` from the file {}: ",
                case.key,
                file.display()
            )),
            "{document}: {stderr}"
        );
        assert!(stderr.contains(&case.message), "{document}: {stderr}");
    }
}
