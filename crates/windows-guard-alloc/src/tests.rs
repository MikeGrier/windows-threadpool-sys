// Copyright (c) 2026 Mike Grier
//! Unit tests for [`GuardAlloc`].
//!
//! These exercise the allocator **as a value**, never as the installed global
//! allocator: `alloc` and `dealloc` are called directly. That is deliberate.
//! A `#[global_allocator]` is process-wide and cannot be swapped per test, and
//! the interesting properties -- where the guard page sits, what protection a
//! freed range carries -- are observable with `VirtualQuery` without
//! dereferencing anything. Faulting behaviour cannot be asserted in-process at
//! all, since an access violation is not catchable; that is covered by the
//! `faults` integration test, which runs violations in a subprocess and checks
//! the exit code.

use std::alloc::{GlobalAlloc, Layout};

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_NOACCESS, PAGE_READWRITE, VirtualQuery,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

use super::{ALLOCATION_GRANULARITY, GuardAlloc, PAGE, data_bytes, data_offset};

/// What the OS says about a page: its state, its protection, and how far the
/// run of identically-attributed pages extends.
fn query(ptr: *const u8) -> MEMORY_BASIC_INFORMATION {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: `info` is a live local of exactly the size passed, and
    // `VirtualQuery` accepts any address, mapped or not.
    let written = unsafe {
        VirtualQuery(
            ptr.cast(),
            &raw mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    assert_ne!(written, 0, "VirtualQuery failed");
    info
}

#[test]
fn the_page_size_constant_matches_the_operating_system() {
    // The whole design is arithmetic over `PAGE`. If it ever disagreed with
    // the OS the guard page would land in the wrong place and instrument
    // nothing, so this is checked rather than assumed.
    let mut info = SYSTEM_INFO::default();
    // SAFETY: `info` is a live local of the right type.
    unsafe { GetSystemInfo(&raw mut info) };
    assert_eq!(info.dwPageSize as usize, PAGE);
    assert_eq!(
        info.dwAllocationGranularity as usize,
        ALLOCATION_GRANULARITY
    );
}

#[test]
fn an_allocation_is_writable_across_its_whole_length() {
    let alloc = GuardAlloc::new();
    for size in [1_usize, 8, 4095, 4096, 4097, 12_000] {
        let layout = Layout::from_size_align(size, 1).expect("valid layout");
        // SAFETY: non-zero-size layout; the pointer is freed below.
        let ptr = unsafe { alloc.alloc(layout) };
        assert!(!ptr.is_null(), "allocation of {size} bytes failed");

        // SAFETY: `alloc` promises `size` writable bytes here.
        unsafe {
            std::ptr::write_bytes(ptr, 0x5A, size);
            assert_eq!(std::ptr::read(ptr), 0x5A);
            assert_eq!(std::ptr::read(ptr.add(size - 1)), 0x5A);
            alloc.dealloc(ptr, layout);
        }
    }
}

#[test]
fn the_byte_after_an_allocation_lies_in_a_no_access_guard_page() {
    // The property that makes an overrun deterministic. Asserted through
    // `VirtualQuery` rather than by touching the byte, because touching it
    // would kill the test process -- which is the point of the guard.
    let alloc = GuardAlloc::new();
    for size in [1_usize, 64, 4096, 8192] {
        let layout = Layout::from_size_align(size, 1).expect("valid layout");
        // SAFETY: non-zero-size layout; freed below.
        let ptr = unsafe { alloc.alloc(layout) };
        assert!(!ptr.is_null());

        // SAFETY: address arithmetic only; the byte is never dereferenced.
        let past_end = unsafe { ptr.add(size) };
        let info = query(past_end);
        assert_eq!(
            info.Protect, PAGE_NOACCESS,
            "the byte past a {size}-byte allocation must be in the guard page"
        );

        // SAFETY: as above.
        unsafe { alloc.dealloc(ptr, layout) };
    }
}

#[test]
fn an_allocation_ends_flush_against_its_guard_page() {
    // Right-alignment is what makes a *one-byte* overrun fault rather than
    // landing in slack. Without it the guard page still exists but a small
    // overrun sails past it, so this asserts the offset arithmetic rather than
    // merely the guard's presence.
    for size in [1_usize, 7, 100, 4095] {
        let data = data_bytes(size);
        assert_eq!(
            data_offset(size, 1, data) + size,
            data,
            "a {size}-byte allocation with align 1 must end exactly at the guard page"
        );
    }
}

#[test]
fn allocations_are_correctly_aligned() {
    let alloc = GuardAlloc::new();
    for align in [1_usize, 2, 8, 64, 512, 4096] {
        for size in [1_usize, 8, 100, 5000] {
            let layout = Layout::from_size_align(size, align).expect("valid layout");
            // SAFETY: non-zero-size layout; freed below.
            let ptr = unsafe { alloc.alloc(layout) };
            assert!(!ptr.is_null());
            assert_eq!(
                ptr as usize % align,
                0,
                "size {size} align {align} produced a misaligned pointer"
            );
            // SAFETY: as above.
            unsafe { alloc.dealloc(ptr, layout) };
        }
    }
}

#[test]
fn a_freed_allocation_is_decommitted_and_its_address_is_never_reused() {
    // Two properties, and both are needed. Decommitting is what makes a
    // use-after-free fault; not reusing the address is what stops a later
    // allocation from making those bytes valid again, which would silently
    // restore the stale-read behaviour this allocator exists to remove.
    let alloc = GuardAlloc::new();
    let layout = Layout::from_size_align(128, 8).expect("valid layout");

    // SAFETY: non-zero-size layout.
    let first = unsafe { alloc.alloc(layout) };
    assert!(!first.is_null());
    assert_eq!(query(first).State, MEM_COMMIT, "live memory is committed");

    // SAFETY: `first` came from this allocator with this layout.
    unsafe { alloc.dealloc(first, layout) };

    let after = query(first);
    assert_eq!(
        after.State, MEM_RESERVE,
        "a freed allocation must be decommitted, not left committed"
    );
    assert_ne!(
        after.State, MEM_COMMIT,
        "reading a freed allocation must not find committed memory"
    );

    // Every later allocation must avoid that address, or the guarantee is
    // worthless. A hundred is enough to catch a reuse policy; the reservation
    // is never released, so the OS cannot hand it back at all.
    let mut seen = Vec::new();
    for _ in 0..100 {
        // SAFETY: non-zero-size layout; all freed below.
        let ptr = unsafe { alloc.alloc(layout) };
        assert!(!ptr.is_null());
        assert_ne!(ptr, first, "a freed address was handed out again");
        seen.push(ptr);
    }
    for ptr in seen {
        // SAFETY: each came from this allocator with this layout.
        unsafe { alloc.dealloc(ptr, layout) };
    }
}

#[test]
fn the_counters_track_live_and_total_allocations() {
    // `total_allocations` is how a test proves the allocator is actually
    // installed, so it has to be right.
    let alloc = GuardAlloc::new();
    assert_eq!(alloc.live_allocations(), 0);
    assert_eq!(alloc.total_allocations(), 0);

    let layout = Layout::from_size_align(64, 8).expect("valid layout");
    // SAFETY: non-zero-size layout.
    let a = unsafe { alloc.alloc(layout) };
    // SAFETY: as above.
    let b = unsafe { alloc.alloc(layout) };
    assert_eq!(alloc.live_allocations(), 2);
    assert_eq!(alloc.total_allocations(), 2);

    // SAFETY: both came from this allocator with this layout.
    unsafe { alloc.dealloc(a, layout) };
    assert_eq!(alloc.live_allocations(), 1);
    assert_eq!(
        alloc.total_allocations(),
        2,
        "the total counts allocations made, not allocations outstanding"
    );

    // SAFETY: as above.
    unsafe { alloc.dealloc(b, layout) };
    assert_eq!(alloc.live_allocations(), 0);
}

#[test]
fn a_zero_sized_allocation_still_gets_a_guard_page() {
    // Rust permits a zero-sized layout, and `Layout::from_size_align(0, 1)` is
    // valid. Treating it as one byte keeps the arithmetic uniform and means
    // even this case is instrumented rather than special-cased into a null.
    let alloc = GuardAlloc::new();
    let layout = Layout::from_size_align(0, 1).expect("valid layout");
    // SAFETY: the allocator maps a zero size onto a one-byte allocation.
    let ptr = unsafe { alloc.alloc(layout) };
    assert!(!ptr.is_null());
    assert_eq!(query(ptr).Protect, PAGE_READWRITE);
    // SAFETY: `ptr` came from this allocator with this layout.
    unsafe { alloc.dealloc(ptr, layout) };
}
