//! JSON written exactly as the records on boards today were written.
//!
//! Those records were written by Python's `json.dumps`, so a record onefollowup rewrites is
//! byte-identical only if it is spelled the same way: the same separators, the same string
//! escaping (`ensure_ascii`), the same float spelling (`repr`), and, for the receipt journal,
//! keys sorted at every depth. This module is that spelling, over [`serde_json::Value`]
//! (whose maps keep their insertion order).

use serde_json::{Map, Number, Value};

/// How a document is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One line: `", "` between items and `": "` after keys (`json.dumps(v)`).
    Line,
    /// Two-space indentation, one item per line (`json.dumps(v, indent=2)`).
    Indented,
}

/// How a document is spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    /// The layout.
    pub layout: Layout,
    /// Every character outside printable ASCII escaped as `\uXXXX` (`ensure_ascii=True`).
    pub ascii: bool,
    /// Keys sorted by code point at every depth (`sort_keys=True`).
    pub sorted: bool,
}

impl Style {
    /// `json.dumps(v)`.
    pub const LINE: Self = Self {
        layout: Layout::Line,
        ascii: true,
        sorted: false,
    };
    /// `json.dumps(v, indent=2)`.
    pub const INDENTED: Self = Self {
        layout: Layout::Indented,
        ascii: true,
        sorted: false,
    };
    /// `json.dumps(v, ensure_ascii=False, sort_keys=True)`: the receipt journal's lines.
    pub const SORTED_RAW: Self = Self {
        layout: Layout::Line,
        ascii: false,
        sorted: true,
    };
}

/// `value` spelled in `style`, with no trailing newline.
#[must_use]
pub fn to_string(value: &Value, style: Style) -> String {
    let mut out = String::new();
    write_value(&mut out, value, style, 0);
    out
}

fn write_value(out: &mut String, value: &Value, style: Style, depth: usize) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => out.push_str(&number_text(number)),
        Value::String(text) => write_string(out, text, style.ascii),
        Value::Array(items) => {
            write_container(
                out,
                '[',
                ']',
                items.iter().map(|item| (None, item)),
                style,
                depth,
            );
        }
        Value::Object(map) => {
            let entries = ordered(map, style.sorted);
            write_container(
                out,
                '{',
                '}',
                entries.into_iter().map(|(key, item)| (Some(key), item)),
                style,
                depth,
            );
        }
    }
}

fn ordered(map: &Map<String, Value>, sorted: bool) -> Vec<(&String, &Value)> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    if sorted {
        // Python sorts by code point, which is the order of the UTF-8 bytes.
        entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    }
    entries
}

fn write_container<'a>(
    out: &mut String,
    open: char,
    close: char,
    entries: impl ExactSizeIterator<Item = (Option<&'a String>, &'a Value)>,
    style: Style,
    depth: usize,
) {
    out.push(open);
    if entries.len() == 0 {
        out.push(close);
        return;
    }
    for (index, (key, item)) in entries.enumerate() {
        match style.layout {
            Layout::Line => {
                if index > 0 {
                    out.push_str(", ");
                }
            }
            Layout::Indented => {
                if index > 0 {
                    out.push(',');
                }
                out.push('\n');
                out.push_str(&"  ".repeat(depth + 1));
            }
        }
        if let Some(key) = key {
            write_string(out, key, style.ascii);
            out.push_str(": ");
        }
        write_value(out, item, style, depth + 1);
    }
    if style.layout == Layout::Indented {
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    }
    out.push(close);
}

fn write_string(out: &mut String, text: &str, ascii: bool) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if u32::from(control) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            printable if !ascii || (' '..='~').contains(&printable) => out.push(printable),
            other => {
                let mut units = [0_u16; 2];
                for unit in other.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

fn number_text(number: &Number) -> String {
    match number.as_f64() {
        Some(float) if number.is_f64() => float_repr(float),
        _ => number.to_string(),
    }
}

/// Python's `repr(float)`: the shortest digits that round-trip, positional between `1e-4`
/// and `1e16` (always with a fraction, `300.0`), and `1e-05` / `1e+16` outside it.
#[must_use]
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_owned();
    }
    // `{:e}` is the shortest round-tripping digits: `d[.ddd]e<exp>`.
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if negative { "-" } else { "" };
    if (-4..16).contains(&exponent) {
        let point = exponent + 1;
        let text = if point <= 0 {
            format!(
                "0.{}{digits}",
                "0".repeat(usize::try_from(-point).unwrap_or(0))
            )
        } else {
            let point = usize::try_from(point).unwrap_or(0);
            if digits.len() <= point {
                format!("{digits}{}.0", "0".repeat(point - digits.len()))
            } else {
                format!("{}.{}", &digits[..point], &digits[point..])
            }
        };
        format!("{sign}{text}")
    } else {
        let fraction = &digits[1..];
        let mantissa = if fraction.is_empty() {
            digits[..1].to_owned()
        } else {
            format!("{}.{fraction}", &digits[..1])
        };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!("{sign}{mantissa}e{exponent_sign}{:02}", exponent.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn floats_are_spelled_as_python_reprs_them() {
        for (value, expected) in [
            (300.0, "300.0"),
            (2.5, "2.5"),
            (0.1, "0.1"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1e16, "1e+16"),
            (123_456_789_012_345.6, "123456789012345.6"),
            (-2.0, "-2.0"),
            (0.0, "0.0"),
            (1.5e300, "1.5e+300"),
        ] {
            assert_eq!(float_repr(value), expected, "{value}");
        }
    }

    #[test]
    fn strings_escape_as_ensure_ascii_says() {
        let value = json!("é — \u{7f}\u{1}\"\\/\t😀");
        assert_eq!(
            to_string(&value, Style::LINE),
            "\"\\u00e9 \\u2014 \\u007f\\u0001\\\"\\\\/\\t\\ud83d\\ude00\""
        );
        assert_eq!(
            to_string(&value, Style::SORTED_RAW),
            "\"é — \u{7f}\\u0001\\\"\\\\/\\t😀\""
        );
    }

    #[test]
    fn empty_containers_stay_on_their_line() {
        let value = json!({"a": [], "b": {}, "c": [1, {"d": null}]});
        assert_eq!(
            to_string(&value, Style::INDENTED),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": null\n    }\n  ]\n}"
        );
        assert_eq!(
            to_string(&json!({"b": 1, "a": true}), Style::SORTED_RAW),
            r#"{"a": true, "b": 1}"#
        );
    }
}
