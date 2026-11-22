//! The grooming pass's records: the `groom` record a groomed ticket carries in its metadata,
//! with the digest it is compared by; the verdict record a verifying agent writes, one per line
//! of a shard's `verdicts.jsonl`; and the batch and plan `groom prepare` and `groom check` write.
//!
//! Each is `schema_version` 1 and refuses unknown keys. The `groom` record is the contract every
//! later pass reads off the tickets, on any machine: `groom-verb`'s publish writes it with
//! [`GroomRecord`] and [`content_digest`], and nothing restates them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use super::FormatError;

/// The schema every grooming record is.
pub const SCHEMA_VERSION: u64 = 1;

/// The metadata key a groomed ticket's record sits under: `<namespace>.groom`.
#[must_use]
pub fn record_key(namespace: &str) -> String {
    format!("{namespace}.groom")
}

/// The digest a ticket's record holds: `sha256:<hex>` over its title and its content as the
/// store reads it, which never holds the metadata block, so recording a ticket never makes it
/// look changed. Comments are not in it. Leading and trailing whitespace of each is ignored.
#[must_use]
pub fn content_digest(title: &str, content: &str) -> String {
    let document = Value::Array(vec![Value::from(title.trim()), Value::from(content.trim())]);
    let hash = Sha256::digest(document.to_string().as_bytes());
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

/// A verifying agent's verdict on one ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroomVerdict {
    /// The ticket still stands.
    Keep,
    /// Its root cause is fixed on `main`.
    Fixed,
    /// It no longer applies to `main`.
    Obsolete,
    /// The agent could not decide.
    Unresolved,
}

impl GroomVerdict {
    /// Every verdict, as the record spells it.
    pub const ALL: [&'static str; 4] = ["keep", "fixed", "obsolete", "unresolved"];

    /// Whether the verdict closes the ticket: `fixed` or `obsolete`.
    #[must_use]
    pub const fn closes(self) -> bool {
        matches!(self, Self::Fixed | Self::Obsolete)
    }

    /// The verdict as the record spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Fixed => "fixed",
            Self::Obsolete => "obsolete",
            Self::Unresolved => "unresolved",
        }
    }
}

/// How sure the agent is of its verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    /// Sure.
    High,
    /// Fairly sure.
    Medium,
    /// Unsure: never enough to close a ticket.
    Low,
}

impl Confidence {
    /// Every confidence, as the record spells it.
    pub const ALL: [&'static str; 3] = ["high", "medium", "low"];
}

/// One comment a record holds as verified inside the comment overlap before its
/// `verified_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlapComment {
    /// The comment's id.
    pub id: String,
    /// When it was last edited, or created when never edited, RFC 3339.
    pub edited_at: String,
}

/// What a groomed ticket records of its last verification, at `<namespace>.groom`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroomRecord {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u64,
    /// The grooming batch that verified it.
    pub batch: String,
    /// When that batch's comment read began, RFC 3339: never its publish time.
    pub verified_at: String,
    /// The verdict it was given.
    pub verdict: GroomVerdict,
    /// The repository it was verified in, `host/owner/name`.
    pub repository: String,
    /// The `main` commit it was verified against, 40 hex.
    pub commit: String,
    /// The repository-relative paths its verdict rests on.
    pub evidence_paths: Vec<String>,
    /// Its [`content_digest`] when it was verified.
    pub content_digest: String,
    /// Each comment, not grooming's own, created or last edited inside the overlap before
    /// `verified_at`.
    pub overlap_comments: Vec<OverlapComment>,
}

impl GroomRecord {
    /// The format's name, as an error names it.
    pub const FORMAT: &'static str = "groom record";

    /// Read the record a ticket's metadata holds at its key.
    ///
    /// # Errors
    ///
    /// [`FormatError::Schema`] for a record of another schema, and [`FormatError::Malformed`]
    /// for anything else that is not this record.
    pub fn from_value(value: &Value) -> Result<Self, FormatError> {
        let schema = value.get("schema_version");
        if schema.and_then(Value::as_u64) != Some(SCHEMA_VERSION)
            || schema.is_some_and(Value::is_f64)
        {
            return Err(FormatError::Schema {
                format: Self::FORMAT,
                found: schema.map_or_else(|| "absent".to_owned(), ToString::to_string),
                reads: SCHEMA_VERSION,
            });
        }
        serde_json::from_value(value.clone()).map_err(|error| FormatError::Malformed {
            format: Self::FORMAT,
            message: error.to_string(),
        })
    }

    /// The record as a ticket's metadata holds it.
    #[must_use]
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// One verdict record, as a verifying agent writes it on one line of `verdicts.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictRecord {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u64,
    /// The ticket, as prepare listed it.
    pub ticket: String,
    /// The verdict.
    pub verdict: GroomVerdict,
    /// How sure the agent is.
    pub confidence: Confidence,
    /// What the verdict rests on.
    pub evidence: String,
    /// The `main` commit that fixed or obsoleted it, 40 hex; `None` for `keep` and
    /// `unresolved`.
    pub deciding_commit: Option<String>,
    /// The repository-relative paths the verdict rests on.
    pub evidence_paths: Vec<String>,
    /// The ticket it shares one root cause with.
    pub duplicate_of: Option<String>,
    /// That root cause, one line; set exactly when `duplicate_of` is.
    pub root_cause: Option<String>,
    /// The repository owning the seam, `host/owner/name`.
    pub seam_repository: Option<String>,
}

/// One rule a verdict record breaks: the rule's name and what is wrong.
pub type Broken = (&'static str, String);

/// The keys a verdict record holds, every one required.
pub const VERDICT_KEYS: [&str; 10] = [
    "schema_version",
    "ticket",
    "verdict",
    "confidence",
    "evidence",
    "deciding_commit",
    "evidence_paths",
    "duplicate_of",
    "root_cause",
    "seam_repository",
];

/// Whether `text` is a full 40-hex commit id.
#[must_use]
pub fn is_commit(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `text` is a `host/owner/name` repository.
#[must_use]
pub fn is_repository(text: &str) -> bool {
    let parts: Vec<&str> = text.split('/').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part != &"."
                && part != &".."
                && part
                    .chars()
                    .all(|one| one.is_ascii_alphanumeric() || "-_.".contains(one))
        })
}

/// Whether `text` is a repository-relative path: not empty, not absolute, never leaving the
/// repository.
#[must_use]
pub fn is_relative_path(text: &str) -> bool {
    !text.trim().is_empty()
        && !text.starts_with('/')
        && !text.contains('\\')
        && !text.split('/').any(|part| part == "..")
}

impl VerdictRecord {
    /// Read one line of `verdicts.jsonl`: the record, or the ticket it names (when it names
    /// one) and every rule it breaks.
    ///
    /// # Errors
    ///
    /// The ticket the line names, when it names one, and every rule it breaks.
    pub fn parse_line(line: &str) -> Result<Self, (Option<String>, Vec<Broken>)> {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                return Err((
                    None,
                    vec![(
                        "verdict-record",
                        format!("the line is not one JSON object: {error}"),
                    )],
                ));
            }
        };
        let Value::Object(map) = value else {
            return Err((
                None,
                vec![("verdict-record", "the line is not a JSON object".to_owned())],
            ));
        };
        let ticket = map
            .get("ticket")
            .and_then(Value::as_str)
            .filter(|ticket| !ticket.trim().is_empty())
            .map(str::to_owned);
        let mut broken = Vec::new();
        Self::shape(&map, &mut broken);
        if broken.is_empty() {
            match serde_json::from_value::<Self>(Value::Object(map)) {
                Ok(record) => {
                    record.rules(&mut broken);
                    if broken.is_empty() {
                        return Ok(record);
                    }
                }
                Err(error) => broken.push(("invalid-value", error.to_string())),
            }
        }
        Err((ticket, broken))
    }

    /// Every key, type and enum rule `map` breaks.
    fn shape(map: &Map<String, Value>, broken: &mut Vec<Broken>) {
        for key in map.keys() {
            if !VERDICT_KEYS.contains(&key.as_str()) {
                broken.push((
                    "unknown-key",
                    format!(
                        "`{key}` is not a verdict record key; the keys are {}",
                        VERDICT_KEYS.join(", ")
                    ),
                ));
            }
        }
        for key in VERDICT_KEYS {
            if !map.contains_key(key) {
                broken.push((
                    "missing-field",
                    format!("`{key}` is required (write `null` where the schema allows it)"),
                ));
            }
        }
        if let Some(schema) = map.get("schema_version")
            && (schema.as_u64() != Some(SCHEMA_VERSION) || schema.is_f64())
        {
            broken.push((
                "schema-version",
                format!(
                    "the verdict record is schema {schema}, and this reads schema {SCHEMA_VERSION}"
                ),
            ));
        }
        let text = |key: &str| map.get(key).map(|value| (value, value.as_str()));
        if let Some((value, None)) = text("ticket") {
            broken.push((
                "invalid-value",
                format!("`ticket` is {value}, not a ticket id"),
            ));
        }
        for (key, allowed) in [
            ("verdict", &GroomVerdict::ALL[..]),
            ("confidence", &Confidence::ALL[..]),
        ] {
            if let Some((value, word)) = text(key)
                && !word.is_some_and(|word| allowed.contains(&word))
            {
                broken.push((
                    "invalid-value",
                    format!("`{key}` is {value}; it is one of {}", allowed.join(", ")),
                ));
            }
        }
        if let Some((value, None)) = text("evidence") {
            broken.push(("invalid-value", format!("`evidence` is {value}, not text")));
        }
        if let Some((value, word)) = text("deciding_commit")
            && !value.is_null()
            && !word.is_some_and(is_commit)
        {
            broken.push((
                "invalid-value",
                format!("`deciding_commit` is {value}, not a full 40-hex commit id or null"),
            ));
        }
        if let Some(paths) = map.get("evidence_paths") {
            let valid = paths.as_array().is_some_and(|paths| {
                paths
                    .iter()
                    .all(|path| path.as_str().is_some_and(is_relative_path))
            });
            if !valid {
                broken.push((
                    "invalid-value",
                    format!("`evidence_paths` is {paths}, not a list of repository-relative paths"),
                ));
            }
        }
        for key in ["duplicate_of", "root_cause"] {
            if let Some((value, word)) = text(key)
                && !value.is_null()
                && !word.is_some_and(|word| !word.trim().is_empty() && !word.contains('\n'))
            {
                broken.push((
                    "invalid-value",
                    format!("`{key}` is {value}, not one line of text or null"),
                ));
            }
        }
        if let Some((value, word)) = text("seam_repository")
            && !value.is_null()
            && !word.is_some_and(is_repository)
        {
            broken.push((
                "invalid-value",
                format!("`seam_repository` is {value}, not `host/owner/name` or null"),
            ));
        }
    }

    /// Every rule between the record's fields it breaks.
    fn rules(&self, broken: &mut Vec<Broken>) {
        if self.evidence.trim().is_empty() {
            broken.push((
                "evidence-required",
                format!(
                    "a `{}` verdict states its evidence: `evidence` is empty",
                    self.verdict.as_str()
                ),
            ));
        }
        match (self.verdict.closes(), &self.deciding_commit) {
            (true, None) => broken.push((
                "deciding-commit",
                format!(
                    "a `{}` verdict names the `main` commit that decided it in `deciding_commit`",
                    self.verdict.as_str()
                ),
            )),
            (false, Some(_)) => broken.push((
                "deciding-commit",
                format!(
                    "a `{}` verdict's `deciding_commit` is null",
                    self.verdict.as_str()
                ),
            )),
            _ => {}
        }
        if self.duplicate_of.is_some() && self.root_cause.is_none() {
            broken.push((
                "root-cause-required",
                "a verdict naming `duplicate_of` states the shared root cause in `root_cause`"
                    .to_owned(),
            ));
        }
        if self.verdict.closes() && self.confidence == Confidence::Low {
            broken.push((
                "low-confidence-close",
                format!(
                    "a `{}` verdict closes the ticket, which `low` confidence never does: raise the evidence to `medium` or `high`, or answer `keep` or `unresolved`",
                    self.verdict.as_str()
                ),
            ));
        }
        if self.duplicate_of.as_deref() == Some(self.ticket.as_str()) {
            broken.push((
                "duplicate-of-self",
                "`duplicate_of` names the ticket itself".to_owned(),
            ));
        }
    }
}

/// Which protection a ticket's status gives it, by onefollowup's status roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Protection {
    /// At a non-claimable status (the proposal or deferred role's, or unknown): it may be
    /// closed or folded as a duplicate.
    Open,
    /// At the accepted role's status (Todo): never folded, and closed only when definitely
    /// outdated.
    Todo,
    /// At the queued or in-progress role's status: never closed or folded.
    Claimed,
}

/// A status as the destination held it when prepare listed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeldStatus {
    /// The store category.
    pub category: String,
    /// The destination's own name for it.
    pub name: String,
}

/// One repository a batch fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchedRepository {
    /// `host/owner/name`.
    pub repository: String,
    /// Its clone's absolute path.
    pub clone: String,
    /// The `main` commit prepare fetched.
    pub commit: String,
}

/// One ticket a batch selected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedTicket {
    /// Its qualified id.
    pub ticket: String,
    /// Its title.
    pub title: String,
    /// Its status when listed.
    pub status: HeldStatus,
    /// The protection that status gives it.
    pub protection: Protection,
    /// Its repositories, `host/owner/name`.
    pub repositories: Vec<String>,
    /// When it was created, where its source says.
    pub created_at: Option<String>,
    /// Its [`content_digest`] when listed.
    pub content_digest: String,
    /// The comments its record holds as verified inside the overlap.
    pub overlap_comments: Vec<OverlapComment>,
    /// Why it was selected.
    pub reasons: Vec<String>,
}

/// One shard of a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shard {
    /// `<batch>-<k>`.
    pub shard: String,
    /// Its task's absolute path.
    pub task: String,
    /// The absolute path of the verdicts its agent writes.
    pub verdicts: String,
    /// Its tickets, by qualified id.
    pub tickets: Vec<String>,
}

/// One open ticket as a batch's snapshot holds it, for duplicate detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotTicket {
    /// Its qualified id.
    pub ticket: String,
    /// Its title.
    pub title: String,
    /// Its status.
    pub status: HeldStatus,
    /// Where a person opens it.
    pub url: Option<String>,
    /// Its repositories.
    pub repositories: Vec<String>,
    /// When it was created, where its source says.
    pub created_at: Option<String>,
    /// Its content.
    pub content: String,
}

/// A grooming batch, as `groom prepare` wrote it: `batch.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroomBatch {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u64,
    /// The batch id.
    pub batch: String,
    /// The destination source.
    pub destination: String,
    /// The namespace its record key is under.
    pub namespace: String,
    /// When its comment read began, RFC 3339: every record it publishes says so.
    pub verified_at: String,
    /// Each repository it fetched.
    pub repositories: Vec<FetchedRepository>,
    /// Every open ticket it listed, by qualified id, with its protection.
    pub open: BTreeMap<String, Protection>,
    /// Each ticket it selected.
    pub selected: Vec<SelectedTicket>,
    /// Its shards.
    pub shards: Vec<Shard>,
}

/// One ticket of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanEntry {
    /// Its qualified id.
    pub ticket: String,
    /// Its status category when listed.
    pub status: String,
    /// The verdict it was given.
    pub verdict: GroomVerdict,
    /// How sure the agent was.
    pub confidence: Confidence,
    /// What the verdict rests on.
    pub evidence: String,
    /// The commit that decided a closing verdict.
    pub deciding_commit: Option<String>,
    /// The paths the verdict rests on.
    pub evidence_paths: Vec<String>,
    /// The repository it was verified in, where it names one.
    pub repository: Option<String>,
    /// That repository's fetched `main` commit.
    pub commit: Option<String>,
    /// For a duplicate, its cluster's survivor.
    pub duplicate_of: Option<String>,
    /// For a duplicate, the shared root cause.
    pub root_cause: Option<String>,
    /// Why a person, not this pass, decides what its verdict asks, when it is left to one.
    pub left_for_person: Option<String>,
}

/// One duplicate cluster's survivor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Survivor {
    /// The survivor.
    pub ticket: String,
    /// Every member of its cluster, the survivor among them.
    pub members: Vec<String>,
    /// Why it survived: the first rule of the priority order that decided it.
    pub chosen_by: String,
}

/// What a passing `groom check` plans: `plan.json`, what `groom-verb`'s publish sends.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroomPlan {
    /// Closed as fixed.
    pub fixed: Vec<PlanEntry>,
    /// Closed as obsolete.
    pub obsolete: Vec<PlanEntry>,
    /// Folded as duplicates of their cluster's survivor.
    pub duplicates: Vec<PlanEntry>,
    /// Each cluster's survivor.
    pub survivors: Vec<Survivor>,
    /// Every ticket not closed or folded, those left for a person among them.
    pub kept: Vec<PlanEntry>,
    /// Each ticket whose verdict is `unresolved`, kept and listed for the report.
    pub unresolved: Vec<PlanEntry>,
}

/// A plan as the batch holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRecord {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u64,
    /// The batch.
    pub batch: String,
    /// The plan.
    pub plan: GroomPlan,
}

/// Read a batch record of schema [`SCHEMA_VERSION`] named `format`.
///
/// # Errors
///
/// A record of another schema, or not of the shape.
pub fn parse<T: serde::de::DeserializeOwned>(
    text: &str,
    format: &'static str,
) -> Result<T, FormatError> {
    let value: Value = serde_json::from_str(text).map_err(|error| FormatError::Malformed {
        format,
        message: format!("it is not JSON: {error}"),
    })?;
    let schema = value.get("schema_version");
    if schema.and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(FormatError::Schema {
            format,
            found: schema.map_or_else(|| "absent".to_owned(), ToString::to_string),
            reads: SCHEMA_VERSION,
        });
    }
    serde_path_to_error::deserialize(value).map_err(|error| FormatError::Malformed {
        format,
        message: format!("`{}`: {}", error.path(), error.inner()),
    })
}

/// Write a batch record as indented JSON, ending in a newline.
#[must_use]
pub fn render<T: Serialize>(record: &T) -> String {
    let mut text = serde_json::to_string_pretty(record).unwrap_or_default();
    text.push('\n');
    text
}

/// Every open ticket a batch listed, for duplicate detection: `snapshot.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroomSnapshot {
    /// [`SCHEMA_VERSION`].
    pub schema_version: u64,
    /// The batch.
    pub batch: String,
    /// Every open ticket, by qualified id.
    pub tickets: Vec<SnapshotTicket>,
}
