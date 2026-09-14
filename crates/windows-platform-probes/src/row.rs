// Copyright (c) Mike Grier.

//! The machine-readable row, as a value with one writer.
//!
//! # Why this exists
//!
//! The row was built by interpolating every value positionally into a `concat!`
//! template. Two defect classes follow from that construction, and both are
//! closed by replacing it rather than by checking it -- see
//! [DESIGN-NOTES.md](../DESIGN-NOTES.md#d-encoded-row-is-the-contract).
//!
//! **Injection.** Caller text reaching the mined artifact is contamination of
//! the contract. Measured on PR #88: an `io::Error` whose text contained `{` was
//! selected as the report's machine-readable row, so a reader checked the
//! caller's text instead of the probe's. A [`Value::Text`] is escaped by the
//! writer, so no string it carries can end the string it is in, let alone start
//! a new row.
//!
//! **Field order and labelling.** A field's name and its value were related only
//! by counting positions, so a reordered argument or a miscounted placeholder
//! yielded mislabelled data that still parses and that nothing downstream can
//! detect. Here a name and its value are one pair, moved together or not at all.
//!
//! # What [`Row::keys`] derives, and what it cannot
//!
//! It reads back the names a caller actually supplied, which is what the
//! writer's own tests need: a row renders the members it was given, in order,
//! and that property is derivable rather than restated.
//!
//! **It is NOT the contract, and this section used to say it was.** The claim
//! here was that the well-formedness check "no longer needs a list of expected
//! keys written beside it". That is false, and falsifiably so: `keys` reports
//! what the builder happened to supply, so a row missing a required field is
//! still perfectly self-consistent. Measured -- with `.with("packages", ...)`
//! deleted from the renderer, the whole suite stayed green.
//!
//! The contract is `topology_report::MEASURED_ROW_KEYS` and its `_SHAPES`
//! sibling, owned by the renderer that owes those fields and stated
//! independently of it. A schema is not derivable from the thing it constrains;
//! the anti-census rule is about facts that CAN be derived, and this is not one.
//! Reported by a review, which found this passage still steering a reader back
//! toward the vacuous check.

use std::fmt::Write as _;

#[cfg(test)]
mod tests;

/// The shape a row's value must have, as a schema states it.
///
/// **The type-level counterpart of [`Value`], and the half the key list was
/// missing.** A schema of names alone pins WHICH fields a row carries and says
/// nothing about what they hold, so a renderer could publish `"processors"` as
/// a string and satisfy every check the crate had. Measured, before this
/// existed: `.with("processors", observation.online_processors.to_string())`
/// left all 230 library tests and all 10 real-host integration tests green.
/// Reported by a review.
///
/// **Stated independently rather than derived from the renderer**, which is the
/// same reasoning as the diagnostic goldens: a shape read back out of the
/// `Value` the renderer produced would move whenever the renderer moved, and so
/// could never disagree with it. A schema is not derivable from the thing it
/// constrains -- writing it down twice is what makes it a schema rather than a
/// restatement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A JSON string.
    Text,
    /// A JSON number.
    Number,
    /// A number, or `null` where the answer is genuinely "there is none".
    NumberOrNull,
    /// A list whose every element is a number.
    ListOfNumbers,
    /// A list whose every element is an object.
    ListOfObjects,
    /// A list whose every element is an object carrying a string `code`.
    ///
    /// The diagnostic lists. `code` is the stable discriminant a survey groups
    /// by, so an entry without one is unmineable even though it is valid JSON.
    ListOfCoded,
    /// An object whose every member is a number.
    ObjectOfNumbers,
}

/// A value the row can carry.
///
/// Deliberately not every JSON shape: there is no floating point, because every
/// quantity this crate publishes is a count, an identifier or a list of them,
/// and a float in a mined artifact invites a consumer to compare values that
/// were never measured that precisely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A string, escaped on the way out.
    Text(String),
    /// A count.
    Number(usize),
    /// The absent case, for a field whose answer is "there is none".
    ///
    /// A variant rather than an omitted key, because a consumer can tell `null`
    /// from a field this probe is too old to publish, and cannot tell an
    /// omission from either.
    Null,
    /// An ordered list.
    List(Vec<Value>),
    /// An ordered set of named members.
    Object(Vec<(&'static str, Value)>),
}

impl From<&str> for Value {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for Value {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<usize> for Value {
    fn from(count: usize) -> Self {
        Self::Number(count)
    }
}

impl From<u8> for Value {
    fn from(count: u8) -> Self {
        Self::Number(count as usize)
    }
}

impl<T: Into<Self>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

impl<T: Into<Self>> FromIterator<T> for Value {
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        Self::List(items.into_iter().map(Into::into).collect())
    }
}

impl Value {
    /// Append this value's rendering to `out`.
    fn write(&self, out: &mut String) {
        match self {
            Self::Text(text) => write_escaped(out, text),
            Self::Number(count) => {
                let _ = write!(out, "{count}");
            }
            Self::Null => out.push_str("null"),
            Self::List(items) => {
                out.push('[');
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (at, (name, value)) in members.iter().enumerate() {
                    if at > 0 {
                        out.push(',');
                    }
                    write_escaped(out, name);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Write `text` as a JSON string, escaped.
///
/// **This is the whole of the injection fix**, so it is deliberately total: a
/// caller's `io::Error` can contain a quote, a backslash, a newline, or a
/// control character from a localised message, and each of those would otherwise
/// end the string or the line.
fn write_escaped(out: &mut String, text: &str) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below a space is a control character JSON forbids
            // unescaped. `\u` form rather than a name, because the named escapes
            // above are the only ones JSON defines.
            control if control < ' ' => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

/// The report's machine-readable row.
///
/// Members are ordered, and the order is the order they were added. That is a
/// property worth keeping even though JSON readers do not care: a human reading
/// accumulated CI output reads them in order, and a stable order makes a diff
/// between two runs legible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    members: Vec<(&'static str, Value)>,
}

impl Row {
    /// A row of the given `reason`, which is how a mining pass selects it.
    #[must_use]
    pub fn new(reason: &'static str) -> Self {
        Self {
            members: vec![("reason", Value::Text(reason.to_owned()))],
        }
    }

    /// Add a member.
    ///
    /// Takes the name and the value together, which is the point: they cannot be
    /// reordered apart, and there is no position to miscount.
    ///
    /// # Panics
    ///
    /// If `name` is already present. A repeated top-level key is the one
    /// malformation that SURVIVES a consumer's parse -- `serde_json` and
    /// `JSON.parse` both accept it and silently keep the last value -- so a row
    /// carrying one is not a broken artifact a survey discards but an ambiguous
    /// one it mines, which is worse. The crate reports it as
    /// `report_oracle::RowDefect::RepeatedKey`; this is the writer being unable
    /// to produce it in the first place.
    ///
    /// (Deliberately not an intra-doc link. `report_oracle` is compiled only
    /// under `cfg(any(test, feature = "oracle-in-renderer"))` while this module
    /// is always built, so a link here cannot resolve in a default `cargo doc`
    /// and emits a broken-intra-doc-link warning. Found by a review, and
    /// confirmed by running `cargo doc -p windows-platform-probes --lib`.)
    ///
    /// **Why a panic and not a `Result`.** Every caller is a renderer in this
    /// crate composing a fixed schema, so a repeat is a programming error at the
    /// call site, not a condition to handle -- and a fallible builder would put
    /// a `?` on nineteen infallible calls to describe a case that must never
    /// happen. Reported by a review, which observed that this public writer
    /// could emit a row the crate's own oracle faults.
    #[must_use]
    pub fn with(mut self, name: &'static str, value: impl Into<Value>) -> Self {
        assert!(
            !self.members.iter().any(|(present, _)| *present == name),
            "the row already carries `{name}`, and a repeated key survives a \
             consumer's parse as whichever value happened to come last"
        );
        self.members.push((name, value.into()));
        self
    }

    /// Every key this row carries, in order.
    ///
    /// Derived rather than declared, so a check over the key set cannot drift
    /// from what is published.
    #[must_use]
    pub fn keys(&self) -> Vec<&'static str> {
        self.members.iter().map(|(name, _)| *name).collect()
    }

    /// The row, rendered as one line of JSON.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        Value::Object(self.members.clone()).write(&mut out);
        out
    }
}
