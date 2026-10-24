// Copyright (c) 2026 Mike Grier

use super::{AllocError, Allocator, Global, Refusing};
use alloc::format;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::error::Error;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};

const fn assert_traits<T: Copy + Default + Send + Sync + core::fmt::Debug + 'static>() {}
const _: () = assert_traits::<Global>();

fn layout(size: usize, align: usize) -> Layout {
    Layout::from_size_align(size, align).expect("a valid layout")
}

/// Allocates through `allocator`, checks the block fits `layout` and can hold a pattern, and
/// frees it.
fn round_trip(allocator: &dyn Allocator, layout: Layout) {
    let block = allocator.allocate(layout).expect("the block");
    assert!(
        block.len() >= layout.size(),
        "{layout:?}: {} bytes",
        block.len()
    );
    let start = block.cast::<u8>();
    assert_eq!(start.as_ptr().addr() % layout.align(), 0, "{layout:?}");
    for i in 0..layout.size() {
        // SAFETY: `i` is inside the block, which is valid for writes of its length.
        unsafe { start.add(i).write(i as u8) };
    }
    for i in 0..layout.size() {
        // SAFETY: as above, and the byte was written.
        assert_eq!(unsafe { start.add(i).read() }, i as u8);
    }
    // SAFETY: the block came from `allocator` with `layout` and is freed once.
    unsafe { allocator.deallocate(start, layout) };
}

#[test]
fn an_alloc_error_says_what_failed() {
    assert_eq!(format!("{AllocError}"), "memory allocation failed");
    assert_eq!(format!("{AllocError:?}"), "AllocError");
    let error: &dyn Error = &AllocError;
    assert!(error.source().is_none());
}

#[test]
fn an_alloc_error_is_a_copyable_value() {
    let a = AllocError;
    let b = a;
    assert_eq!(a, b);
}

#[test]
fn global_serves_layouts_of_every_shape() {
    for (size, align) in [
        (1, 1),
        (7, 1),
        (8, 8),
        (3, 16),
        (1000, 2),
        (64, 64),
        (4096, 4096),
        (100, 8192),
        (65_536, 8),
        (1, 32),
    ] {
        round_trip(&Global, layout(size, align));
    }
}

#[test]
fn global_serves_a_zero_sized_layout_without_allocating() {
    for align in [1, 2, 8, 16, 4096] {
        let layout = layout(0, align);
        let block = Global.allocate(layout).expect("a zero-sized block");
        assert_eq!(block.len(), 0);
        assert_eq!(
            block.cast::<u8>().as_ptr().addr(),
            align,
            "dangling at the alignment"
        );
        // SAFETY: the block came from `Global` with `layout`; freeing it does nothing.
        unsafe { Global.deallocate(block.cast(), layout) };
    }
}

#[test]
fn global_reports_a_layout_it_cannot_serve() {
    let layout = layout(isize::MAX as usize, 1);
    assert_eq!(Global.allocate(layout), Err(AllocError));
}

#[test]
fn global_hands_out_distinct_blocks() {
    let layout = layout(48, 16);
    let blocks: Vec<NonNull<[u8]>> = (0..32)
        .map(|_| Global.allocate(layout).expect("a block"))
        .collect();
    let mut starts: Vec<usize> = blocks
        .iter()
        .map(|b| b.cast::<u8>().as_ptr().addr())
        .collect();
    starts.sort_unstable();
    for pair in starts.windows(2) {
        assert!(pair[1] - pair[0] >= layout.size(), "blocks overlap");
    }
    for block in blocks {
        // SAFETY: each block came from `Global` with `layout` and is freed once.
        unsafe { Global.deallocate(block.cast(), layout) };
    }
}

#[test]
fn global_is_usable_as_a_static_trait_object() {
    let allocator: &'static (dyn Allocator + Sync) = &Global;
    round_trip(allocator, layout(24, 8));
}

#[test]
fn the_refusing_allocator_refuses_every_layout() {
    for (size, align) in [(0, 1), (1, 1), (64, 64), (4096, 8)] {
        assert_eq!(Refusing.allocate(layout(size, align)), Err(AllocError));
    }
}

#[test]
fn an_allocator_that_refuses_is_heard_through_a_trait_object() {
    static REFUSING: Refusing = Refusing;
    let allocator: &'static (dyn Allocator + Sync) = &REFUSING;
    assert_eq!(allocator.allocate(layout(8, 8)), Err(AllocError));
}

/// Counts what passes through it to `Global`.
struct Counting {
    allocated: AtomicUsize,
    freed: AtomicUsize,
}

// SAFETY: every block is `Global`'s, and is freed through `Global` with the same layout.
unsafe impl Allocator for Counting {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        self.allocated.fetch_add(1, Ordering::Relaxed);
        Global.allocate(layout)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        self.freed.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's promise, passed on to `Global`, which made the block.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

#[test]
fn an_allocator_of_its_own_is_called_through_a_trait_object() {
    static COUNTING: Counting = Counting {
        allocated: AtomicUsize::new(0),
        freed: AtomicUsize::new(0),
    };
    let allocator: &'static (dyn Allocator + Sync) = &COUNTING;
    for size in 1..=5 {
        round_trip(allocator, layout(size * 8, 8));
    }
    assert_eq!(COUNTING.allocated.load(Ordering::Relaxed), 5);
    assert_eq!(COUNTING.freed.load(Ordering::Relaxed), 5);
}

#[test]
fn a_block_from_one_thread_is_freed_on_another() {
    let layout = layout(256, 32);
    let block = Global.allocate(layout).expect("a block");
    let start = block.cast::<u8>().as_ptr().expose_provenance();
    std::thread::spawn(move || {
        let ptr =
            NonNull::new(core::ptr::with_exposed_provenance_mut::<u8>(start)).expect("not null");
        // SAFETY: the block came from `Global` with `layout`, is not used again by the thread
        // that allocated it, and is freed once.
        unsafe { Global.deallocate(ptr, layout) };
    })
    .join()
    .expect("the freeing thread");
}
