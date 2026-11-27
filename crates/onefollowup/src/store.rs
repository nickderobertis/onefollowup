//! The store: every access onefollowup makes to drafts, tickets and destinations.
//!
//! **This is the one module that calls onetaskgraph**, and it calls only the public API of
//! its crates (`tests/architecture.rs` holds that). Nothing here names a destination's API,
//! a plugin's internals, or branches on a plugin or destination name: a destination is a
//! source onetaskgraph's configuration declares, and what it can do is onetaskgraph's to say.
//!
//! **It is also the one counting point.** Every call this module makes to onetaskgraph is one
//! [`StoreOperation`] in [`Store::operations`], tagged with the source it reached and whether
//! that source is the local drafts source, so what a verb spent at a destination is read off
//! the operations it reports rather than estimated.
//!
//! **And the one governed point.** Every destination call is admitted by the store's
//! [`Governor`] before it is sent (`Store::send`), and every backend request inside it
//! through the [`RequestGate`] installed on the engine, so a store call any verb adds is paced
//! without the verb knowing (README.md, "The quota governor"). The governor's quota reads are
//! not store operations: a verb reports them as `quota_reads` beside its `store_operations`.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use onetaskgraph_core::config::{self, Layer, Routes, SourceVisibility};
use onetaskgraph_core::{
    BATCH_SCHEMA_VERSION, Body, CommentBatch, CommentBatchItem, CommentsSinceRequest, CopyAction,
    CopyConditions, CopyItems, CopyRequest, CopyScope, Engine, EngineError, Environment, Filters,
    FindIn, FindQuery, FindRequest, GlobalId, LinkBatch, LinkItem, OtherEnd, Paging, ParentBatch,
    ParentEdge, ProjectCreate, ProjectSelector, Qualified, RELATIONS_SCHEMA_VERSION, SourceState,
    StatusPolicy, TaskCreate, TaskRequest, UpdateBatch, UpdateBatchItem,
};
use onetaskgraph_plugin_api::{
    AuthorKind, Classification, CommentBody, GatedRequest, LinkDirection, Location, MetadataKey,
    MetadataMatch, NativeId, Priority, QuotaReading, Repository, RequestGate, SourceError,
    SourceName, Status, StatusCategory, Task, TaskLink, TaskUpdate,
};
use serde::Serialize;
use serde_json::Value;

use crate::config::GovernorSettings;
use crate::governor::{self, Governor, Kind, Pending, Reading, Readings, Report, Request};

/// The store status categories, in onetaskgraph's spelling, that a status role may map to.
/// `tests/store.rs` reconciles this list with onetaskgraph's own vocabulary.
pub const STATUS_CATEGORIES: &[&str] = &[
    "draft",
    "backlog",
    "todo",
    "queued",
    "in-progress",
    "in-review",
    "done",
    "cancelled",
];

/// The store a snapshot records it was gathered through: onetaskgraph at the version this
/// crate pins. `tests/store.rs` holds it to the workspace's pin.
pub const STORE_LINE: &str = "onetaskgraph 0.17.0";

/// The name of the onetaskgraph configuration document a `store` directory holds.
pub const STORE_DOCUMENT: &str = "onetaskgraph.yaml";

/// How many tasks one page of a search asks for.
const PAGE_SIZE: u32 = 100;

/// The type of the link onetaskgraph records from a task closed as a duplicate to its
/// survivor, on both of them.
pub const DUPLICATE_LINK: &str = "duplicate";

/// Whether `name` is a store status category a status role may map to: one onetaskgraph's
/// status vocabulary names, other than the `unknown` it reports for a status it cannot place.
#[must_use]
pub fn is_status_category(name: &str) -> bool {
    category(name).is_some_and(|category| category != StatusCategory::Unknown)
}

fn category(name: &str) -> Option<StatusCategory> {
    serde_json::from_value::<StatusCategory>(Value::from(name)).ok()
}

fn word<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|word| word.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// A store that could not be opened or used.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The `store` setting names something that is not an onetaskgraph configuration.
    #[error(
        "the store `{0}` is not an onetaskgraph configuration: name an onetaskgraph.yaml or the directory holding one"
    )]
    NotAConfiguration(PathBuf),
    /// No `onetaskgraph.yaml` was given or found.
    #[error(
        "no onetaskgraph.yaml was found in {0} or any directory above it; set `store` to the onetaskgraph configuration"
    )]
    NotFound(PathBuf),
    /// onetaskgraph refused its configuration.
    #[error("the store's configuration was refused: {0}")]
    Refused(String),
    /// A value onefollowup handed the store is not one it takes.
    #[error("{0}")]
    Invalid(String),
    /// The store refused or failed an operation.
    #[error("the store refused {operation} on {source_name}: {message}")]
    Failed {
        /// The operation.
        operation: &'static str,
        /// The source it reached.
        source_name: String,
        /// What the store said.
        message: String,
    },
    /// The store refused an operation because the item is private and where it would land is
    /// not: a source not declared private, or a project held public.
    #[error("the store refused {operation} on {source_name}: {message}")]
    Private {
        /// The operation.
        operation: &'static str,
        /// The source it reached.
        source_name: String,
        /// What the store said.
        message: String,
    },
}

/// Who a source's configuration declares can read it: its `visibility` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Declared `public`.
    Public,
    /// Declared `private`: the one visibility onetaskgraph writes a private item to.
    Private,
    /// The key left off, which onetaskgraph never treats as private.
    Unknown,
}

impl Visibility {
    /// The value as the store's configuration spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Unknown => "unknown",
        }
    }
}

/// One source the store offers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Source {
    /// The source's name.
    pub name: String,
    /// Whether it built.
    pub available: bool,
}

/// One call onefollowup made to onetaskgraph: what it was, the source it reached, and how
/// many items it read or wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct StoreOperation {
    /// The call: `list`, `find`, `read`, `copy`, `comment-add-many`, `update-many`, `create-task`,
    /// `update-task`, `create-project`, `relations`, `parent-set-many`, `link-add-many`,
    /// `comments-since` or `comment-edit`.
    pub op: String,
    /// The source it reached; for a call over several, their names joined by `,`.
    pub source: String,
    /// Whether that source is a destination — anything but the drafts source.
    pub destination: bool,
    /// How many items it read or wrote.
    pub items: usize,
    /// For a `find`, how many searches it carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub searches: Option<usize>,
}

/// A status as a source holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ItemStatus {
    /// The store category.
    pub category: String,
    /// The source's own name for it.
    pub name: String,
}

/// One task as a source reported it.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct StoreItem {
    /// `<source>:<native>`.
    pub id: String,
    /// Its title.
    pub title: String,
    /// Its content.
    pub content: String,
    /// Its status.
    pub status: ItemStatus,
    /// Its priority, in the store's vocabulary (`none` for none).
    pub priority: String,
    /// Its labels' names.
    pub labels: Vec<String>,
    /// Its change token.
    pub version: Option<String>,
    /// Where a person opens it.
    pub url: Option<String>,
    /// Its caller-defined metadata.
    pub metadata: BTreeMap<String, Value>,
    /// The project it is filed under, by its source's own id.
    pub project: Option<String>,
    /// The file it is read from, for a source that says.
    pub path: Option<PathBuf>,
    /// Its repositories, as normalized origins (`host/owner/name`). Not part of any record a
    /// verb writes, so a record holding a store item reads and writes as before.
    #[serde(skip)]
    pub repositories: Vec<String>,
    /// When its source says it was created, RFC 3339. Not part of any record, as above.
    #[serde(skip)]
    pub created_at: Option<String>,
}

impl StoreItem {
    fn of(qualified: Qualified<Task>) -> Self {
        let task = qualified.item;
        Self {
            id: qualified.id.to_string(),
            title: task.title,
            content: task.content.unwrap_or_default(),
            status: ItemStatus {
                category: word(&task.status.category),
                name: task.status.name,
            },
            priority: word(&task.priority),
            labels: task.labels.into_iter().map(|label| label.name).collect(),
            version: task.version,
            url: task.url,
            metadata: task.metadata,
            project: task.project.map(|project| project.to_string()),
            path: match task.location {
                Some(Location::Path(path)) => Some(PathBuf::from(path)),
                _ => None,
            },
            repositories: task
                .repositories
                .iter()
                .map(|repository| repository.as_str().to_owned())
                .collect(),
            created_at: task
                .created_at
                .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
        }
    }
}

impl StoreItem {
    /// The survivor this task's metadata links it to as a duplicate, qualified: what a
    /// duplicate close records. `None` when it holds no such link, or holds links that are not
    /// onetaskgraph's list of them.
    #[must_use]
    pub fn duplicate_of(&self) -> Option<String> {
        TaskLink::held(&self.metadata)
            .ok()?
            .into_iter()
            .find(|link| link.link_type == DUPLICATE_LINK && link.direction == LinkDirection::Out)
            .map(|link| link.other)
    }
}

/// One comment as a source reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct StoreComment {
    /// Its id.
    pub id: String,
    /// Its author, where the source names one.
    pub author: Option<String>,
    /// Where it is shown.
    pub url: Option<String>,
    /// When it last changed, RFC 3339.
    pub last_changed: Option<String>,
    /// Its text.
    pub body: String,
}

/// One item read with its comments.
#[derive(Debug, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct ReadItem {
    /// The item, `None` when the source holds no such task.
    pub item: Option<StoreItem>,
    /// Its comments, oldest first; `None` when they could not be read.
    pub comments: Option<Vec<StoreComment>>,
    /// Why it could not be read, when it could not.
    pub error: Option<String>,
}

/// Which text fields a search's text is looked for in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchIn {
    /// Titles.
    Title,
    /// Contents.
    Content,
    /// Either.
    #[default]
    Both,
}

/// One narrowed search of one source.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Search {
    /// The caller's name for it, unique within a call.
    pub key: String,
    /// Text the task holds.
    pub text: Option<String>,
    /// Where the text is looked for.
    pub within: SearchIn,
    /// Metadata values the task holds, each at `<KEY>[/<SEGMENT>…]`.
    pub metadata: Vec<(String, String)>,
    /// The qualified id the task was copied from.
    pub origin: Option<String>,
    /// The status categories kept; empty keeps every one.
    pub statuses: Vec<String>,
    /// The most items of its answer kept; `None` keeps every one. Handed to onetaskgraph's
    /// `task find` as the search's limit, so the store stops reading once it holds this many,
    /// and [`Found::total`] counts what it answered, at most this.
    pub limit: Option<usize>,
}

/// What one search found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The search's key.
    pub key: String,
    /// The tasks it matched, in the store's order, at most its search's limit.
    pub items: Vec<StoreItem>,
    /// How many tasks the store answered it with: every match, or for a limited search at most
    /// its limit, the store having stopped there.
    pub total: usize,
    /// Why it stopped short, when it did.
    pub incomplete: Option<String>,
}

/// A task to create.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTask {
    /// The source.
    pub source: String,
    /// The project it is filed under, by the source's own id.
    pub project: String,
    /// Its title.
    pub title: String,
    /// Its content.
    pub body: String,
    /// Its status category.
    pub status: String,
    /// Its priority, in the store's vocabulary.
    pub priority: String,
    /// Its labels.
    pub labels: Vec<String>,
    /// Its repositories, as normalized origins.
    pub repositories: Vec<String>,
    /// Its metadata.
    pub metadata: BTreeMap<String, Value>,
    /// The one person it is assigned to, for a source holding assignees.
    pub assignee: Option<String>,
}

/// A change to one task: every field named is written, the rest kept.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TaskChange {
    /// Its title.
    pub title: Option<String>,
    /// Its content, replaced whole.
    pub body: Option<String>,
    /// Its status category.
    pub status: Option<String>,
    /// Its priority.
    pub priority: Option<String>,
    /// Its repositories.
    pub repositories: Option<Vec<String>>,
    /// Metadata keys set.
    pub metadata: BTreeMap<String, Value>,
    /// The one person it is assigned to, replacing whoever it held.
    pub assignee: Option<String>,
}

/// A many-item copy into one destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyBatch {
    /// The destination source.
    pub destination: String,
    /// The tasks copied, qualified.
    pub items: Vec<String>,
    /// Whether the caller asserts none has a counterpart there: no origin lookup is made.
    pub create: bool,
    /// Whether an existing counterpart keeps its own status.
    pub preserve_status: bool,
    /// The version each named task's counterpart must be at, keyed by the task.
    pub versions: Vec<(String, String)>,
}

/// What one item of a copy came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    /// The task copied, qualified.
    pub source: String,
    /// `created`, `updated`, `unchanged`, `failed`, `skipped`, `orphaned` or `oversize`.
    pub action: String,
    /// The counterpart written or found.
    pub item: Option<String>,
    /// The counterpart's state after the write, where its answer says.
    pub result: Option<WriteState>,
    /// What the counterpart held, on a refusal after reading it.
    pub held: Option<WriteState>,
    /// Why it failed: the failure's kind and message.
    pub error: Option<(String, String)>,
}

/// An item's state as a write left or found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteState {
    /// Its version.
    pub version: Option<String>,
    /// Its status.
    pub status: Option<ItemStatus>,
    /// Its priority.
    pub priority: Option<String>,
}

/// One comment of a many-comment write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentPost {
    /// The task, qualified.
    pub item: String,
    /// The comment's text.
    pub body: String,
}

/// What became of one comment, or one update, of a batched write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchAnswer {
    /// The task, as named.
    pub item: String,
    /// `written`, `held`, `version-moved` or `error`.
    pub outcome: String,
    /// The comment written, for a comment.
    pub comment: Option<StoreComment>,
    /// The task's version after the write, for an update that says.
    pub version: Option<String>,
    /// The version it was found at, on `version-moved`.
    pub current_version: Option<String>,
    /// Why, on `error`.
    pub reason: Option<String>,
}

/// One held update of a many-update write.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeldUpdate {
    /// The task, qualified.
    pub item: String,
    /// The version it must be at.
    pub if_version: Option<String>,
    /// Its title.
    pub title: Option<String>,
    /// Its content, replaced whole.
    pub body: Option<String>,
    /// Its status category.
    pub status: Option<String>,
    /// Its repositories, replaced whole.
    pub repositories: Option<Vec<String>>,
    /// Its priority.
    pub priority: Option<String>,
    /// Write only while it holds one of these priorities.
    pub when_priority: Vec<String>,
    /// Metadata keys set.
    pub metadata: BTreeMap<String, Value>,
    /// The one person it is assigned to, replacing whoever it held.
    pub assignee: Option<String>,
    /// Write only while it is assigned to exactly these people, `[]` while unassigned.
    pub when_assignees: Option<Vec<String>>,
    /// Close it as a duplicate of this task, qualified, its survivor: the store writes it
    /// `cancelled` and records a [`DUPLICATE_LINK`] link from it to the survivor.
    pub duplicate_of: Option<String>,
}

/// One task's relations as its source holds them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct ItemRelations {
    /// The task, qualified.
    pub item: String,
    /// Its parent, qualified.
    pub parent: Option<String>,
    /// Its children, qualified.
    pub children: Vec<String>,
    /// Every link it holds.
    pub links: Vec<HeldLink>,
}

/// Who wrote a comment, as its source tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Author {
    /// A person.
    Person,
    /// A bot, an integration or an app.
    Bot,
    /// The source cannot say.
    Unknown,
}

/// One comment of a comments-since listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListedComment {
    /// The task it is on, qualified.
    pub item: String,
    /// Its id, as its source issued it.
    pub id: String,
    /// Where a person opens it.
    pub url: Option<String>,
    /// Who wrote it, in the source's spelling.
    pub author: Option<String>,
    /// Whether that is a person, a bot, or neither the source can tell.
    pub author_kind: Author,
    /// When it last changed, RFC 3339.
    pub updated_at: String,
    /// Its version: when it last changed, at the backend's own precision.
    pub version: String,
    /// Its text.
    pub body: String,
}

/// Every comment one source's tasks hold since an instant, and where a later read starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listing {
    /// The instant a later read may start from, set back by the source's consistency window.
    pub watermark: String,
    /// Every comment made or edited since, oldest first.
    pub comments: Vec<ListedComment>,
}

/// What a comments-since listing keeps.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Narrowing {
    /// Metadata values each task holds, each at `<KEY>[/<SEGMENT>…]`.
    pub metadata: Vec<(String, String)>,
    /// The status categories kept; empty keeps every one.
    pub statuses: Vec<String>,
    /// The tasks kept, qualified; empty keeps every one.
    pub items: Vec<String>,
}

/// One link as one endpoint holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct HeldLink {
    /// The link's type name.
    pub link_type: String,
    /// Whether this endpoint is the link's `from`.
    pub outgoing: bool,
    /// The other endpoint, qualified.
    pub other: String,
    /// Whether the other end holds the link too.
    pub other_end: LinkEnd,
}

/// Whether a link's other end holds it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkEnd {
    /// It does.
    Present,
    /// It was read, and does not: a whole-body rewrite there lost it.
    Missing,
    /// It was not among the items read.
    Unread,
}

/// One edge of a many-edge write: a child's parent, or a typed link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeWrite {
    /// The child, or the link's `from`, qualified.
    pub from: String,
    /// The parent, or the link's `to`, qualified.
    pub to: String,
    /// The link's type name; ignored for a parent.
    pub link_type: String,
}

/// What became of one edge of a many-edge write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeAnswer {
    /// For a parent: `written`, `unchanged`, `version-moved`, `refused` or `error`; for a
    /// link: `written`, `exists`, `partial` or `error`.
    pub outcome: String,
    /// Why, when it did not land.
    pub reason: Option<String>,
}

/// An opened store.
pub struct Store {
    engine: Arc<Engine>,
    governor: Arc<Governor>,
    settings: BTreeMap<String, Value>,
    visibilities: BTreeMap<String, Visibility>,
    runtime: tokio::runtime::Runtime,
    local: Option<String>,
    log: RefCell<Vec<StoreOperation>>,
    /// Every source's routes, as the configuration states them.
    routes: Routes,
    /// The classification of each task the store answered, as onetaskgraph holds it: what a
    /// ticket about it would be placed as.
    classified: RefCell<HashMap<String, Classification>>,
    /// What the verb's phases have sent, and where their lines go.
    progress: Arc<Progress>,
}

/// The directory holding the onetaskgraph configuration `store` names, or the nearest one at
/// or above `working_directory` when it names none.
///
/// # Errors
///
/// Refused when `store` is not an onetaskgraph configuration, or none is found.
pub fn directory(store: Option<&Path>, working_directory: &Path) -> Result<PathBuf, StoreError> {
    match store {
        None => working_directory
            .ancestors()
            .find(|directory| directory.join(STORE_DOCUMENT).is_file())
            .map(Path::to_path_buf)
            .ok_or_else(|| StoreError::NotFound(working_directory.to_path_buf())),
        Some(path) if path.is_dir() && path.join(STORE_DOCUMENT).is_file() => {
            Ok(path.to_path_buf())
        }
        Some(path)
            if path.is_file() && path.file_name().is_some_and(|name| name == STORE_DOCUMENT) =>
        {
            Ok(path
                .parent()
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf))
        }
        Some(path) => Err(StoreError::NotAConfiguration(path.to_path_buf())),
    }
}

fn source_name(name: &str) -> Result<SourceName, StoreError> {
    SourceName::new(name)
        .map_err(|error| StoreError::Invalid(format!("`{name}` is not a source name: {error}")))
}

fn global(id: &str) -> Result<GlobalId, StoreError> {
    id.parse::<GlobalId>().map_err(|error| {
        StoreError::Invalid(format!(
            "`{id}` is not a qualified id, `<source>:<native>`: {error}"
        ))
    })
}

fn metadata_keys(
    metadata: &BTreeMap<String, Value>,
) -> Result<BTreeMap<MetadataKey, Value>, StoreError> {
    metadata
        .iter()
        .map(|(key, value)| {
            MetadataKey::new(key.as_str())
                .map(|key| (key, value.clone()))
                .map_err(|error| StoreError::Invalid(format!("metadata key `{key}`: {error}")))
        })
        .collect()
}

fn repositories(origins: &[String]) -> Result<Vec<Repository>, StoreError> {
    origins
        .iter()
        .map(|origin| {
            Repository::try_from(origin.clone()).map_err(|error| {
                StoreError::Invalid(format!("`{origin}` is not a repository origin: {error}"))
            })
        })
        .collect()
}

fn priority(word: &str) -> Result<Priority, StoreError> {
    serde_json::from_value(Value::from(word))
        .map_err(|_| StoreError::Invalid(format!("`{word}` is not a priority")))
}

fn status_category(word: &str) -> Result<StatusCategory, StoreError> {
    category(word).ok_or_else(|| StoreError::Invalid(format!("`{word}` is not a status category")))
}

impl Store {
    /// Open the store: the onetaskgraph configuration at `store` (an `onetaskgraph.yaml` or
    /// the directory holding one), or else the nearest one at or above `working_directory`,
    /// read with `environment` as onetaskgraph reads its own.
    ///
    /// # Errors
    ///
    /// Refused when `store` is not an onetaskgraph configuration, none is found, or
    /// onetaskgraph refuses it.
    pub fn open(
        store: Option<&Path>,
        working_directory: &Path,
        environment: &[(String, String)],
    ) -> Result<Self, StoreError> {
        let directory = directory(store, working_directory)?;
        let environment = Environment::from_pairs(environment.iter().cloned());
        let loaded = config::load(&directory, &environment, &Layer::new(Vec::new()))
            .map_err(|error| StoreError::Refused(error.to_string()))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| StoreError::Refused(format!("no runtime to drive it: {error}")))?;
        let settings = loaded
            .config
            .sources()
            .iter()
            .map(|(name, source)| (name.to_string(), source.config().clone()))
            .collect();
        let visibilities = loaded
            .config
            .sources()
            .iter()
            .map(|(name, source)| {
                let declared = match source.visibility() {
                    SourceVisibility::Public => Visibility::Public,
                    SourceVisibility::Private => Visibility::Private,
                    SourceVisibility::Unknown => Visibility::Unknown,
                };
                (name.to_string(), declared)
            })
            .collect();
        // Every source of one backend kind shares one governor state on the host.
        let backends = loaded
            .config
            .sources()
            .iter()
            .map(|(name, source)| (name.to_string(), source.plugin().as_str().to_owned()))
            .collect();
        let parts = governor::installed();
        let engine_of_readings = Arc::new(OnceLock::new());
        let readings = parts.readings.clone().unwrap_or_else(|| {
            Arc::new(EngineReadings {
                engine: Arc::clone(&engine_of_readings),
            })
        });
        let progress = Arc::new(Progress::new(Arc::clone(&parts.notices)));
        let readings: Arc<dyn Readings> = Arc::new(Watched {
            readings,
            progress: Arc::clone(&progress),
        });
        let governor = Arc::new(Governor::new(
            crate::config::defaults("").governor,
            parts.clock,
            readings,
            parts.notices,
        ));
        governor.set_backends(backends);
        let engine = Arc::new(
            Engine::build(&loaded.config, &loaded.secrets).with_request_gate(Arc::new(Gate {
                governor: Arc::clone(&governor),
                progress: Arc::clone(&progress),
            })),
        );
        // Set once, here, before any call can ask for a reading.
        let _ = engine_of_readings.set(Arc::downgrade(&engine));
        let routes = loaded.config.routes();
        Ok(Self {
            routes,
            classified: RefCell::new(HashMap::new()),
            progress,
            engine,
            governor,
            settings,
            visibilities,
            runtime,
            local: None,
            log: RefCell::new(Vec::new()),
        })
    }

    /// The same store, counting operations on `source` as local rather than a destination's.
    #[must_use]
    pub fn with_local(mut self, source: &str) -> Self {
        self.local = Some(source.to_owned());
        self
    }

    /// The same store, its destination calls paced by `settings` rather than the shipped
    /// defaults.
    #[must_use]
    pub fn with_governor(self, settings: &GovernorSettings) -> Self {
        self.governor.configure(settings.clone());
        self
    }

    /// How many quota readings the governor took so far, which no store operation counts.
    #[must_use]
    pub fn quota_reads(&self) -> u64 {
        self.governor.quota_reads()
    }

    /// What the governor did so far: `None` before any destination call.
    #[must_use]
    pub fn governor_report(&self) -> Option<Report> {
        self.governor.report()
    }

    /// The one chokepoint every call to onetaskgraph passes: `call`, to the sources `named`,
    /// sent once the governor admits it to every destination among them.
    fn send<F: Future>(&self, named: &[&str], kind: Kind, call: F) -> F::Output {
        self.send_creating(named, kind, None, call)
    }

    /// [`Self::send`] for a call that may create up to `contents` content items (comments,
    /// issues), which the governor counts against its content-write caps however the source
    /// batches them into requests.
    fn send_creating<F: Future>(
        &self,
        named: &[&str],
        kind: Kind,
        contents: Option<usize>,
        call: F,
    ) -> F::Output {
        let mut destinations: Vec<String> = named
            .iter()
            .filter(|source| self.local.as_deref() != Some(**source))
            .map(|source| (*source).to_owned())
            .collect();
        destinations.sort_unstable();
        destinations.dedup();
        if destinations.is_empty() {
            return self.runtime.block_on(call);
        }
        self.runtime.block_on(async {
            match contents {
                Some(contents) => {
                    self.governor
                        .admit_content_call(&destinations, kind, contents)
                        .await;
                }
                None => self.governor.admit_call(&destinations, kind).await,
            }
            let answered = call.await;
            self.governor.end_call();
            answered
        })
    }

    /// How many items a call to `named` that may create content takes: the governor's
    /// [`content_chunk`](crate::governor::Governor::content_chunk), so each call's items fit the
    /// content-write caps; all of them for a store governing nothing among `named`.
    fn content_chunk(&self, named: &[&str], items: usize) -> usize {
        let governed = named
            .iter()
            .any(|source| self.local.as_deref() != Some(*source));
        self.governor
            .content_chunk()
            .filter(|_| governed)
            .unwrap_or(items)
            .max(1)
    }

    /// Every source the store's configuration declares, in name order.
    #[must_use]
    pub fn sources(&self) -> Vec<Source> {
        self.engine
            .listing()
            .into_iter()
            .map(|listing| Source {
                name: listing.source.to_string(),
                available: matches!(listing.state, SourceState::Available { .. }),
            })
            .collect()
    }

    /// The setting `key` of the source `name`'s own `config` block, as its configuration
    /// states it — read from the configuration alone, asking the source nothing.
    #[must_use]
    pub fn source_setting(&self, name: &str, key: &str) -> Option<Value> {
        self.settings.get(name)?.get(key).cloned()
    }

    /// Who the store's configuration declares can read the source `name`, as declared, asking
    /// the source nothing. `None` for a source it does not declare.
    #[must_use]
    pub fn declared_visibility(&self, name: &str) -> Option<Visibility> {
        self.visibilities.get(name).copied()
    }

    /// Whether the store's configuration declares the source `name`.
    #[must_use]
    pub fn declares(&self, name: &str) -> bool {
        self.settings.contains_key(name)
    }

    /// Where the quota governor admits a grooming publish's batch before it is sent: `reads`
    /// tasks of `source` read for their status, then `updates` tasks updated in one
    /// `update-many` and `comments` comments added in one `comment add-many`. `groom publish`
    /// calls it once, immediately before its status read, and sends that read and both writes
    /// back to back.
    ///
    /// It waits until the governor's estimate admits the status read and the `updates` writes
    /// (the closes) together, so no wait falls between that read and the closes it decided on.
    /// It reserves nothing and admits nothing on its own: every request of the batch is still
    /// admitted at the store's chokepoint and its request gate, against the floor, the reserve
    /// and the content-write caps. The comments come after the closes are confirmed, so they
    /// are paced there alone. It sends nothing and is no store operation.
    ///
    /// # Errors
    ///
    /// None: the governor only ever waits.
    pub fn admit_batch(
        &self,
        source: &str,
        reads: usize,
        updates: usize,
        comments: usize,
    ) -> Result<(), StoreError> {
        let _ = comments;
        if self.local.as_deref() == Some(source) {
            return Ok(());
        }
        self.runtime
            .block_on(self.governor.admit_batch(source, reads > 0, updates));
        Ok(())
    }

    /// Where a ticket about each of `inputs` — `(id, repository)` — written to `source` lands:
    /// the source onetaskgraph's routes place it in, by its repository and its classification,
    /// read from configuration alone. An input's classification is what the store holds for
    /// the task it names — a draft, read in the same verb, or read here, in one read of every
    /// one not yet read, only when `source` routes anything — and public for an input naming
    /// no task, as a ticket about it is created. Answers one source per input, in order.
    ///
    /// # Errors
    ///
    /// A source or repository the store does not take, or its refusal of the read.
    pub fn routed(
        &self,
        source: &str,
        inputs: &[(&str, Option<&str>)],
    ) -> Result<Vec<String>, StoreError> {
        let name = source_name(source)?;
        if !self.routes.routes(&name) {
            return Ok(vec![source.to_owned(); inputs.len()]);
        }
        let unread: Vec<String> = inputs
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| !self.classified.borrow().contains_key(*id))
            .filter(|id| {
                id.parse::<GlobalId>()
                    .is_ok_and(|global| self.declares(global.source.as_str()))
            })
            .map(str::to_owned)
            .collect();
        if !unread.is_empty() {
            self.read(&unread, false)?;
        }
        inputs
            .iter()
            .map(|(id, repository)| {
                let classification = self
                    .classified
                    .borrow()
                    .get(*id)
                    .copied()
                    .unwrap_or(Classification::Public);
                let repositories = repositories(
                    &repository
                        .map(str::to_owned)
                        .into_iter()
                        .collect::<Vec<_>>(),
                )?;
                Ok(self
                    .routes
                    .place_classified(&name, classification, &repositories)
                    .destination
                    .to_string())
            })
            .collect()
    }

    /// Start one phase of `verb`: one line on standard error naming it and `what` it covers,
    /// then one every [`PROGRESS_EVERY`] requests its calls send, with the requests and points
    /// spent so far, until it is finished. For people: no record holds these lines.
    #[must_use]
    pub fn phase(&self, verb: &str, name: &str, what: &str) -> Phase<'_> {
        self.progress
            .start(&format!("onefollowup {verb}: {name}"), what);
        Phase { store: self }
    }

    /// Every operation made so far, in order.
    #[must_use]
    pub fn operations(&self) -> Vec<StoreOperation> {
        self.log.borrow().clone()
    }

    /// `task` as a store item, its classification remembered.
    fn item(&self, task: Qualified<Task>) -> StoreItem {
        self.classified
            .borrow_mut()
            .insert(task.id.to_string(), task.item.classification);
        StoreItem::of(task)
    }

    fn count(&self, op: &str, source: &str, items: usize) {
        self.log.borrow_mut().push(StoreOperation {
            op: op.to_owned(),
            source: source.to_owned(),
            destination: self.local.as_deref() != Some(source),
            items,
            searches: None,
        });
    }

    fn failed(operation: &'static str, source: &str, message: impl ToString) -> StoreError {
        StoreError::Failed {
            operation,
            source_name: source.to_owned(),
            message: message.to_string(),
        }
    }

    /// Every search of `searches` over the tasks of `source`, in one call.
    ///
    /// # Errors
    ///
    /// The store's refusal of the request.
    pub fn find(&self, source: &str, searches: &[Search]) -> Result<Vec<Found>, StoreError> {
        let name = source_name(source)?;
        let request = FindRequest {
            schema_version: BATCH_SCHEMA_VERSION,
            queries: searches
                .iter()
                .map(|search| {
                    Ok(FindQuery {
                        key: search.key.clone(),
                        text: search.text.clone(),
                        fields: match search.within {
                            SearchIn::Title => FindIn::Title,
                            SearchIn::Content => FindIn::Content,
                            SearchIn::Both => FindIn::Both,
                        },
                        metadata: search.metadata.iter().cloned().collect(),
                        origin: search.origin.as_deref().map(global).transpose()?,
                        status: search
                            .statuses
                            .iter()
                            .map(|word| {
                                category(word).ok_or_else(|| {
                                    StoreError::Invalid(format!(
                                        "`{word}` is not a status category"
                                    ))
                                })
                            })
                            .collect::<Result<_, _>>()?,
                        // The store stops each search's walk once it holds this many.
                        limit: search
                            .limit
                            .map(|limit| {
                                NonZeroU32::new(u32::try_from(limit).unwrap_or(u32::MAX))
                                    .ok_or_else(|| {
                                        StoreError::Invalid(format!(
                                            "the search `{}` keeps no item; a limit is at least 1",
                                            search.key
                                        ))
                                    })
                            })
                            .transpose()?,
                    })
                })
                .collect::<Result<Vec<_>, StoreError>>()?,
        };
        let page = NonZeroU32::new(PAGE_SIZE).unwrap_or(NonZeroU32::MIN);
        let answered = self.send(
            &[source],
            Kind::Read,
            self.engine.find_tasks(&name, &request, page),
        );
        let found = answered.map_err(|error| Self::failed("a search", source, error))?;
        // The operation counts every distinct task the store answered, kept or not.
        let mut seen: Vec<String> = found
            .answers
            .iter()
            .flat_map(|answer| answer.items.iter().map(|item| item.id.to_string()))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        let found: Vec<Found> = found
            .answers
            .into_iter()
            .map(|answer| {
                let total = answer.items.len();
                let limit = searches
                    .iter()
                    .find(|search| search.key == answer.key)
                    .and_then(|search| search.limit)
                    .unwrap_or(usize::MAX);
                Found {
                    key: answer.key,
                    items: answer
                        .items
                        .into_iter()
                        .take(limit)
                        .map(|task| self.item(task))
                        .collect(),
                    total,
                    incomplete: answer.reason,
                }
            })
            .collect();
        self.count("find", source, seen.len());
        if let Some(last) = self.log.borrow_mut().last_mut() {
            last.searches = Some(searches.len());
        }
        Ok(found)
    }

    /// Every task of `source` at one of the status categories `statuses`, bodies included, every
    /// page of it, in one listing.
    ///
    /// # Errors
    ///
    /// A value the store does not take, or its refusal of the listing.
    pub fn list(&self, source: &str, statuses: &[String]) -> Result<Vec<StoreItem>, StoreError> {
        let request = TaskRequest {
            sources: vec![source_name(source)?],
            filters: Filters {
                statuses: statuses
                    .iter()
                    .map(|word| status_category(word))
                    .collect::<Result<_, _>>()?,
                ..Filters::default()
            },
            project: ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: false,
            paging: Paging {
                limit: NonZeroU32::new(PAGE_SIZE).unwrap_or(NonZeroU32::MIN),
                token: None,
            },
        };
        let answered = self.send(&[source], Kind::Read, self.engine.tasks_all(&request));
        let listed = answered.map_err(|error| Self::failed("a task listing", source, error))?;
        if let Some(failure) = listed.errors.into_iter().next() {
            self.count("list", source, 0);
            return Err(Self::failed("a task listing", source, failure.error));
        }
        self.count("list", source, listed.items.len());
        Ok(listed
            .items
            .into_iter()
            .map(|task| self.item(task))
            .collect())
    }

    /// Every task of `ids`, all of one source, each with its comments when `comments` is set,
    /// in one read.
    ///
    /// # Errors
    ///
    /// An id that is not a qualified id.
    pub fn read(&self, ids: &[String], comments: bool) -> Result<Vec<ReadItem>, StoreError> {
        let globals = ids
            .iter()
            .map(|id| global(id))
            .collect::<Result<Vec<_>, _>>()?;
        let mut sources: Vec<String> = globals.iter().map(|id| id.source.to_string()).collect();
        sources.dedup();
        let named: Vec<&str> = sources.iter().map(String::as_str).collect();
        let details = self.send(
            &named,
            Kind::Read,
            self.engine.task_details(&globals, comments),
        );
        for source in &sources {
            let items = globals
                .iter()
                .filter(|id| &id.source.to_string() == source)
                .count();
            self.count("read", source, items);
        }
        Ok(details
            .details
            .into_iter()
            .map(|detail| {
                let error = (!detail.response.errors.is_empty()).then(|| {
                    detail
                        .response
                        .errors
                        .iter()
                        .map(|failure| failure.error.to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                });
                ReadItem {
                    item: detail
                        .response
                        .items
                        .into_iter()
                        .next()
                        .map(|task| self.item(task)),
                    comments: detail.comments.map(|list| {
                        list.into_iter()
                            .map(|comment| StoreComment {
                                id: comment.id.to_string(),
                                author: comment.author,
                                url: comment.url,
                                last_changed: comment
                                    .updated_at
                                    .or(comment.created_at)
                                    .map(|at| at.to_rfc3339_opts(chrono_seconds(), true)),
                                body: comment.body,
                            })
                            .collect()
                    }),
                    error,
                }
            })
            .collect())
    }

    /// Every comment made or edited at or after `since` on the tasks of `source` that
    /// `narrowing` keeps, every page of each, in one listing, with the watermark a later
    /// listing may start from.
    ///
    /// # Errors
    ///
    /// A value the store does not take, or its refusal of the listing.
    pub fn comments_since(
        &self,
        source: &str,
        since: &chrono::DateTime<chrono::Utc>,
        narrowing: &Narrowing,
    ) -> Result<Listing, StoreError> {
        let metadata = narrowing
            .metadata
            .iter()
            .map(|(location, value)| {
                let mut segments = location.split('/').map(str::to_owned);
                let key = segments.next().unwrap_or_default();
                MetadataMatch::new(key, segments.collect(), value.clone())
                    .map_err(StoreError::Invalid)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let request = CommentsSinceRequest {
            source: source_name(source)?,
            since: *since,
            metadata,
            statuses: narrowing
                .statuses
                .iter()
                .map(|word| status_category(word))
                .collect::<Result<_, _>>()?,
            items: narrowing
                .items
                .iter()
                .map(|id| global(id))
                .collect::<Result<_, _>>()?,
            page_size: NonZeroU32::new(PAGE_SIZE).unwrap_or(NonZeroU32::MIN),
        };
        let answered = self.send(&[source], Kind::Read, self.engine.comments_since(&request));
        let listed = answered.map_err(|error| Self::failed("a comments listing", source, error))?;
        let mut items: Vec<String> = listed
            .comments
            .iter()
            .map(|comment| comment.item.to_string())
            .collect();
        items.sort_unstable();
        items.dedup();
        self.count("comments-since", source, items.len());
        Ok(Listing {
            watermark: listed
                .watermark
                .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
            comments: listed
                .comments
                .into_iter()
                .map(|comment| ListedComment {
                    item: comment.item.to_string(),
                    id: comment.id.to_string(),
                    url: comment.url,
                    author: comment.author,
                    author_kind: match comment.author_kind {
                        AuthorKind::Person => Author::Person,
                        AuthorKind::Bot => Author::Bot,
                        AuthorKind::Unknown => Author::Unknown,
                    },
                    updated_at: comment
                        .updated_at
                        .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
                    version: comment.version,
                    body: comment.body,
                })
                .collect(),
        })
    }

    /// Replace the body of the comment `comment` on the task `item`, answering with the comment
    /// as its source now holds it.
    ///
    /// # Errors
    ///
    /// A value the store does not take, or its refusal of the edit.
    pub fn edit_comment(
        &self,
        item: &str,
        comment: &str,
        body: &str,
    ) -> Result<StoreComment, StoreError> {
        let task = global(item)?;
        let source = task.source.to_string();
        let body = CommentBody::new(body)
            .map_err(|error| StoreError::Invalid(format!("a comment body: {error}")))?;
        let edited = self.send(
            &[source.as_str()],
            Kind::Write,
            self.engine
                .edit_comment(&task, &NativeId(comment.to_owned()), &body),
        );
        self.count("comment-edit", &source, 1);
        let edited = edited.map_err(|error| Self::failed("a comment edit", &source, error))?;
        Ok(StoreComment {
            id: edited.id.to_string(),
            author: edited.author,
            url: edited.url,
            last_changed: edited
                .updated_at
                .or(edited.created_at)
                .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)),
            body: edited.body,
        })
    }

    /// Write the project `id` of `source`, creating it or keeping the one there. A `private`
    /// project is created private, or tightened to private when it is held public, so a task
    /// of either classification can be filed under it while each task keeps its own; otherwise
    /// a new project is public and a held one keeps its classification. onetaskgraph writes a
    /// private project only to a source declared private.
    ///
    /// # Errors
    ///
    /// The store's refusal.
    pub fn ensure_project(
        &self,
        source: &str,
        id: &str,
        title: &str,
        metadata: &BTreeMap<String, Value>,
        private: bool,
    ) -> Result<String, StoreError> {
        let request = ProjectCreate {
            source: source_name(source)?,
            id: NativeId(id.to_owned()),
            title: title.to_owned(),
            body: Body::plain(""),
            status: None,
            labels: None,
            repositories: None,
            metadata: metadata_keys(metadata)?,
            classification: private.then_some(Classification::Private),
        };
        let created = self
            .send(&[source], Kind::Write, self.engine.create_project(&request))
            .map_err(|error| Self::failed("a project write", source, error))?;
        self.count("create-project", source, 1);
        Ok(created.id.native.to_string())
    }

    /// Create one task.
    ///
    /// # Errors
    ///
    /// The store's refusal: [`StoreError::Private`] for a task classified private that the
    /// source or its project would not keep private.
    pub fn create_task(&self, task: &NewTask) -> Result<StoreItem, StoreError> {
        let request = TaskCreate {
            source: source_name(&task.source)?,
            project: NativeId(task.project.clone()),
            title: task.title.clone(),
            body: Body::plain(task.body.clone()),
            status: Some(status_category(&task.status)?),
            labels: task.labels.clone(),
            repositories: repositories(&task.repositories)?,
            depends_on: Vec::new(),
            delivers: Vec::new(),
            metadata: metadata_keys(&task.metadata)?,
            assets: Vec::new(),
            classification: Classification::Public,
            assignee: task.assignee.clone(),
        };
        let created = self
            .send(
                &[task.source.as_str()],
                Kind::Write,
                self.engine.create_task(&request),
            )
            .map_err(|error| match error {
                EngineError::PrivateMemberOfPublicProject { .. }
                | EngineError::NotPrivateDestination { .. } => StoreError::Private {
                    operation: "a task create",
                    source_name: task.source.clone(),
                    message: error.to_string(),
                },
                other => Self::failed("a task create", &task.source, other),
            })?;
        self.count("create-task", &task.source, 1);
        let mut item = StoreItem::of(created.task);
        if task.priority != "none" && item.priority == "none" {
            item = self.update_task(
                &item.id,
                &TaskChange {
                    priority: Some(task.priority.clone()),
                    ..TaskChange::default()
                },
            )?;
        }
        Ok(item)
    }

    /// Write `change` to the task `id`.
    ///
    /// # Errors
    ///
    /// The store's refusal.
    pub fn update_task(&self, id: &str, change: &TaskChange) -> Result<StoreItem, StoreError> {
        let global = global(id)?;
        let source = global.source.to_string();
        let update = TaskUpdate {
            title: change.title.clone(),
            content: change.body.clone(),
            status: change
                .status
                .as_deref()
                .map(|word| {
                    status_category(word).map(|category| Status {
                        category,
                        name: word.to_owned(),
                    })
                })
                .transpose()?,
            priority: change.priority.as_deref().map(priority).transpose()?,
            metadata_set: metadata_keys(&change.metadata)?,
            metadata_remove: std::collections::BTreeSet::new(),
            delivers: None,
            depends_on: None,
            repositories: change
                .repositories
                .as_deref()
                .map(repositories)
                .transpose()?,
            assignees: change.assignee.clone().map(|person| vec![person]),
            duplicate_of: None,
        };
        let updated = self
            .send(
                &[source.as_str()],
                Kind::Write,
                self.engine.update_task(&global, &update),
            )
            .map_err(|error| Self::failed("a task update", &source, error))?;
        self.count("update-task", &source, 1);
        Ok(StoreItem::of(Qualified {
            id: updated.id,
            item: updated.task,
        }))
    }

    /// Copy every task `batch` names into its destination: in one call, or — more items than
    /// the governor's content-write caps take in one call, since each may be created — in calls
    /// of as many as they take, in order. A call refused after an earlier one wrote answers each
    /// of its items `failed` with the refusal, so what the earlier calls wrote is still answered.
    ///
    /// # Errors
    ///
    /// The store's refusal of the whole copy, before any item was written.
    pub fn copy(&self, batch: &CopyBatch) -> Result<Vec<Copied>, StoreError> {
        let mut named: Vec<&str> = batch
            .items
            .iter()
            .filter_map(|id| id.split_once(':').map(|(source, _)| source))
            .collect();
        named.push(&batch.destination);
        let chunk = self.content_chunk(&[batch.destination.as_str()], batch.items.len());
        if batch.items.len() <= chunk {
            return self.copy_call(batch, &named);
        }
        let mut copied = Vec::with_capacity(batch.items.len());
        for (at, items) in batch.items.chunks(chunk).enumerate() {
            let part = CopyBatch {
                items: items.to_vec(),
                versions: batch
                    .versions
                    .iter()
                    .filter(|(id, _)| items.contains(id))
                    .map(|(id, version)| (id.clone(), version.clone()))
                    .collect(),
                ..batch.clone()
            };
            match self.copy_call(&part, &named) {
                Ok(answered) => copied.extend(answered),
                Err(error) if at == 0 => return Err(error),
                Err(error) => copied.extend(items.iter().map(|id| Copied {
                    source: id.clone(),
                    action: "failed".to_owned(),
                    item: None,
                    result: None,
                    held: None,
                    error: Some(("store-refused".to_owned(), error.to_string())),
                })),
            }
        }
        Ok(copied)
    }

    /// One copy call of `batch`, to the sources `named`, declared to create up to every item.
    fn copy_call(&self, batch: &CopyBatch, named: &[&str]) -> Result<Vec<Copied>, StoreError> {
        let ids = batch
            .items
            .iter()
            .map(|id| global(id))
            .collect::<Result<Vec<_>, _>>()?;
        let items = CopyItems::new(ids)
            .ok_or_else(|| StoreError::Invalid("a copy names at least one task".to_owned()))?;
        let versions: HashMap<GlobalId, String> = batch
            .versions
            .iter()
            .map(|(id, version)| Ok((global(id)?, version.clone())))
            .collect::<Result<_, StoreError>>()?;
        let request = CopyRequest {
            items,
            scope: CopyScope::Tasks,
            status_policy: if batch.preserve_status {
                StatusPolicy::PreserveDestination
            } else {
                StatusPolicy::Source
            },
            destination: source_name(&batch.destination)?,
            match_by: None,
            target_task: None,
            recreate: false,
            create: batch.create,
            dry_run: false,
            comments: false,
            no_link: false,
            expect: None,
            statuses: Vec::new(),
            conditions: CopyConditions {
                versions,
                ..CopyConditions::default()
            },
        };
        let report = self.send_creating(
            named,
            Kind::Write,
            Some(batch.items.len()),
            self.engine.copy(&request),
        );
        self.count("copy", &batch.destination, batch.items.len());
        let report = report.map_err(|error| Self::failed("a copy", &batch.destination, error))?;
        Ok(report
            .items
            .into_iter()
            .map(|outcome| {
                let state = |result: &onetaskgraph_core::WriteResult| WriteState {
                    version: result.version.clone(),
                    status: result.status.as_ref().map(|status| ItemStatus {
                        category: word(&status.category),
                        name: status.name.clone(),
                    }),
                    priority: result.priority.as_ref().map(word),
                };
                let item = outcome.action.destination().map(ToString::to_string);
                let (action, error) = match &outcome.action {
                    CopyAction::Failed { error } => (
                        "failed",
                        Some((error.kind().to_owned(), error.message().to_owned())),
                    ),
                    CopyAction::Created { .. } => ("created", None),
                    CopyAction::Updated { .. } => ("updated", None),
                    CopyAction::Unchanged { .. } => ("unchanged", None),
                    CopyAction::Orphaned { .. } => ("orphaned", None),
                    CopyAction::Skipped { reason } => {
                        ("skipped", Some(("skipped".to_owned(), reason.clone())))
                    }
                    CopyAction::Oversize { size, limit } => (
                        "failed",
                        Some((
                            "oversize".to_owned(),
                            format!("the body is {size} bytes and the destination holds {limit}"),
                        )),
                    ),
                };
                Copied {
                    source: outcome.source.to_string(),
                    action: action.to_owned(),
                    item,
                    result: outcome.expected.result.as_ref().map(state),
                    held: outcome.held.as_ref().map(state),
                    error,
                }
            })
            .collect())
    }

    /// Add every comment of `posts`, all on tasks of `source`: in one call, or — more than the
    /// governor's content-write caps take in one call — in calls of as many as they take, in
    /// order. A call refused after an earlier one wrote answers each of its comments `error`
    /// with the refusal, so what the earlier calls wrote is still answered.
    ///
    /// # Errors
    ///
    /// The store's refusal of the whole batch, before anything was written.
    pub fn add_comments(
        &self,
        source: &str,
        posts: &[CommentPost],
    ) -> Result<Vec<BatchAnswer>, StoreError> {
        let chunk = self.content_chunk(&[source], posts.len());
        if posts.len() <= chunk {
            return self.add_comments_call(source, posts);
        }
        let mut answers = Vec::with_capacity(posts.len());
        for (at, part) in posts.chunks(chunk).enumerate() {
            match self.add_comments_call(source, part) {
                Ok(answered) => answers.extend(answered),
                Err(error) if at == 0 => return Err(error),
                // No test drives a later call refused whole: onetaskgraph refuses a comment
                // batch whole only for its schema or a source that takes no comments, which the
                // first call meets first, so only a doubled engine — which no store test uses —
                // could. This keeps the earlier calls' written comments answered regardless.
                Err(error) => answers.extend(part.iter().map(|post| BatchAnswer {
                    item: post.item.clone(),
                    outcome: "error".to_owned(),
                    comment: None,
                    version: None,
                    current_version: None,
                    reason: Some(error.to_string()),
                })),
            }
        }
        Ok(answers)
    }

    /// One call adding every comment of `posts`, declared to create each of them.
    fn add_comments_call(
        &self,
        source: &str,
        posts: &[CommentPost],
    ) -> Result<Vec<BatchAnswer>, StoreError> {
        let batch = CommentBatch {
            schema_version: BATCH_SCHEMA_VERSION,
            comments: posts
                .iter()
                .map(|post| CommentBatchItem {
                    item: post.item.clone(),
                    body: post.body.clone(),
                    if_version: None,
                })
                .collect(),
        };
        let name = source_name(source)?;
        let added = self.send_creating(
            &[source],
            Kind::Write,
            Some(posts.len()),
            self.engine.add_comments(&name, &batch),
        );
        self.count("comment-add-many", source, posts.len());
        let added = added.map_err(|error| Self::failed("a comment batch", source, error))?;
        Ok(added
            .results
            .into_iter()
            .zip(posts)
            .map(|(result, post)| BatchAnswer {
                item: result.item,
                outcome: word(&result.outcome),
                comment: result.comment.map(|comment| StoreComment {
                    id: comment.id.to_string(),
                    author: None,
                    url: comment.url,
                    last_changed: comment.version,
                    body: post.body.clone(),
                }),
                version: None,
                current_version: result.current_version,
                reason: result.reason,
            })
            .collect())
    }

    /// Write every held update of `updates`, all of tasks of `source`, in one call.
    ///
    /// # Errors
    ///
    /// The store's refusal of the whole batch, before anything was written.
    pub fn update_many(
        &self,
        source: &str,
        updates: &[HeldUpdate],
    ) -> Result<Vec<BatchAnswer>, StoreError> {
        let batch = UpdateBatch {
            schema_version: BATCH_SCHEMA_VERSION,
            updates: updates
                .iter()
                .map(|update| {
                    Ok(UpdateBatchItem {
                        item: update.item.clone(),
                        if_version: update.if_version.clone(),
                        title: update.title.clone(),
                        body: update.body.clone(),
                        status: update.status.as_deref().map(status_category).transpose()?,
                        repositories: update
                            .repositories
                            .as_deref()
                            .map(repositories)
                            .transpose()?,
                        priority: update.priority.as_deref().map(priority).transpose()?,
                        when_priority: update
                            .when_priority
                            .iter()
                            .map(|word| priority(word))
                            .collect::<Result<_, _>>()?,
                        metadata: update.metadata.clone(),
                        assignee: update.assignee.clone().map(Some),
                        when_assignees: update.when_assignees.clone(),
                        duplicate_of: update.duplicate_of.clone(),
                        ..UpdateBatchItem::default()
                    })
                })
                .collect::<Result<Vec<_>, StoreError>>()?,
        };
        let name = source_name(source)?;
        let updated = self.send(
            &[source],
            Kind::Write,
            self.engine.update_tasks(&name, &batch),
        );
        self.count("update-many", source, updates.len());
        let updated = updated.map_err(|error| Self::failed("an update batch", source, error))?;
        Ok(updated
            .results
            .into_iter()
            .map(|result| BatchAnswer {
                item: result.item,
                outcome: word(&result.outcome),
                comment: None,
                version: result.version,
                current_version: result.current_version,
                reason: result.reason,
            })
            .collect())
    }

    fn count_over(&self, op: &str, ids: &[&str], items: usize) {
        let mut sources: Vec<&str> = ids
            .iter()
            .filter_map(|id| id.split_once(':').map(|(source, _)| source))
            .collect();
        sources.sort_unstable();
        sources.dedup();
        self.log.borrow_mut().push(StoreOperation {
            op: op.to_owned(),
            source: sources.join(","),
            destination: sources
                .iter()
                .any(|source| self.local.as_deref() != Some(*source)),
            items,
            searches: None,
        });
    }

    /// The relations of every task `ids` names, of any sources, in one call: each one's
    /// parent and links, a link's other end compared when that end is among `ids`.
    ///
    /// # Errors
    ///
    /// An id that is not a qualified id, or the store's refusal of the read.
    pub fn relations(&self, ids: &[String]) -> Result<Vec<ItemRelations>, StoreError> {
        let globals = ids
            .iter()
            .map(|id| global(id))
            .collect::<Result<Vec<_>, _>>()?;
        let named: Vec<&str> = ids.iter().map(String::as_str).collect();
        let answered = self.send(
            &sources_of(&named),
            Kind::Read,
            self.engine.task_relations(&globals, false),
        );
        self.count_over("relations", &named, ids.len());
        let first = named.first().copied().unwrap_or_default();
        let answered = answered.map_err(|error| Self::failed("a relations read", first, error))?;
        Ok(answered
            .items
            .into_iter()
            .map(|item| ItemRelations {
                item: item.item.to_string(),
                parent: item.parent.map(|parent| parent.to_string()),
                children: item.children.iter().map(ToString::to_string).collect(),
                links: item
                    .links
                    .into_iter()
                    .map(|link| HeldLink {
                        link_type: link.link_type,
                        outgoing: link.direction == LinkDirection::Out,
                        other: link.other.to_string(),
                        other_end: match link.other_end {
                            OtherEnd::Present => LinkEnd::Present,
                            OtherEnd::Missing => LinkEnd::Missing,
                            OtherEnd::Unread => LinkEnd::Unread,
                        },
                    })
                    .collect(),
            })
            .collect())
    }

    /// Set each child's parent of `edges`, in one call, each answered on its own.
    ///
    /// # Errors
    ///
    /// The store's refusal of the whole batch, before anything was written.
    pub fn set_parents(&self, edges: &[EdgeWrite]) -> Result<Vec<EdgeAnswer>, StoreError> {
        let batch = ParentBatch {
            schema_version: RELATIONS_SCHEMA_VERSION,
            edges: edges
                .iter()
                .map(|edge| ParentEdge {
                    child: edge.from.clone(),
                    parent: Some(edge.to.clone()),
                    if_version: None,
                })
                .collect(),
        };
        let ends: Vec<&str> = edges
            .iter()
            .flat_map(|edge| [edge.from.as_str(), edge.to.as_str()])
            .collect();
        let answered = self.send(
            &sources_of(&ends),
            Kind::Write,
            self.engine.set_parents(&batch),
        );
        let named: Vec<&str> = edges.iter().map(|edge| edge.from.as_str()).collect();
        self.count_over("parent-set-many", &named, edges.len());
        let first = named.first().copied().unwrap_or_default();
        let answered = answered.map_err(|error| Self::failed("a parent batch", first, error))?;
        Ok(answered
            .results
            .into_iter()
            .map(|result| EdgeAnswer {
                outcome: word(&result.outcome),
                reason: result.reason,
            })
            .collect())
    }

    /// Add each typed link of `links`, in one call, each answered on its own.
    ///
    /// # Errors
    ///
    /// The store's refusal of the whole batch, before anything was written.
    pub fn add_links(&self, links: &[EdgeWrite]) -> Result<Vec<EdgeAnswer>, StoreError> {
        let batch = LinkBatch {
            schema_version: RELATIONS_SCHEMA_VERSION,
            links: links
                .iter()
                .map(|link| LinkItem {
                    from: link.from.clone(),
                    to: link.to.clone(),
                    link_type: link.link_type.clone(),
                })
                .collect(),
        };
        let named: Vec<&str> = links
            .iter()
            .flat_map(|link| [link.from.as_str(), link.to.as_str()])
            .collect();
        let answered = self.send(
            &sources_of(&named),
            Kind::Write,
            self.engine.add_links(&batch),
        );
        self.count_over("link-add-many", &named, links.len());
        let first = named.first().copied().unwrap_or_default();
        let answered = answered.map_err(|error| Self::failed("a link batch", first, error))?;
        Ok(answered
            .results
            .into_iter()
            .map(|result| EdgeAnswer {
                outcome: word(&result.outcome),
                reason: result.reason,
            })
            .collect())
    }
}

/// The sources of qualified ids, each once.
fn sources_of<'a>(ids: &[&'a str]) -> Vec<&'a str> {
    let mut sources: Vec<&str> = ids
        .iter()
        .filter_map(|id| id.split_once(':').map(|(source, _)| source))
        .collect();
    sources.sort_unstable();
    sources.dedup();
    sources
}

/// The governor's readings, taken through the store's own engine: `GET /rate_limit` on a
/// GitHub source, nothing on a source that meters nothing.
struct EngineReadings {
    engine: Arc<OnceLock<Weak<Engine>>>,
}

impl Readings for EngineReadings {
    fn read<'a>(&'a self, source: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        Box::pin(async move {
            let engine = self
                .engine
                .get()
                .and_then(Weak::upgrade)
                .ok_or_else(|| "the store is closed".to_owned())?;
            let name = SourceName::new(source).map_err(|error| error.to_string())?;
            let readings = engine
                .quota(&name)
                .await
                .map_err(|error| error.to_string())?;
            Ok(readings.iter().map(reading).collect())
        })
    }
}

fn reading(reading: &QuotaReading) -> Reading {
    Reading {
        resource: reading.resource.clone(),
        limit: reading.limit,
        remaining: reading.remaining,
        resets_at: reading.resets_at,
    }
}

/// The governor as the engine's request gate: asked before every backend request a call sends,
/// told every response's reading. It only ever waits, never refuses. Each request is counted
/// toward the phase in progress, too.
struct Gate {
    governor: Arc<Governor>,
    progress: Arc<Progress>,
}

#[async_trait::async_trait]
impl RequestGate for Gate {
    async fn before_request(&self, request: &GatedRequest) -> Result<(), SourceError> {
        self.progress.hinted(request.cost_hint);
        self.governor
            .before_request(&Request {
                resource: request.resource.clone(),
                cost_hint: request.cost_hint,
                content_creating: request.content_creating,
            })
            .await;
        Ok(())
    }

    fn after_response(&self, readings: &[QuotaReading]) {
        let readings: Vec<Reading> = readings.iter().map(reading).collect();
        self.progress.answered(&readings);
        self.governor.after_response(&readings);
    }

    fn request_ended(&self) {
        self.progress.ended();
        self.governor.request_ended();
    }
}

/// The governor's quota readings, watched by the progress count too, so the first response of
/// a phase has a reading to measure what it cost against.
struct Watched {
    readings: Arc<dyn Readings>,
    progress: Arc<Progress>,
}

impl Readings for Watched {
    fn read<'a>(&'a self, source: &'a str) -> Pending<'a, Result<Vec<Reading>, String>> {
        Box::pin(async move {
            let read = self.readings.read(source).await;
            if let Ok(readings) = &read {
                self.progress.seen(readings);
            }
            read
        })
    }
}

/// How many requests a phase sends between two of its progress lines.
pub const PROGRESS_EVERY: u64 = 20;

/// What the phase in progress has sent: its requests, and the points they spent — each
/// response's fall in remaining from the last reading of its window, or, for a window not read
/// before, the request's own hint.
#[derive(Default)]
struct Counted {
    phase: Option<String>,
    requests: u64,
    points: u64,
    /// The hint of the request last sent, until its response measures it.
    hint: Option<u64>,
    /// The last remaining read of each window, by resource and reset.
    remaining: HashMap<(String, chrono::DateTime<chrono::Utc>), u64>,
}

/// Progress lines for people: one as each phase starts and ends, and one every
/// [`PROGRESS_EVERY`] requests while it runs, on the governor's notices.
struct Progress {
    notices: governor::Notices,
    counted: std::sync::Mutex<Counted>,
}

impl Progress {
    fn new(notices: governor::Notices) -> Self {
        Self {
            notices,
            counted: std::sync::Mutex::new(Counted::default()),
        }
    }

    fn counted(&self) -> std::sync::MutexGuard<'_, Counted> {
        self.counted
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn start(&self, phase: &str, what: &str) {
        {
            let mut counted = self.counted();
            counted.phase = Some(phase.to_owned());
            counted.requests = 0;
            counted.points = 0;
        }
        (self.notices)(&format!("{phase}: started: {what}"));
    }

    fn finish(&self) {
        let line = {
            let mut counted = self.counted();
            counted.phase.take().map(|phase| {
                format!(
                    "{phase}: finished: {} requests, {} points",
                    counted.requests, counted.points
                )
            })
        };
        if let Some(line) = line {
            (self.notices)(&line);
        }
    }

    fn hinted(&self, hint: Option<u64>) {
        self.counted().hint = hint;
    }

    /// Readings taken outside any response: what later falls are measured from.
    fn seen(&self, readings: &[Reading]) {
        let mut counted = self.counted();
        for reading in readings {
            counted.remaining.insert(
                (reading.resource.clone(), reading.resets_at),
                reading.remaining,
            );
        }
    }

    fn answered(&self, readings: &[Reading]) {
        let mut counted = self.counted();
        let mut cost = None;
        for reading in readings {
            let window = (reading.resource.clone(), reading.resets_at);
            if let Some(before) = counted.remaining.insert(window, reading.remaining) {
                *cost.get_or_insert(0) += before.saturating_sub(reading.remaining);
            }
        }
        let hint = counted.hint.take();
        if counted.phase.is_some() {
            counted.points += cost.or(hint).unwrap_or(0);
        }
    }

    fn ended(&self) {
        let line = {
            let mut counted = self.counted();
            match counted.phase.clone() {
                Some(phase) => {
                    counted.requests += 1;
                    counted.requests.is_multiple_of(PROGRESS_EVERY).then(|| {
                        format!(
                            "{phase}: {} requests, {} points so far",
                            counted.requests, counted.points
                        )
                    })
                }
                None => None,
            }
        };
        if let Some(line) = line {
            (self.notices)(&line);
        }
    }
}

/// One phase of a verb in progress, from [`Store::phase`]: finished, it prints its line.
pub struct Phase<'a> {
    store: &'a Store,
}

impl Phase<'_> {
    /// The phase is over: one line naming the requests and points it spent.
    pub fn finish(self) {
        self.store.progress.finish();
    }
}

fn chrono_seconds() -> chrono::SecondsFormat {
    chrono::SecondsFormat::Secs
}
