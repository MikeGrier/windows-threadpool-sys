// Copyright (c) 2026 Mike Grier
//! Tests for predicting a directory's flush domains and confirming a file's against it.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use durable_ioring::{FlushDomain, NoData};
use win_shared_os_owned_handle::SharedHandle;

use super::DirectoryDomains;
use crate::dioring::tests::{TempFile, wtf};
use crate::windows_backend::file_domains::StorageStack;
use crate::windows_backend::{
    Cached, FileOptions, OpenedFile, OpenedVolume, VolumeOptions, VolumePath, disk_flush_domain,
};

/// A directory of its own under the temporary directory, removed when dropped.
struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "durable-ioring-dir-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("after the epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("create the directory");
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn drive_of(path: &Path) -> VolumePath {
    let text = path.to_str().expect("a UTF-8 temp path");
    let plain = text.strip_prefix(r"\\?\").unwrap_or(text);
    VolumePath::drive(plain.chars().next().expect("a drive")).expect("a drive letter")
}

fn expected_for(path: &Path) -> Vec<FlushDomain> {
    OpenedVolume::<Cached, NoData>::open(&drive_of(path), VolumeOptions::new())
        .expect("open the volume with no data access")
        .flush_domains()
}

/// A storage stack whose every step fails.
struct Broken;

impl StorageStack for Broken {
    fn volume_of(&self, _file: &SharedHandle) -> io::Result<VolumePath> {
        Err(io::Error::other("no volume"))
    }
    fn disks_of(&self, _volume: &VolumePath) -> io::Result<Vec<u32>> {
        Err(io::Error::other("no extents"))
    }
    fn is_file_backed(&self, _disk: u32) -> io::Result<bool> {
        Err(io::Error::other("no descriptor"))
    }
    fn hosts_of(&self, _volume: &VolumePath) -> io::Result<Vec<VolumePath>> {
        Err(io::Error::other("no dependency"))
    }
}

#[test]
fn a_directory_predicts_the_disks_of_its_volume() {
    let directory = TempDirectory::new("predict");

    let predicted = DirectoryDomains::predict(wtf(&directory.0)).expect("a local directory");

    assert!(!predicted.domains().is_empty());
    assert_eq!(predicted.domains(), expected_for(&directory.0).as_slice());
    assert_eq!(predicted.directory(), &wtf(&directory.0));
}

#[test]
fn a_file_created_in_the_directory_confirms_the_prediction() {
    let directory = TempDirectory::new("confirm");
    let predicted = DirectoryDomains::predict(wtf(&directory.0)).expect("predict");
    let path = directory.0.join("a-file");
    File::create(&path).expect("create");

    let file = OpenedFile::<Cached>::open(wtf(&path), FileOptions::new().read().write())
        .expect("open the file");

    predicted
        .confirm(&file)
        .expect("the file is where the directory is");
    drop(file);
}

#[test]
fn a_file_whose_domains_differ_from_the_prediction_is_reported_with_both() {
    let temp = TempFile::new("dd-mismatch");
    let file = temp.open();
    let elsewhere = DirectoryDomains {
        directory: wtf(Path::new(r"C:\elsewhere")),
        domains: vec![disk_flush_domain(99)],
    };

    let mismatch = elsewhere.confirm(&file).expect_err("a different disk");

    assert_eq!(mismatch.predicted, [disk_flush_domain(99)]);
    assert_eq!(mismatch.found, file.flush_domains());
    assert!(mismatch.to_string().contains("not the ones predicted"));
    let _: &dyn std::error::Error = &mismatch;
}

#[test]
fn a_prediction_that_differs_by_one_extra_domain_is_a_mismatch() {
    // A prediction is made by the same derivation as a file's domains, so anything that is not the
    // same bytes in the same order is a mismatch, and not a near miss.
    let temp = TempFile::new("dd-extra");
    let file = temp.open();
    let mut more = file.flush_domains().to_vec();
    more.push(FlushDomain::new(b"extra".to_vec()));
    let wider = DirectoryDomains {
        directory: wtf(Path::new(r"C:\wider")),
        domains: more,
    };

    assert!(wider.confirm(&file).is_err());
}

#[test]
fn a_directory_that_does_not_exist_is_the_opens_own_error() {
    let directory = TempDirectory::new("absent");
    let absent = directory.0.join("not-there");

    let error = DirectoryDomains::predict(wtf(&absent)).expect_err("no such directory");

    assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
}

#[test]
fn a_prediction_that_cannot_be_made_is_an_error_and_never_an_empty_set() {
    let temp = TempFile::new("dd-broken");
    let handle = SharedHandle::from(File::open(&temp.0).expect("open"));

    let error = DirectoryDomains::from_handle(&Broken, wtf(&temp.0), &handle)
        .expect_err("every step fails");

    assert!(error.to_string().contains("no volume"), "{error}");
}

/// The administrative share of this volume, `\\localhost\X$\...`, naming `local`.
fn admin_share(local: &Path) -> PathBuf {
    let text = local.to_str().expect("a UTF-8 temp path");
    let plain = text.strip_prefix(r"\\?\").unwrap_or(text);
    let drive = plain.chars().next().expect("a drive letter");
    PathBuf::from(format!(r"\\localhost\{drive}${}", &plain[2..]))
}

// Named to begin as the share test of `file_domains` does, so the CI step that runs that test by
// name runs this one too.
#[test]
#[ignore = "needs the administrative share \\\\localhost\\C$, which a CI runner has"]
fn a_file_on_a_share_has_no_domains_found_so_a_local_prediction_does_not_confirm_it() {
    let directory = TempDirectory::new("share");
    let predicted = DirectoryDomains::predict(wtf(&directory.0)).expect("the local directory");
    let path = directory.0.join("a-file");
    File::create(&path).expect("create");

    let file =
        OpenedFile::<Cached>::open(wtf(&admin_share(&path)), FileOptions::new().read().write())
            .expect("the same file, through the share");
    let mismatch = predicted
        .confirm(&file)
        .expect_err("the file through the share found none");
    assert!(mismatch.found.is_empty());
    assert_eq!(mismatch.predicted, predicted.domains());

    // Measured on 2026-11-05: a remote handle's final path in GUID form is refused by the call itself
    // (ERROR_PATH_NOT_FOUND) rather than given in another form; either way no prediction is made.
    let error = DirectoryDomains::predict(wtf(&admin_share(&directory.0)))
        .expect_err("a directory on a share is not predicted");
    eprintln!("a directory on a share: {error}");
    drop(file);
}
