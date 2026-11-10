//! The configuration's rules, through the library's public resolution: the shipped
//! defaults, the layers' precedence, and every refusal naming its key and its layer.
//! (`crates/onefollowup-e2e` drives the same through the built binary.)

use std::path::{Path, PathBuf};

use onefollowup::config::{self, ConfigError, Inputs, KEYS, Origin, Settings};

fn inputs(directory: &Path) -> Inputs {
    Inputs {
        working_directory: directory.to_path_buf(),
        hostname: Some("test-host".to_owned()),
        ..Inputs::default()
    }
}

fn with_file(text: &str) -> (tempfile::TempDir, Inputs) {
    let directory = tempfile::tempdir().expect("a temporary directory");
    std::fs::write(directory.path().join("onefollowup.yaml"), text).expect("the file is written");
    let inputs = inputs(directory.path());
    (directory, inputs)
}

fn file_origin(directory: &tempfile::TempDir) -> Origin {
    Origin::File(directory.path().join("onefollowup.yaml"))
}

fn refused(text: &str) -> (ConfigError, Origin) {
    let (directory, inputs) = with_file(text);
    let error = config::resolve(&inputs).expect_err("the configuration is refused");
    (error, file_origin(&directory))
}

/// The refusal cases `tests/fixtures/config/refusals.yaml` holds, shared with the e2e suite.
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

fn refusals() -> Refusals {
    let path = format!(
        "{}/tests/fixtures/config/refusals.yaml",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_saphyr::from_str(&std::fs::read_to_string(path).expect("the refusal cases"))
        .expect("the refusal cases are YAML")
}

#[test]
fn the_defaults_are_todays_strategy() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let settings = config::resolve(&inputs(directory.path())).expect("the defaults resolve");
    let config = settings.config();
    assert_eq!(config, &config::defaults("test-host"));
    assert_eq!(config.store, None);
    assert_eq!(config.drafts_source, "drafts");
    assert_eq!(config.destination, "followups");
    assert_eq!(config.namespace, "orchestrator");
    assert_eq!(config.host_label, "test-host");
    assert_eq!(config.estimate.raise_at, 3);
    assert_eq!(config.estimate.raise_cap, config::Priority::High);
    assert!(config.fields.is_empty() && config.relations.is_empty() && config.routes.is_empty());
    assert_eq!(config.hooks, config::Hooks::default());
    assert_eq!(config.default_kind, "root-cause");
    assert_eq!(config.kinds.len(), 1);
    assert_eq!(config.kinds[0].duplicates, config::Duplicates::Agent);
    assert_eq!(config.kinds[0].priority, config::PriorityModel::Estimate);
    assert_eq!(config.kinds[0].initial_status, config::StatusRole::Proposal);
    let names: Vec<&str> = config
        .dispositions
        .iter()
        .map(|one| one.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "filed",
            "not-reproducible",
            "already-fixed",
            "too-low-impact"
        ]
    );
    assert_eq!(
        config.dispositions[0].requires,
        [config::Requirement::Ticket]
    );
    assert!(
        config.dispositions[1..]
            .iter()
            .all(|one| one.requires == [config::Requirement::Reason])
    );
    assert_eq!(config.default_dispositions, None);
    assert_eq!(config.snapshot.launchable_for, "24h");
    let roles: Vec<(&str, &str, bool, bool)> = config
        .statuses
        .iter()
        .iter()
        .map(|(role, mapping)| {
            (
                role.as_str(),
                mapping.category.as_str(),
                mapping.protected,
                mapping.selected,
            )
        })
        .collect();
    assert_eq!(
        roles,
        [
            ("proposal", "backlog", false, false),
            ("accepted", "todo", true, true),
            ("deferred", "draft", true, false),
            ("queued", "queued", true, false),
            ("in-progress", "in-progress", true, false),
            ("done", "done", true, false),
            ("cancelled", "cancelled", false, false),
        ]
    );
    assert!(
        settings
            .sources()
            .iter()
            .all(|(_, origin)| *origin == Origin::Default)
    );
    assert_eq!(settings.sources().len(), KEYS.len());
}

#[test]
fn a_file_overrides_what_it_states_and_leaves_the_rest_defaulted() {
    let (directory, inputs) = with_file("destination: bugs\nestimate: {raise_at: 5}\n");
    let settings = config::resolve(&inputs).expect("the file resolves");
    assert_eq!(settings.config().destination, "bugs");
    assert_eq!(settings.config().estimate.raise_at, 5);
    assert_eq!(settings.config().estimate.raise_cap, config::Priority::High);
    let source = |key: &str| {
        settings
            .sources()
            .iter()
            .find(|(name, _)| name == key)
            .cloned()
    };
    assert_eq!(
        source("destination"),
        Some(("destination".to_owned(), file_origin(&directory)))
    );
    assert_eq!(
        source("namespace"),
        Some(("namespace".to_owned(), Origin::Default))
    );
}

#[test]
fn a_refusal_names_its_layer_in_words() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let mut inputs = inputs(directory.path());
    inputs.environment = vec![("ONEFOLLOWUP_ESTIMATE__RAISE_AT".to_owned(), "x".to_owned())];
    let message = config::resolve(&inputs).expect_err("refused").to_string();
    assert!(message.starts_with("configuration key `estimate.raise_at` from the environment variable ONEFOLLOWUP_ESTIMATE__RAISE_AT: "), "{message}");
    inputs.environment.clear();
    inputs.set = vec!["estimate.raise_at=x".to_owned()];
    let message = config::resolve(&inputs).expect_err("refused").to_string();
    assert!(message.contains("from the flag --set: "), "{message}");
    let (error, _) = refused("estimate: {raise_at: x}\n");
    assert!(error.to_string().contains("from the file "), "{error}");
    let mut broken = config::defaults("h");
    broken.kinds.clear();
    let message = Settings::new(broken).expect_err("refused").to_string();
    assert!(message.contains("from the default: "), "{message}");
}

#[test]
fn an_empty_file_states_nothing() {
    for text in ["", "# nothing\n", "{}\n", "~\n"] {
        let (_directory, inputs) = with_file(text);
        let settings = config::resolve(&inputs).expect("an empty file resolves");
        assert_eq!(
            settings.config(),
            &config::defaults("test-host"),
            "{text:?}"
        );
    }
}

#[test]
fn the_layers_resolve_flag_then_variable_then_file_then_default() {
    let (directory, mut inputs) = with_file("destination: from-file\nnamespace: from-file\n");
    inputs.environment = vec![
        ("ONEFOLLOWUP_DESTINATION".to_owned(), "from-env".to_owned()),
        (
            "ONEFOLLOWUP_DRAFTS_SOURCE".to_owned(),
            "from-env".to_owned(),
        ),
        ("ONEFOLLOWUP_ESTIMATE__RAISE_AT".to_owned(), "7".to_owned()),
    ];
    inputs.set = vec![
        "destination=from-set".to_owned(),
        "store=from-set".to_owned(),
    ];
    inputs.store = Some("from-flag".to_owned());
    let settings = config::resolve(&inputs).expect("the layers resolve");
    let config = settings.config();
    assert_eq!(config.destination, "from-set");
    assert_eq!(config.drafts_source, "from-env");
    assert_eq!(config.namespace, "from-file");
    assert_eq!(config.store.as_deref(), Some("from-flag"));
    assert_eq!(config.estimate.raise_at, 7);
    let sources: Vec<(String, Origin)> = settings.sources().to_vec();
    let of = |key: &str| {
        sources
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, origin)| origin.clone())
    };
    assert_eq!(of("destination"), Some(Origin::Flag("--set".to_owned())));
    assert_eq!(of("store"), Some(Origin::Flag("--store".to_owned())));
    assert_eq!(
        of("drafts_source"),
        Some(Origin::Env("ONEFOLLOWUP_DRAFTS_SOURCE".to_owned()))
    );
    assert_eq!(
        of("estimate"),
        Some(Origin::Env("ONEFOLLOWUP_ESTIMATE__RAISE_AT".to_owned()))
    );
    assert_eq!(of("namespace"), Some(file_origin(&directory)));
}

#[test]
fn the_file_is_found_by_flag_then_variable_then_working_directory() {
    let (directory, mut inputs) = with_file("destination: cwd\n");
    let named = |name: &str, text: &str| -> PathBuf {
        let path = directory.path().join(name);
        std::fs::write(&path, text).expect("written");
        path
    };
    let variable = named("variable.yaml", "destination: variable\n");
    let flag = named("flag.yaml", "destination: flag\n");
    let destination = |inputs: &Inputs| {
        config::resolve(inputs)
            .expect("resolves")
            .config()
            .destination
            .clone()
    };
    assert_eq!(destination(&inputs), "cwd");
    inputs.environment = vec![(
        "ONEFOLLOWUP_CONFIG".to_owned(),
        variable.display().to_string(),
    )];
    assert_eq!(destination(&inputs), "variable");
    inputs.config_file = Some(flag);
    assert_eq!(destination(&inputs), "flag");
    inputs.config_file = Some(directory.path().join("missing.yaml"));
    let error = config::resolve(&inputs).expect_err("a named file that is missing is refused");
    assert_eq!(
        error.origin,
        Origin::File(directory.path().join("missing.yaml"))
    );
    inputs.config_file = None;
    inputs.environment = vec![("ONEFOLLOWUP_CONFIG".to_owned(), String::new())];
    let error = config::resolve(&inputs).expect_err("an empty variable is refused");
    assert_eq!(error.origin, Origin::Env("ONEFOLLOWUP_CONFIG".to_owned()));
}

#[test]
fn an_unknown_key_or_a_wrong_type_is_refused_naming_the_key_and_the_layer() {
    let (error, origin) = refused("destinaton: typo\n");
    assert_eq!((error.key.as_str(), error.origin), ("destinaton", origin));
    let (error, origin) = refused("estimate: {raise_at: many}\n");
    assert_eq!(
        (error.key.as_str(), error.origin),
        ("estimate.raise_at", origin)
    );
    let (error, origin) = refused("kinds: {name: root-cause}\n");
    assert_eq!((error.key.as_str(), error.origin), ("kinds", origin));
    let (error, origin) = refused("estimate: {raise_soon: 1}\n");
    assert_eq!(
        (error.key.as_str(), error.origin),
        ("estimate.raise_soon", origin)
    );

    let directory = tempfile::tempdir().expect("a temporary directory");
    let mut inputs = inputs(directory.path());
    inputs.environment = vec![("ONEFOLLOWUP_NOT_A_KEY".to_owned(), "1".to_owned())];
    let error = config::resolve(&inputs).expect_err("refused");
    assert_eq!(
        (error.key.as_str(), error.origin),
        ("not_a_key", Origin::Env("ONEFOLLOWUP_NOT_A_KEY".to_owned()))
    );
    inputs.environment = vec![("ONEFOLLOWUP_HOOKS".to_owned(), "[1".to_owned())];
    let error = config::resolve(&inputs).expect_err("refused");
    assert_eq!(error.origin, Origin::Env("ONEFOLLOWUP_HOOKS".to_owned()));

    let mut inputs = self::inputs(directory.path());
    inputs.set = vec!["kinds=7".to_owned()];
    let error = config::resolve(&inputs).expect_err("refused");
    assert_eq!(
        (error.key.as_str(), error.origin),
        ("kinds", Origin::Flag("--set".to_owned()))
    );
    inputs.set = vec!["no-equals".to_owned()];
    assert_eq!(
        config::resolve(&inputs).expect_err("refused").origin,
        Origin::Flag("--set".to_owned())
    );

    for (text, why) in [("- a list\n", "a mapping"), ("{", "YAML")] {
        let (error, origin) = refused(text);
        assert_eq!(error.origin, origin);
        assert!(error.message.contains(why), "{error}");
    }
}

#[test]
fn every_rule_refuses_naming_its_key_and_its_layer() {
    let refusals = refusals();
    assert!(refusals.cases.len() > 40);
    for case in &refusals.cases {
        let document = if case.over_base {
            format!("{}{}", refusals.base, case.document)
        } else {
            case.document.clone()
        };
        let (error, origin) = refused(&document);
        assert_eq!(error.key, case.key, "{document}: {error}");
        assert_eq!(error.origin, origin, "{document}: {error}");
        assert!(error.message.contains(&case.message), "{document}: {error}");
    }
}

#[test]
fn a_full_triage_configuration_resolves() {
    let (_directory, inputs) = with_file(&format!(
        "{}
relations:
  - {{name: of-error, from: client-error, to: error, cardinality: many-to-one, decided_by: {{match: [error_code]}}, store: parent, rollup: occurrences}}
  - {{name: caused-by, from: error, to: root-cause, cardinality: many-to-many, decided_by: agent, store: link, rollup: max}}
dispositions:
  - {{name: filed, description: Filed., requires: [ticket]}}
  - {{name: fixed-in-change, description: Fixed., requires: [change]}}
  - {{name: wont-fix, description: No., requires: [reason]}}
default_dispositions: [filed, wont-fix]
routes:
  - {{name: on-call, match: {{kind: [error], severity: [critical], repository: ['github.com/example/*'], path: ['src/**'], relations: {{of-error: {{min: 2}}}}}}, set: {{status_role: accepted, priority: urgent, labels: [incident], assignee: alice}}, command: [rotation, --today]}}
hooks: {{batch_preparing: [inputs, --json], batch_settled: [notify]}}
",
        refusals().base
    ));
    let settings: Settings = config::resolve(&inputs).expect("a full configuration resolves");
    let written = serde_json::to_value(settings.config()).expect("the configuration serializes");
    assert_eq!(
        written["kinds"][1]["duplicates"],
        serde_json::json!({"same": ["error_code"]})
    );
    assert_eq!(
        written["kinds"][3]["priority"],
        serde_json::json!({"fixed": "urgent"})
    );
    assert_eq!(
        written["relations"][0]["decided_by"],
        serde_json::json!({"match": ["error_code"]})
    );
    assert_eq!(
        written["routes"][0]["match"]["relations"],
        serde_json::json!({"of-error": {"min": 2}})
    );
    let reread: onefollowup::Config = serde_json::from_value(written).expect("and reads back");
    assert_eq!(&reread, settings.config());
    let kinds = onefollowup::api::kinds(&settings, onefollowup::api::NoArgs {}).expect("kinds");
    assert_eq!(kinds.kinds[0].dispositions, ["filed", "wont-fix"]);
    assert_eq!(kinds.kinds[0].dispositions_from, "default_dispositions");
    assert_eq!(settings.config().relations.len(), 2);
    assert_eq!(
        settings.config().routes[0].set.assignee.as_deref(),
        Some("alice")
    );
    let again =
        Settings::new(settings.config().clone()).expect("a resolved configuration is valid");
    assert_eq!(again.config(), settings.config());
    let mut broken = settings.config().clone();
    broken.default_kind = "missing".to_owned();
    let error =
        Settings::new(broken).expect_err("a configuration given directly is held to the rules");
    assert_eq!(
        (error.key.as_str(), error.origin),
        ("default_kind", Origin::Default)
    );
}
