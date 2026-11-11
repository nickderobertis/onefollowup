//! Resolving the effective configuration: flag, then environment variable, then file, then
//! default, the first layer that states a key winning.
//!
//! Each layer is flattened into settings — a key path and the value stated there, a mapping
//! recursed into key by key, a list or a scalar taken whole — so a file stating
//! `estimate.raise_at` leaves `estimate.raise_cap` at its default. The merged settings are
//! read into [`Config`]; every refusal names the key and the layer that stated it.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use super::types::{Config, KEYS};
use super::validate;

/// The configuration file's name, looked for in the working directory.
pub const FILE_NAME: &str = "onefollowup.yaml";
/// The environment variable naming the configuration file.
pub const CONFIG_VARIABLE: &str = "ONEFOLLOWUP_CONFIG";
/// The prefix of every configuration variable.
pub const VARIABLE_PREFIX: &str = "ONEFOLLOWUP_";

/// The shipped defaults, embedded so the installed binary carries them.
const DEFAULTS: &str = include_str!("../../defaults/onefollowup.yaml");

/// Keys a configuration once had, each with the hook that replaced it.
const REPLACED_KEYS: &[(&str, &str)] = &[
    ("review_sink_command", "hooks.batch_settled"),
    ("inputs_command", "hooks.batch_preparing"),
];

/// Where one effective value came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "layer", content = "name", rename_all = "kebab-case")]
pub enum Origin {
    /// The shipped default.
    Default,
    /// The configuration file at this path.
    File(PathBuf),
    /// This environment variable.
    Env(String),
    /// This command-line flag.
    Flag(String),
}

impl fmt::Display for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => formatter.write_str("the default"),
            Self::File(path) => write!(formatter, "the file {}", path.display()),
            Self::Env(name) => write!(formatter, "the environment variable {name}"),
            Self::Flag(flag) => write!(formatter, "the flag {flag}"),
        }
    }
}

/// A configuration refused, naming the key and the layer that stated it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("configuration key `{key}` from {origin}: {message}")]
pub struct ConfigError {
    /// The key, as a dotted path (`kinds[0].duplicates`).
    pub key: String,
    /// The layer that stated it.
    pub origin: Origin,
    /// What is wrong, and what to do.
    pub message: String,
}

/// Everything the resolution reads besides the shipped defaults.
///
/// The CLI fills this from its flags and the process; a library caller may fill it
/// directly, or skip resolution altogether and hand a [`Config`] to [`Settings::new`].
#[derive(Debug, Clone, Default)]
pub struct Inputs {
    /// `--config`: the configuration file, over `ONEFOLLOWUP_CONFIG` and the working directory.
    pub config_file: Option<PathBuf>,
    /// `--set KEY=VALUE`, in order; a later one for the same key wins.
    pub set: Vec<String>,
    /// `--destination`, over any `--set destination=`.
    pub destination: Option<String>,
    /// `--drafts-source`, over any `--set drafts_source=`.
    pub drafts_source: Option<String>,
    /// `--store`, over any `--set store=`.
    pub store: Option<String>,
    /// The environment, as name and value pairs.
    pub environment: Vec<(String, String)>,
    /// The directory a `onefollowup.yaml` is looked for in.
    pub working_directory: PathBuf,
    /// The host label default; `None` reads the machine's hostname.
    pub hostname: Option<String>,
}

impl Inputs {
    /// Inputs reading this process's environment and working directory.
    ///
    /// # Errors
    ///
    /// Refused when the working directory cannot be read.
    pub fn from_process() -> Result<Self, ConfigError> {
        let working_directory = std::env::current_dir().map_err(|error| ConfigError {
            key: "(working directory)".to_owned(),
            origin: Origin::Default,
            message: format!("could not read the working directory: {error}"),
        })?;
        Ok(Self {
            environment: std::env::vars().collect(),
            working_directory,
            ..Self::default()
        })
    }
}

/// The effective configuration and where each of its top-level keys came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    config: Config,
    sources: Vec<(String, Origin)>,
    working_directory: PathBuf,
    environment: Vec<(String, String)>,
}

impl Settings {
    /// Settings over a configuration given directly; every key reads as the default's.
    ///
    /// # Errors
    ///
    /// Refused when the configuration breaks a rule [`resolve`] holds a file to.
    pub fn new(config: Config) -> Result<Self, ConfigError> {
        let sources: Vec<(String, Origin)> = KEYS
            .iter()
            .map(|key| ((*key).to_owned(), Origin::Default))
            .collect();
        validate::validate(&config).map_err(|(key, message)| ConfigError {
            key,
            origin: Origin::Default,
            message,
        })?;
        let process = Inputs::from_process()?;
        Ok(Self {
            config,
            sources,
            working_directory: process.working_directory,
            environment: process.environment,
        })
    }

    /// The same settings, run from `working_directory` with `environment`: where the store is
    /// discovered from, a relative path is resolved against, and a hook or route command runs.
    #[must_use]
    pub fn in_environment(
        mut self,
        working_directory: PathBuf,
        environment: Vec<(String, String)>,
    ) -> Self {
        self.working_directory = working_directory;
        self.environment = environment;
        self
    }

    /// The directory the verb runs in.
    #[must_use]
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }

    /// The environment the verb runs with, as name and value pairs.
    #[must_use]
    pub fn environment(&self) -> &[(String, String)] {
        &self.environment
    }

    /// `path`, a value of the top-level key `key`, made absolute: a relative path is read
    /// against the directory of the configuration file that stated the key, and otherwise
    /// against the working directory.
    #[must_use]
    pub fn path(&self, key: &str, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            return path.to_path_buf();
        }
        let base = self
            .sources
            .iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, origin)| match origin {
                Origin::File(file) => file.parent().map(Path::to_path_buf),
                _ => None,
            })
            .map_or_else(
                || self.working_directory.clone(),
                |directory| self.working_directory.join(directory),
            );
        base.join(path)
    }

    /// The effective configuration.
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Each top-level key, in [`KEYS`] order, with the layer its effective value came from.
    /// A mapping some of whose keys several layers stated reads as the highest of them.
    #[must_use]
    pub fn sources(&self) -> &[(String, Origin)] {
        &self.sources
    }
}

/// The shipped defaults, with the machine's hostname as `host_label`.
///
/// # Panics
///
/// Never for the shipped file, which the tests read.
#[must_use]
pub fn defaults(hostname: &str) -> Config {
    let mut value = defaults_value();
    value["host_label"] = Value::from(hostname);
    serde_json::from_value(value).expect("the shipped defaults are a valid configuration")
}

fn defaults_value() -> Value {
    serde_saphyr::from_str(DEFAULTS).expect("the shipped defaults are YAML")
}

fn machine_hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

/// One stated value: a key path and the layer that stated it.
#[derive(Debug, Clone)]
struct Setting {
    path: Vec<String>,
    value: Value,
    origin: Origin,
}

/// Resolve the effective configuration from `inputs`.
///
/// # Errors
///
/// Refused, naming the key and the layer, for a file that cannot be read or parsed, an
/// unknown key, a wrongly typed value, or a configuration breaking one of the rules
/// `README.md` states.
pub fn resolve(inputs: &Inputs) -> Result<Settings, ConfigError> {
    let hostname = inputs.hostname.clone().unwrap_or_else(machine_hostname);
    let mut settings = Vec::new();
    let mut default = defaults_value();
    default["host_label"] = Value::from(hostname);
    flatten(&mut Vec::new(), &default, &Origin::Default, &mut settings);

    if let Some(path) = config_file(inputs)? {
        let origin = Origin::File(path.clone());
        let document = read_file(&path, &origin)?;
        stated(&[], &document, &origin)?;
        flatten(&mut Vec::new(), &document, &origin, &mut settings);
    }

    let mut variables: Vec<&(String, String)> = inputs
        .environment
        .iter()
        .filter(|(name, _)| name.starts_with(VARIABLE_PREFIX) && name != CONFIG_VARIABLE)
        .collect();
    variables.sort();
    for (name, text) in variables {
        let origin = Origin::Env(name.clone());
        let path: Vec<String> = name[VARIABLE_PREFIX.len()..]
            .split("__")
            .map(str::to_lowercase)
            .collect();
        let value = value_from_text(text, &path, &origin)?;
        stated(&path, &value, &origin)?;
        flatten(&mut path.clone(), &value, &origin, &mut settings);
    }

    for assignment in &inputs.set {
        let origin = Origin::Flag("--set".to_owned());
        let Some((key, text)) = assignment.split_once('=') else {
            return Err(ConfigError {
                key: assignment.clone(),
                origin,
                message: "a --set is KEY=VALUE, a nested key a dotted path".to_owned(),
            });
        };
        let path: Vec<String> = key.split('.').map(str::to_owned).collect();
        let value = value_from_text(text, &path, &origin)?;
        stated(&path, &value, &origin)?;
        flatten(&mut path.clone(), &value, &origin, &mut settings);
    }

    for (flag, key, value) in [
        ("--destination", "destination", &inputs.destination),
        ("--drafts-source", "drafts_source", &inputs.drafts_source),
        ("--store", "store", &inputs.store),
    ] {
        if let Some(value) = value {
            let origin = Origin::Flag(flag.to_owned());
            flatten(
                &mut vec![key.to_owned()],
                &Value::from(value.as_str()),
                &origin,
                &mut settings,
            );
        }
    }

    let merged = merge(settings);
    let value = unflatten(&merged);
    validate::retired(&value).map_err(|(key, message)| ConfigError {
        origin: origin_of(&merged, &key),
        key,
        message,
    })?;
    let config: Config = serde_path_to_error::deserialize(&value).map_err(|error| {
        let key = error.path().to_string();
        let message = error.inner().to_string();
        ConfigError {
            origin: origin_of(&merged, &key),
            key,
            message,
        }
    })?;
    validate::validate(&config).map_err(|(key, message)| ConfigError {
        origin: origin_of(&merged, &key),
        key,
        message,
    })?;
    let sources = KEYS
        .iter()
        .map(|key| ((*key).to_owned(), highest(&merged, key)))
        .collect();
    Ok(Settings {
        config,
        sources,
        working_directory: inputs.working_directory.clone(),
        environment: inputs.environment.clone(),
    })
}

/// The configuration file to read, if any: `--config`, then `ONEFOLLOWUP_CONFIG`, then the
/// working directory's `onefollowup.yaml` when it exists.
fn config_file(inputs: &Inputs) -> Result<Option<PathBuf>, ConfigError> {
    if let Some(path) = &inputs.config_file {
        return Ok(Some(path.clone()));
    }
    if let Some((_, path)) = inputs
        .environment
        .iter()
        .find(|(name, _)| name == CONFIG_VARIABLE)
    {
        if path.is_empty() {
            return Err(ConfigError {
                key: "(configuration file)".to_owned(),
                origin: Origin::Env(CONFIG_VARIABLE.to_owned()),
                message: "names no file; unset it or name the configuration file".to_owned(),
            });
        }
        return Ok(Some(PathBuf::from(path)));
    }
    let found = inputs.working_directory.join(FILE_NAME);
    Ok(found.is_file().then_some(found))
}

fn read_file(path: &Path, origin: &Origin) -> Result<Value, ConfigError> {
    let refused = |message: String| ConfigError {
        key: "(document)".to_owned(),
        origin: origin.clone(),
        message,
    };
    let text = std::fs::read_to_string(path)
        .map_err(|error| refused(format!("could not be read: {error}")))?;
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let document: Value =
        serde_saphyr::from_str(&text).map_err(|error| refused(format!("is not YAML: {error}")))?;
    match document {
        Value::Object(_) => Ok(document),
        Value::Null => Ok(Value::Object(Map::new())),
        other => Err(refused(format!(
            "is a mapping of configuration keys, not {other}"
        ))),
    }
}

/// A variable's or flag's text as a value: YAML, so `5` is a number, `[a, b]` a list and
/// `{same: [x]}` a mapping; an empty text is the empty string.
fn value_from_text(text: &str, path: &[String], origin: &Origin) -> Result<Value, ConfigError> {
    if text.is_empty() {
        return Ok(Value::from(""));
    }
    serde_saphyr::from_str::<Value>(text).map_err(|error| ConfigError {
        key: path.join("."),
        origin: origin.clone(),
        message: format!("its value is not YAML: {error}"),
    })
}

/// Refuse, before anything is merged, a key this layer states that no configuration has.
fn stated(path: &[String], value: &Value, origin: &Origin) -> Result<(), ConfigError> {
    let refused = |key: String, message: String| ConfigError {
        key,
        origin: origin.clone(),
        message,
    };
    let tops: Vec<String> = match path.first() {
        Some(top) => vec![top.clone()],
        None => value
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default(),
    };
    for top in tops {
        if let Some((_, hook)) = REPLACED_KEYS.iter().find(|(old, _)| *old == top) {
            return Err(refused(
                top,
                format!("is no longer a key; the `{hook}` hook replaced it"),
            ));
        }
        if !KEYS.contains(&top.as_str()) {
            return Err(refused(
                top,
                format!(
                    "is not a configuration key; the keys are {}",
                    KEYS.join(", ")
                ),
            ));
        }
    }
    Ok(())
}

fn flatten(path: &mut Vec<String>, value: &Value, origin: &Origin, into: &mut Vec<Setting>) {
    match value {
        // An empty document states nothing; it is not a whole configuration replaced.
        Value::Object(map) if map.is_empty() && path.is_empty() => {}
        Value::Object(map) if !map.is_empty() => {
            for (key, inner) in map {
                path.push(key.clone());
                flatten(path, inner, origin, into);
                path.pop();
            }
        }
        _ => into.push(Setting {
            path: path.clone(),
            value: value.clone(),
            origin: origin.clone(),
        }),
    }
}

/// Later settings win: one replaces every earlier setting it is an ancestor or a descendant
/// of, so a whole mapping stated later replaces the keys stated under it earlier.
fn merge(settings: Vec<Setting>) -> Vec<Setting> {
    let mut merged: Vec<Setting> = Vec::new();
    for setting in settings {
        merged.retain(|held| {
            !prefixes(&held.path, &setting.path) && !prefixes(&setting.path, &held.path)
        });
        merged.push(setting);
    }
    merged
}

fn prefixes(shorter: &[String], longer: &[String]) -> bool {
    shorter.len() <= longer.len() && longer[..shorter.len()] == *shorter
}

fn unflatten(settings: &[Setting]) -> Value {
    let mut root = Value::Object(Map::new());
    for setting in settings {
        let mut at = &mut root;
        for (index, key) in setting.path.iter().enumerate() {
            if index + 1 == setting.path.len() {
                if let Value::Object(map) = at {
                    map.insert(key.clone(), setting.value.clone());
                }
                break;
            }
            let Value::Object(map) = at else { break };
            at = map
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Map::new()));
        }
    }
    root
}

/// The layer that stated `key`: the most specific setting at or above it, or else — for a
/// key stated only as the keys beneath it, such as a mapping where a list belongs — the
/// highest layer among those. A key no layer stated is the default's.
fn origin_of(settings: &[Setting], key: &str) -> Origin {
    settings
        .iter()
        .filter(|setting| under(key, &dotted(&setting.path)))
        .max_by_key(|setting| setting.path.len())
        .map(|setting| setting.origin.clone())
        .or_else(|| {
            settings
                .iter()
                .filter(|setting| under(&dotted(&setting.path), key))
                .map(|setting| setting.origin.clone())
                .max_by_key(rank)
        })
        .unwrap_or(Origin::Default)
}

fn dotted(path: &[String]) -> String {
    path.join(".")
}

/// Whether the dotted key `inner` is `outer` or lies under it (`kinds[0].name` under `kinds`).
fn under(inner: &str, outer: &str) -> bool {
    inner == outer
        || inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

fn highest(settings: &[Setting], key: &str) -> Origin {
    settings
        .iter()
        .filter(|setting| setting.path.first().map(String::as_str) == Some(key))
        .map(|setting| setting.origin.clone())
        .max_by_key(rank)
        .unwrap_or(Origin::Default)
}

fn rank(origin: &Origin) -> u8 {
    match origin {
        Origin::Default => 0,
        Origin::File(_) => 1,
        Origin::Env(_) => 2,
        Origin::Flag(flag) if flag == "--set" => 3,
        Origin::Flag(_) => 4,
    }
}
