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
    "fields",
    "kinds",
    "default_kind",
    "relations",
    "dispositions",
    "default_dispositions",
    "routes",
    "hooks",
    "snapshot",
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
    /// See [`StatusRole::Done`].
    pub done: StatusMapping,
    /// See [`StatusRole::Cancelled`].
    pub cancelled: StatusMapping,
}

impl Statuses {
    /// Each role and its mapping, in role order.
    #[must_use]
    pub fn iter(&self) -> [(StatusRole, &StatusMapping); 7] {
        [
            (StatusRole::Proposal, &self.proposal),
            (StatusRole::Accepted, &self.accepted),
            (StatusRole::Deferred, &self.deferred),
            (StatusRole::Queued, &self.queued),
            (StatusRole::InProgress, &self.in_progress),
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

/// How a priority estimate is raised.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    /// How many occurrences raise an estimate one level.
    pub raise_at: u32,
    /// The level a raise is capped at.
    pub raise_cap: Priority,
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
}

/// The comment snapshot's settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotSettings {
    /// How long a snapshot stays launchable: a whole number of `s`, `m`, `h` or `d`.
    pub launchable_for: String,
}

fn strings(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|one| one.as_str().map(str::to_owned))
        .collect()
}
