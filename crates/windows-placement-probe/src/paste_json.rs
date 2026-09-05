// Copyright (c) Mike Grier.

//! JSON laid out to be read in a terminal and pasted into a thread.
//!
//! `serde_json::to_string_pretty` puts every array element on its own line,
//! which is the right default for a file nobody reads and the wrong one here.
//! This record is read by a person before they send it, and scrolled past by
//! everyone who reads the thread afterwards. A machine reporting eight cache
//! domains spent eight lines saying `2`; an eight-socket host would spend
//! sixty-four lines listing its nodes one integer at a time -- and that is
//! exactly the machine whose submission matters most.
//!
//! # The rule
//!
//! Two decisions, in this order:
//!
//! 1. **An object always expands**, one field per line, even when it would fit.
//!    Objects are where the meaning is, and a reader scanning for `os_build`
//!    should find it in a predictable place rather than somewhere in the middle
//!    of a long line.
//! 2. **An array holding no object collapses** onto one line when it fits, and
//!    otherwise fills lines up to the width budget rather than going one
//!    element per line. So `[2, 2, 2, 2, 2, 2, 2, 2]` is one line, `[[0, 16]]`
//!    is one line, sixty-four node sizes are a few lines, and `placements` --
//!    an array of objects -- expands as before.
//!
//! # Field order is the record's, not the alphabet's
//!
//! The layout walks an order-preserving tree of its own rather than
//! [`serde_json::Value`], whose object is a `BTreeMap` and so sorts keys.
//! Sorting looks harmless and is not: `schema_version` and `recorded_at` are
//! written first because that is what a reader needs first, and alphabetical
//! order buries them under `build` and `by_class`. Each measurement likewise
//! leads with `placement` and `strategy`, not `consumer_batch`. This was not
//! foreseen -- it was read off the output of a run, having been introduced by
//! an earlier draft of this very module.
//!
//! `serde_json`'s `preserve_order` feature would fix it by making its map an
//! `IndexMap`. It is deliberately not used: cargo unifies features across a
//! build, four other crates in this workspace share `serde_json`, and switching
//! their map type to satisfy this module's typography would be a change to
//! them.
//!
//! # This is layout only
//!
//! The bytes of every scalar come from `serde_json` itself, so numbers keep
//! their exact rendering and strings keep their exact escaping; this module
//! chooses only where the whitespace goes. That claim is worth the test that
//! backs it: the output is parsed back and compared against the value it came
//! from, so a layout that changed the data could not pass.

use std::fmt;

use serde::Serialize;
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

/// The column a laid-out line tries not to exceed.
///
/// Well under the 120 the submission's own width test enforces, because that
/// test is about surviving a paste and this is about being comfortable to read
/// in a terminal that is probably 80 or 100 wide. It is a target rather than a
/// guarantee: a single long string cannot be broken and is emitted whole.
const MAX_WIDTH: usize = 96;

/// Spaces per level of nesting.
const INDENT: usize = 2;

/// A JSON value that remembers the order its fields were written in.
///
/// Scalars keep a [`serde_json::Value`] so their rendering stays `serde_json`'s
/// job; the ordering of object fields is this type's only contribution.
enum Node {
    /// Anything that is not a container.
    Scalar(Value),
    /// Elements in order.
    Array(Vec<Node>),
    /// Fields in the order they were serialized, which is declaration order.
    Object(Vec<(String, Node)>),
}

/// Builds a [`Node`] from whatever the deserializer hands over, in order.
struct NodeVisitor;

impl<'de> Visitor<'de> for NodeVisitor {
    type Value = Node;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("any JSON value")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Node::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Node, A::Error> {
        // `next_entry` yields fields in document order, which is the whole
        // reason this type exists.
        let mut fields = Vec::new();
        while let Some(entry) = map.next_entry::<String, Node>()? {
            fields.push(entry);
        }
        Ok(Node::Object(fields))
    }

    fn visit_bool<E>(self, value: bool) -> Result<Node, E> {
        Ok(Node::Scalar(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Node, E> {
        Ok(Node::Scalar(Value::from(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Node, E> {
        Ok(Node::Scalar(Value::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Node, E> {
        Ok(Node::Scalar(Value::from(value)))
    }

    fn visit_str<E>(self, value: &str) -> Result<Node, E> {
        Ok(Node::Scalar(Value::String(value.to_owned())))
    }

    fn visit_unit<E>(self) -> Result<Node, E> {
        Ok(Node::Scalar(Value::Null))
    }

    fn visit_none<E>(self) -> Result<Node, E> {
        Ok(Node::Scalar(Value::Null))
    }
}

impl<'de> Deserialize<'de> for Node {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(NodeVisitor)
    }
}

/// Serialize `value` as JSON laid out for a person.
///
/// # Errors
///
/// Returns whatever serializing `value` failed with. The round trip through
/// text is how field order is kept: `serde_json` writes fields in declaration
/// order, and reading them straight back preserves it.
pub fn to_paste_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let ordered = serde_json::to_string(value)?;
    let node: Node = serde_json::from_str(&ordered)?;
    let mut out = String::new();
    write_value(&node, 0, 0, &mut out)?;
    Ok(out)
}

/// Whether an object appears anywhere inside `node`, including at its root.
///
/// This is what decides that an array expands. Checking the whole subtree and
/// not just the immediate elements keeps rule 1 honest: an object must not slip
/// onto one line by being nested inside an array that happened to fit.
fn holds_an_object(node: &Node) -> bool {
    match node {
        Node::Object(_) => true,
        Node::Array(items) => items.iter().any(holds_an_object),
        Node::Scalar(_) => false,
    }
}

/// The one-line form of `node`, with a space after each separator.
///
/// `serde_json`'s own compact form omits those spaces, which is right for a
/// wire format and cramped for something a person reads.
fn compact(node: &Node) -> Result<String, serde_json::Error> {
    match node {
        Node::Array(items) => {
            let parts = items
                .iter()
                .map(compact)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            Ok(format!("[{parts}]"))
        }
        Node::Object(fields) => {
            let mut parts = Vec::with_capacity(fields.len());
            for (key, child) in fields {
                parts.push(format!(
                    "{}: {}",
                    serde_json::to_string(key)?,
                    compact(child)?
                ));
            }
            Ok(format!("{{{}}}", parts.join(", ")))
        }
        // Scalars are `serde_json`'s to render, never this module's.
        Node::Scalar(scalar) => serde_json::to_string(scalar),
    }
}

/// Append `count` spaces.
fn indent_by(out: &mut String, count: usize) {
    for _ in 0..count {
        out.push(' ');
    }
}

/// Write `node` starting at `column`, indenting any continuation to `indent`.
///
/// `column` is where the value's first character lands, which is past the key
/// on an object field. Passing the indent instead would let a field's value
/// overrun the budget by the width of its own name.
fn write_value(
    node: &Node,
    indent: usize,
    column: usize,
    out: &mut String,
) -> Result<(), serde_json::Error> {
    let inline = compact(node)?;
    if !holds_an_object(node) && column + inline.len() <= MAX_WIDTH {
        out.push_str(&inline);
        return Ok(());
    }

    match node {
        Node::Object(fields) if !fields.is_empty() => {
            out.push_str("{\n");
            let inner = indent + INDENT;
            for (position, (key, child)) in fields.iter().enumerate() {
                indent_by(out, inner);
                let key_text = serde_json::to_string(key)?;
                out.push_str(&key_text);
                out.push_str(": ");
                write_value(child, inner, inner + key_text.len() + ": ".len(), out)?;
                if position + 1 < fields.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            indent_by(out, indent);
            out.push('}');
        }
        Node::Array(items) if !items.is_empty() => {
            out.push_str("[\n");
            let inner = indent + INDENT;
            if holds_an_object(node) {
                for (position, item) in items.iter().enumerate() {
                    indent_by(out, inner);
                    write_value(item, inner, inner, out)?;
                    if position + 1 < items.len() {
                        out.push(',');
                    }
                    out.push('\n');
                }
            } else {
                write_filled(items, inner, out)?;
            }
            indent_by(out, indent);
            out.push(']');
        }
        // An empty container, or a scalar too long for the budget. Nothing can
        // be laid out, so it goes as it is rather than being broken.
        other => out.push_str(&compact(other)?),
    }

    Ok(())
}

/// Write `items` as filled lines, wrapping at the width budget.
///
/// The alternative for a long array is one element per line, which is what this
/// module exists to avoid.
fn write_filled(items: &[Node], indent: usize, out: &mut String) -> Result<(), serde_json::Error> {
    indent_by(out, indent);
    let mut column = indent;
    let mut line_is_empty = true;

    for (position, item) in items.iter().enumerate() {
        let piece = compact(item)?;
        let comma = usize::from(position + 1 < items.len());

        if !line_is_empty && column + " ".len() + piece.len() + comma > MAX_WIDTH {
            out.push('\n');
            indent_by(out, indent);
            column = indent;
            line_is_empty = true;
        }
        if !line_is_empty {
            out.push(' ');
            column += " ".len();
        }

        out.push_str(&piece);
        column += piece.len();
        if comma == 1 {
            out.push(',');
            column += 1;
        }
        line_is_empty = false;
    }

    out.push('\n');
    Ok(())
}

#[cfg(test)]
mod tests;
