//! `draft`: record one draft in the drafts source, under the batch's project, with the
//! caller's author label and stamps.

use std::collections::BTreeMap;
use std::io::Read as _;

use serde_json::Value;

use crate::api::{DraftArgs, Drafted, SCHEMA_VERSION};
use crate::config::Settings;
use crate::error::Error;
use crate::formats::Versioned;
use crate::formats::draft::{Author, AuthorKind, Dispatch, DraftRecord, DraftsProjectRecord};
use crate::store::{NewTask, Store, StoreError, Visibility};
use crate::verify::context::Batch;
use crate::verify::ticket::{ORIGIN, TITLE_LIMIT};

/// The stamps a draft records, each where its record keeps it.
pub const STAMPS: &[&str] = &[
    "node",
    "member",
    "scratch",
    "session",
    "cwd",
    "branch",
    "head",
    "transcript",
    "run_id",
];

/// What a node draft names when nothing names its node.
pub const UNRESOLVED: &str = "unresolved";

fn author(label: Option<&str>, stamps: &BTreeMap<String, String>) -> Result<Author, Error> {
    let label = label.unwrap_or("node");
    let (kind, name) = label
        .split_once(':')
        .map_or((label, None), |(kind, name)| (kind, Some(name.to_owned())));
    let refused = || {
        Error::Refused(format!(
            "the author `{label}` is not `node[:<node>]`, `monitor:<member>`, `pacemaker:<member>` or `manager`"
        ))
    };
    if name.as_deref().is_some_and(str::is_empty) {
        return Err(refused());
    }
    Ok(match kind {
        "node" => Author {
            kind: AuthorKind::Node,
            node: Some(
                name.or_else(|| stamps.get("node").cloned())
                    .unwrap_or_else(|| UNRESOLVED.to_owned()),
            ),
            member: None,
        },
        "monitor" | "pacemaker" => Author {
            kind: if kind == "monitor" {
                AuthorKind::Monitor
            } else {
                AuthorKind::Pacemaker
            },
            node: None,
            member: Some(
                name.or_else(|| stamps.get("member").cloned())
                    .ok_or_else(refused)?,
            ),
        },
        "manager" if name.is_none() => Author {
            kind: AuthorKind::Manager,
            node: None,
            member: None,
        },
        _ => return Err(refused()),
    })
}

/// Whether a batch's drafts project in `source` is created private: when the drafts source is
/// declared private, so a draft about a private repository — or one the store cannot classify,
/// which it treats as private — can be filed under it beside a public one, each draft keeping
/// its own classification. onetaskgraph is the one authority on visibility, and writes a
/// private item only to a source declared private, so a source declared public or declaring
/// nothing keeps a public drafts project and refuses a private draft.
#[must_use]
pub fn drafts_project_private(store: &Store, source: &str) -> bool {
    store.declared_visibility(source) == Some(Visibility::Private)
}

/// A private draft the drafts source would not keep private, named with the source's declared
/// visibility and what records it.
fn private_draft(error: StoreError, store: &Store, source: &str) -> Error {
    match error {
        StoreError::Private { message, .. } => Error::Refused(format!(
            "the draft is private and was not recorded: the drafts source `{source}` is declared `{declared}`, so a batch's drafts project there is public and holds no private draft; nothing was written\nthe store said: {said}\nnext: declare `visibility: private` on the drafts source `{source}` in its onetaskgraph configuration (`onetaskgraph.yaml`), whose backend must agree, then draft again",
            declared = store
                .declared_visibility(source)
                .map_or("unknown", Visibility::as_str),
            // The store's own next step names a project write onefollowup makes, not the host.
            said = message.lines().next().unwrap_or_default(),
        )),
        other => Error::Store(other),
    }
}

/// Record one draft.
///
/// # Errors
///
/// A refused batch id, title, repository, author or stamp, an empty body, or the store's
/// refusal.
pub fn run(settings: &Settings, args: DraftArgs) -> Result<Drafted, Error> {
    let batch = Batch::new(settings, &args.batch)?;
    let title = args.title.trim().to_owned();
    if title.is_empty()
        || title.chars().count() > TITLE_LIMIT
        || title.chars().any(char::is_control)
    {
        return Err(Error::Refused(format!(
            "a draft's title is one line of 1 to {TITLE_LIMIT} characters"
        )));
    }
    if !ORIGIN.is_match(&args.repository) {
        return Err(Error::Refused(format!(
            "`{}` is not a normalized repository origin, `host/owner/name`",
            args.repository
        )));
    }
    let mut stamps = BTreeMap::new();
    for stamp in &args.stamps {
        let (key, value) = stamp.split_once('=').unwrap_or((stamp, ""));
        if !STAMPS.contains(&key) {
            return Err(Error::Refused(format!(
                "`{key}` is not a stamp a draft records; the stamps are {}",
                STAMPS.join(", ")
            )));
        }
        if value.chars().any(char::is_control) {
            return Err(Error::Refused(format!(
                "the stamp `{key}` carries a control character"
            )));
        }
        stamps.insert(key.to_owned(), value.to_owned());
    }
    let body = match args.body {
        Some(body) => body,
        None => {
            let mut body = String::new();
            std::io::stdin()
                .read_to_string(&mut body)
                .map_err(|error| Error::Failed(format!("stdin could not be read: {error}")))?;
            body
        }
    };
    if body.trim().is_empty() {
        return Err(Error::Refused(
            "a draft's body, read from stdin, is empty".to_owned(),
        ));
    }
    let author = author(args.author.as_deref(), &stamps)?;
    let stamp = |key: &str| stamps.get(key).cloned().filter(|value| !value.is_empty());
    let record = DraftRecord {
        batch: batch.batch.clone(),
        author,
        dispatch: Dispatch {
            scratch: stamp("scratch"),
            session: stamp("session"),
            cwd: stamp("cwd").unwrap_or_else(|| settings.working_directory().display().to_string()),
            branch: stamp("branch"),
            head: stamp("head"),
        },
        transcript: stamp("transcript"),
        repository: args.repository.clone(),
        paths: args.paths.clone(),
        drafted_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        run_id: stamp("run_id"),
    };
    let store = batch.store()?;
    let source = &settings.config().drafts_source;
    let project_record = DraftsProjectRecord {
        batch: batch.batch.clone(),
        run_id: record.run_id.clone(),
    };
    let project = store.ensure_project(
        source,
        &batch.batch,
        &batch.batch,
        &BTreeMap::from([(
            format!("{}.follow-up-drafts", settings.config().namespace),
            json(&project_record.render()),
        )]),
        drafts_project_private(&store, source),
    )?;
    let created = store
        .create_task(&NewTask {
            source: source.clone(),
            project,
            title,
            body: body.trim().to_owned() + "\n",
            status: "todo".to_owned(),
            priority: "none".to_owned(),
            labels: Vec::new(),
            repositories: vec![args.repository],
            metadata: BTreeMap::from([(batch.draft_key(), json(&record.render()))]),
            assignee: None,
        })
        .map_err(|error| private_draft(error, &store, source))?;
    Ok(Drafted {
        schema_version: SCHEMA_VERSION,
        batch: batch.batch,
        draft: created.id,
        path: created.path.unwrap_or_default(),
    })
}

fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}
