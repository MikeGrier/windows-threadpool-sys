// Copyright (c) Mike Grier.

//! Tests for the backup file's overwrite protection.
//!
//! The collision this guards is not reachable by running the tool twice from a
//! shell -- process startup alone puts the two runs in different seconds -- so
//! it has to be tested directly. That is exactly why it survived: the failing
//! case is the one nobody trips over by hand.

use std::io::Write as _;

use super::write_backup_to_new_file;

/// A directory of this test's own, so a failure cannot be caused by, or blamed
/// on, another test's files.
///
/// Keyed by process id as well as by name. The names are unique within one test
/// binary, but the documented mutation workflow runs two `cargo test` processes
/// at once (`-j 2`), and both would otherwise resolve to the same path under the
/// system temp directory -- so one would delete the other's fixture mid-test and
/// the failure would look like a defect in the code under test.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "placement-probe-backup-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

#[test]
fn the_first_write_uses_the_name_it_was_given() {
    let dir = scratch("first");
    let name = dir.join("record.json");

    let written =
        write_backup_to_new_file(name.to_str().expect("utf-8 path"), "{}").expect("must write");

    assert_eq!(written, name.to_str().expect("utf-8 path"));
    assert_eq!(std::fs::read_to_string(&name).expect("readable"), "{}");
}

#[test]
fn a_second_write_in_the_same_second_does_not_destroy_the_first() {
    // The defect. The name carries a one-second timestamp and the previous
    // implementation used `fs::write`, which truncates -- so two runs landing
    // in the same second silently lost the first result, which is the worst
    // case for someone re-running the tool because they doubted the first.
    let dir = scratch("collision");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    let first = write_backup_to_new_file(name, "FIRST").expect("must write");
    let second = write_backup_to_new_file(name, "SECOND").expect("must write");

    assert_ne!(first, second, "the second write reused the first name");
    assert_eq!(
        std::fs::read_to_string(&first).expect("readable"),
        "FIRST",
        "the first record was overwritten"
    );
    assert_eq!(
        std::fs::read_to_string(&second).expect("readable"),
        "SECOND"
    );
}

#[test]
fn the_suffix_goes_before_the_extension() {
    // So a collection of records still sorts and filters as `*.json`, and the
    // gitignore pattern that keeps these out of commits keeps matching.
    let dir = scratch("suffix");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    write_backup_to_new_file(name, "a").expect("must write");
    let second = write_backup_to_new_file(name, "b").expect("must write");

    assert!(second.ends_with(".json"), "got {second}");
    assert!(second.contains("record-1"), "got {second}");
}

#[test]
fn many_collisions_keep_producing_distinct_files() {
    // Each retry must advance rather than fight over one alternative name.
    let dir = scratch("many");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    let mut written = Vec::new();
    for index in 0..10 {
        written.push(write_backup_to_new_file(name, &index.to_string()).expect("must write"));
    }

    let mut unique = written.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), written.len(), "names repeated: {written:?}");

    for (index, path) in written.iter().enumerate() {
        assert_eq!(
            std::fs::read_to_string(path).expect("readable"),
            index.to_string(),
            "{path} does not hold what was written to it"
        );
    }
}

#[test]
fn a_write_that_cannot_be_placed_reports_rather_than_loops() {
    // The exhaustion path. A caller waiting on the tool must not wait forever,
    // and the error has to name the problem rather than surface as a mystery.
    let dir = scratch("exhausted");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    // Occupy every name the helper will try.
    for attempt in 0..100 {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            format!("{}-{attempt}.json", name.trim_end_matches(".json"))
        };
        let mut file = std::fs::File::create(&candidate).expect("a placeholder");
        file.write_all(b"taken").expect("writable");
    }

    let error = write_backup_to_new_file(name, "{}").expect_err("every name is taken");

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
}
