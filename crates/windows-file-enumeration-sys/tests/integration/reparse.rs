// Copyright (c) 2026 Mike Grier
//! Reparse points: a directory junction, created without any elevated
//! privilege, so the crate's reparse attribute and tag reporting is checked
//! against a real one rather than only against synthetic records.

use windows_file_enumeration_sys::{Completion, EnumerationRequest, Session};

use crate::support::{Scratch, create_junction};

/// `FILE_ATTRIBUTE_REPARSE_POINT`.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// `IO_REPARSE_TAG_MOUNT_POINT`, what a junction reports itself as.
const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;

#[test]
fn a_directory_junction_is_reported_as_a_reparse_point_with_its_tag() {
    let scratch = Scratch::with_files(&["real.txt"]);
    let target = scratch.subdir("target");
    std::fs::write(target.join("inside-target.txt"), b"").expect("a file inside the target");
    let link = scratch.child("junction-to-target");
    create_junction(&link, &target);

    let (session, receiver) = Session::new(8, 8).expect("room");
    let request = EnumerationRequest::for_path(scratch.path()).expect("resolvable");
    let handle = session.try_begin(request).expect("room");
    let enumeration = handle.id();
    handle.detach();

    let mut saw_junction = false;
    loop {
        match receiver.recv_timeout(std::time::Duration::from_secs(10)) {
            Some(Completion::Entry { entry, .. }) => {
                if entry.name().to_string_lossy() == "junction-to-target" {
                    saw_junction = true;
                    assert!(entry.is_reparse_point(), "{entry:?}");
                    assert_eq!(entry.reparse_tag(), Some(IO_REPARSE_TAG_MOUNT_POINT));
                    assert_eq!(
                        entry.attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
                        FILE_ATTRIBUTE_REPARSE_POINT
                    );
                }
            }
            Some(Completion::Terminal {
                enumeration: id,
                outcome,
            }) => {
                assert_eq!(id, enumeration);
                assert!(outcome.is_completed(), "{outcome:?}");
                break;
            }
            None => panic!("no terminal arrived"),
        }
    }
    assert!(saw_junction, "the junction was never delivered as an entry");
}
