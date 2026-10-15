// Copyright (c) 2026 Mike Grier
//! Unit tests for `AlignedBuf`.
//!
//! This test binary installs a counting allocator, so a test can see whether a buffer's allocation
//! was freed: a leak is invisible to every other assertion here. The count is per thread, so tests
//! running side by side do not disturb each other.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use super::{AlignedBuf, AlignedBufError};
use crate::{IoBuf, IoBufMut};

thread_local! {
    /// Bytes this thread has allocated and not yet freed. A `const` initialiser with no destructor,
    /// so reading it from inside the allocator can neither allocate nor fail.
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

fn adjust(bytes: isize) {
    LIVE.with(|live| live.set(live.get() + bytes));
}

fn live() -> isize {
    LIVE.with(Cell::get)
}

struct Counting;

// SAFETY: every method forwards to `System` with the caller's own arguments, so `System`'s contract
// is the caller's; the only addition is arithmetic on a thread-local that cannot allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded under the caller's obligations.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            adjust(layout.size() as isize);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded under the caller's obligations.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            adjust(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        adjust(-(layout.size() as isize));
        // SAFETY: forwarded under the caller's obligations.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: forwarded under the caller's obligations.
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            adjust(new_size as isize - layout.size() as isize);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Every power of two a buffer might be asked for, from byte alignment to a 64 KiB boundary.
const ALIGNMENTS: [usize; 17] = [
    1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536,
];

/// Lengths that are empty, tiny, awkward, and a multiple of a sector.
const LENGTHS: [usize; 9] = [0, 1, 7, 8, 511, 512, 4095, 4096, 4097];

fn buffer(len: usize, align: usize) -> AlignedBuf {
    AlignedBuf::zeroed(len, align).expect("a valid buffer")
}

#[test]
fn the_address_is_aligned_for_every_alignment_and_length() {
    for align in ALIGNMENTS {
        for len in LENGTHS {
            let buffer = buffer(len, align);
            assert_eq!(
                buffer.as_ptr() as usize % align,
                0,
                "len {len}, align {align}"
            );
            assert_eq!(buffer.align(), align);
            assert_eq!(buffer.len(), len);
        }
    }
}

#[test]
fn the_traits_report_an_aligned_address_too() {
    for align in ALIGNMENTS {
        let mut buffer = buffer(100, align);
        assert_eq!(buffer.stable_ptr() as usize % align, 0, "align {align}");
        assert_eq!(buffer.stable_mut_ptr() as usize % align, 0, "align {align}");
    }
}

#[test]
fn a_new_buffer_is_zeroed_whatever_the_allocator_held_before() {
    // Sizes the allocator reuses, so a block just freed with 0xAB in it is the likeliest thing the
    // next allocation is handed.
    for (len, align) in [(16, 16), (100, 8), (4096, 4096), (5000, 512)] {
        for _ in 0..32 {
            let mut dirty = buffer(len, align);
            dirty.fill(0xAB);
            drop(dirty);
            let fresh = buffer(len, align);
            assert!(
                fresh.iter().all(|&byte| byte == 0),
                "len {len}, align {align}"
            );
        }
    }
}

#[test]
fn from_bytes_copies_the_bytes_into_an_aligned_buffer() {
    let source: Vec<u8> = (0..=255).collect();
    for align in ALIGNMENTS {
        let copy = AlignedBuf::from_bytes(&source, align).expect("a valid buffer");
        assert_eq!(copy.as_slice(), source.as_slice(), "align {align}");
        assert_eq!(copy.as_ptr() as usize % align, 0);
        assert_ne!(copy.as_ptr(), source.as_ptr(), "a copy, not a view");
    }
}

#[test]
fn from_bytes_of_nothing_is_an_empty_buffer() {
    let empty = AlignedBuf::from_bytes(&[], 64).expect("a valid buffer");
    assert!(empty.is_empty());
    assert_eq!(empty.align(), 64);
}

#[test]
fn the_contents_can_be_written_and_read_back() {
    let mut buffer = buffer(32, 16);
    buffer[..5].copy_from_slice(b"hello");
    buffer.as_mut_slice()[31] = 0x7F;
    assert_eq!(&buffer[..5], b"hello");
    assert_eq!(buffer.as_slice()[31], 0x7F);
    assert_eq!(buffer[5], 0, "bytes not written are still zero");
}

#[test]
fn the_slice_views_agree_with_the_pointers() {
    let mut buffer = buffer(64, 32);
    assert_eq!(buffer.as_slice().as_ptr(), buffer.as_ptr());
    assert_eq!(buffer.as_mut_slice().as_mut_ptr(), buffer.as_mut_ptr());
    assert_eq!(buffer.as_ref().as_ptr(), buffer.as_ptr());
    let as_mut: &mut [u8] = buffer.as_mut();
    assert_eq!(as_mut.len(), 64);
}

#[test]
fn an_empty_buffer_has_an_aligned_non_null_address_and_no_bytes() {
    for align in ALIGNMENTS {
        let buffer = buffer(0, align);
        assert!(buffer.is_empty());
        assert!(!buffer.as_ptr().is_null());
        assert_eq!(buffer.as_ptr() as usize % align, 0, "align {align}");
        assert!(buffer.as_slice().is_empty());
    }
}

#[test]
fn an_empty_buffer_allocates_and_frees_nothing() {
    let before = live();
    {
        let _empty = buffer(0, 4096);
        assert_eq!(live(), before, "no allocation was made");
    }
    assert_eq!(live(), before);
}

#[test]
fn a_bad_alignment_is_refused_with_the_alignment_it_was_given() {
    for align in [0_usize, 3, 5, 6, 7, 9, 12, 100, 4095, 4097, usize::MAX] {
        assert_eq!(
            AlignedBuf::zeroed(16, align).expect_err("not a power of two"),
            AlignedBufError::InvalidAlignment { align },
            "for {align}"
        );
        assert_eq!(
            AlignedBuf::from_bytes(&[1, 2, 3], align).expect_err("not a power of two"),
            AlignedBufError::InvalidAlignment { align },
        );
    }
}

#[test]
fn a_length_no_allocation_can_have_is_refused_as_too_large() {
    // Rounded up to the alignment these exceed `isize::MAX`, the most a layout can describe.
    for (len, align) in [
        (usize::MAX, 1),
        (usize::MAX, 4096),
        (isize::MAX as usize, 2),
        (isize::MAX as usize - 4094, 4096),
    ] {
        assert_eq!(
            AlignedBuf::zeroed(len, align).expect_err("too large"),
            AlignedBufError::TooLarge { len, align },
            "for {len} at {align}"
        );
    }
}

#[test]
fn an_allocation_the_allocator_cannot_make_is_reported_and_leaks_nothing() {
    // A valid layout for a length no machine has: the largest that fits an `isize` at this
    // alignment. The request reaches the allocator, which refuses.
    let align = 4096;
    let len = isize::MAX as usize - (align - 1);
    let before = live();
    assert_eq!(
        AlignedBuf::zeroed(len, align).expect_err("no machine has this"),
        AlignedBufError::AllocationFailed { len, align }
    );
    assert_eq!(live(), before);
}

#[test]
fn every_reason_is_described_with_its_numbers() {
    let invalid = AlignedBufError::InvalidAlignment { align: 6 }.to_string();
    assert!(
        invalid.contains('6') && invalid.contains("power of two"),
        "{invalid}"
    );
    let large = AlignedBufError::TooLarge { len: 9, align: 4 }.to_string();
    assert!(large.contains('9') && large.contains('4'), "{large}");
    let failed = AlignedBufError::AllocationFailed { len: 7, align: 2 }.to_string();
    assert!(failed.contains('7') && failed.contains('2'), "{failed}");
    let boxed: Box<dyn std::error::Error> =
        Box::new(AlignedBufError::InvalidAlignment { align: 0 });
    assert!(!boxed.to_string().is_empty());
}

#[test]
fn a_length_that_is_not_a_multiple_of_the_alignment_is_allowed() {
    let buffer = buffer(5, 4096);
    assert_eq!(buffer.len(), 5);
    assert_eq!(buffer.as_ptr() as usize % 4096, 0);
}

#[test]
fn dropping_a_buffer_frees_its_allocation() {
    let before = live();
    {
        let buffer = buffer(1_000_003, 4096);
        assert!(live() >= before + 1_000_003, "the allocation is counted");
        assert_eq!(buffer.len(), 1_000_003);
    }
    assert_eq!(live(), before, "and dropping it gave it back");
}

#[test]
fn a_clone_and_its_original_each_free_what_they_allocated() {
    let before = live();
    {
        let original = buffer(8192, 512);
        let copy = original.try_clone().expect("a copy");
        assert_eq!(copy, original);
    }
    assert_eq!(live(), before);
}

#[test]
fn a_clone_has_the_same_contents_and_alignment_at_another_address() {
    let mut original = AlignedBuf::from_bytes(&[1, 2, 3, 4, 5], 64).expect("a buffer");
    let copy = original.try_clone().expect("a copy");
    assert_eq!(copy.as_slice(), original.as_slice());
    assert_eq!(copy.align(), 64);
    assert_eq!(copy.as_ptr() as usize % 64, 0);
    assert_ne!(copy.as_ptr(), original.as_ptr());

    original[0] = 99;
    assert_eq!(copy[0], 1, "the copy is independent of the original");
}

#[test]
fn equality_is_by_contents_and_alignment_not_by_address() {
    let a = AlignedBuf::from_bytes(&[1, 2, 3], 8).expect("a buffer");
    let same = AlignedBuf::from_bytes(&[1, 2, 3], 8).expect("a buffer");
    let other_alignment = AlignedBuf::from_bytes(&[1, 2, 3], 16).expect("a buffer");
    let other_contents = AlignedBuf::from_bytes(&[1, 2, 4], 8).expect("a buffer");
    let other_length = AlignedBuf::from_bytes(&[1, 2], 8).expect("a buffer");
    assert_ne!(a.as_ptr(), same.as_ptr());
    assert_eq!(a, same);
    assert_ne!(a, other_alignment);
    assert_ne!(a, other_contents);
    assert_ne!(a, other_length);
}

#[test]
fn debug_shows_the_shape_and_never_the_contents() {
    let secret = AlignedBuf::from_bytes(b"correct horse battery", 16).expect("a buffer");
    let text = format!("{secret:?}");
    assert!(text.contains("AlignedBuf"), "{text}");
    assert!(
        text.contains("len: 21") && text.contains("align: 16"),
        "{text}"
    );
    assert!(
        !text.contains("horse") && !text.contains("correct"),
        "{text}"
    );
}

#[test]
fn the_io_traits_report_the_allocation_and_its_length() {
    let mut buffer = buffer(1000, 512);
    assert_eq!(buffer.stable_ptr(), buffer.as_ptr());
    assert_eq!(buffer.bytes_len(), 1000);
    assert_eq!(buffer.stable_mut_ptr().cast_const(), buffer.as_ptr());
}

#[test]
fn the_address_survives_moving_the_buffer_and_putting_it_in_a_collection() {
    let buffer = buffer(4096, 4096);
    let before = buffer.stable_ptr();
    let moved = buffer;
    assert_eq!(moved.stable_ptr(), before);
    let mut collection = vec![moved];
    for _ in 0..1000 {
        collection.push(AlignedBuf::zeroed(16, 16).expect("a buffer"));
    }
    assert_eq!(
        collection[0].stable_ptr(),
        before,
        "the collection grew and moved its elements"
    );
}

#[test]
fn bytes_written_through_the_mutable_pointer_are_read_through_the_shared_one() {
    let mut buffer = buffer(256, 64);
    let len = buffer.bytes_len();
    let pointer = buffer.stable_mut_ptr();
    // SAFETY: the pointer is this buffer's own allocation of `len` initialised bytes, and no other
    // reference to it is live while the slice is.
    unsafe { std::slice::from_raw_parts_mut(pointer, len) }.fill(0xA5);
    // SAFETY: the same region, now read through the shared address, still owned here.
    let read = unsafe { std::slice::from_raw_parts(buffer.stable_ptr(), len) };
    assert!(read.iter().all(|&byte| byte == 0xA5));
}

#[test]
fn a_buffer_satisfies_the_traits_as_a_bound() {
    fn through_the_bound<B: IoBufMut>(mut buffer: B) -> (usize, bool) {
        let shared = buffer.stable_ptr();
        (
            buffer.bytes_len(),
            buffer.stable_mut_ptr().cast_const() == shared,
        )
    }
    assert_eq!(through_the_bound(buffer(777, 128)), (777, true));
}

#[test]
fn a_buffer_is_send_and_sync() {
    fn assert_send_and_sync<T: Send + Sync>() {}
    assert_send_and_sync::<AlignedBuf>();
    assert_send_and_sync::<AlignedBufError>();
}

#[test]
fn a_buffer_can_be_filled_on_another_thread_and_read_here() {
    let buffer = buffer(4096, 4096);
    let address = buffer.as_ptr() as usize;
    let filled = std::thread::spawn(move || {
        let mut buffer = buffer;
        buffer.fill(0x5A);
        buffer
    })
    .join()
    .expect("the worker");
    assert_eq!(
        filled.as_ptr() as usize,
        address,
        "ownership moved, the bytes did not"
    );
    assert!(filled.iter().all(|&byte| byte == 0x5A));
}

#[test]
fn a_shared_reference_can_be_read_from_several_threads() {
    let buffer = AlignedBuf::from_bytes(&[7; 512], 512).expect("a buffer");
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| assert!(buffer.iter().all(|&byte| byte == 7)));
        }
    });
}
