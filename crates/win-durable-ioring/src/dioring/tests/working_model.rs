// Copyright (c) 2026 Mike Grier
//! The working model end to end (root `M39.4`): a database's data directory and its log directory,
//! each file opened through this crate so its flush domains are derived ([DI-D-117]), the
//! directories predicted and the files confirmed ([DI-D-118]), and a failure of the data file's
//! storage shown to reach the log's writes exactly when the two share a disk.
//!
//! **Which case runs depends on the machine.** The data directory is under the temporary directory;
//! the log directory is under `RUNNER_TEMP` when it is set, which on a CI runner is on a work disk of
//! its own, and under the temporary directory otherwise. On one disk the shared case runs; on two,
//! the separate case. Each run says which, and nothing guarantees the separate case runs anywhere.
//!
//! **Why an imported failure and not a failed flush.** A seal flushes both files, and a flush that
//! fails on the data file may complete before or after the log's succeeds, which decides whether the
//! log's write was already covered. An import scoped to the data file's domains is the same question
//! -- the data disk's failure -- with no order in it.
//!
//! [DI-D-117]: ../../../../durable-ioring/DESIGN-NOTES.md#di-d-117
//! [DI-D-118]: ../../../../durable-ioring/DESIGN-NOTES.md#di-d-118

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use durable_ioring::{DurableRing, Entry, FlushDomain, ImportScope, IoTargetKey};

use super::pushes::{Harness, instance};
use crate::dioring::TargetSetup;
use crate::dioring::tests::wtf;
use crate::scratch_volume::ScratchVolume;
use crate::windows_backend::{Cached, DirectoryDomains, FileOptions, OpenedFile, Win32FileTarget};

const LOG: IoTargetKey = IoTargetKey(10);
const DATA: IoTargetKey = IoTargetKey(11);

/// A directory of its own under `parent`, removed when dropped.
struct Directory(PathBuf);

impl Directory {
    fn under(parent: &Path, tag: &str) -> Self {
        let path = parent.join(format!(
            "dioring-model-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("after the epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("create the directory");
        Self(path)
    }

    /// A file named `name` in the directory, created, then opened through this crate.
    fn open(&self, name: &str) -> (OpenedFile, DirectoryDomains) {
        let predicted = DirectoryDomains::predict(wtf(&self.0)).expect("predict the directory");
        let path = self.0.join(name);
        File::create(&path).expect("create the file");
        let file = OpenedFile::<Cached>::open(wtf(&path), FileOptions::new().read().write())
            .expect("open the file");
        predicted
            .confirm(&file)
            .expect("the file is where its directory is");
        (file, predicted)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The ops a failure scoped to `domains` suspects, by sequence number, after one write to the log
/// (sequence 0) and one to the data file (sequence 1).
fn suspects_of_a_failure_of(harness: &mut Harness, domains: &[FlushDomain]) -> Vec<u64> {
    let imported = harness
        .ring
        .import_failure(ImportScope::Domains(Arc::from(domains)), |_| {})
        .expect("import");
    let failed = match harness.next_entry() {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    };
    assert_eq!(failed.id, imported.failure);
    harness
        .suspects(failed.id)
        .iter()
        .map(|op| op.seq)
        .collect()
}

/// A ring over `log` and `data`, each with the domains its open derived, and one write to each.
fn ring_with_a_write_to_each(log: OpenedFile, data: OpenedFile) -> Harness {
    let mut harness = Harness::new(instance(
        vec![
            TargetSetup::new(LOG, Win32FileTarget::new(log)),
            TargetSetup::new(DATA, Win32FileTarget::new(data)),
        ],
        Vec::new(),
    ));
    harness.write(LOG, 0, b"log record", 1, 1);
    harness.write(DATA, 0, b"data page", 1, 2);
    harness.next_n(2);
    harness
}

fn shared(a: &[FlushDomain], b: &[FlushDomain]) -> bool {
    a.iter().any(|domain| b.contains(domain))
}

#[test]
fn a_failure_of_the_data_files_storage_reaches_the_log_exactly_when_they_share_a_disk() {
    let log_root = std::env::var_os("RUNNER_TEMP").map_or_else(std::env::temp_dir, PathBuf::from);
    let data_directory = Directory::under(&std::env::temp_dir(), "data");
    let log_directory = Directory::under(&log_root, "log");

    let (data, _) = data_directory.open("data.db");
    let (log, _) = log_directory.open("log.ldf");
    let data_domains = data.flush_domains().to_vec();
    let log_domains = log.flush_domains().to_vec();
    assert!(
        !data_domains.is_empty() && !log_domains.is_empty(),
        "both derived"
    );
    let together = shared(&data_domains, &log_domains);
    eprintln!(
        "the {} case: data on {data_domains:?}, log on {log_domains:?}",
        if together { "shared" } else { "separate" }
    );

    let mut harness = ring_with_a_write_to_each(log, data);
    let suspects = suspects_of_a_failure_of(&mut harness, &data_domains);

    let expected: &[u64] = if together { &[0, 1] } else { &[1] };
    assert_eq!(
        suspects, expected,
        "the data file's write is suspect, and the log's exactly when the files share a disk"
    );
    harness.finish();
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_file_on_a_virtual_disk_shares_fate_with_the_volume_holding_its_backing_file() {
    // The scratch disk's file is in the temporary directory, so a file on it and a file in the
    // temporary directory share the host's disks, which only the virtual-disk hop finds.
    let scratch = ScratchVolume::create_formatted(64).expect("a formatted scratch volume");
    let on_disk = Directory::under(&scratch.root(), "vhd");
    let on_host = Directory::under(&std::env::temp_dir(), "host");

    let (vhd_file, _) = on_disk.open("log.ldf");
    let (host_file, _) = on_host.open("data.db");
    let vhd_domains = vhd_file.flush_domains().to_vec();
    let host_domains = host_file.flush_domains().to_vec();
    for domain in &host_domains {
        assert!(
            vhd_domains.contains(domain),
            "the host's {domain:?} is among the virtual disk's: {vhd_domains:?}"
        );
    }
    let own: Vec<FlushDomain> = vhd_domains
        .iter()
        .filter(|domain| !host_domains.contains(domain))
        .cloned()
        .collect();
    assert!(!own.is_empty(), "and the virtual disk's own disk besides");

    let mut harness = ring_with_a_write_to_each(vhd_file, host_file);
    assert_eq!(
        suspects_of_a_failure_of(&mut harness, &host_domains),
        [0, 1],
        "a failure of the host's disks reaches the file on the virtual disk"
    );
    assert_eq!(
        suspects_of_a_failure_of(&mut harness, &own),
        [0],
        "a failure of the virtual disk alone does not reach the host's file"
    );
    harness.finish();
    drop(on_disk);
}
