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

use windows_ioring_sys::{Batch, FlushCoverage, FlushMode, IoRing, PushOptions, SharedFile};

const LEN: usize = 4096;

mod common;

fn fixture(tag: &str) -> (common::TempPath, std::fs::File) {
    let path = common::TempPath::new("inventory", tag);
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

    // Bounded, not spun. An unbounded `loop` around `try_pop` states this
    // crate's contract as whatever the handle this test happened to open does
    // -- `RS-P-5` lets a kernel complete later than the submit returns -- and
    // turns a completion that never arrives into a hung harness with no test
    // name attached. `response_space_census.rs` refuses the shape.
    let (completion, held) = ring
        .pop_within(std::time::Duration::from_secs(30))
        .expect("pop")
        .expect("the read completes within the bound");

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
    // `pop_within` rather than a `try_pop` spin: RS-P-5 leaves a kernel free
    // to complete later than the submit returns, so spinning on `try_pop`
    // states this crate's contract as whatever the handle this test happened
    // to open does. `response_space_census.rs` enforces that, and caught this
    // line when `M28.4.1d.3` renamed the reclaiming pop over the spinning one.
    ring.pop_within(std::time::Duration::from_secs(30))
        .expect("pop")
        .expect("the read completes within the bound");
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

    let (_completion, held) = ring
        .pop_within(std::time::Duration::from_secs(30))
        .expect("pop")
        .expect("the read completes within the bound");
    let (buffer, ()) = held.expect("the ring held this operation's buffer");
    let buffer = buffer.expect("a read carries a buffer");
    assert!(
        buffer.iter().all(|&byte| byte == 0xC3),
        "the read completed against a file only the ring was still holding"
    );

    let _ = std::fs::remove_file(&path);
}

/// A `_raw` push holds nothing; an `_owned` push holds something (`M28.5`).
///
/// Both directions on one ring, because the claim is about the *difference*
/// and a test showing only one side would pass against a ring that answered
/// the same way every time.
///
/// This is what `M28.5` settled: a raw push creates no entry, so its pop hands
/// back an outer `None`, and the caller can tell why because it is the same
/// caller that chose the push. (A completion for an identity never minted no
/// longer reaches a caller as a quiet `None` at all -- the pop panics on it,
/// D-79.) That is a real contract with a real consequence, so it is asserted
/// rather than only documented.
///
/// It is also where `held` and `outstanding` are seen to differ in valid use:
/// both operations are outstanding, only one is held, and the gap is the raw
/// flush. `held() <= outstanding()` is the invariant, never equality.
#[test]
fn a_raw_push_holds_nothing_and_an_owned_push_holds_its_payload() {
    use std::os::windows::io::AsRawHandle;

    let (path, file) = fixture("raw-vs-owned");
    let handle = file.as_raw_handle();
    let mut ring: IoRing<Vec<u8>> = IoRing::with_inventory(8, 8).expect("create ring");

    let (read_id, flush_id) = {
        let mut batch = Batch::new(&mut ring);
        // SAFETY: `file` outlives both operations -- it is dropped at the end
        // of this test, after both completions have been popped.
        let read_id =
            unsafe { batch.read_raw_owned(handle, vec![0_u8; LEN], (), 0, PushOptions::new()) }
                .expect("queue an owned read");
        // SAFETY: as above. This is the tokenless shape: a borrowed handle, a
        // bare `user_data` back, and no entry in the ring.
        let flush_id =
            unsafe { batch.flush_raw(handle, FlushCoverage::Unordered, FlushMode::Default) }
                .expect("queue a raw flush");
        batch
            .submit_and_wait(2, 30_000)
            .expect("submit and wait for both");
        (read_id.user_data(), flush_id)
    };

    assert_eq!(
        ring.held(),
        1,
        "the ring holds the read's buffer and nothing for the raw flush"
    );
    assert_eq!(
        ring.outstanding(),
        2,
        "both operations are owed a completion, the raw flush included"
    );

    let mut saw_read = false;
    let mut saw_flush = false;
    for _ in 0..2 {
        let (completion, held) = ring
            .pop_within(std::time::Duration::from_secs(30))
            .expect("pop")
            .expect("both operations complete within the bound");
        if completion.user_data() == read_id {
            let (buffer, ()) = held.expect("the ring was holding the read's buffer");
            assert_eq!(
                buffer.expect("a read carries a buffer").len(),
                LEN,
                "the owned push hands its payload back"
            );
            saw_read = true;
        } else if completion.user_data() == flush_id {
            assert!(
                held.is_none(),
                "a raw push creates no entry, so the ring holds nothing for it"
            );
            saw_flush = true;
        } else {
            panic!("a completion arrived for an operation this test never pushed");
        }
        assert!(
            ring.held() <= ring.outstanding(),
            "everything held is outstanding: held {} > outstanding {}",
            ring.held(),
            ring.outstanding()
        );
    }
    assert!(saw_read && saw_flush, "both completions must be observed");
    assert_eq!(ring.held(), 0, "nothing is left held");
    assert_eq!(ring.outstanding(), 0, "and nothing is owed");

    drop(file);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_cancel_refuses_an_operation_id_minted_by_a_different_ring() {
    // Every ring hands out `UserData` from its own counter starting at the same
    // value, so one integer legitimately names a different operation on each
    // ring. While `cancel_owned` took the bare integer, an identity from one
    // ring could cancel an unrelated operation on another -- silently, and most
    // readily when both address the same file, which is why both rings here are
    // given the same one.
    //
    // BOTH directions. A `cancel_owned` that refused every identity would
    // satisfy the rejection on its own, so the acceptance below is what stops
    // this test passing for the wrong reason.
    let path = common::TempPath::new("inventory", "cross-ring-cancel");
    std::fs::write(&path, b"x").expect("create fixture");
    let file = SharedFile::new(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open fixture")
            .into(),
    );

    let mut home: IoRing<Vec<u8>, &'static str> =
        IoRing::with_inventory(8, 8).expect("create the home ring");
    let mut foreign: IoRing<Vec<u8>, &'static str> =
        IoRing::with_inventory(8, 8).expect("create the foreign ring");

    // An identity the *foreign* ring minted. It is submitted and drained rather
    // than left pending, because what this test needs from it is which
    // counter produced it, not that it is still live.
    let foreign_id = {
        let mut batch = Batch::new(&mut foreign);
        let id = batch
            .cancel_owned_raw(&file, 123_456, "foreign")
            .expect("queue a cancel on the foreign ring");
        batch.submit_and_wait(1, 5_000).expect("submit and wait");
        id
    };
    // `pop_within`, not a `try_pop` straight after the submit: RS-P-5 leaves
    // the kernel free to complete later than the submit returns, so unwrapping
    // `try_pop`'s `Option` here would assert a guarantee this crate does not
    // make. `response_space_census` refuses that shape at the source.
    foreign
        .pop_within(std::time::Duration::from_secs(30))
        .expect("pop_within")
        .expect("the foreign ring's cancel completes");

    // The rejection. Nothing is queued by a refused push, so there is nothing
    // to drain afterwards.
    let refused = {
        let mut batch = Batch::new(&mut home);
        batch
            .cancel_owned(&file, foreign_id, "home")
            .expect_err("an OperationId from another ring must be refused")
    };
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::InvalidInput,
        "a foreign identity is bad input, not a kernel failure"
    );

    // The acceptance.
    let home_id = {
        let mut batch = Batch::new(&mut home);
        let id = batch
            .cancel_owned_raw(&file, 654_321, "home")
            .expect("queue a cancel on the home ring");
        batch.submit_and_wait(1, 5_000).expect("submit and wait");
        id
    };
    home.pop_within(std::time::Duration::from_secs(30))
        .expect("pop_within")
        .expect("the home ring's first cancel completes");

    {
        let mut batch = Batch::new(&mut home);
        batch
            .cancel_owned(&file, home_id, "home")
            .expect("an OperationId this ring minted must be accepted");
        batch.submit_and_wait(1, 5_000).expect("submit and wait");
    }
    home.pop_within(std::time::Duration::from_secs(30))
        .expect("pop_within")
        .expect("the accepted cancel completes");
}
