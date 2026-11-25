//! The configuration's typed shape: every key, its type and how it serializes.
//!
//! [`Config`] is what every verb reads. Its key list, [`KEYS`], is the one statement of
//! which top-level keys exist; the resolution in [`super::resolve`] refuses any other and the
//! precedence tests are parameterized over it.

use indexmap::IndexMap;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Every top-level configuration key, in the order `config show` prints them.
pub const KEYS: &[&str] = &[
    "store",
    "drafts_source",
    "destination",
    "namespace",
    "host_label",
    "batch_id_pattern",
    "statuses",
    "rubric",
    "estimate",
    "task_template",
    "persona",
    "comment_templates",
    "fields",
    "kinds",
    "default_kind",
    "relations",
    "dispositions",
    "default_dispositions",
    "routes",
    "hooks",
    "snapshot",
    "comments",
    "governor",
    "groom",
];

/// The effective configuration of onefollowup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The onetaskgraph configuration the store is opened with; `None` is the store's own
    /// discovery from the working directory.
    pub store: Option<String>,
    /// The source drafts are recorded in.
    pub drafts_source: String,
    /// The source every ticket is copied to unless its kind names another.
    pub destination: String,
    /// The prefix of every comment marker and of the ticket record's metadata key.
    pub namespace: String,
    /// The label a ticket records as the host that verified it.
    pub host_label: String,
    /// The pattern a batch id matches in full.
    pub batch_id_pattern: String,
    /// Each status role, mapped to a store status category.
    pub statuses: Statuses,
    /// The severity rubric tickets are judged by.
    pub rubric: Rubric,
    /// How a priority estimate is raised.
    pub estimate: Estimate,
    /// The verifying agent's task template (`builtin:` names a shipped file).
    pub task_template: String,
    /// The verifying agent's persona (`builtin:` names a shipped file).
    pub persona: String,
    /// The templates a batch's comments are rendered from.
    pub comment_templates: CommentTemplates,
    /// The fields a ticket may record beyond the record's own keys.
    pub fields: IndexMap<String, FieldDeclaration>,
    /// The ticket kinds the verifying agent chooses from.
    pub kinds: Vec<Kind>,
    /// The kind a record without one is read as.
    pub default_kind: String,
    /// The typed edges between kinds.
    pub relations: Vec<Relation>,
    /// The catalogue of dispositions an input may be given.
    pub dispositions: Vec<Disposition>,
    /// The dispositions every kind allows unless it lists its own; `None` allows all.
    pub default_dispositions: Option<Vec<String>>,
    /// The ordered routes, first match wins.
    pub routes: Vec<Route>,
    /// The commands run at named moments.
    pub hooks: Hooks,
    /// The comment snapshot's settings.
    pub snapshot: SnapshotSettings,
    /// How `comments answer` launches each batch's answering.
    pub comments: CommentsSettings,
    /// How every destination call is paced against the quota the destination reports.
    pub governor: GovernorSettings,
    /// How `groom prepare` selects, shards and tasks a destination's open tickets.
    pub groom: GroomSettings,
}

/// A status role: what a ticket's status means to the loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatusRole {
    /// Awaiting a person's decision.
    Proposal,
    /// Accepted and not yet taken up.
    Accepted,
    /// Deferred by a person.
    Deferred,
    /// Claimed by work that has not started.
    Queued,
    /// Taken up.
    InProgress,
    /// Taken up and waiting on a reviewer before it is finished.
    InReview,
    /// Finished.
    Done,
    /// Withdrawn.
    Cancelled,
}

impl StatusRole {
    /// The role's name as configuration spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposal => "proposal",
            Self::Accepted => "accepted",
            Self::Deferred => "deferred",
            Self::Queued => "queued",
            Self::InProgress => "in-progress",
            Self::InReview => "in-review",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Every status role, each mapped to the store category it is written as.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Statuses {
    /// See [`StatusRole::Proposal`].
    pub proposal: StatusMapping,
    /// See [`StatusRole::Accepted`].
    pub accepted: StatusMapping,
    /// See [`StatusRole::Deferred`].
    pub deferred: StatusMapping,
    /// See [`StatusRole::Queued`].
    pub queued: StatusMapping,
    /// See [`StatusRole::InProgress`].
    #[serde(rename = "in-progress")]
    pub in_progress: StatusMapping,
    /// See [`StatusRole::InReview`].
    #[serde(rename = "in-review")]
    pub in_review: StatusMapping,
    /// See [`StatusRole::Done`].
    pub done: StatusMapping,
    /// See [`StatusRole::Cancelled`].
    pub cancelled: StatusMapping,
}

impl Statuses {
    /// Each role and its mapping, in role order.
    #[must_use]
    pub fn iter(&self) -> [(StatusRole, &StatusMapping); 8] {
        [
            (StatusRole::Proposal, &self.proposal),
            (StatusRole::Accepted, &self.accepted),
            (StatusRole::Deferred, &self.deferred),
            (StatusRole::Queued, &self.queued),
            (StatusRole::InProgress, &self.in_progress),
            (StatusRole::InReview, &self.in_review),
            (StatusRole::Done, &self.done),
            (StatusRole::Cancelled, &self.cancelled),
        ]
    }
}

/// One status role's mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusMapping {
    /// The store status category the role is written as.
    pub category: String,
    /// What the role means, as the verifying agent reads it.
    pub description: String,
    /// Whether no batch withdraws an item at this role, because a person decided on it.
    pub protected: bool,
    /// Whether an agent picking up accepted tickets selects an item at this role.
    pub selected: bool,
    /// Whether people's comments on an item at this role are answered by its owning batch.
    pub answered: bool,
}

/// A priority, in the store's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Priority {
    /// Most urgent.
    Urgent,
    /// High.
    High,
    /// Medium.
    Medium,
    /// Low.
    Low,
    /// No priority.
    None,
}

/// The severity rubric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rubric {
    /// The severity levels, most severe first, each with the priority it estimates at.
    pub levels: Vec<SeverityLevel>,
    /// The impact areas' headings.
    pub areas: ImpactAreas,
    /// The rubric's prose.
    pub guidance: String,
    /// The lines an `## Impact` section ends with.
    pub impact_lines: Vec<String>,
    /// The workaround that says there is none.
    pub no_workaround: String,
    /// The labelled parts of the impact prose.
    pub impact_parts: Vec<String>,
    /// What a workaround's cost is judged on.
    pub workaround_costs: Vec<String>,
}

/// One severity level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeverityLevel {
    /// The level's name, as a ticket records it.
    pub name: String,
    /// The priority this severity with the workaround estimates at before any raise.
    pub priority: Priority,
    /// What the level means for users.
    pub users: String,
    /// What the level means for development.
    pub development: String,
    /// What the level means for resources.
    pub resources: String,
}

/// The impact areas' headings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpactAreas {
    /// Users.
    pub users: String,
    /// Development.
    pub development: String,
    /// Resources.
    pub resources: String,
}

/// How a priority estimate is raised, and the line that states it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    /// How many occurrences raise an estimate one level.
    pub raise_at: u32,
    /// The level a raise is capped at.
    pub raise_cap: Priority,
    /// The label the estimate line is found by in a body: a line that, after an optional `- `
    /// or `* ` bullet, opens with `<label>:`.
    #[serde(default = "line_label")]
    pub line_label: String,
    /// The template the estimate line is rendered from (`builtin:` names a shipped file).
    #[serde(default = "line_template")]
    pub line_template: String,
}

fn line_label() -> String {
    "Priority estimate".to_owned()
}

fn line_template() -> String {
    "builtin:estimate-line.j2".to_owned()
}

/// The templates a batch's comments are rendered from, each a `builtin:` name or a path:
/// the visible part of the comment, after which onefollowup appends the marker line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentTemplates {
    /// An evidence comment's template.
    pub evidence: String,
    /// A reply's template.
    pub reply: String,
}

/// A declared field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDeclaration {
    /// What the field records, as the verifying agent is shown it.
    pub description: String,
}

/// A ticket kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kind {
    /// The kind's name.
    pub name: String,
    /// What a ticket of the kind is, as the verifying agent reads it.
    pub description: String,
    /// The kind's ticket template; `None` is the shipped one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_template: Option<String>,
    /// The template the destination body is rendered from; `None` is the authored body with
    /// its estimate line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_template: Option<String>,
    /// Each projection's template, by name, rendered into the record's `projected`; `None`
    /// writes no `projected`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projections: Option<IndexMap<String, String>>,
    /// The section a `same` duplicate's merge stages as its evidence comment's evidence.
    #[serde(default = "evidence_section")]
    pub evidence_section: String,
    /// The shape checks a ticket of the kind is held to, in order; `None` is 0.5.0's:
    /// [`Check::defaults`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<Check>>,
    /// How a ticket of the kind is found to be one already filed.
    #[serde(default)]
    pub duplicates: Duplicates,
    /// How a ticket of the kind is given a priority.
    #[serde(default)]
    pub priority: PriorityModel,
    /// The role a new ticket of the kind is copied in.
    #[serde(default = "proposal")]
    pub initial_status: StatusRole,
    /// The source tickets of the kind go to; `None` is the top-level destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// The dispositions inputs of the kind may be given; `None` is `default_dispositions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispositions: Option<Vec<String>>,
}

fn evidence_section() -> String {
    "Evidence".to_owned()
}

impl Kind {
    /// The checks a ticket of the kind is held to: its `checks`, or 0.5.0's.
    #[must_use]
    pub fn checks(&self) -> Vec<Check> {
        self.checks
            .clone()
            .unwrap_or_else(|| Check::defaults(&self.priority))
    }
}

/// One shape check a kind holds its tickets to: a built-in, with its parameters, or a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// The level-2 headings in the ticket template's order, `<optional` sections optional.
    Headings,
    /// The title's prefix and length.
    Title {
        /// What the title opens with.
        prefix: TitlePrefix,
        /// The most characters it carries.
        limit: usize,
    },
    /// The rubric's labelled parts, in order, in the named section.
    Impact {
        /// The section.
        section: String,
    },
    /// The fix's units and its contributing locations, in the one-unit or the several-unit form.
    FixUnits {
        /// The fix's section.
        section: String,
        /// The labelled part listing the locations.
        locations: String,
    },
    /// The named section is prose, carrying nothing the record holds.
    EvidenceProse {
        /// The section.
        section: String,
    },
    /// The record's witnesses.
    Witnesses {
        /// Whether at least one is required.
        required: bool,
    },
    /// A command given the ticket on stdin: exit 0 and no output accepts it.
    Command(Vec<String>),
}

/// What a ticket's title opens with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitlePrefix {
    /// `<repository name>: `, the last part of the record's repository.
    RepositoryName,
    /// Nothing in particular.
    None,
}

/// Every built-in check's name and the parameters it takes.
pub const BUILT_IN_CHECKS: &[(&str, &[&str])] = &[
    ("headings", &[]),
    ("title", &["prefix", "limit"]),
    ("impact", &["section"]),
    ("fix-units", &["section", "locations"]),
    ("evidence-prose", &["section"]),
    ("witnesses", &["required"]),
];

impl Check {
    /// The checks a kind naming none is held to, 0.5.0's: `headings`, `title`, and `impact`
    /// for an estimated kind.
    #[must_use]
    pub fn defaults(priority: &PriorityModel) -> Vec<Self> {
        let mut checks = vec![Self::Headings, Self::title()];
        if *priority == PriorityModel::Estimate {
            checks.push(Self::Impact {
                section: "Impact".to_owned(),
            });
        }
        checks
    }

    fn title() -> Self {
        Self::Title {
            prefix: TitlePrefix::RepositoryName,
            limit: 120,
        }
    }

    /// The built-in `name` with every parameter at its default.
    fn built_in(name: &str) -> Option<Self> {
        Some(match name {
            "headings" => Self::Headings,
            "title" => Self::title(),
            "impact" => Self::Impact {
                section: "Impact".to_owned(),
            },
            "fix-units" => Self::FixUnits {
                section: "Suggested fix".to_owned(),
                locations: "Contributing locations".to_owned(),
            },
            "evidence-prose" => Self::EvidenceProse {
                section: "Evidence".to_owned(),
            },
            "witnesses" => Self::Witnesses { required: true },
            _ => return None,
        })
    }
}

const CHECK_SHAPE: &str =
    "a check is {check: <built-in>, ...parameters}, a built-in's name, or {command: [argv...]}";

impl Serialize for Check {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map: IndexMap<&str, Value> = IndexMap::new();
        let mut named = |name: &str| {
            map.insert("check", Value::from(name));
        };
        match self {
            Self::Headings => named("headings"),
            Self::Title { prefix, limit } => {
                named("title");
                map.insert(
                    "prefix",
                    match prefix {
                        TitlePrefix::RepositoryName => Value::from("repository-name"),
                        TitlePrefix::None => Value::Null,
                    },
                );
                map.insert("limit", Value::from(*limit));
            }
            Self::Impact { section } => {
                named("impact");
                map.insert("section", Value::from(section.as_str()));
            }
            Self::FixUnits { section, locations } => {
                named("fix-units");
                map.insert("section", Value::from(section.as_str()));
                map.insert("locations", Value::from(locations.as_str()));
            }
            Self::EvidenceProse { section } => {
                named("evidence-prose");
                map.insert("section", Value::from(section.as_str()));
            }
            Self::Witnesses { required } => {
                named("witnesses");
                map.insert("required", Value::from(*required));
            }
            Self::Command(argv) => {
                map.insert("command", Value::from(argv.clone()));
            }
        }
        map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Check {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::read(&value).map_err(D::Error::custom)
    }
}

impl Check {
    fn read(value: &Value) -> Result<Self, String> {
        let names = || {
            BUILT_IN_CHECKS
                .iter()
                .map(|(name, _)| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let unknown = |name: &str| {
            format!(
                "`{name}` is not a built-in check; the built-ins are {}, and a command is {{command: [argv...]}}",
                names()
            )
        };
        let map = match value {
            Value::String(name) => return Self::built_in(name).ok_or_else(|| unknown(name)),
            Value::Object(map) => map,
            other => return Err(format!("{CHECK_SHAPE}, not {other}")),
        };
        if let Some(argv) = map.get("command") {
            if map.len() != 1 {
                return Err(format!(
                    "a command check carries `command` alone, not {}",
                    map.keys()
                        .filter(|key| *key != "command")
                        .map(|key| format!("`{key}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            return match strings(argv) {
                Some(argv) if !argv.is_empty() => Ok(Self::Command(argv)),
                _ => Err("`command` is a non-empty list of strings, the argv run".to_owned()),
            };
        }
        let name = match map.get("check") {
            Some(Value::String(name)) => name.as_str(),
            Some(other) => return Err(format!("`check` names a built-in check, not {other}")),
            None => {
                return Err(format!(
                    "{CHECK_SHAPE}: it carries neither `check` nor `command`"
                ));
            }
        };
        let (_, parameters) = BUILT_IN_CHECKS
            .iter()
            .find(|(built_in, _)| *built_in == name)
            .ok_or_else(|| unknown(name))?;
        for key in map.keys().filter(|key| *key != "check") {
            if !parameters.contains(&key.as_str()) {
                return Err(format!(
                    "the check `{name}` takes {}, not `{key}`",
                    if parameters.is_empty() {
                        "no parameter".to_owned()
                    } else {
                        parameters
                            .iter()
                            .map(|one| format!("`{one}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                ));
            }
        }
        let text = |key: &str, default: &str| -> Result<String, String> {
            match map.get(key) {
                None => Ok(default.to_owned()),
                Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
                Some(other) => Err(format!(
                    "the check `{name}`'s `{key}` is a non-empty section or part name, not {other}"
                )),
            }
        };
        let mut check = Self::built_in(name).ok_or_else(|| unknown(name))?;
        match &mut check {
            Self::Headings | Self::Command(_) => {}
            Self::Title { prefix, limit } => {
                match map.get("prefix") {
                    None => {}
                    Some(Value::String(word)) if word == "repository-name" => {
                        *prefix = TitlePrefix::RepositoryName;
                    }
                    Some(Value::Null) => *prefix = TitlePrefix::None,
                    Some(other) => {
                        return Err(format!(
                            "the check `title`'s `prefix` is `repository-name` or null, not {other}"
                        ));
                    }
                }
                if let Some(given) = map.get("limit") {
                    *limit = given
                        .as_u64()
                        .filter(|limit| *limit > 0)
                        .and_then(|limit| usize::try_from(limit).ok())
                        .ok_or_else(|| {
                            format!(
                                "the check `title`'s `limit` is a positive whole number of characters, not {given}"
                            )
                        })?;
                }
            }
            Self::Impact { section } | Self::EvidenceProse { section } => {
                *section = text("section", section)?;
            }
            Self::FixUnits { section, locations } => {
                *section = text("section", section)?;
                *locations = text("locations", locations)?;
            }
            Self::Witnesses { required } => {
                if let Some(given) = map.get("required") {
                    *required = given.as_bool().ok_or_else(|| {
                        format!("the check `witnesses`' `required` is true or false, not {given}")
                    })?;
                }
            }
        }
        Ok(check)
    }
}

fn proposal() -> StatusRole {
    StatusRole::Proposal
}

/// How a ticket is found to be one already filed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Duplicates {
    /// The verifying agent searches the destination and judges.
    #[default]
    Agent,
    /// Two tickets are one when every listed declared field is equal.
    Same(Vec<String>),
}

impl Duplicates {
    /// The fields a `same` rule compares, or `None` for `agent`.
    #[must_use]
    pub fn same(&self) -> Option<&[String]> {
        match self {
            Self::Agent => None,
            Self::Same(fields) => Some(fields),
        }
    }
}

const DUPLICATES_SHAPE: &str = "`duplicates` is `agent` or {same: [<declared field>...]}";

impl Serialize for Duplicates {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Agent => serializer.serialize_str("agent"),
            Self::Same(fields) => {
                let mut map = IndexMap::new();
                map.insert("same", fields);
                map.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for Duplicates {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(word) if word == "agent" => Ok(Self::Agent),
            Value::Object(map) if map.len() == 1 && map.contains_key("same") => {
                let fields = strings(&map["same"]).ok_or_else(|| {
                    D::Error::custom(format!("{DUPLICATES_SHAPE}: `same` is a list of names"))
                })?;
                Ok(Self::Same(fields))
            }
            other => Err(D::Error::custom(format!("{DUPLICATES_SHAPE}, not {other}"))),
        }
    }
}

/// How a ticket of a kind is given a priority.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PriorityModel {
    /// The severity × frequency estimate over `rubric` and `estimate`.
    #[default]
    Estimate,
    /// Every ticket of the kind has this priority.
    Fixed(Priority),
}

const PRIORITY_SHAPE: &str = "`priority` is {model: estimate} or {fixed: <priority>}";

impl Serialize for PriorityModel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = IndexMap::new();
        match self {
            Self::Estimate => map.insert("model", Value::from("estimate")),
            Self::Fixed(priority) => map.insert(
                "fixed",
                serde_json::to_value(priority).map_err(serde::ser::Error::custom)?,
            ),
        };
        map.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PriorityModel {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if let Value::Object(map) = &value
            && map.len() == 1
        {
            if map.get("model").and_then(Value::as_str) == Some("estimate") {
                return Ok(Self::Estimate);
            }
            if let Some(fixed) = map.get("fixed") {
                let priority: Priority = serde_json::from_value(fixed.clone())
                    .map_err(|error| D::Error::custom(format!("{PRIORITY_SHAPE}: {error}")))?;
                return Ok(Self::Fixed(priority));
            }
        }
        Err(D::Error::custom(format!("{PRIORITY_SHAPE}, not {value}")))
    }
}

/// A typed edge between two kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    /// The relation's name.
    pub name: String,
    /// The more specific kind, the edge's origin.
    pub from: String,
    /// The more general kind, the edge's target.
    pub to: String,
    /// How many `to` tickets one `from` ticket may relate to.
    pub cardinality: Cardinality,
    /// Who decides an edge.
    pub decided_by: DecidedBy,
    /// How the store holds an edge.
    pub store: RelationStore,
    /// What the `to` ticket's priority estimate takes from its related tickets.
    #[serde(default)]
    pub rollup: Rollup,
}

/// A relation's cardinality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cardinality {
    /// Each `from` ticket relates to at most one `to` ticket.
    ManyToOne,
    /// Any number either way.
    ManyToMany,
}

/// Who decides a relation's edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecidedBy {
    /// The verifying agent asserts edges as judgment.
    Agent,
    /// The `to` ticket whose values of these fields equal the `from` ticket's.
    Match(Vec<String>),
}

const DECIDED_BY_SHAPE: &str = "`decided_by` is `agent` or {match: [<field>...]}";

impl Serialize for DecidedBy {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Agent => serializer.serialize_str("agent"),
            Self::Match(fields) => {
                let mut map = IndexMap::new();
                map.insert("match", fields);
                map.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for DecidedBy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::String(word) if word == "agent" => Ok(Self::Agent),
            Value::Object(map) if map.len() == 1 && map.contains_key("match") => {
                let fields = strings(&map["match"]).ok_or_else(|| {
                    D::Error::custom(format!("{DECIDED_BY_SHAPE}: `match` is a list of names"))
                })?;
                Ok(Self::Match(fields))
            }
            other => Err(D::Error::custom(format!("{DECIDED_BY_SHAPE}, not {other}"))),
        }
    }
}

/// How the store holds a relation's edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationStore {
    /// The store's parent relation.
    Parent,
    /// A typed link named after the relation.
    Link,
}

/// What a `to` ticket's estimate takes from its related tickets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rollup {
    /// Each related ticket's occurrences add to the `to` ticket's.
    Occurrences,
    /// The `to` ticket's estimate is no lower than its related tickets' highest.
    Max,
    /// Nothing rolls up.
    #[default]
    None,
}

/// A disposition an input may be given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disposition {
    /// The disposition's name.
    pub name: String,
    /// What it means, as the verifying agent reads it.
    pub description: String,
    /// What an input given it carries.
    pub requires: Vec<Requirement>,
}

/// What a disposition requires an input to carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Requirement {
    /// The ticket it is filed as or on: the only statement that an input is filed.
    Ticket,
    /// The change that addressed it.
    Change,
    /// Why it is not filed.
    Reason,
}

/// One route: a deterministic match over a ticket, and what it sets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// The route's name.
    pub name: String,
    /// What a ticket matches on; every key optional.
    #[serde(default, rename = "match")]
    pub matches: RouteMatch,
    /// What a match sets over the kind's defaults.
    #[serde(default)]
    pub set: RouteSet,
    /// The escape hatch run before the ticket's write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
}

/// A route's match.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteMatch {
    /// Kinds matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<String>>,
    /// Rubric severities matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<Vec<String>>,
    /// Repository globs matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<Vec<String>>,
    /// Path globs matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<String>>,
    /// Minimum edge counts, by relation name, at either end, staged ones counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relations: Option<IndexMap<String, MinimumEdges>>,
}

/// A minimum count of edges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinimumEdges {
    /// The fewest edges that match.
    pub min: u32,
}

/// What a route sets.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteSet {
    /// The status role a new ticket is copied in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_role: Option<StatusRole>,
    /// The ticket's priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    /// Labels the ticket's item carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// The one person assigned, named as the destination names people.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
}

/// The commands run at named moments, each named by when it fires.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    /// Run by `prepare` before it snapshots; its stdout supplies additional inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_preparing: Option<Vec<String>>,
    /// Run by `settle` with the settled batch's summary on stdin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_settled: Option<Vec<String>>,
    /// Who owns what a hook naming [`CONTEXT_FILE`] raises: the context file's `owner`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<HookOwner>,
}

/// The argv placeholder a hook names to be handed a context file: onefollowup replaces it,
/// wherever an argument holds it, with the path of a JSON file it writes for that one call
/// and removes once the hook exits.
pub const CONTEXT_FILE: &str = "{context_file}";

impl Hooks {
    /// Every configured hook, by name, with its argv.
    #[must_use]
    pub fn named(&self) -> [(&'static str, Option<&Vec<String>>); 2] {
        [
            ("batch_preparing", self.batch_preparing.as_ref()),
            ("batch_settled", self.batch_settled.as_ref()),
        ]
    }
}

/// The owner a hook's context file names, as an engine's human-item owner spells it: each
/// value a literal, or a whole `${NAME}` read from the verb's environment when the hook runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookOwner {
    /// The session that owns it.
    pub session: String,
    /// The run, when one owns it; `null` in the file when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The plan, when one owns it; `null` in the file when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// The comment snapshot's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotSettings {
    /// How long a snapshot stays launchable: a whole number of `s`, `m`, `h` or `d`.
    pub launchable_for: String,
}

/// How the quota governor paces every destination call (README.md, "The quota governor").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GovernorSettings {
    /// Whether destination calls are paced at all.
    pub enabled: bool,
    /// The hard floor, a percentage of each window's limit no request is admitted under.
    pub floor_percent: f64,
    /// What others' peak burn until the reset is multiplied by to make the reserve.
    pub reserve_factor: f64,
    /// The span others' peak burn rate is measured over.
    pub peak_window_seconds: u64,
    /// How far back others' burn is remembered.
    pub burn_history_seconds: u64,
    /// The least rate, in points a minute, others are ever assumed to spend each resource at;
    /// a resource it does not name is assumed at 0.
    pub min_peak_per_minute: std::collections::BTreeMap<String, f64>,
    /// The longest a reading is used before it is taken again.
    pub sample_seconds: u64,
    /// The most content-creating writes (an issue, a comment) every onefollowup process on the
    /// host sends one backend kind in any rolling hour: a count, since no backend reports
    /// GitHub's secondary content-creation limit.
    pub max_content_writes_per_hour: u64,
    /// The same in any rolling minute.
    pub max_content_writes_per_minute: u64,
    /// The host-level directory every onefollowup process shares the governor's state in;
    /// resolution fills it from `XDG_STATE_HOME`, else `HOME`, and `None` keeps the state to
    /// the process alone.
    pub state_dir: Option<std::path::PathBuf>,
}

/// How `comments answer` launches each batch's answering.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommentsSettings {
    /// The argv `comments answer` runs once per launched batch, each argument's
    /// [`LAUNCH_PLACEHOLDERS`] replaced with that launch's values; `None` refuses the verb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_command: Option<Vec<String>>,
}

/// How `groom prepare` selects, shards and tasks a destination's open tickets, and how
/// `groom publish` comments on the tickets it closes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroomSettings {
    /// How many tickets one shard's task holds, at least 1.
    pub shard_size: usize,
    /// How far before a record's `verified_at` a comment still counts as possibly unseen,
    /// in seconds: comments indexed late are discovered, and those the record holds are not
    /// counted twice.
    pub comment_overlap_seconds: u64,
    /// How old a record may be before its ticket is verified again, in days, at least 1.
    pub max_age_days: u64,
    /// Where each repository's blob-less clone is kept; a relative path is read against the
    /// store's directory.
    pub clones: String,
    /// The shard task's template (`builtin:` names a shipped file).
    pub task_template: String,
    /// The persona the shard task gives its verifying agent (`builtin:` names a shipped file).
    pub persona: String,
    /// The visible part of every comment `groom publish` posts — a closed ticket's evidence
    /// and a survivor's consolidation — before the marker line onefollowup appends
    /// (`builtin:` names a shipped file).
    pub comment_template: String,
}

/// The placeholders `comments.launch_command` is given, each replaced wherever an argument
/// holds it: the batch, its feedback file's path, and the snapshot directory's path.
pub const LAUNCH_PLACEHOLDERS: [&str; 3] = ["{batch}", "{feedback}", "{snapshot}"];

fn strings(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|one| one.as_str().map(str::to_owned))
        .collect()
}
