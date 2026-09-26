// Copyright (c) 2026 Mike Grier
#![cfg(windows)]
//! The ring-owned inventory: a push hands over a payload and gets a name, and
//! the payload comes back from the pop that observed its completion (M28.3.3).
//!
//! This is the round trip `D-55` exists to make possible and `D-71`/`D-73`
//! shaped. What it demonstrates is not that a read works -- every other test
//! here does that -- but that **the caller never holds the buffer** between
//! push and completion, so there is no token to lose and no map to keep.

use std::io::Write;

use windows_ioring_sys::{Batch, IoRing, PushOptions};

const LEN: usize = 4096;

fn fixture(tag: &str) -> (std::path::PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-inventory-{}-{tag}.tmp",
        std::process::id()
    ));
    let mut file = std::fs::File::create(&path).expect("create fixture");
    file.write_all(&vec![0xC3_u8; LEN]).expect("fill fixture");
    file.sync_all().expect("flush fixture");
    let opened = std::fs::File::open(&path).expect("reopen for reading");
    (path, opened)
}

#[test]
fn a_payload_handed_to_the_ring_comes_back_from_the_pop_that_completes_it() {
    use std::os::windows::io::AsRawHandle;

    let (path, file) = fixture("round-trip");
    // The sidecar is the point of `X`: two thirds of the census sites carry
    // one, so a ring that could only hold buffers would serve a minority.
    let mut ring: IoRing<Vec<u8>, &'static str> =
        IoRing::with_inventory(8, 8).expect("create ring");

    let id = {
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `file` outlives the operation -- it is dropped at the end of
        // this test, after the completion has been popped.
        unsafe {
            batch.read_raw_owned(
                file.as_raw_handle(),
                vec![0_u8; LEN],
                "the sidecar",
                0,
                PushOptions::new(),
            )
        }
        .expect("queue a read the ring holds the buffer for")
    };

    assert_eq!(ring.held(), 1, "the ring holds the buffer, not the caller");

    let (completion, held) = loop {
        if let Some(popped) = ring.try_pop_held().expect("pop") {
            break popped;
        }
    };

    assert_eq!(
        completion.user_data(),
        id.user_data(),
        "the completion names the operation the push named"
    );
    let (buffer, extra) = held.expect("the ring was holding this operation's payload");
    let buffer = buffer.expect("a read carries a buffer");
    assert_eq!(extra, "the sidecar", "the sidecar comes back with it");
    assert_eq!(buffer.len(), LEN);
    assert!(
        buffer.iter().all(|&byte| byte == 0xC3),
        "the kernel filled the buffer the ring was holding"
    );
    assert_eq!(
        ring.held(),
        0,
        "the pop that returned the payload also retired its entry"
    );

    drop(file);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_second_pop_finds_nothing_held_for_the_same_identity() {
    // The inventory is not a log: an entry leaves at the pop that returns it,
    // so a duplicate completion -- which `RingContract` is what reports --
    // cannot hand the same buffer out twice.
    use std::os::windows::io::AsRawHandle;

    let (path, file) = fixture("retired-once");
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 8).expect("create ring");

    {
        let mut batch = Batch::new(&mut ring);
        // SAFETY: as above.
        unsafe {
            batch.read_raw_owned(
                file.as_raw_handle(),
                vec![0_u8; LEN],
                (),
                0,
                PushOptions::new(),
            )
        }
        .expect("queue");
    }
    while ring.try_pop_held().expect("pop").is_none() {}
    assert_eq!(ring.held(), 0, "the entry retired with its completion");

    drop(file);
    let _ = std::fs::remove_file(&path);
}

/// The guarded push holds the file for the operation, not the caller.
///
/// `read_owned` is the first shape to populate `Held.guard`. What it buys is
/// the reason `Held` exists at all: the caller may drop its own handle the
/// instant the push returns, and the read still completes correctly, because
/// the ring is holding a guard that outlives it.
#[test]
fn a_guarded_push_keeps_the_file_alive_after_the_caller_drops_its_handle() {
    use windows_ioring_sys::SharedFile;

    let (path, file) = fixture("guarded");
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 8).expect("create ring");
    let shared = SharedFile::new(file.into());

    {
        let mut batch = Batch::new(&mut ring);
        batch
            .read_owned(&shared, vec![0_u8; LEN], (), 0, PushOptions::new())
            .expect("queue a guarded read");
    }

    // The caller's own reference goes away here. The ring's guard is what
    // keeps the handle valid for the kernel.
    drop(shared);

    let (_completion, held) = loop {
        if let Some(popped) = ring.try_pop_held().expect("pop") {
            break popped;
        }
    };
    let (buffer, ()) = held.expect("the ring held this operation's buffer");
    let buffer = buffer.expect("a read carries a buffer");
    assert!(
        buffer.iter().all(|&byte| byte == 0xC3),
        "the read completed against a file only the ring was still holding"
    );

    let _ = std::fs::remove_file(&path);
}
