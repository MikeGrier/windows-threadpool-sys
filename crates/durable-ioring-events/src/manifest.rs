// Copyright (c) 2026 Mike Grier
//! The provider's manifest (DE-D-3), rendered from the table of events, so that it cannot disagree with it.
//!
//! The manifest is what turns an event's bytes back into names and fields for a reader. The provider records its events
//! without it ([DE-D-1](../DESIGN-NOTES.md#de-d-1)), and a reader installs it to see them decoded.

use std::fmt::Write;

use crate::schema::{ColumnType, EVENTS, EventSchema, KEYWORDS, PROVIDER_ID, PROVIDER_NAME};
use win_etw_sys::level;

#[cfg(test)]
mod tests;

/// The file the manifest is kept in, beside the crate's `Cargo.toml`, which a test holds equal to [`manifest`].
pub const MANIFEST_FILE: &str = "durable-ioring-events.man";

/// The name a manifest's resource and message files are given, which the installer overrides with the files it is told of.
const RESOURCE_FILE: &str = "durable-ioring-events.dll";

/// The manifest, as the text of an instrumentation manifest file.
///
/// It names the provider and its GUID, its keywords, each event with its id, level, keyword and template, and each template
/// with the types of its fields. An event's fields are what the sink writes, in that order, with no padding.
#[must_use]
pub fn manifest() -> String {
    let mut xml = String::new();
    let guid = PROVIDER_ID.to_string();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<instrumentationManifest\n");
    xml.push_str("    xmlns=\"http://schemas.microsoft.com/win/2004/08/events\"\n");
    xml.push_str("    xmlns:win=\"http://manifests.microsoft.com/win/2004/08/windows/events\"\n");
    xml.push_str("    xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">\n");
    xml.push_str("  <instrumentation>\n    <events>\n");
    let _ = writeln!(
        xml,
        "      <provider name=\"{PROVIDER_NAME}\" guid=\"{guid}\" symbol=\"DURABLE_IORING_EVENTS\"\n          resourceFileName=\"{RESOURCE_FILE}\" messageFileName=\"{RESOURCE_FILE}\">"
    );

    xml.push_str("        <keywords>\n");
    for keyword in KEYWORDS {
        let _ = writeln!(
            xml,
            "          <keyword name=\"{}\" mask=\"{:#x}\"/>",
            keyword.name, keyword.mask
        );
    }
    xml.push_str("        </keywords>\n");

    xml.push_str("        <events>\n");
    for event in &EVENTS {
        let _ = writeln!(
            xml,
            "          <event value=\"{}\" symbol=\"{}\" version=\"{}\" level=\"{}\" keywords=\"{}\" template=\"{}\"/>",
            event.id,
            event.name,
            event.descriptor().version,
            level_name(event.level),
            event.keyword.name,
            template_name(event)
        );
    }
    xml.push_str("        </events>\n");

    xml.push_str("        <templates>\n");
    for event in &EVENTS {
        let _ = writeln!(xml, "          <template tid=\"{}\">", template_name(event));
        for column in event.columns {
            match column.ty {
                ColumnType::Binary { length } => {
                    let _ = writeln!(
                        xml,
                        "            <data name=\"{}\" inType=\"{}\" length=\"{length}\"/>",
                        column.name,
                        in_type(column.ty)
                    );
                }
                _ => {
                    let _ = writeln!(
                        xml,
                        "            <data name=\"{}\" inType=\"{}\"/>",
                        column.name,
                        in_type(column.ty)
                    );
                }
            }
        }
        xml.push_str("          </template>\n");
    }
    xml.push_str("        </templates>\n");

    xml.push_str(
        "      </provider>\n    </events>\n  </instrumentation>\n</instrumentationManifest>\n",
    );
    xml
}

/// The name of an event's template.
fn template_name(event: &EventSchema) -> String {
    format!("{}Template", event.name)
}

/// The manifest's name for a level.
fn level_name(value: u8) -> &'static str {
    match value {
        level::CRITICAL => "win:Critical",
        level::ERROR => "win:Error",
        level::WARNING => "win:Warning",
        level::INFO => "win:Informational",
        level::VERBOSE => "win:Verbose",
        other => panic!(
            "level {other} has no name in a manifest: the table uses only the levels the page defines"
        ),
    }
}

/// The manifest's name for a type.
fn in_type(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::UInt32 => "win:UInt32",
        ColumnType::UInt64 => "win:UInt64",
        ColumnType::Boolean => "win:Boolean",
        ColumnType::UnicodeString => "win:UnicodeString",
        ColumnType::Binary { .. } => "win:Binary",
    }
}
