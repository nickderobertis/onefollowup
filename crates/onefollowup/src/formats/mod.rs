//! The record formats onefollowup reads and writes, each declared once as a Rust type with
//! its parse and its render.
//!
//! Every format but the outbox is adopted from the records on boards and disks today, at
//! its current version, so nothing already written is rewritten: rendering what was parsed
//! reproduces it byte for byte. **Each format reads only its current schema** and refuses
//! any other, naming the format, the schema found and the schema it reads; there is no
//! migration. The batch id stands where today's records say `run`, with no change to the
//! bytes: the Rust fields are named `batch` and serialize as `run`.

pub mod batch;
pub mod dispositions;
pub mod draft;
pub mod existing_tickets;
pub mod gathering;
pub mod markers;
pub mod outbox;
pub mod pending;
pub mod pyjson;
pub mod receipts;
pub mod responses;
pub mod snapshot;
pub mod ticket;
pub mod watermark;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use pyjson::Style;

/// A record that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// The record is of a schema this does not read.
    #[error(
        "the {format} is schema {found}, and this reads schema {reads}; onefollowup reads only the current schema of each record and migrates none"
    )]
    Schema {
        /// The format's name.
        format: &'static str,
        /// The schema the record states, as written.
        found: String,
        /// The schema this reads.
        reads: u64,
    },
    /// The record is not of the format's shape.
    #[error("the {format} is not one this reads: {message}")]
    Malformed {
        /// The format's name.
        format: &'static str,
        /// What is wrong.
        message: String,
    },
}

/// A versioned JSON document: one object whose first key is `schema`.
pub trait Versioned: Serialize + DeserializeOwned {
    /// The format's name, as an error names it.
    const FORMAT: &'static str;
    /// The schema this reads and writes.
    const SCHEMA: u64;
    /// How it is spelled.
    const STYLE: Style;
    /// Whether the file ends with a newline.
    const TRAILING_NEWLINE: bool = true;

    /// Read a document of this format.
    ///
    /// # Errors
    ///
    /// [`FormatError::Schema`] for any schema but [`Self::SCHEMA`], and
    /// [`FormatError::Malformed`] for anything else that is not this format.
    fn parse(text: &str) -> Result<Self, FormatError> {
        let value: Value = serde_json::from_str(text).map_err(|error| FormatError::Malformed {
            format: Self::FORMAT,
            message: format!("it is not JSON: {error}"),
        })?;
        from_value(value, Self::FORMAT, Self::SCHEMA)
    }

    /// Write this document as its format spells it.
    #[must_use]
    fn render(&self) -> String {
        let mut text = pyjson::to_string(&to_value(self, Self::SCHEMA), Self::STYLE);
        if Self::TRAILING_NEWLINE {
            text.push('\n');
        }
        text
    }
}

/// Check `value`'s `schema` and read the rest of it as `T`.
pub(crate) fn from_value<T: DeserializeOwned>(
    value: Value,
    format: &'static str,
    schema: u64,
) -> Result<T, FormatError> {
    let malformed = |message: String| FormatError::Malformed { format, message };
    let Value::Object(mut map) = value else {
        return Err(malformed("it is not a JSON object".to_owned()));
    };
    match map.shift_remove("schema") {
        Some(Value::Number(number)) if number.as_u64() == Some(schema) && !number.is_f64() => {}
        Some(found) => {
            return Err(FormatError::Schema {
                format,
                found: found.to_string(),
                reads: schema,
            });
        }
        None => return Err(malformed("it states no `schema`".to_owned())),
    }
    serde_path_to_error::deserialize(Value::Object(map)).map_err(|error| {
        let at = error.path().to_string();
        malformed(if at == "." {
            error.inner().to_string()
        } else {
            format!("`{at}`: {}", error.inner())
        })
    })
}

/// `record` as a JSON object with `schema` its first key.
pub(crate) fn to_value<T: Serialize>(record: &T, schema: u64) -> Value {
    let mut map = Map::new();
    map.insert("schema".to_owned(), Value::from(schema));
    match serde_json::to_value(record) {
        Ok(Value::Object(fields)) => map.extend(fields),
        // A format is always a struct, which serializes to an object.
        _ => unreachable!("a record format serializes to a JSON object"),
    }
    Value::Object(map)
}

/// A priority estimate: one of the four levels, never `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// Urgent.
    Urgent,
    /// High.
    High,
    /// Medium.
    Medium,
    /// Low.
    Low,
}

/// A priority as a destination holds one: a level, or `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HeldPriority {
    /// Urgent.
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

/// Whether a root cause fires every time its conditions hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Frequency {
    /// Every time.
    Consistent,
    /// Only sometimes.
    Intermittent,
}

/// Whether a reply's batch judged the comment it answers to confirm the ticket's root cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// It confirms the root cause: one more occurrence.
    Confirms,
    /// It does not.
    DoesNotConfirm,
}

impl Verdict {
    /// The verdict as a marker spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirms => "confirms",
            Self::DoesNotConfirm => "does-not-confirm",
        }
    }
}
