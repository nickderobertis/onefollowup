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

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use onetaskgraph_core::config::{self, Layer};
use onetaskgraph_core::{
    BATCH_SCHEMA_VERSION, Body, CommentBatch, CommentBatchItem, CopyAction, CopyConditions,
    CopyItems, CopyRequest, CopyScope, Engine, Environment, FindIn, FindQuery, FindRequest,
    GlobalId, LinkBatch, LinkItem, OtherEnd, ParentBatch, ParentEdge, ProjectCreate, Qualified,
    RELATIONS_SCHEMA_VERSION, SourceState, StatusPolicy, TaskCreate, UpdateBatch,
    UpdateBatchItem,
};
use onetaskgraph_plugin_api::{
    Classification, LinkDirection, Location, MetadataKey, NativeId, Priority, Repository,
    SourceName, Status, StatusCategory, Task, TaskUpdate,
};
use serde::Serialize;
use serde_json::Value;

/// The store status categories, in onetaskgraph's spelling, that a status role may map to.
/// `tests/store.rs` reconciles this list with onetaskgraph's own vocabulary.
pub const STATUS_CATEGORIES: &[&str] = &[
    "draft",
    "backlog",
    "todo",
    "queued",
    "in-progress",
    "done",
    "cancelled",
];

/// The name of the onetaskgraph configuration document a `store` directory holds.
pub const STORE_DOCUMENT: &str = "onetaskgraph.yaml";

/// How many tasks one page of a search asks for.
const PAGE_SIZE: u32 = 100;

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
    /// The call: `find`, `read`, `copy`, `comment-add-many`, `update-many`, `create-task`,
    /// `update-task`, `create-project`, `relations`, `parent-set-many` or `link-add-many`.
    pub op: String,
    /// The source it reached; for a call over several, their names joined by `,`.
    pub source: String,
    /// Whether that source is a destination — anything but the drafts source.
    pub destination: bool,
    /// How many items it read or wrote.
    pub items: usize,
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
        }
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
}

/// What one search found.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The search's key.
    pub key: String,
    /// Every task it matched.
    pub items: Vec<StoreItem>,
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
    /// Its content, replaced whole.
    pub body: Option<String>,
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
}

/// One task's relations as its source holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRelations {
    /// The task, qualified.
    pub item: String,
    /// Its parent, qualified.
    pub parent: Option<String>,
    /// Every link it holds.
    pub links: Vec<HeldLink>,
}

/// One link as one endpoint holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldLink {
    /// The link's type name.
    pub link_type: String,
    /// Whether this endpoint is the link's `from`.
    pub outgoing: bool,
    /// The other endpoint, qualified.
    pub other: String,
    /// Whether the other end holds the link too: `present`, `missing` (read, and it does
    /// not) or `unread`.
    pub other_end: String,
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
    engine: Engine,
    settings: BTreeMap<String, Value>,
    runtime: tokio::runtime::Runtime,
    local: Option<String>,
    log: RefCell<Vec<StoreOperation>>,
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
        Ok(Self {
            engine: Engine::build(&loaded.config, &loaded.secrets),
            settings,
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

    /// Whether the store's configuration declares the source `name`.
    #[must_use]
    pub fn declares(&self, name: &str) -> bool {
        self.settings.contains_key(name)
    }

    /// Every operation made so far, in order.
    #[must_use]
    pub fn operations(&self) -> Vec<StoreOperation> {
        self.log.borrow().clone()
    }

    fn count(&self, op: &str, source: &str, items: usize) {
        self.log.borrow_mut().push(StoreOperation {
            op: op.to_owned(),
            source: source.to_owned(),
            destination: self.local.as_deref() != Some(source),
            items,
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
                        status: Vec::new(),
                    })
                })
                .collect::<Result<Vec<_>, StoreError>>()?,
        };
        let page = NonZeroU32::new(PAGE_SIZE).unwrap_or(NonZeroU32::MIN);
        let answered = self
            .runtime
            .block_on(self.engine.find_tasks(&name, &request, page));
        let found = answered.map_err(|error| Self::failed("a search", source, error))?;
        let found: Vec<Found> = found
            .answers
            .into_iter()
            .map(|answer| Found {
                key: answer.key,
                items: answer.items.into_iter().map(StoreItem::of).collect(),
                incomplete: answer.reason,
            })
            .collect();
        let mut seen: Vec<&str> = found
            .iter()
            .flat_map(|one| one.items.iter().map(|item| item.id.as_str()))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        self.count("find", source, seen.len());
        Ok(found)
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
        let details = self
            .runtime
            .block_on(self.engine.task_details(&globals, comments));
        let mut sources: Vec<String> = globals.iter().map(|id| id.source.to_string()).collect();
        sources.dedup();
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
                    item: detail.response.items.into_iter().next().map(StoreItem::of),
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

    /// Write the project `id` of `source`, creating it or keeping the one there.
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
            classification: None,
        };
        let created = self
            .runtime
            .block_on(self.engine.create_project(&request))
            .map_err(|error| Self::failed("a project write", source, error))?;
        self.count("create-project", source, 1);
        Ok(created.id.native.to_string())
    }

    /// Create one task.
    ///
    /// # Errors
    ///
    /// The store's refusal.
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
            .runtime
            .block_on(self.engine.create_task(&request))
            .map_err(|error| Self::failed("a task create", &task.source, error))?;
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
        };
        let updated = self
            .runtime
            .block_on(self.engine.update_task(&global, &update))
            .map_err(|error| Self::failed("a task update", &source, error))?;
        self.count("update-task", &source, 1);
        Ok(StoreItem::of(Qualified {
            id: updated.id,
            item: updated.task,
        }))
    }

    /// Copy every task `batch` names into its destination, in one call.
    ///
    /// # Errors
    ///
    /// The store's refusal of the whole copy, before any item was written.
    pub fn copy(&self, batch: &CopyBatch) -> Result<Vec<Copied>, StoreError> {
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
        let report = self.runtime.block_on(self.engine.copy(&request));
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

    /// Add every comment of `posts`, all on tasks of `source`, in one call.
    ///
    /// # Errors
    ///
    /// The store's refusal of the whole batch, before anything was written.
    pub fn add_comments(
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
        let added = self
            .runtime
            .block_on(self.engine.add_comments(&source_name(source)?, &batch));
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
                        body: update.body.clone(),
                        priority: update.priority.as_deref().map(priority).transpose()?,
                        when_priority: update
                            .when_priority
                            .iter()
                            .map(|word| priority(word))
                            .collect::<Result<_, _>>()?,
                        metadata: update.metadata.clone(),
                        assignee: update.assignee.clone().map(Some),
                        when_assignees: update.when_assignees.clone(),
                        ..UpdateBatchItem::default()
                    })
                })
                .collect::<Result<Vec<_>, StoreError>>()?,
        };
        let updated = self
            .runtime
            .block_on(self.engine.update_tasks(&source_name(source)?, &batch));
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
        let answered = self
            .runtime
            .block_on(self.engine.task_relations(&globals, false));
        let named: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.count_over("relations", &named, ids.len());
        let first = named.first().copied().unwrap_or_default();
        let answered = answered.map_err(|error| Self::failed("a relations read", first, error))?;
        Ok(answered
            .items
            .into_iter()
            .map(|item| ItemRelations {
                item: item.item.to_string(),
                parent: item.parent.map(|parent| parent.to_string()),
                links: item
                    .links
                    .into_iter()
                    .map(|link| HeldLink {
                        link_type: link.link_type,
                        outgoing: link.direction == LinkDirection::Out,
                        other: link.other.to_string(),
                        other_end: match link.other_end {
                            OtherEnd::Present => "present",
                            OtherEnd::Missing => "missing",
                            OtherEnd::Unread => "unread",
                        }
                        .to_owned(),
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
        let answered = self.runtime.block_on(self.engine.set_parents(&batch));
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
        let answered = self.runtime.block_on(self.engine.add_links(&batch));
        let named: Vec<&str> = links
            .iter()
            .flat_map(|link| [link.from.as_str(), link.to.as_str()])
            .collect();
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

fn chrono_seconds() -> chrono::SecondsFormat {
    chrono::SecondsFormat::Secs
}
