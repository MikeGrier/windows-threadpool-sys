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
use windows_guard_alloc::witness::Witness;
use windows_ioring_sys::contract::RingContract;
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

/// A slot's role across the whole run, which is what makes the teardown check
/// stricter than the sum of the per-operation ones.
struct Slot {
    witness: Witness,
    /// Whether this slot was ever a read *destination*. A slot that only ever
    /// sourced writes must be byte-identical at teardown, with nothing
    /// permitted at all.
    ever_written_into: bool,
}

#[test]
fn a_mixed_workload_leaves_every_unaccounted_byte_poisoned() {
    // The whole-arena form of the per-operation checks above, and it asks a
    // question none of them can: after *many* operations across *many* slots,
    // is there a byte that changed which no completion ever accounted for?
    //
    // A per-operation check looks only at the buffer it was told about, over
    // the span it was told about. A write attributable to no particular
    // operation -- a kernel touching a buffer after its completion was
    // reported, an index computed from the wrong base, a slot recycled while
    // still in flight -- is invisible to every one of them individually and is
    // exactly what this catches.
    ALLOC.announce_seed();
    let seed = ALLOC.seed();

    const SLOTS: u32 = 4;

    let read_path = temp_file("mixed-read");
    let write_path = temp_file("mixed-write");
    let payload = vec![0x6D_u8; 4096];
    std::fs::write(&read_path, &payload).expect("create the read fixture");
    std::fs::write(&write_path, vec![0_u8; 8192]).expect("create the write fixture");
    let read_file = open_shared(&read_path, false);
    let write_file = open_shared(&write_path, true);

    let mut ring = IoRing::new(32, 64).expect("create a ring");
    let buffers = registered_arena(&mut ring, SLOTS, seed);

    let mut slots: Vec<Slot> = (0..SLOTS)
        .map(|slot| Slot {
            witness: Witness::new(ordinal_for(slot), SLOT_LEN),
            ever_written_into: false,
        })
        .collect();

    // A deliberately uneven workload: different slots, different offsets,
    // different lengths, and one slot used only as a write source. Uniform
    // operations would leave the interesting gaps unexercised.
    let reads: [(u32, u32, u32); 4] = [
        (0, 0, 512),     // slot 0, from the very start
        (1, 1024, 2048), // slot 1, offset into the middle
        (0, 4096, 1024), // slot 0 again, a second disjoint region
        (2, 7168, 1024), // slot 2, hard against the end of the slot
    ];
    // Slot 3 appears only here, as a source. Nothing about it may change.
    let writes: [(u32, u32, u32); 2] = [(3, 0, 4096), (3, 4096, 2048)];

    // Conservation, alongside the poison accounting (M16.2). The two answer
    // different questions and neither subsumes the other: the witness asks
    // "did anything write where it should not have", the contract asks "did
    // every operation complete exactly once and give back what it held".
    let mut contract = RingContract::new();

    let mut pending = Vec::new();
    for (slot, offset, len) in reads {
        let span = RegisteredSpan {
            buffer_index: slot,
            offset,
            len,
        };
        let mut batch = Batch::new(&mut ring);
        let token = batch
            .read_registered(&read_file, &buffers, span, 0, PushOptions::new())
            .expect("queue a registered read");
        batch.submit().expect("submit the read");
        contract.observe_push(token.id());
        pending.push((token, slot, offset, true));
    }
    for (slot, offset, len) in writes {
        let span = RegisteredSpan {
            buffer_index: slot,
            offset,
            len,
        };
        let mut batch = Batch::new(&mut ring);
        let token = batch
            .write_registered(
                &write_file,
                &buffers,
                span,
                u64::from(offset),
                PushOptions::new(),
                WriteCaching::Cached,
            )
            .expect("queue a registered write");
        batch.submit().expect("submit the write");
        contract.observe_push(token.id());
        pending.push((token, slot, offset, false));
    }

    // Drain every completion, accounting for each as it arrives. Completions
    // come back in whatever order the ring reports them, which is why the
    // witness merges permissions rather than requiring ascending offsets.
    while !pending.is_empty() {
        let completion = await_one(&mut ring);
        contract.observe_completion(completion.user_data());
        let transferred = completion.result().expect("the operation succeeded");
        let position = pending
            .iter()
            .position(|(token, ..)| token.id() == completion.user_data())
            .expect("a completion must belong to a pushed operation");
        let (token, slot, offset, is_read) = pending.swap_remove(position);
        let user_data = token.id();
        let released = token
            .claim_if(&completion)
            .expect("the token claims its own completion");
        // Claimed *after* the `RegisteredUse` is dropped, since that is what
        // returns the buffer's outstanding count to zero -- and the count is
        // what `observe_buffer` reports below.
        drop(released);
        contract.observe_claim(user_data);

        if is_read {
            let entry = &mut slots[slot as usize];
            entry.ever_written_into = true;
            // The *transferred* count, not the requested length. Permitting
            // the whole span would forgive a write past what actually arrived.
            entry.witness.permit(offset as usize, transferred);
        }
    }

    // Conservation at teardown: every operation completed exactly once, every
    // token was claimed, and every registered buffer is quiet.
    for slot in 0..SLOTS {
        contract.observe_buffer(
            slot,
            buffers
                .outstanding(slot)
                .expect("every slot index is in range"),
        );
    }
    contract.assert_quiescent();

    // Teardown: every byte nobody accounted for must still be poison.
    for (index, slot) in slots.iter().enumerate() {
        let bytes = buffers
            .get(index as u32)
            .expect("every slot is quiet once all completions are claimed");
        if let Err(breach) = slot.witness.verify(seed, bytes) {
            panic!(
                "slot {index} holds an unaccounted-for write: {breach}; seed {seed:#x}, \
                 {} byte(s) were legitimately written",
                slot.witness.permitted_bytes()
            );
        }
    }

    // The slot that only ever sourced writes had nothing permitted, so the
    // check above already covers it -- but stating it separately makes the
    // intent legible, and would catch a future edit that quietly permits
    // something there.
    assert_eq!(
        slots[3].witness.permitted_bytes(),
        0,
        "a write-source slot must never have anything permitted"
    );
    assert!(
        !slots[3].ever_written_into,
        "slot 3 was used only as a write source"
    );

    // And the reads must actually have landed, or every check above passes
    // vacuously against a ring that did nothing.
    let slot_zero = buffers.get(0).expect("slot 0 is quiet");
    assert_eq!(
        &slot_zero[..512],
        &payload[..512],
        "the first read should have landed its bytes"
    );
    assert!(
        slots[0].witness.permitted_bytes() > 0,
        "reads must have been accounted for, or the teardown check proves nothing"
    );

    let _ = std::fs::remove_file(&read_path);
    let _ = std::fs::remove_file(&write_path);
}
