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

use super::{
    ALLOCATION_GRANULARITY, GuardAlloc, PAGE, PoisonCheck, data_bytes, data_offset, poison,
};

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

#[test]
fn a_fresh_allocation_arrives_poisoned_rather_than_zeroed() {
    // Fresh pages from the OS are already zero, so without poison an untouched
    // buffer is indistinguishable from one that something wrote zeros into --
    // and a kernel writing outside the span it was handed would leave no
    // trace. This is the property M15.3's checks rest on.
    let alloc = GuardAlloc::new();
    let layout = Layout::from_size_align(64, 8).expect("valid layout");
    // SAFETY: non-zero-size layout; freed below.
    let ptr = unsafe { alloc.alloc(layout) };
    assert!(!ptr.is_null());

    // SAFETY: `alloc` promises 64 readable bytes here.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, 64) };
    assert!(
        bytes.iter().any(|b| *b != 0),
        "a fresh allocation must not be all zeros, or poison is not being written"
    );

    // SAFETY: `ptr` came from this allocator with this layout.
    unsafe { alloc.dealloc(ptr, layout) };
}

#[test]
fn an_allocation_identifies_itself_from_its_own_bytes() {
    // The payoff of a *tracked* pattern over a constant: a caller can ask "is
    // this still pristine?" without having snapshotted the buffer, because the
    // leading word names the allocation it belongs to.
    let alloc = GuardAlloc::new();
    let layout = Layout::from_size_align(128, 8).expect("valid layout");
    // SAFETY: non-zero-size layout; freed below.
    let ptr = unsafe { alloc.alloc(layout) };
    assert!(!ptr.is_null());

    // SAFETY: 128 readable bytes, as promised by `alloc`.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, 128) };
    match alloc.poison_check(bytes, 0, 128) {
        PoisonCheck::Pristine { .. } => {}
        other => panic!("a fresh allocation must verify as pristine poison, got {other:?}"),
    }

    // SAFETY: as above.
    unsafe { alloc.dealloc(ptr, layout) };
}

#[test]
fn poison_check_reports_the_first_byte_something_wrote() {
    // A stray write must be located, not merely detected: "the write began at
    // offset 70" is what makes an out-of-span kernel write attributable to a
    // particular operation.
    let alloc = GuardAlloc::new();
    let layout = Layout::from_size_align(128, 8).expect("valid layout");
    // SAFETY: non-zero-size layout; freed below.
    let ptr = unsafe { alloc.alloc(layout) };
    assert!(!ptr.is_null());

    // SAFETY: writing inside the allocation this test owns.
    unsafe { std::ptr::write(ptr.add(70), 0x00) };
    // SAFETY: 128 readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, 128) };

    match alloc.poison_check(bytes, 0, 128) {
        PoisonCheck::Overwritten { at, .. } => assert_eq!(
            at, 70,
            "the reported offset must be where the write actually landed"
        ),
        // A one-in-256 chance the poison byte there was already 0x00, in which
        // case nothing was overwritten and pristine is the truthful answer.
        PoisonCheck::Pristine { .. } => {
            let seed = alloc.seed();
            let mut leading = [0_u8; 8];
            leading.copy_from_slice(&bytes[..8]);
            let ordinal = poison::identify(seed, u64::from_le_bytes(leading), u64::MAX)
                .expect("the allocation identifies itself");
            assert_eq!(
                poison::byte_at(seed, ordinal, 70),
                0x00,
                "pristine is only correct if the poison byte there was already zero"
            );
        }
        other => panic!("unexpected {other:?}"),
    }

    // SAFETY: `ptr` came from this allocator with this layout.
    unsafe { alloc.dealloc(ptr, layout) };
}

#[test]
fn poison_check_can_examine_a_region_that_starts_partway_in() {
    // "Everything outside the span is untouched" means checking a slice that
    // begins partway into the allocation, so the phase has to shift with the
    // offset rather than restarting.
    let alloc = GuardAlloc::new();
    let layout = Layout::from_size_align(256, 8).expect("valid layout");
    // SAFETY: non-zero-size layout; freed below.
    let ptr = unsafe { alloc.alloc(layout) };
    assert!(!ptr.is_null());

    // Overwrite a "span" the kernel was permitted to fill, leaving the rest.
    // SAFETY: writing inside the allocation this test owns.
    unsafe { std::ptr::write_bytes(ptr, 0xCC, 100) };
    // SAFETY: 256 readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, 256) };

    // Identification needs pristine leading bytes, which the write above
    // destroyed -- so this is checked from a copy whose head is intact, which
    // is exactly how M15.3 will hold a snapshot of the buffer's identity.
    let mut intact = bytes.to_vec();
    // SAFETY: restoring the leading word from the same allocation's pattern.
    let seed = alloc.seed();
    let ordinal = alloc.total_allocations() as u64 - 1;
    unsafe { poison::fill(intact.as_mut_ptr(), 8, seed, ordinal) };

    match alloc.poison_check(&intact, 100, 156) {
        PoisonCheck::Pristine { .. } => {}
        other => panic!("the region beyond the written span must still be poison, got {other:?}"),
    }

    // SAFETY: `ptr` came from this allocator with this layout.
    unsafe { alloc.dealloc(ptr, layout) };
}

#[test]
fn the_seed_is_stable_within_a_run() {
    // Every allocation in the process must share one pattern space, or
    // `identify` would resolve ordinals against the wrong seed.
    let a = GuardAlloc::new();
    let b = GuardAlloc::new();
    assert_eq!(
        a.seed(),
        b.seed(),
        "the seed is per-process, not per-allocator"
    );
    assert_ne!(a.seed(), u64::MAX, "the unset sentinel must never escape");
}

#[test]
fn alloc_zeroed_still_returns_zeros_despite_the_poison() {
    // `GlobalAlloc::alloc_zeroed`'s default implementation calls `alloc` and
    // then zeroes, so poisoning underneath it is safe -- but only as long as
    // this crate does not override it. If someone ever does, this test is what
    // catches the resulting silent breakage of every `vec![0; n]`.
    let alloc = GuardAlloc::new();
    let layout = Layout::from_size_align(64, 8).expect("valid layout");
    // SAFETY: non-zero-size layout; freed below.
    let ptr = unsafe { alloc.alloc_zeroed(layout) };
    assert!(!ptr.is_null());
    // SAFETY: 64 readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(ptr, 64) };
    assert!(
        bytes.iter().all(|b| *b == 0),
        "alloc_zeroed must hand back zeros even though alloc poisons"
    );
    // SAFETY: `ptr` came from this allocator with this layout.
    unsafe { alloc.dealloc(ptr, layout) };
}

// ---------------------------------------------------------------------------
// Seed parsing.
//
// None of this was reachable before `parse_seed` was split out of
// `seed_from_environment`: the seed came from a process-global environment
// variable, and this workspace runs tests as threads in one process, so setting
// it from a test would be visible to every other test. A mutation run reported
// the prefix arm, the truncation guard, and both of its comparisons as
// surviving -- all of them correctly, because no test could reach them.
// ---------------------------------------------------------------------------

/// The wide form the environment gives us.
fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn a_decimal_seed_parses_in_base_ten() {
    assert_eq!(super::parse_seed(&units("1234")), Some(1234));
    // Leading zeros are digits, not a prefix: `0755` is seven hundred and
    // fifty-five here, not an octal escape.
    assert_eq!(super::parse_seed(&units("0755")), Some(755));
}

#[test]
fn a_prefixed_seed_parses_in_base_sixteen_in_either_case() {
    // The `0x` arm is what a mutation run deleted, and deleting it does not
    // fail loudly: `0x10` then parses as base ten, and `0` and `x` are both
    // rejected... except that `0x1` would silently become an error rather than
    // 1. The values here are chosen so the two radixes disagree, which is what
    // makes the arm's absence visible.
    assert_eq!(super::parse_seed(&units("0x10")), Some(16));
    assert_eq!(super::parse_seed(&units("0X10")), Some(16));
    assert_eq!(super::parse_seed(&units("0xff")), Some(255));
    assert_eq!(super::parse_seed(&units("0xFF")), Some(255));
}

#[test]
fn a_prefix_with_no_digits_after_it_is_not_a_seed() {
    // `0x` alone leaves an empty digit run. Accepting it would seed the whole
    // process with zero while the caller believed they had asked for something.
    assert_eq!(super::parse_seed(&units("0x")), None);
    assert_eq!(super::parse_seed(&units("0X")), None);
    assert_eq!(super::parse_seed(&units("")), None);
}

#[test]
fn a_seed_that_is_not_a_number_is_refused_rather_than_partially_read() {
    // Refusing beats guessing: a seed silently truncated at the first bad
    // character would produce a run whose poison pattern nobody could
    // reproduce from the value they set.
    assert_eq!(super::parse_seed(&units("12x4")), None);
    assert_eq!(super::parse_seed(&units("nonsense")), None);
    // A hex digit is not a decimal one, which is the same rule read the other
    // way round.
    assert_eq!(super::parse_seed(&units("ff")), None);
    // ...and `0x` makes it one.
    assert_eq!(super::parse_seed(&units("0xff")), Some(255));
}

#[test]
fn a_seed_too_large_for_a_u64_is_refused_rather_than_wrapped() {
    // `checked_mul`/`checked_add` are what make this a refusal. Wrapping would
    // accept a value and then use a *different* one, which is the worst of the
    // three outcomes: reproducible-looking and wrong.
    assert_eq!(
        super::parse_seed(&units("18446744073709551615")),
        Some(u64::MAX)
    );
    assert_eq!(super::parse_seed(&units("18446744073709551616")), None);
    assert_eq!(
        super::parse_seed(&units("0xFFFFFFFFFFFFFFFF")),
        Some(u64::MAX)
    );
    assert_eq!(super::parse_seed(&units("0x1FFFFFFFFFFFFFFFF")), None);
}
