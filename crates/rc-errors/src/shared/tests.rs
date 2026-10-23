// Copyright (c) 2026 Mike Grier

use super::{Block, SharedError};
use crate::allocator::{AllocError, Allocator, Global};
use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::error::Error;
use core::fmt;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

const fn assert_send_sync<T: Send + Sync>() {}
const _: () = assert_send_sync::<SharedError>();
const _: () = assert!(size_of::<SharedError>() == 2 * size_of::<usize>());

/// The byte a freed block is overwritten with, so a read after the free sees nonsense.
const POISON: u8 = 0xDD;

/// An allocator over `Global` that records every live block, checks that each free names a live
/// block with the layout it was allocated with, poisons what it frees, and can be made to refuse
/// (RCE-D-8).
struct Recording {
    live: Mutex<Vec<(usize, Layout)>>,
    allocated: AtomicUsize,
    freed: AtomicUsize,
    refuse: AtomicBool,
}

impl Recording {
    /// A fresh one per test, leaked so it is `'static` and no two tests share counts.
    fn new() -> &'static Self {
        Box::leak(Box::new(Self {
            live: Mutex::new(Vec::new()),
            allocated: AtomicUsize::new(0),
            freed: AtomicUsize::new(0),
            refuse: AtomicBool::new(false),
        }))
    }

    fn allocated(&self) -> usize {
        self.allocated.load(Ordering::SeqCst)
    }

    fn freed(&self) -> usize {
        self.freed.load(Ordering::SeqCst)
    }

    fn live_layouts(&self) -> Vec<Layout> {
        self.live
            .lock()
            .expect("live")
            .iter()
            .map(|&(_, layout)| layout)
            .collect()
    }
}

// SAFETY: every block is `Global`'s and is freed through `Global` with the layout it was
// allocated with, which `deallocate` checks before freeing.
unsafe impl Allocator for Recording {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(AllocError);
        }
        let block = Global.allocate(layout)?;
        let start = block.cast::<u8>().as_ptr().addr();
        self.live.lock().expect("live").push((start, layout));
        self.allocated.fetch_add(1, Ordering::SeqCst);
        Ok(block)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        {
            let mut live = self.live.lock().expect("live");
            let at = live
                .iter()
                .position(|&(start, _)| start == ptr.as_ptr().addr())
                .expect("a free names a live block");
            assert_eq!(
                live[at].1, layout,
                "a block is freed with the layout it was allocated with"
            );
            live.swap_remove(at);
        }
        // SAFETY: the block is live, `layout.size()` bytes long, and no longer read by anyone.
        unsafe { ptr.write_bytes(POISON, layout.size()) };
        // SAFETY: it came from `Global` with `layout`, and is freed once: it has left `live`.
        unsafe { Global.deallocate(ptr, layout) };
        self.freed.fetch_add(1, Ordering::SeqCst);
    }
}

fn counter() -> &'static AtomicUsize {
    Box::leak(Box::new(AtomicUsize::new(0)))
}

/// An error that counts its drops, and may carry a source.
#[derive(Debug)]
struct Probe {
    id: u32,
    drops: &'static AtomicUsize,
    source: Option<Cause>,
}

impl Probe {
    fn new(id: u32, drops: &'static AtomicUsize) -> Self {
        Self {
            id,
            drops,
            source: None,
        }
    }
}

impl fmt::Display for Probe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "probe {}", self.id)
    }
}

impl Error for Probe {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|cause| cause as &(dyn Error + 'static))
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct Cause;

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the cause")
    }
}

impl Error for Cause {}

#[derive(Debug)]
struct Unit;

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unit")
    }
}

impl Error for Unit {}

#[derive(Debug)]
#[repr(align(64))]
struct Aligned(u8);

impl fmt::Display for Aligned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "aligned {}", self.0)
    }
}

impl Error for Aligned {}

fn share<E: Error + Send + Sync + 'static>(error: E, allocator: &'static Recording) -> SharedError {
    SharedError::try_new_in(error, allocator)
        .map_err(|(_, cause)| cause)
        .expect("the block")
}

#[test]
fn making_one_allocates_one_block_laid_out_for_the_concrete_error() {
    let allocator = Recording::new();
    let drops = counter();
    let error = share(Probe::new(1, drops), allocator);
    assert_eq!(allocator.allocated(), 1);
    assert_eq!(allocator.live_layouts(), [Layout::new::<Block<Probe>>()]);
    drop(error);
    assert_eq!(allocator.freed(), 1);
}

#[test]
fn the_last_drop_frees_the_block_and_drops_the_error_once() {
    let allocator = Recording::new();
    let drops = counter();
    let first = share(Probe::new(2, drops), allocator);
    let second = first.clone();
    let third = second.clone();
    drop(first);
    drop(third);
    assert_eq!(
        (allocator.freed(), drops.load(Ordering::SeqCst)),
        (0, 0),
        "a reference remains"
    );
    assert_eq!(second.to_string(), "probe 2");
    drop(second);
    assert_eq!((allocator.freed(), drops.load(Ordering::SeqCst)), (1, 1));
    assert!(allocator.live_layouts().is_empty());
}

#[test]
fn the_last_drop_may_happen_on_another_thread() {
    let allocator = Recording::new();
    let drops = counter();
    let error = share(Probe::new(3, drops), allocator);
    let elsewhere = error.clone();
    let reader = std::thread::spawn(move || {
        let text = elsewhere.to_string();
        drop(elsewhere);
        text
    });
    drop(error);
    assert_eq!(reader.join().expect("the other thread"), "probe 3");
    assert_eq!((allocator.freed(), drops.load(Ordering::SeqCst)), (1, 1));
}

#[test]
fn clones_dropped_on_many_threads_free_the_block_once() {
    let allocator = Recording::new();
    let drops = counter();
    let error = share(Probe::new(4, drops), allocator);
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let mine = error.clone();
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    let clone = mine.clone();
                    assert_eq!(clone, mine);
                    drop(clone);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("a cloning thread");
    }
    assert_eq!(allocator.freed(), 0);
    drop(error);
    assert_eq!((allocator.freed(), drops.load(Ordering::SeqCst)), (1, 1));
}

#[test]
fn a_refused_allocation_hands_the_error_back_undropped() {
    let allocator = Recording::new();
    allocator.refuse.store(true, Ordering::SeqCst);
    let drops = counter();
    let (returned, cause) =
        SharedError::try_new_in(Probe::new(5, drops), allocator).expect_err("refused");
    assert_eq!(cause, AllocError);
    assert_eq!((returned.id, drops.load(Ordering::SeqCst)), (5, 0));
    assert_eq!(allocator.allocated(), 0);
    drop(returned);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn a_zero_sized_error_still_has_its_block() {
    let allocator = Recording::new();
    let error = share(Unit, allocator);
    assert_eq!(allocator.live_layouts(), [Layout::new::<Block<Unit>>()]);
    assert_eq!(error.to_string(), "unit");
    drop(error);
    assert_eq!(allocator.freed(), 1);
}

#[test]
fn an_over_aligned_error_is_aligned_and_freed_with_its_alignment() {
    let allocator = Recording::new();
    let error = share(Aligned(7), allocator);
    let layout = allocator.live_layouts()[0];
    assert_eq!(layout.align(), 64);
    let concrete = error.downcast_ref::<Aligned>().expect("an Aligned");
    assert_eq!(core::ptr::from_ref(concrete).addr() % 64, 0);
    assert_eq!(error.to_string(), "aligned 7");
    drop(error);
    assert_eq!(allocator.freed(), 1);
}

#[test]
fn it_displays_and_debugs_as_its_error() {
    let allocator = Recording::new();
    let error = share(Probe::new(6, counter()), allocator);
    assert_eq!(alloc::format!("{error}"), "probe 6");
    assert!(alloc::format!("{error:?}").starts_with("Probe { id: 6"));
}

#[test]
fn it_downcasts_to_the_concrete_error_and_to_nothing_else() {
    let allocator = Recording::new();
    let error = share(Probe::new(7, counter()), allocator);
    assert_eq!(error.downcast_ref::<Probe>().map(|probe| probe.id), Some(7));
    assert!(error.is::<Probe>());
    assert!(error.downcast_ref::<Unit>().is_none());
}

#[test]
fn its_source_is_the_errors_source() {
    let allocator = Recording::new();
    let mut probe = Probe::new(8, counter());
    probe.source = Some(Cause);
    let error = share(probe, allocator);
    let source = Error::source(&error).expect("a source");
    assert_eq!(source.to_string(), "the cause");
    assert!(source.is::<Cause>());
    let plain = share(Probe::new(9, counter()), allocator);
    assert!(Error::source(&plain).is_none());
}

#[test]
fn equality_is_sharing_one_block() {
    let allocator = Recording::new();
    let drops = counter();
    let a = share(Probe::new(10, drops), allocator);
    let b = share(Probe::new(10, drops), allocator);
    let a2 = a.clone();
    assert_eq!(a, a2);
    assert!(SharedError::ptr_eq(&a, &a2));
    assert_ne!(a, b, "equal content in two blocks is two errors");
    assert!(!SharedError::ptr_eq(&a, &b));
}

#[test]
fn the_global_variant_shares_an_error_too() {
    let error = SharedError::try_new(std::io::Error::other("disk gone"))
        .map_err(|(_, cause)| cause)
        .expect("the block");
    let clone = error.clone();
    assert_eq!(clone.to_string(), "disk gone");
    assert_eq!(
        clone
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::Other)
    );
}

#[test]
fn it_is_itself_an_error_that_can_be_boxed_as_a_source() {
    let allocator = Recording::new();
    let error = share(Probe::new(11, counter()), allocator);
    let boxed: Box<dyn Error + Send + Sync> = Box::new(error.clone());
    assert_eq!(boxed.to_string(), "probe 11");
    drop(boxed);
    assert_eq!(allocator.freed(), 0);
    drop(error);
    assert_eq!(allocator.freed(), 1);
}

#[test]
fn many_errors_from_one_allocator_are_each_freed_with_their_own_layout() {
    let allocator = Recording::new();
    let drops = counter();
    let errors: Vec<SharedError> = (0..100)
        .map(|i| {
            if i % 3 == 0 {
                share(Aligned(i as u8), allocator)
            } else {
                share(Probe::new(i, drops), allocator)
            }
        })
        .collect();
    assert_eq!(allocator.allocated(), 100);
    drop(errors);
    assert_eq!(allocator.freed(), 100);
    assert_eq!(drops.load(Ordering::SeqCst), 66);
    assert!(allocator.live_layouts().is_empty());
}
