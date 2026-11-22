//! The comment markers and anchor lines: how a batch's comments on a destination item are
//! recognised as its own, and how a gathering's feedback file anchors what it quotes.
//!
//! Every marker opens with the configured namespace (`orchestrator` by default), so the
//! default spells them as the records on boards today do:
//! `<!-- orchestrator:follow-up-comment run="…" root_cause="…" -->`.

use std::sync::LazyLock;

use regex::Regex;

use super::{FormatError, Verdict};

const FORMAT: &str = "comment marker";

/// A value a marker's attribute can carry: no whitespace, `"` or `>`.
static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^[^\s">]+$"#).expect("valid"));

fn malformed(message: String) -> FormatError {
    FormatError::Malformed {
        format: FORMAT,
        message,
    }
}

fn attribute(value: &str, what: &str) -> Result<(), FormatError> {
    if ATTRIBUTE.is_match(value) {
        Ok(())
    } else {
        Err(malformed(format!(
            "its {what} `{value}` carries whitespace, `\"` or `>`"
        )))
    }
}

/// The marker closing a batch's evidence comment or reply, or a grooming batch's comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentMarker {
    /// The batch that wrote the comment.
    pub batch: String,
    /// The root cause it is for; empty on a grooming comment, whose marker names none.
    pub root_cause: String,
    /// For a reply: the comment it answers, and its verdict (absent on a reply written
    /// before verdicts were recorded).
    pub reply: Option<ReplyTo>,
    /// For a grooming comment (`kind="groom"`): what it says of its ticket. Such a comment is
    /// maintenance, never a person's to answer.
    pub groom: Option<GroomMark>,
}

/// What a grooming comment says of the ticket it is on, as its marker's `verdict` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroomMark {
    /// The ticket was closed as fixed on its repository's default branch.
    Fixed,
    /// The ticket was closed as no longer applying to its repository's default branch.
    Obsolete,
    /// The ticket was closed as a duplicate of its cluster's survivor.
    Duplicate,
    /// The ticket survives its cluster, the duplicates closed into it named.
    Survivor,
}

impl GroomMark {
    /// The mark as the marker spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Obsolete => "obsolete",
            Self::Duplicate => "duplicate",
            Self::Survivor => "survivor",
        }
    }

    fn of(word: &str) -> Option<Self> {
        [Self::Fixed, Self::Obsolete, Self::Duplicate, Self::Survivor]
            .into_iter()
            .find(|mark| mark.as_str() == word)
    }
}

/// What a reply answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyTo {
    /// The answered comment's id.
    pub answers: String,
    /// Whether it confirms the root cause.
    pub verdict: Option<Verdict>,
}

impl CommentMarker {
    /// The marker line.
    #[must_use]
    pub fn render(&self, namespace: &str) -> String {
        if let Some(mark) = self.groom {
            return format!(
                "<!-- {namespace}:follow-up-comment run=\"{}\" kind=\"groom\" verdict=\"{}\" -->",
                self.batch,
                mark.as_str()
            );
        }
        let mut line = format!(
            "<!-- {namespace}:follow-up-comment run=\"{}\" root_cause=\"{}\"",
            self.batch, self.root_cause
        );
        if let Some(reply) = &self.reply {
            line.push_str(&format!(" kind=\"reply\" answers=\"{}\"", reply.answers));
            if let Some(verdict) = reply.verdict {
                line.push_str(&format!(" verdict=\"{}\"", verdict.as_str()));
            }
        }
        line.push_str(" -->");
        line
    }

    /// Read a marker line.
    ///
    /// # Errors
    ///
    /// [`FormatError::Malformed`] for a line that is not a batch's comment marker.
    pub fn parse(line: &str, namespace: &str) -> Result<Self, FormatError> {
        let groom = Regex::new(&format!(
            r#"^<!-- {}:follow-up-comment run="([^"\s]+)" kind="groom" verdict="([^"\s]*)" -->$"#,
            regex::escape(namespace)
        ))
        .map_err(|error| malformed(error.to_string()))?;
        if let Some(captures) = groom.captures(line) {
            let word = captures.get(2).map_or("", |one| one.as_str());
            let mark = GroomMark::of(word).ok_or_else(|| {
                malformed(format!(
                    "`{word}` is not a grooming comment's verdict: fixed, obsolete, duplicate or survivor"
                ))
            })?;
            return Ok(Self {
                batch: captures
                    .get(1)
                    .map_or_else(String::new, |one| one.as_str().to_owned()),
                root_cause: String::new(),
                reply: None,
                groom: Some(mark),
            });
        }
        let pattern = Regex::new(&format!(
            r#"^<!-- {}:follow-up-comment run="([^"\s]+)" root_cause="([^"\s]+)"(?: kind="([^"\s]*)")?(?: answers="([^"]*)")?(?: verdict="([^"]*)")? -->$"#,
            regex::escape(namespace)
        ))
        .map_err(|error| malformed(error.to_string()))?;
        let captures = pattern
            .captures(line)
            .ok_or_else(|| malformed(format!("`{line}` is not a {namespace} comment marker")))?;
        let text = |index: usize| captures.get(index).map(|one| one.as_str().to_owned());
        let reply = match (text(3), text(4), text(5)) {
            (None, None, None) => None,
            (Some(kind), Some(answers), verdict) if kind == "reply" => {
                attribute(&answers, "answered comment")?;
                let verdict = verdict
                    .map(|word| {
                        serde_json::from_value(serde_json::Value::from(word.as_str()))
                            .map_err(|_| malformed(format!("`{word}` is not a verdict")))
                    })
                    .transpose()?;
                Some(ReplyTo { answers, verdict })
            }
            _ => {
                return Err(malformed(format!(
                    "`{line}` is neither evidence nor a reply"
                )));
            }
        };
        Ok(Self {
            batch: text(1).unwrap_or_default(),
            root_cause: text(2).unwrap_or_default(),
            reply,
            groom: None,
        })
    }
}

/// What owns a comment: the marker its last non-blank line is, or `None` when that line is no
/// batch's marker. Ownership is read from the marker alone, so 0.5.0's comments (an opening
/// line, the text, the marker), those opening with a collapsed block, and those any configured
/// template rendered are each owned by the batch and root cause their marker names.
#[must_use]
pub fn owner(body: &str, namespace: &str) -> Option<CommentMarker> {
    let last = body
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    CommentMarker::parse(last, namespace).ok()
}

/// The line 0.5.0's evidence comment opens with, which the shipped evidence template writes.
#[must_use]
pub fn evidence_opening(batch: &str) -> String {
    format!("Additional evidence from follow-up run `{batch}`.")
}

/// The line 0.5.0's reply opens with, which the shipped reply template writes: attributed
/// when the destination named the author.
#[must_use]
pub fn reply_opening(batch: &str, url: &str, author: Option<&str>) -> String {
    match author {
        Some(author) => format!("Reply from follow-up run `{batch}` to @{author}'s comment: {url}"),
        None => format!("Reply from follow-up run `{batch}` to the comment: {url}"),
    }
}

/// The anchor a feedback file quotes one comment under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotedAnchor {
    /// The issue the comment sits on, qualified.
    pub issue: String,
    /// The comment's id.
    pub comment: String,
}

impl QuotedAnchor {
    /// The anchor line.
    #[must_use]
    pub fn render(&self, namespace: &str) -> String {
        format!(
            "<!-- {namespace}:follow-up-quoted issue=\"{}\" comment=\"{}\" -->",
            self.issue, self.comment
        )
    }

    /// Read an anchor line.
    ///
    /// # Errors
    ///
    /// [`FormatError::Malformed`] for a line that is not a quoted-comment anchor.
    pub fn parse(line: &str, namespace: &str) -> Result<Self, FormatError> {
        let (issue, comment) = anchored(line, namespace, "follow-up-quoted", &["issue", "comment"])
            .map(|mut values| (values.remove(0), values.remove(0)))
            .ok_or_else(|| malformed(format!("`{line}` is not a quoted-comment anchor")))?;
        attribute(&issue, "issue")?;
        attribute(&comment, "comment")?;
        Ok(Self { issue, comment })
    }
}

/// The anchor a feedback file opens with: from when its comments were read, or `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackBoundary(pub Option<String>);

impl FeedbackBoundary {
    /// The anchor line.
    #[must_use]
    pub fn render(&self, namespace: &str) -> String {
        format!(
            "<!-- {namespace}:follow-up-feedback boundary=\"{}\" -->",
            self.0.as_deref().unwrap_or("none")
        )
    }

    /// Read the anchor line.
    ///
    /// # Errors
    ///
    /// [`FormatError::Malformed`] for a line that is not a feedback boundary anchor.
    pub fn parse(line: &str, namespace: &str) -> Result<Self, FormatError> {
        let value = anchored(line, namespace, "follow-up-feedback", &["boundary"])
            .map(|mut values| values.remove(0))
            .ok_or_else(|| malformed(format!("`{line}` is not a feedback boundary anchor")))?;
        Ok(Self((value != "none").then_some(value)))
    }
}

/// The anchor naming the snapshot a feedback file was gathered from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackSnapshot(pub String);

impl FeedbackSnapshot {
    /// The anchor line.
    #[must_use]
    pub fn render(&self, namespace: &str) -> String {
        format!(
            "<!-- {namespace}:follow-up-feedback snapshot=\"{}\" -->",
            self.0
        )
    }

    /// Read the anchor line.
    ///
    /// # Errors
    ///
    /// [`FormatError::Malformed`] for a line that is not a feedback snapshot anchor.
    pub fn parse(line: &str, namespace: &str) -> Result<Self, FormatError> {
        anchored(line, namespace, "follow-up-feedback", &["snapshot"])
            .map(|mut values| Self(values.remove(0)))
            .ok_or_else(|| malformed(format!("`{line}` is not a feedback snapshot anchor")))
    }
}

/// A feedback file's `- Issue:` value: the issue, its snapshot copy and the version read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLine {
    /// The issue, qualified.
    pub issue: String,
    /// Its copy in the snapshot.
    pub snapshot_id: String,
    /// The version read, or `None`.
    pub version: Option<String>,
}

impl IssueLine {
    /// The line.
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "{}, read from {} at version {}",
            self.issue,
            self.snapshot_id,
            self.version.as_deref().unwrap_or("none")
        )
    }

    /// Read the line.
    ///
    /// # Errors
    ///
    /// [`FormatError::Malformed`] for a line that is not an issue line.
    pub fn parse(line: &str) -> Result<Self, FormatError> {
        let refused = || malformed(format!("`{line}` is not an issue line"));
        let (issue, rest) = line.split_once(", read from ").ok_or_else(refused)?;
        let (snapshot_id, version) = rest.split_once(" at version ").ok_or_else(refused)?;
        Ok(Self {
            issue: issue.to_owned(),
            snapshot_id: snapshot_id.to_owned(),
            version: (version != "none").then(|| version.to_owned()),
        })
    }
}

/// The attribute values of `<!-- <namespace>:<name> a="…" b="…" -->`, in `keys` order.
fn anchored(line: &str, namespace: &str, name: &str, keys: &[&str]) -> Option<Vec<String>> {
    let mut rest = line
        .strip_prefix(&format!("<!-- {namespace}:{name}"))?
        .strip_suffix(" -->")?;
    let mut values = Vec::new();
    for key in keys {
        rest = rest.strip_prefix(&format!(" {key}=\""))?;
        let (value, after) = rest.split_once('"')?;
        values.push(value.to_owned());
        rest = after;
    }
    rest.is_empty().then_some(values)
}
