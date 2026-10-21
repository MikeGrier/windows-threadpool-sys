// Copyright (c) 2026 Mike Grier
//! The manifest (DE-D-3): that the file kept with the crate is what the table renders, and that the rendering says what the table
//! does. That the manifest compiles and decodes is `tests/session.rs` and the steps in the README.

use super::{MANIFEST_FILE, level_name, manifest};
use crate::schema::{ColumnType, EVENTS, KEYWORDS, PROVIDER_ID, PROVIDER_NAME};

/// The file kept beside the crate's `Cargo.toml`.
const FILE: &str = include_str!("../../durable-ioring-events.man");

#[test]
fn the_file_kept_with_the_crate_is_the_manifest_the_table_renders() {
    assert_eq!(
        FILE,
        manifest(),
        "durable-ioring-events.man is out of date: run `cargo test -p durable-ioring-events --lib -- --ignored rewrite_the_manifest_file`"
    );
}

/// Rewrites the file from the table. Ignored: it changes the tree, and is how the file is brought up to date.
#[test]
#[ignore = "rewrites durable-ioring-events.man from the table"]
fn rewrite_the_manifest_file() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(MANIFEST_FILE);
    std::fs::write(path, manifest()).expect("the manifest file was written");
}

#[test]
fn the_manifest_names_the_provider_and_its_guid() {
    let text = manifest();
    assert!(text.contains(&format!("name=\"{PROVIDER_NAME}\"")));
    assert!(text.contains(&format!("guid=\"{PROVIDER_ID}\"")));
}

#[test]
fn the_manifest_has_each_event_with_its_id_level_keyword_and_template() {
    let text = manifest();
    for event in &EVENTS {
        let line = text
            .lines()
            .find(|line| line.contains(&format!("<event value=\"{}\"", event.id)))
            .unwrap_or_else(|| panic!("no line for {}", event.name));
        assert!(
            line.contains(&format!("symbol=\"{}\"", event.name)),
            "{line}"
        );
        assert!(
            line.contains(&format!("level=\"{}\"", level_name(event.level))),
            "{line}"
        );
        assert!(
            line.contains(&format!("keywords=\"{}\"", event.keyword.name)),
            "{line}"
        );
        assert!(
            line.contains(&format!("template=\"{}Template\"", event.name)),
            "{line}"
        );
        assert!(line.contains("version=\"0\""), "{line}");
    }
}

#[test]
fn the_manifest_has_one_event_element_per_event_and_one_template_per_event() {
    let text = manifest();
    assert_eq!(text.matches("<event ").count(), EVENTS.len());
    assert_eq!(text.matches("<template ").count(), EVENTS.len());
    assert_eq!(text.matches("</template>").count(), EVENTS.len());
}

#[test]
fn the_manifest_has_each_keyword_with_its_mask() {
    let text = manifest();
    for keyword in KEYWORDS {
        assert!(
            text.contains(&format!(
                "<keyword name=\"{}\" mask=\"{:#x}\"/>",
                keyword.name, keyword.mask
            )),
            "{}",
            keyword.name
        );
    }
}

#[test]
fn each_template_has_the_events_fields_in_order_with_their_types() {
    let text = manifest();
    for event in &EVENTS {
        let start = text
            .find(&format!("<template tid=\"{}Template\">", event.name))
            .expect("a template");
        let end = start + text[start..].find("</template>").expect("closed");
        let body = &text[start..end];
        let names: Vec<&str> = body
            .lines()
            .filter(|line| line.contains("<data "))
            .map(|line| {
                line.split("name=\"")
                    .nth(1)
                    .expect("a name")
                    .split('"')
                    .next()
                    .expect("closed")
            })
            .collect();
        let expected: Vec<&str> = event.columns.iter().map(|column| column.name).collect();
        assert_eq!(names, expected, "{}", event.name);
        for column in event.columns {
            let wanted = match column.ty {
                ColumnType::UInt32 => "win:UInt32",
                ColumnType::UInt64 => "win:UInt64",
                ColumnType::Boolean => "win:Boolean",
                ColumnType::UnicodeString => "win:UnicodeString",
                ColumnType::Binary { .. } => "win:Binary",
            };
            assert!(
                body.contains(&format!(
                    "<data name=\"{}\" inType=\"{wanted}\"",
                    column.name
                )),
                "{} {}",
                event.name,
                column.name
            );
        }
    }
}

#[test]
fn a_binary_field_names_its_length_field() {
    let text = manifest();
    assert!(text.contains("<data name=\"Domain\" inType=\"win:Binary\" length=\"DomainLength\"/>"));
    assert_eq!(text.matches("length=\"").count(), 1);
}

#[test]
fn the_manifest_is_lf_only_ends_with_a_newline_and_is_ascii() {
    let text = manifest();
    assert!(!text.contains('\r'));
    assert!(text.ends_with("</instrumentationManifest>\n"));
    assert!(text.is_ascii());
    assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"));
}

#[test]
fn the_manifest_is_the_same_each_time() {
    assert_eq!(manifest(), manifest());
}

#[test]
fn the_levels_the_page_defines_have_names() {
    assert_eq!(level_name(win_etw_sys::level::CRITICAL), "win:Critical");
    assert_eq!(level_name(win_etw_sys::level::ERROR), "win:Error");
    assert_eq!(level_name(win_etw_sys::level::WARNING), "win:Warning");
    assert_eq!(level_name(win_etw_sys::level::INFO), "win:Informational");
    assert_eq!(level_name(win_etw_sys::level::VERBOSE), "win:Verbose");
}

#[test]
#[should_panic(expected = "has no name in a manifest")]
fn a_level_the_page_does_not_define_has_no_name() {
    let _ = level_name(16);
}
