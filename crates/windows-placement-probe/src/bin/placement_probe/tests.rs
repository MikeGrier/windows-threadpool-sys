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

#[test]
fn a_failed_write_leaves_no_file_behind() {
    // The defect: the name was reserved with `create_new` and written into, so
    // a write that failed part-way -- a full disk, a quota, a killed process --
    // left a truncated `.json` under the name a COMPLETE record would have had.
    // Nothing downstream can tell those apart: a collector sees a record, and
    // the next run's collision suffix steps politely around the wreckage.
    let dir = scratch("failed-write");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    let error = super::write_backup_with(name, "{}", |_, _| {
        Err(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "no space left on device",
        ))
    })
    .expect_err("the injected write fails");
    assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);

    assert!(
        !std::path::Path::new(name).exists(),
        "a failed write must not leave a file under the record's own name"
    );
    let left: Vec<_> = std::fs::read_dir(&dir)
        .expect("readable")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert!(
        left.is_empty(),
        "the partial file must be cleaned up too, found {left:?}"
    );
}

#[test]
fn a_failed_write_does_not_consume_the_name_for_the_next_run() {
    // The consequence that makes the leftover worse than untidy. If the failed
    // attempt kept the name, the retry would be pushed onto a `-1` suffix and
    // the good record would sit beside a broken one that sorts first.
    let dir = scratch("failed-then-retry");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    let _ = super::write_backup_with(name, "{}", |_, _| Err(std::io::Error::other("interrupted")))
        .expect_err("the injected write fails");

    let written = write_backup_to_new_file(name, "GOOD").expect("the name must be free again");

    assert_eq!(written, name, "the retry must get the canonical name");
    assert_eq!(std::fs::read_to_string(&written).expect("readable"), "GOOD");
}

#[test]
fn a_successful_write_leaves_only_the_record() {
    // Publication is by rename through a temporary, so the temporary must not
    // survive a successful run either.
    let dir = scratch("no-litter");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    let written = write_backup_to_new_file(name, "{}").expect("must write");

    let left: Vec<_> = std::fs::read_dir(&dir)
        .expect("readable")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(left.len(), 1, "expected only the record, found {left:?}");
    assert_eq!(left[0].to_str().expect("utf-8 path"), written);
}

#[test]
fn publication_never_replaces_a_record_another_run_already_placed() {
    // The no-replace half of the publication guarantee. `std::fs::rename` on
    // Windows always passes MOVEFILE_REPLACE_EXISTING, so publishing with it
    // would silently overwrite a complete record another run had written --
    // destroying exactly what the collision suffix exists to protect, and doing
    // it in the window where this run had not yet finished writing.
    let dir = scratch("no-replace");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    // Stand in for a record another process completed a moment ago.
    std::fs::write(name, "PLACED BY SOMEONE ELSE").expect("writable");

    let written = write_backup_to_new_file(name, "MINE").expect("must find a free name");

    assert_ne!(written, name, "the existing record's name was taken");
    assert_eq!(
        std::fs::read_to_string(name).expect("readable"),
        "PLACED BY SOMEONE ELSE",
        "the existing record must survive byte-for-byte"
    );
    assert_eq!(std::fs::read_to_string(&written).expect("readable"), "MINE");
}

#[test]
fn a_failed_write_never_creates_the_records_name_at_all() {
    // The absent-or-complete half. An earlier version reserved the final name
    // with an empty file and renamed onto it afterwards, which left that empty
    // file visible under the record's own name for the whole duration of the
    // write -- and permanently, if the process was killed in that window. The
    // name must now come into existence already complete, or not at all.
    let dir = scratch("never-created");
    let name = dir.join("record.json");
    let name = name.to_str().expect("utf-8 path");

    let _ = super::write_backup_with(name, "{}", |_, _| {
        Err(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "no space left on device",
        ))
    })
    .expect_err("the injected write fails");

    assert!(
        !std::path::Path::new(name).exists(),
        "the record's name must never have been created"
    );
    let left: Vec<_> = std::fs::read_dir(&dir)
        .expect("readable")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert!(left.is_empty(), "nothing at all should remain: {left:?}");
}
