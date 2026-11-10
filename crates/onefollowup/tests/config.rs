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

/// The configuration a refusal case starts from: two declared fields, kinds of both
/// `duplicates` forms, so each case changes one thing.
const BASE: &str = r"
fields:
  error_code: {description: The error's code.}
  client: {description: The client's id.}
kinds:
  - {name: root-cause, description: One root cause.}
  - {name: error, description: One error code., duplicates: {same: [error_code]}}
  - {name: client-error, description: One client's error., duplicates: {same: [error_code, client]}}
  - {name: incident, description: An incident., priority: {fixed: urgent}}
";

fn refused_over_base(extra: &str) -> (ConfigError, Origin) {
    refused(&format!("{BASE}{extra}"))
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
    let cases: &[(&str, &str, &str)] = &[
        (
            "default_kind: nothing\n",
            "default_kind",
            "names no configured kind",
        ),
        ("kinds: []\ndefault_kind: x\n", "kinds", "names no kind"),
        (
            "routes: [{name: r, match: {kind: [nothing]}}]\n",
            "routes[0].match.kind",
            "names no configured kind",
        ),
        (
            "routes: [{name: r, set: {status_role: nothing}}]\n",
            "routes[0].set.status_role",
            "unknown variant",
        ),
        (
            "routes: [{name: r, match: {relations: {nothing: {min: 1}}}}]\n",
            "routes[0].match.relations",
            "names no configured relation",
        ),
        (
            "routes: [{name: r, match: {severity: [dire]}}]\n",
            "routes[0].match.severity",
            "names no rubric level",
        ),
        (
            "routes: [{name: r, set: {assignees: [alice]}}]\n",
            "routes[0].set.assignees",
            "one assignee",
        ),
        (
            "routes: [{name: r, command: []}]\n",
            "routes[0].command",
            "empty argv",
        ),
        (
            "routes: [{name: r, set: {assignee: ' '}}]\n",
            "routes[0].set.assignee",
            "names nobody",
        ),
        ("routes: [{name: r}, {name: r}]\n", "routes", "twice"),
        (
            "dispositions: [{name: d, description: D, requires: [ticket, nothing]}]\ndefault_kind: root-cause\n",
            "dispositions[0].requires[1]",
            "unknown variant",
        ),
        (
            "dispositions: [{name: d, description: D, requires: [ticket], files_ticket: true}]\n",
            "dispositions[0].files_ticket",
            "requires: [ticket]",
        ),
        (
            "dispositions: []\n",
            "dispositions",
            "catalogues no disposition",
        ),
        (
            "kinds: [{name: k, description: K, identity: x}]\ndefault_kind: k\n",
            "kinds[0].identity",
            "not a kind key",
        ),
        (
            "kinds: [{name: k, description: K, dedupe: x}]\ndefault_kind: k\n",
            "kinds[0].dedupe",
            "not a kind key",
        ),
        (
            "kinds: [{name: k, description: K, classification: x}]\ndefault_kind: k\n",
            "kinds[0].classification",
            "not a kind key",
        ),
        (
            "kinds: [{name: k, description: K, purpose: x}]\ndefault_kind: k\n",
            "kinds[0].purpose",
            "not a kind key",
        ),
        (
            "kinds: [{name: k, description: K, duplicates: judge}]\ndefault_kind: k\n",
            "kinds[0].duplicates",
            "`duplicates` is `agent`",
        ),
        (
            "kinds: [{name: k, description: K, duplicates: {same: x}}]\ndefault_kind: k\n",
            "kinds[0].duplicates",
            "list of names",
        ),
        (
            "kinds: [{name: k, description: K, duplicates: {same: []}}]\ndefault_kind: k\n",
            "kinds[0].duplicates.same",
            "is empty",
        ),
        (
            "kinds: [{name: k, description: K, duplicates: {same: [undeclared]}}]\ndefault_kind: k\n",
            "kinds[0].duplicates.same",
            "does not declare",
        ),
        (
            "kinds: [{name: k, description: K, priority: {model: guess}}]\ndefault_kind: k\n",
            "kinds[0].priority",
            "`priority` is",
        ),
        (
            "kinds: [{name: k, description: K, priority: {fixed: soon}}]\ndefault_kind: k\n",
            "kinds[0].priority",
            "`priority` is",
        ),
        (
            "kinds: [{name: k, description: K}, {name: k, description: K}]\ndefault_kind: k\n",
            "kinds",
            "twice",
        ),
        (
            "kinds: [{name: k, description: K, dispositions: []}]\ndefault_kind: k\n",
            "kinds[0].dispositions",
            "is empty",
        ),
        (
            "kinds: [{name: k, description: K, dispositions: [nothing]}]\ndefault_kind: k\n",
            "kinds[0].dispositions",
            "does not catalogue",
        ),
        (
            "fields: {code: {}}\n",
            "fields.code",
            "missing field `description`",
        ),
        (
            "fields: {code: {description: ' '}}\n",
            "fields.code.description",
            "is empty",
        ),
        (
            "review_sink_command: [x]\n",
            "review_sink_command",
            "`hooks.batch_settled` hook replaced it",
        ),
        (
            "inputs_command: [x]\n",
            "inputs_command",
            "`hooks.batch_preparing` hook replaced it",
        ),
        (
            "hooks: {batch_started: [x]}\n",
            "hooks.batch_started",
            "unknown field",
        ),
        (
            "hooks: {batch_settled: []}\n",
            "hooks.batch_settled",
            "empty argv",
        ),
        (
            "default_dispositions: []\n",
            "default_dispositions",
            "is empty",
        ),
        (
            "default_dispositions: [nothing]\n",
            "default_dispositions",
            "does not catalogue",
        ),
        (
            "relations: [{name: r, from: nothing, to: root-cause, cardinality: many-to-one, decided_by: agent, store: link}]\n",
            "relations[0].from",
            "names no configured kind",
        ),
        (
            "relations: [{name: r, from: root-cause, to: nothing, cardinality: many-to-one, decided_by: agent, store: link}]\n",
            "relations[0].to",
            "names no configured kind",
        ),
        (
            "relations: [{name: r, from: root-cause, to: root-cause, cardinality: many-to-one, decided_by: guess, store: link}]\n",
            "relations[0].decided_by",
            "`decided_by` is",
        ),
        (
            "relations: [{name: r, from: root-cause, to: root-cause, cardinality: many-to-one, decided_by: agent, store: link}, {name: r, from: root-cause, to: root-cause, cardinality: many-to-one, decided_by: agent, store: link}]\n",
            "relations",
            "twice",
        ),
        (
            "statuses: {done: {category: finished, description: D, protected: true, selected: false}}\n",
            "statuses.done.category",
            "not a store status category",
        ),
        (
            "rubric: {levels: []}\n",
            "rubric.levels",
            "names no severity level",
        ),
        (
            "snapshot: {launchable_for: soon}\n",
            "snapshot.launchable_for",
            "not a duration",
        ),
        (
            "batch_id_pattern: '('\n",
            "batch_id_pattern",
            "not a regular expression",
        ),
    ];
    for (text, key, message) in cases {
        let (error, origin) = refused(text);
        assert_eq!(error.key, *key, "{text}: {error}");
        assert_eq!(error.origin, origin, "{text}: {error}");
        assert!(error.message.contains(message), "{text}: {error}");
    }

    let relation = |rest: &str| format!("relations: [{{name: r, {rest}}}]\n");
    let over_base: &[(String, &str, &str)] = &[
        (
            relation(
                "from: error, to: root-cause, cardinality: many-to-many, decided_by: agent, store: parent",
            ),
            "relations[0].store",
            "many-to-one",
        ),
        (
            relation(
                "from: client-error, to: error, cardinality: many-to-many, decided_by: {match: [error_code]}, store: link",
            ),
            "relations[0].decided_by",
            "many-to-one",
        ),
        (
            relation(
                "from: root-cause, to: error, cardinality: many-to-one, decided_by: {match: [error_code]}, store: link",
            ),
            "relations[0].decided_by",
            "both find duplicates by `same`",
        ),
        (
            relation(
                "from: client-error, to: root-cause, cardinality: many-to-one, decided_by: {match: [error_code]}, store: link",
            ),
            "relations[0].decided_by",
            "both find duplicates by `same`",
        ),
        (
            relation(
                "from: client-error, to: error, cardinality: many-to-one, decided_by: {match: [client]}, store: link",
            ),
            "relations[0].decided_by.match",
            "exactly the `to` kind's `same` fields",
        ),
        (
            relation(
                "from: error, to: client-error, cardinality: many-to-one, decided_by: {match: [error_code, client]}, store: link",
            ),
            "relations[0].decided_by.match",
            "all among the `from` kind's",
        ),
        (
            relation(
                "from: error, to: incident, cardinality: many-to-one, decided_by: agent, store: link, rollup: max",
            ),
            "relations[0].rollup",
            "priority is fixed",
        ),
    ];
    for (text, key, message) in over_base {
        let (error, origin) = refused_over_base(text);
        assert_eq!(error.key, *key, "{text}: {error}");
        assert_eq!(error.origin, origin, "{text}: {error}");
        assert!(error.message.contains(message), "{text}: {error}");
    }
}

#[test]
fn a_full_triage_configuration_resolves() {
    let (_directory, inputs) = with_file(&format!(
        "{BASE}
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
"
    ));
    let settings: Settings = config::resolve(&inputs).expect("a full configuration resolves");
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
