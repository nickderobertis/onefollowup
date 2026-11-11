//! The library's verbs: **one public function per command of the CLI**, each taking the
//! resolved [`Settings`] and the same arguments the command parses, and returning the typed
//! result the command prints. The CLI is a thin layer over these (`tests/parity.rs` holds
//! that), so anything the command line does, a Rust caller does by calling the function.
//!
//! The verify loop — [`draft`], [`prepare`], the agent's `ticket_*` tools, [`check`],
//! [`publish`] and [`settle`] — and [`config_show`] and [`kinds`] are implemented; every
//! other verb refuses with [`Error::NotImplemented`], after its arguments are parsed.

use std::path::PathBuf;

use clap::{ArgGroup, Args};
use indexmap::IndexMap;
use serde::Serialize;

use crate::config::{
    Config, Disposition, Duplicates, FieldDeclaration, Origin, PriorityModel, Relation, Route,
    Settings, StatusRole,
};
use crate::error::Error;
use crate::formats::outbox::StagedWrite;
use crate::formats::{HeldPriority, Level};
use crate::store::StoreOperation;
use crate::verify;

fn no_operations(operations: &[StoreOperation]) -> bool {
    operations.is_empty()
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

/// The version of every JSON document a verb prints.
pub const SCHEMA_VERSION: u32 = 1;

fn not_implemented<T>(verb: &'static str) -> Result<T, Error> {
    Err(Error::NotImplemented { verb })
}

/// A `KEY=VALUE` argument, refused at parse time when it is not one.
fn key_value(text: &str) -> Result<String, String> {
    match text.split_once('=') {
        Some((key, _)) if !key.is_empty() => Ok(text.to_owned()),
        _ => Err(format!("`{text}` is not KEY=VALUE")),
    }
}

/// The status an item holds: the store's category and the destination's own name for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemStatus {
    /// The store status category.
    pub category: String,
    /// The destination's own name for it.
    pub name: String,
}

/// What `draft` recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drafted {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// The draft, as the store qualifies it.
    pub draft: String,
    /// The draft's file.
    pub path: PathBuf,
}

/// What `ticket validate` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TicketValidated {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// The ticket.
    pub ticket: String,
    /// Whether it breaks no rule.
    pub sound: bool,
    /// Each rule it breaks.
    pub problems: Vec<String>,
}

/// One comment on a destination item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardComment {
    /// The comment's id.
    pub id: String,
    /// Its author, when the destination names one.
    pub author: Option<String>,
    /// Its URL.
    pub url: Option<String>,
    /// When it last changed.
    pub last_changed: String,
    /// Its text.
    pub body: String,
}

/// What `ticket board-status` read of one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoardStatus {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// The item, qualified.
    pub item: String,
    /// Its title.
    pub title: String,
    /// Its status.
    pub status: Option<ItemStatus>,
    /// Its priority.
    pub priority: HeldPriority,
    /// Its version.
    pub version: Option<String>,
    /// Its URL.
    pub url: Option<String>,
    /// Its comments.
    pub comments: Vec<BoardComment>,
    /// The status role it is at, or a ticket would be copied at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_role: Option<StatusRole>,
    /// The priority its facts estimate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_estimate: Option<Level>,
    /// For a ticket of an estimated kind: the `## Impact` estimate line its copy carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_line: Option<String>,
}

/// One item a search found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    /// The item, qualified.
    pub item: String,
    /// Its title.
    pub title: String,
    /// Its status.
    pub status: Option<ItemStatus>,
    /// Its URL.
    pub url: Option<String>,
}

/// What `ticket search` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchResults {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// The ticket the question is for.
    pub ticket: String,
    /// The question.
    pub query: String,
    /// What it found.
    pub hits: Vec<SearchHit>,
    /// Each store operation it made.
    #[serde(default, skip_serializing_if = "no_operations")]
    pub store_operations: Vec<StoreOperation>,
}

/// What `ticket copy|comment|re-estimate|relate` staged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Staged {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// The write's position in the batch's outbox.
    pub index: usize,
    /// The outbox entry staged.
    pub write: StagedWrite,
    /// For a copy of a ticket of a `same` kind: the item or staged ticket it was treated as,
    /// the write above being its evidence there (or that ticket's own copy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched: Option<String>,
    /// Each store operation it made.
    #[serde(default, skip_serializing_if = "no_operations")]
    pub store_operations: Vec<StoreOperation>,
}

/// One rule a batch breaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// What breaks it: an input, a ticket or a staged write.
    pub subject: String,
    /// The rule, and what to do.
    pub message: String,
}

/// What `check` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Checked {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// Whether the batch breaks no rule.
    pub sound: bool,
    /// Each rule it breaks.
    pub problems: Vec<Problem>,
}

/// The operation of a staged write, as the outbox spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StagedOp {
    /// A ticket's copy.
    Copy,
    /// A comment.
    Comment,
    /// A re-estimate.
    ReEstimate,
    /// An edge.
    Relate,
}

/// One staged write's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishOutcome {
    /// The write's position in the outbox.
    pub index: usize,
    /// What it was.
    pub op: StagedOp,
    /// The destination item written, qualified.
    pub item: Option<String>,
    /// The receipt's action, or the error's kind.
    pub outcome: String,
    /// Why it did not land, when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What `publish` sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Published {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// The receipt journal every outcome is in.
    pub receipts: PathBuf,
    /// Each staged write's outcome.
    pub outcomes: Vec<PublishOutcome>,
    /// Each store operation it made.
    #[serde(default, skip_serializing_if = "no_operations")]
    pub store_operations: Vec<StoreOperation>,
}

/// A hook that ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookRun {
    /// Its argv.
    pub argv: Vec<String>,
    /// Its exit code.
    pub exit: i32,
}

/// What `settle` closed out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Settled {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// How many writes no answer confirmed, read back.
    pub read_back: usize,
    /// How many of those the read confirmed.
    pub confirmed: usize,
    /// The `batch_settled` hook, when one is configured.
    pub hook: Option<HookRun>,
    /// The working files retired.
    pub retired: Vec<PathBuf>,
    /// How many writes did not certainly land, now in `pending.json`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending: usize,
    /// Whether any did not: a person should look at `pending.json`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub needs_attention: bool,
    /// Each store operation it made.
    #[serde(default, skip_serializing_if = "no_operations")]
    pub store_operations: Vec<StoreOperation>,
}

/// What `comments gather` gathered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gathered {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The snapshot taken; `None` on a dry run.
    pub snapshot: Option<PathBuf>,
    /// Whether it was a dry run.
    pub dry_run: bool,
    /// How many items it read.
    pub items: usize,
    /// How many comments each batch is to answer.
    pub selected: IndexMap<String, usize>,
    /// How many comments or items it left out.
    pub left: usize,
}

/// One answering dispatch a snapshot launches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Launch {
    /// The batch that answers.
    pub batch: String,
    /// Its feedback file.
    pub feedback: PathBuf,
}

/// What `comments launches` planned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Launches {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The snapshot.
    pub snapshot: PathBuf,
    /// Each dispatch.
    pub launches: Vec<Launch>,
}

/// What `comments check` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommentsChecked {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The feedback file.
    pub feedback: PathBuf,
    /// Whether its responses break no rule.
    pub sound: bool,
    /// Each rule they break.
    pub problems: Vec<String>,
}

/// One launched batch's settled answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettledLaunch {
    /// The batch.
    pub batch: String,
    /// Its feedback file.
    pub feedback: PathBuf,
    /// How many writes it made.
    pub writes: usize,
    /// How many writes are pending.
    pub pending: usize,
}

/// What `comments settle` settled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommentsSettled {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The snapshot.
    pub snapshot: PathBuf,
    /// Each launched batch.
    pub settled: Vec<SettledLaunch>,
}

/// One status role against the destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoleStatus {
    /// The role.
    pub role: StatusRole,
    /// The store category it maps to.
    pub category: String,
    /// The destination's own names for that category.
    pub destination_names: Vec<String>,
}

/// What `statuses` read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusesShown {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The destination read.
    pub destination: String,
    /// Each role.
    pub roles: Vec<RoleStatus>,
}

/// `onefollowup draft`: record one draft. Its body is read from stdin.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct DraftArgs {
    /// The batch the draft belongs to.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// The draft's title.
    #[arg(long, value_name = "T")]
    pub title: String,
    /// The repository it is about, as an origin.
    #[arg(long, value_name = "ORIGIN")]
    pub repository: String,
    /// A path it names; repeat for each.
    #[arg(long = "path", value_name = "P")]
    pub paths: Vec<String>,
    /// Who drafted it.
    #[arg(long, value_name = "LABEL")]
    pub author: Option<String>,
    /// A stamp recorded with it; repeat for each.
    #[arg(long = "stamp", value_name = "KEY=VALUE", value_parser = key_value)]
    pub stamps: Vec<String>,
    /// The body; `None` reads it from stdin.
    #[arg(skip)]
    pub body: Option<String>,
}

/// Record one draft.
///
/// # Errors
///
/// A refused batch id, title, repository, author or stamp, an empty body, or the store's
/// refusal.
pub fn draft(settings: &Settings, args: DraftArgs) -> Result<Drafted, Error> {
    verify::draft::run(settings, args)
}

/// `onefollowup prepare`: snapshot a batch's inputs and write the verifying agent's task.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct PrepareArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// Another id the batch's drafts were recorded under; repeat for each.
    #[arg(long = "alias", value_name = "ID")]
    pub aliases: Vec<String>,
    /// Prepare only the inputs no answer accounts for yet.
    #[arg(long)]
    pub unanswered: bool,
}

/// What `prepare` prepared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Prepared {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The batch.
    pub batch: String,
    /// How many drafts were snapshotted.
    pub drafts: usize,
    /// How many additional inputs `hooks.batch_preparing` supplied.
    pub inputs: usize,
    /// Where the verifying agent writes its answers.
    pub answers: PathBuf,
    /// The verifying agent's rendered task.
    pub task_template: PathBuf,
    /// Each store operation it made.
    #[serde(default, skip_serializing_if = "no_operations")]
    pub store_operations: Vec<StoreOperation>,
}

/// Snapshot a batch's input set, make its destination reads, and write the verifying
/// agent's task.
///
/// # Errors
///
/// A refused batch id or alias, a failing `hooks.batch_preparing` command (nothing
/// snapshotted and nothing read), a template that cannot be rendered, or the store's refusal.
pub fn prepare(settings: &Settings, args: PrepareArgs) -> Result<Prepared, Error> {
    verify::prepare::run(settings, args)
}

/// `onefollowup ticket validate`: hold one ticket to the rules.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TicketValidateArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// The ticket: its root cause, or its file.
    #[arg(value_name = "TICKET")]
    pub ticket: String,
}

/// Hold one ticket to the rules.
///
/// # Errors
///
/// A batch id that is none, or no such ticket file.
pub fn ticket_validate(
    settings: &Settings,
    args: TicketValidateArgs,
) -> Result<TicketValidated, Error> {
    verify::tools::validate(settings, args)
}

/// `onefollowup ticket board-status`: read one destination item's state and comments.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TicketBoardStatusArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// The item, qualified.
    #[arg(value_name = "ITEM")]
    pub item: String,
}

/// Read one destination item's state and comments, from the batch's prepared reads.
///
/// # Errors
///
/// A batch not prepared, or an item it did not read.
pub fn ticket_board_status(
    settings: &Settings,
    args: TicketBoardStatusArgs,
) -> Result<BoardStatus, Error> {
    verify::tools::board_status(settings, args)
}

/// `onefollowup ticket search`: search the destination for a ticket's duplicates.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TicketSearchArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// The ticket the question is for.
    #[arg(long, value_name = "TICKET")]
    pub ticket: String,
    /// What to search for.
    #[arg(value_name = "QUERY")]
    pub query: String,
    /// A further question, asked in the same search; repeat for each.
    #[arg(long = "query", value_name = "QUERY")]
    pub queries: Vec<String>,
}

/// Search the destination, from the batch's prepared reads.
///
/// # Errors
///
/// A batch not prepared, a question narrowed by nothing, or the store's refusal.
pub fn ticket_search(settings: &Settings, args: TicketSearchArgs) -> Result<SearchResults, Error> {
    verify::tools::search(settings, args)
}

/// `onefollowup ticket copy`: stage a ticket's copy to its destination.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TicketCopyArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// The ticket: its root cause, or its file.
    #[arg(value_name = "TICKET")]
    pub ticket: String,
}

/// Stage a ticket's copy into the batch's outbox; nothing is written to the destination.
///
/// # Errors
///
/// A batch not prepared, or a ticket `ticket validate` refuses: nothing is staged.
pub fn ticket_copy(settings: &Settings, args: TicketCopyArgs) -> Result<Staged, Error> {
    verify::tools::copy(settings, args)
}

/// `onefollowup ticket comment`: stage the batch's evidence comment on an item. The
/// comment's text is read from stdin.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TicketCommentArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// The item, qualified.
    #[arg(long, value_name = "ITEM")]
    pub item: String,
    /// The root cause the evidence is for.
    #[arg(long, value_name = "ROOT_CAUSE")]
    pub root_cause: String,
    /// The evidence; `None` reads it from stdin.
    #[arg(skip)]
    pub evidence: Option<String>,
}

/// Stage an evidence comment into the batch's outbox.
///
/// # Errors
///
/// A batch not prepared, a root cause that is not a slug, an item the batch did not
/// read or that is its own, or empty evidence: nothing is staged.
pub fn ticket_comment(settings: &Settings, args: TicketCommentArgs) -> Result<Staged, Error> {
    verify::tools::comment(settings, args)
}

/// `onefollowup ticket re-estimate`: stage an item's re-estimate.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TicketReEstimateArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// The item, qualified.
    #[arg(value_name = "ITEM")]
    pub item: String,
}

/// Stage a re-estimate into the batch's outbox.
///
/// # Errors
///
/// A batch not prepared, or an item whose occurrences it did not read: nothing is staged.
pub fn ticket_re_estimate(
    settings: &Settings,
    args: TicketReEstimateArgs,
) -> Result<Staged, Error> {
    verify::tools::re_estimate(settings, args)
}

/// `onefollowup ticket relate`: stage an edge of a configured relation.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TicketRelateArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
    /// The relation's name.
    #[arg(long, value_name = "NAME")]
    pub relation: String,
    /// The `from` ticket.
    #[arg(long, value_name = "TICKET")]
    pub from: String,
    /// The `to` ticket.
    #[arg(long, value_name = "TICKET")]
    pub to: String,
}

/// Stage an edge into the batch's outbox.
///
/// # Errors
///
/// [`Error::NotImplemented`] in this release.
pub fn ticket_relate(_settings: &Settings, _args: TicketRelateArgs) -> Result<Staged, Error> {
    not_implemented("ticket relate")
}

/// The arguments of a verb over one batch.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct BatchArgs {
    /// The batch.
    #[arg(long, value_name = "ID")]
    pub batch: String,
}

/// Hold the batch's dispositions, tickets and staged writes to the rules.
///
/// # Errors
///
/// A batch not prepared, or a working file that cannot be read or written.
pub fn check(settings: &Settings, args: BatchArgs) -> Result<Checked, Error> {
    verify::check::run(settings, args)
}

/// Send the batch's staged writes and receipt every outcome.
///
/// # Errors
///
/// A batch not prepared; an outbox the latest passing `check` did not hold exactly as it
/// stands, refused with nothing written; or a journal that could not record every answer.
pub fn publish(settings: &Settings, args: BatchArgs) -> Result<Published, Error> {
    verify::publish::run(settings, args)
}

/// Close out a published batch.
///
/// # Errors
///
/// A batch not prepared, the store's refusal of the read-back, or a `hooks.batch_settled`
/// command that could not run.
pub fn settle(settings: &Settings, args: BatchArgs) -> Result<Settled, Error> {
    verify::settle::run(settings, args)
}

/// `onefollowup comments gather`: gather people's comments to answer.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
#[command(group(ArgGroup::new("scope").args(["batch", "item"])))]
pub struct CommentsGatherArgs {
    /// Only this batch's tickets.
    #[arg(long, value_name = "ID")]
    pub batch: Option<String>,
    /// Only this item.
    #[arg(long, value_name = "ID")]
    pub item: Option<String>,
    /// Report what would be gathered, writing nothing.
    #[arg(long)]
    pub dry_run: bool,
}

/// Gather comments into a snapshot.
///
/// # Errors
///
/// [`Error::NotImplemented`] in this release.
pub fn comments_gather(_settings: &Settings, _args: CommentsGatherArgs) -> Result<Gathered, Error> {
    not_implemented("comments gather")
}

/// `onefollowup comments launches`: the answering dispatches a snapshot launches.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct CommentsLaunchesArgs {
    /// The snapshot.
    #[arg(long, value_name = "S")]
    pub snapshot: PathBuf,
}

/// Plan a snapshot's launches.
///
/// # Errors
///
/// [`Error::NotImplemented`] in this release.
pub fn comments_launches(
    _settings: &Settings,
    _args: CommentsLaunchesArgs,
) -> Result<Launches, Error> {
    not_implemented("comments launches")
}

/// `onefollowup comments check`: hold a gathering's responses to the rules.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct CommentsCheckArgs {
    /// The feedback file.
    #[arg(long, value_name = "F")]
    pub feedback: PathBuf,
}

/// Check a gathering's responses.
///
/// # Errors
///
/// [`Error::NotImplemented`] in this release.
pub fn comments_check(
    _settings: &Settings,
    _args: CommentsCheckArgs,
) -> Result<CommentsChecked, Error> {
    not_implemented("comments check")
}

/// `onefollowup comments settle`: settle a snapshot's answers.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct CommentsSettleArgs {
    /// The snapshot.
    #[arg(long, value_name = "S")]
    pub snapshot: PathBuf,
    /// A launched batch and its feedback file; repeat for each.
    #[arg(long = "launched", value_name = "BATCH=FEEDBACK", value_parser = key_value)]
    pub launched: Vec<String>,
}

/// Settle a snapshot's answers.
///
/// # Errors
///
/// [`Error::NotImplemented`] in this release.
pub fn comments_settle(
    _settings: &Settings,
    _args: CommentsSettleArgs,
) -> Result<CommentsSettled, Error> {
    not_implemented("comments settle")
}

/// The arguments of a verb that takes none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Args)]
pub struct NoArgs {}

/// Read the destination's statuses against the configured status roles.
///
/// # Errors
///
/// [`Error::NotImplemented`] in this release.
pub fn statuses(_settings: &Settings, _args: NoArgs) -> Result<StatusesShown, Error> {
    not_implemented("statuses")
}

/// `onefollowup kinds`: the effective kinds, relations, dispositions and routes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KindsShown {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The declared fields.
    pub fields: IndexMap<String, FieldDeclaration>,
    /// Every kind, as the verifying agent reads it.
    pub kinds: Vec<KindShown>,
    /// The kind a record without one is read as.
    pub default_kind: String,
    /// The relations.
    pub relations: Vec<Relation>,
    /// The disposition catalogue.
    pub dispositions: Vec<Disposition>,
    /// The dispositions every kind allows unless it lists its own.
    pub default_dispositions: Option<Vec<String>>,
    /// The routes, in order.
    pub routes: Vec<Route>,
}

/// One kind with every default applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KindShown {
    /// The kind's name.
    pub name: String,
    /// What it is.
    pub description: String,
    /// Its ticket template (`builtin:` names the shipped one).
    pub ticket_template: String,
    /// How its duplicates are found.
    pub duplicates: Duplicates,
    /// How its tickets are given a priority.
    pub priority: PriorityModel,
    /// The role a new ticket is copied in.
    pub initial_status: StatusRole,
    /// The source its tickets go to.
    pub destination: String,
    /// The dispositions its inputs may be given.
    pub dispositions: Vec<String>,
    /// Where `dispositions` came from.
    pub dispositions_from: DispositionsFrom,
}

/// Where a kind's allowed dispositions came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DispositionsFrom {
    /// The kind's own `dispositions`.
    Kind,
    /// The top-level `default_dispositions`.
    DefaultDispositions,
    /// Neither, so every catalogued disposition.
    Catalogue,
}

/// The shipped ticket template's name.
pub const BUILTIN_TICKET_TEMPLATE: &str = "builtin:ticket-template.md.j2";

/// The effective kinds, relations, dispositions and routes, as the verifying agent reads
/// them: each kind with its ticket template, destination and allowed dispositions resolved.
///
/// # Errors
///
/// Never in this release; the signature leaves room for a store read.
pub fn kinds(settings: &Settings, _args: NoArgs) -> Result<KindsShown, Error> {
    let config: &Config = settings.config();
    let catalogue: Vec<String> = config
        .dispositions
        .iter()
        .map(|one| one.name.clone())
        .collect();
    let kinds = config
        .kinds
        .iter()
        .map(|kind| {
            let (dispositions, from) = match (&kind.dispositions, &config.default_dispositions) {
                (Some(own), _) => (own.clone(), DispositionsFrom::Kind),
                (None, Some(defaults)) => (defaults.clone(), DispositionsFrom::DefaultDispositions),
                (None, None) => (catalogue.clone(), DispositionsFrom::Catalogue),
            };
            KindShown {
                name: kind.name.clone(),
                description: kind.description.clone(),
                ticket_template: kind
                    .ticket_template
                    .clone()
                    .unwrap_or_else(|| BUILTIN_TICKET_TEMPLATE.to_owned()),
                duplicates: kind.duplicates.clone(),
                priority: kind.priority.clone(),
                initial_status: kind.initial_status,
                destination: kind
                    .destination
                    .clone()
                    .unwrap_or_else(|| config.destination.clone()),
                dispositions,
                dispositions_from: from,
            }
        })
        .collect();
    Ok(KindsShown {
        schema_version: SCHEMA_VERSION,
        fields: config.fields.clone(),
        kinds,
        default_kind: config.default_kind.clone(),
        relations: config.relations.clone(),
        dispositions: config.dispositions.clone(),
        default_dispositions: config.default_dispositions.clone(),
        routes: config.routes.clone(),
    })
}

/// `onefollowup config show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Args)]
pub struct ConfigShowArgs {
    /// Also name the layer each top-level key's effective value came from.
    #[arg(long)]
    pub sources: bool,
}

/// The effective configuration.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigShown {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// The effective configuration.
    pub config: Config,
    /// With `--sources`: each top-level key and the layer its value came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<IndexMap<String, Origin>>,
}

/// The effective configuration, and with `sources` where each key came from.
///
/// # Errors
///
/// Never in this release; the signature is every verb's.
pub fn config_show(settings: &Settings, args: ConfigShowArgs) -> Result<ConfigShown, Error> {
    Ok(ConfigShown {
        schema_version: SCHEMA_VERSION,
        config: settings.config().clone(),
        sources: args
            .sources
            .then(|| settings.sources().iter().cloned().collect()),
    })
}
