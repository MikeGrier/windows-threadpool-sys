// Copyright (c) 2026 Mike Grier
//! Holding the **kernel** to the span it was given (M15.3).
//!
//! Every other test in this crate checks what this crate does. These check
//! what the kernel does with what this crate hands it, and they exist because
//! two of this crate's load-bearing claims were asserted in prose and verified
//! nowhere:
//!
//! - [`Batch::write_registered`] says the kernel only **reads** the slot.
//! - [`Batch::read_registered`] says the kernel writes only within the
//!   [`RegisteredSpan`] it was given.
//!
//! Neither is observable with a guard page: a guard page catches access to
//! memory that should not be touched *at all*, and both claims are about a
//! live, committed, entirely legitimate buffer. The only way to see a stray
//! write into valid memory is to know what was there before, which is what the
//! poison pattern is for
//! ([D-38](../DESIGN-NOTES.md#d-38), [D-39](../DESIGN-NOTES.md#d-39)).
//!
//! # These checks cut both ways
//!
//! They are described as holding the kernel to its contract, and they do -- but
//! the span the kernel receives is computed by *this crate*, in
//! `checked_span`, from a `RegisteredSpan` and a registered base index. A
//! failure here is at least as likely to be our arithmetic as the kernel's
//! behaviour, and that is a feature: an off-by-one in the offset we pass is
//! exactly as invisible to every other test as a kernel bug would be.
//!
//! # On reproducibility
//!
//! The poison varies per run and the seed is announced, per
//! [D-39](../DESIGN-NOTES.md#d-39). Re-run a failure with
//! `WINDOWS_GUARD_ALLOC_SEED` set to the printed value to reproduce it exactly.

#![cfg(windows)]

use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;

use windows_guard_alloc::poison;
use windows_ioring_sys::{
    Batch, IoRing, PushOptions, RegisteredBuffers, RegisteredSpan, SharedFile, WriteCaching,
};

#[global_allocator]
static ALLOC: windows_guard_alloc::GuardAlloc = windows_guard_alloc::GuardAlloc::new();

/// Bytes per registered slot. Several pages, so an out-of-span write has room
/// to land somewhere a smaller buffer would not have covered.
const SLOT_LEN: usize = 8192;

/// How long to wait for a completion before declaring the ring stuck.
const WAIT_MS: u32 = 30_000;

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-kernel-span-{tag}-{}.tmp",
        std::process::id()
    ))
}

/// The poison ordinal used for slot `i`.
///
/// Deliberately *not* the allocator's own ordinal for that buffer: `vec![]`
/// overwrites whatever the allocator wrote, so the pattern has to be applied
/// again after the buffer exists. Deriving it from the slot index keeps each
/// slot's pattern distinct, which is what makes a write that lands in the
/// wrong slot visible rather than merely a write that lands at the wrong
/// offset.
fn ordinal_for(slot: u32) -> u64 {
    0x5000_0000 + u64::from(slot)
}

/// Overwrite `buffers`' slot `slot` with its poison pattern.
fn poison_slot(buffers: &mut RegisteredBuffers<Vec<u8>>, slot: u32, seed: u64) {
    let bytes = buffers.get_mut(slot).expect("slot is quiet");
    let len = bytes.len();
    // SAFETY: `bytes` is a live, exclusively-borrowed slice of exactly `len`
    // bytes.
    unsafe { poison::fill(bytes.as_mut_ptr(), len, seed, ordinal_for(slot)) };
}

/// Register `count` buffers of [`SLOT_LEN`] bytes on `ring`, each filled with
/// its poison pattern.
fn registered_arena(ring: &mut IoRing, count: u32, seed: u64) -> RegisteredBuffers<Vec<u8>> {
    let buffers: Vec<Vec<u8>> = (0..count).map(|_| vec![0_u8; SLOT_LEN]).collect();
    let mut batch = Batch::new(ring);
    let pending = batch
        .register_buffers(buffers)
        .expect("queue buffer registration");
    batch
        .submit_and_wait(1, WAIT_MS)
        .expect("submit the registration");
    let completion = ring
        .try_pop()
        .expect("pop the registration completion")
        .expect("a completion is ready");
    let mut registered = pending
        .claim_if(&completion)
        .expect("the registration token claims its own completion")
        .expect("buffer registration succeeded");

    for slot in 0..count {
        poison_slot(&mut registered, slot, seed);
    }
    registered
}

/// Drain until a completion arrives, then hand it back.
fn await_one(ring: &mut IoRing) -> windows_ioring_sys::Completion {
    loop {
        if let Some(completion) = ring.try_pop().expect("pop a completion") {
            return completion;
        }
        Batch::new(ring)
            .submit_and_wait(1, WAIT_MS)
            .expect("wait for a completion");
    }
}

fn open_shared(path: &std::path::Path, write: bool) -> SharedFile {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(write)
        .open(path)
        .expect("open the fixture file");
    SharedFile::new(OwnedHandle::from(file))
}

#[test]
fn the_seed_is_announced_so_a_failure_here_can_be_reproduced() {
    ALLOC.announce_seed();
    assert!(
        ALLOC.total_allocations() > 0,
        "the guard allocator is not installed for this binary"
    );
}

#[test]
fn a_registered_write_leaves_its_source_slot_byte_identical() {
    // The kernel is permitted to *read* a write's source buffer and nothing
    // else. Nothing in this crate has ever checked that, because the buffer is
    // valid memory throughout -- a guard page sees nothing here.
    ALLOC.announce_seed();
    let seed = ALLOC.seed();

    let path = temp_file("write-source");
    std::fs::write(&path, vec![0_u8; SLOT_LEN]).expect("create the fixture file");
    let file = open_shared(&path, true);

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let buffers = registered_arena(&mut ring, 1, seed);

    // What the slot holds before the kernel sees it.
    let before = buffers.get(0).expect("slot 0 is quiet").to_vec();

    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: u32::try_from(SLOT_LEN).expect("slot length fits u32"),
    };
    let mut batch = Batch::new(&mut ring);
    let token = batch
        .write_registered(
            &file,
            &buffers,
            span,
            0,
            PushOptions::new(),
            WriteCaching::Cached,
        )
        .expect("queue a registered write");
    batch.submit().expect("submit the write");

    let completion = await_one(&mut ring);
    let written = completion.result().expect("the write succeeded");
    let released = token
        .claim_if(&completion)
        .expect("the token claims its own completion");
    drop(released);

    assert_eq!(written, SLOT_LEN, "the whole slot should have been written");

    let after = buffers.get(0).expect("slot 0 is quiet again");
    assert_eq!(
        after,
        before.as_slice(),
        "a registered write must not modify its source slot; seed {seed:#x}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_registered_read_writes_only_inside_the_span_it_was_given() {
    // The span carries an *offset*, so the kernel is being asked to write into
    // the middle of a buffer and leave both ends alone. This is the check that
    // binds it to that -- and equally binds this crate's own `checked_span`
    // arithmetic, since the offset the kernel receives is computed here.
    ALLOC.announce_seed();
    let seed = ALLOC.seed();

    const OFFSET: u32 = 1024;
    const LEN: u32 = 2048;

    let path = temp_file("read-span");
    // A recognisable payload that is not the poison, so "the span was filled"
    // and "the rest was left alone" are separately checkable.
    let payload = vec![0xC7_u8; LEN as usize];
    std::fs::write(&path, &payload).expect("create the fixture file");
    let file = open_shared(&path, false);

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let buffers = registered_arena(&mut ring, 1, seed);

    let span = RegisteredSpan {
        buffer_index: 0,
        offset: OFFSET,
        len: LEN,
    };
    let mut batch = Batch::new(&mut ring);
    let token = batch
        .read_registered(&file, &buffers, span, 0, PushOptions::new())
        .expect("queue a registered read");
    batch.submit().expect("submit the read");

    let completion = await_one(&mut ring);
    let read = completion.result().expect("the read succeeded");
    let released = token
        .claim_if(&completion)
        .expect("the token claims its own completion");
    drop(released);

    assert_eq!(read, LEN as usize, "the whole span should have been read");

    let slot = buffers.get(0).expect("slot 0 is quiet again");
    let ordinal = ordinal_for(0);

    // Before the span: untouched poison.
    assert_eq!(
        poison::first_mismatch(seed, ordinal, 0, &slot[..OFFSET as usize]),
        None,
        "the kernel wrote before the span's offset; seed {seed:#x}"
    );

    // Inside the span: the file's bytes, which is what proves the read
    // actually happened rather than the check passing vacuously.
    assert_eq!(
        &slot[OFFSET as usize..(OFFSET + LEN) as usize],
        payload.as_slice(),
        "the span should hold the file's contents"
    );

    // After the span: untouched poison, with the phase carried across.
    let tail_start = (OFFSET + LEN) as usize;
    assert_eq!(
        poison::first_mismatch(seed, ordinal, tail_start, &slot[tail_start..]),
        None,
        "the kernel wrote past the span's end; seed {seed:#x}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_short_read_leaves_the_unfilled_remainder_of_the_span_untouched() {
    // The span is a *permission*, not a promise: a read of 2048 bytes from a
    // 512-byte file transfers 512. The bytes between what arrived and the end
    // of the span must still be poison, or the kernel has written past what it
    // actually had -- and `information` is the only thing that says where the
    // real data stops.
    ALLOC.announce_seed();
    let seed = ALLOC.seed();

    const OFFSET: u32 = 512;
    const SPAN_LEN: u32 = 2048;
    const FILE_LEN: usize = 512;

    let path = temp_file("short-read");
    let payload = vec![0x9E_u8; FILE_LEN];
    std::fs::write(&path, &payload).expect("create the fixture file");
    let file = open_shared(&path, false);

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let buffers = registered_arena(&mut ring, 1, seed);

    let span = RegisteredSpan {
        buffer_index: 0,
        offset: OFFSET,
        len: SPAN_LEN,
    };
    let mut batch = Batch::new(&mut ring);
    let token = batch
        .read_registered(&file, &buffers, span, 0, PushOptions::new())
        .expect("queue a registered read");
    batch.submit().expect("submit the read");

    let completion = await_one(&mut ring);
    let read = completion.result().expect("the read succeeded");
    let released = token
        .claim_if(&completion)
        .expect("the token claims its own completion");
    drop(released);

    assert_eq!(read, FILE_LEN, "only the file's bytes should have arrived");

    let slot = buffers.get(0).expect("slot 0 is quiet again");
    let ordinal = ordinal_for(0);

    let filled_end = OFFSET as usize + read;
    assert_eq!(
        &slot[OFFSET as usize..filled_end],
        payload.as_slice(),
        "the transferred bytes should be the file's"
    );
    assert_eq!(
        poison::first_mismatch(seed, ordinal, filled_end, &slot[filled_end..]),
        None,
        "the kernel wrote beyond what it transferred; seed {seed:#x}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_read_into_one_slot_leaves_its_neighbours_untouched() {
    // Each slot carries a *different* pattern, so a write landing in the wrong
    // slot is visible as such rather than merely as "something changed". That
    // is what a per-allocation ordinal buys over a single constant.
    ALLOC.announce_seed();
    let seed = ALLOC.seed();

    const TARGET: u32 = 1;
    const LEN: u32 = 1024;

    let path = temp_file("neighbours");
    std::fs::write(&path, vec![0x3B_u8; LEN as usize]).expect("create the fixture file");
    let file = open_shared(&path, false);

    let mut ring = IoRing::new(16, 16).expect("create a ring");
    let buffers = registered_arena(&mut ring, 3, seed);

    let span = RegisteredSpan {
        buffer_index: TARGET,
        offset: 0,
        len: LEN,
    };
    let mut batch = Batch::new(&mut ring);
    let token = batch
        .read_registered(&file, &buffers, span, 0, PushOptions::new())
        .expect("queue a registered read");
    batch.submit().expect("submit the read");

    let completion = await_one(&mut ring);
    completion.result().expect("the read succeeded");
    let released = token
        .claim_if(&completion)
        .expect("the token claims its own completion");
    drop(released);

    for slot in [0_u32, 2] {
        let bytes = buffers.get(slot).expect("neighbour slot is quiet");
        assert_eq!(
            poison::first_mismatch(seed, ordinal_for(slot), 0, bytes),
            None,
            "a read into slot {TARGET} disturbed slot {slot}; seed {seed:#x}"
        );
    }

    let _ = std::fs::remove_file(&path);
}
