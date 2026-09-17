// Copyright (c) Mike Grier.

//! What does caching the peer's index buy an SPSC ring?
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! # The technique under test
//!
//! In the ring this workspace ships, each side reads the *other's* index on
//! every single operation: the producer acquire-loads `head` to find room, the
//! consumer acquire-loads `tail` to find work. Both of those lines are written
//! by the opposite core, so every operation is a guaranteed cross-core
//! coherence miss and the line ping-pongs at interconnect speed.
//!
//! **Peer-index caching** removes almost all of them. Each side keeps a plain,
//! non-atomic copy of the peer's index and consults the shared line *only* when
//! its cached copy says the queue looks full (producer) or empty (consumer).
//! In the common case -- neither full nor empty -- there is no shared read at
//! all, so one acquire load is amortised across a whole batch. It is the
//! central trick in Erik Rigtorp's "Optimizing a ring buffer for throughput",
//! and the same idea appears in Boost's `spsc_queue` and DPDK's rings.
//!
//! It is safe because both indices are **monotonic**. A stale cached `head`
//! under-reports free space and a stale cached `tail` under-reports available
//! items, so the error is always conservative: a spurious "full" or "empty",
//! never a wrong write or a double read.
//!
//! # And a control, because the alternative memory deserves testing
//!
//! A second reading of the same technique is that the extra load is *only*
//! there to warm the cache line -- that its value cannot be used, and the
//! authoritative load still has to happen. That is a different mechanism with a
//! different ceiling, so it is measured rather than argued about:
//! [`Strategy::Warmed`](crate::peer_index_cache::Strategy::Warmed) issues a
//! discarded relaxed load of the peer index and
//! then does exactly the work the baseline does.
//!
//! # Why this measures a model rather than the shipping queue
//!
//! This crate's rule is that a probe measures the real API, because a stand-in
//! only measures itself. That rule cannot apply here: the variants do not exist
//! in the shipping crate, and the whole question is whether one of them should.
//!
//! So the model is kept structurally identical to `spsc` -- same split indices,
//! same padding, same orderings, same release/acquire pairing -- and the
//! **baseline strategy is calibrated against the real queue** in the same run.
//! If the model's baseline does not reproduce the shipping queue's number, the
//! model is wrong and the variant comparison means nothing. That calibration
//! row is printed first for exactly that reason.

use std::cell::UnsafeCell;
use std::hint::black_box;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;

use core::ffi::c_void;
use core::ptr;

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocExNuma, VirtualFree,
};
use windows_sys::Win32::System::ProcessStatus::{
    PSAPI_WORKING_SET_EX_INFORMATION, QueryWorkingSetEx,
};
use windows_sys::Win32::System::SystemInformation::{GROUP_AFFINITY, GetSystemInfo, SYSTEM_INFO};
#[cfg(test)]
use windows_sys::Win32::System::Threading::GetThreadGroupAffinity;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, SetThreadGroupAffinity,
};
use windows_waitable_queues::spsc;

#[cfg(test)]
mod tests;

/// Items handed across the ring in one timed run.
pub const ITEMS: usize = 2_000_000;

/// Ring capacity, in items. Deep enough that a consumer keeping up leaves the
/// producer's cached index valid for long stretches, which is the regime the
/// technique is for.
pub const CAPACITY: usize = 1024;

/// Repetitions per configuration; the median is reported.
const REPETITIONS: usize = 5;

/// Which peer-index strategy a run uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Read the peer's index on every operation. What `spsc` does today.
    Baseline,
    /// Keep a local copy and consult the shared line only when it says the
    /// queue looks full or empty.
    Cached,
    /// Issue a discarded load of the peer's index, then do exactly what the
    /// baseline does. Tests whether the benefit is cache warming rather than
    /// the avoided load.
    Warmed,
}

impl Strategy {
    fn label(self) -> &'static str {
        match self {
            Self::Baseline => "model: baseline",
            Self::Cached => "model: cached index",
            Self::Warmed => "model: warming load",
        }
    }

    /// A stable identifier for a record.
    ///
    /// Separate from the private `label` on purpose, and not a duplicate of it.
    /// The label is prose for a terminal table and may be reworded whenever the
    /// table reads better a different way; this is a token a stored record is
    /// keyed on, so rewording it would silently break every collector that ever
    /// grouped by it. Keeping them apart is what lets the prose stay free.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Cached => "cached",
            Self::Warmed => "warmed",
        }
    }
}

/// One configuration's result.
#[derive(Debug, Clone, Copy)]
pub struct Run {
    /// What was measured.
    pub label: &'static str,
    /// Median nanoseconds per item handed across the ring.
    pub nanos_per_item: f64,
    /// Items per second.
    pub items_per_second: f64,
    /// How many times the consumer actually read the shared `tail`.
    ///
    /// **This is the number that says whether the technique was even
    /// exercised.** Peer-index caching only avoids a shared read when the
    /// cached copy says there is work; a consumer that finds the ring empty
    /// every time refreshes every time, and caching can do nothing for it. A
    /// count near [`ITEMS`] means the ring never had a backlog to batch over,
    /// and any comparison drawn from that run is a comparison of branches
    /// rather than of cache traffic.
    pub consumer_refreshes: u64,
    /// How many times the producer actually read the shared `head`.
    pub producer_refreshes: u64,
}

/// Everything one invocation measured.
#[derive(Debug, Clone)]
pub struct Observation {
    /// The shipping `spsc`, so the model can be checked against it.
    pub calibration: Run,
    /// The model under each strategy.
    pub strategies: Vec<Run>,
}

impl Observation {
    /// Look one strategy's run up.
    #[must_use]
    pub fn get(&self, strategy: Strategy) -> Option<Run> {
        self.strategies
            .iter()
            .find(|run| run.label == strategy.label())
            .copied()
    }
}

/// Time the shipping queue and every model strategy.
///
/// # Errors
///
/// **Cannot refuse today, and the signature says `Result` anyway.** Every run
/// this starts is unpinned -- `time_model` passes `None` for both sides, and
/// `pin_current_thread(None)` returns before it touches anything -- so no host
/// can make this return `Err`. The `Result` is here because the helpers it
/// calls are fallible in general, not because this path has a failure mode; an
/// earlier version of this section claimed it "refuses when a thread cannot be
/// confined to the processor it was given", which describes [`time_model_on`]'s
/// pinned callers rather than anything reachable from here.
///
/// The probe that *can* refuse is `core_affinity::measure`, which chooses
/// processors and pins to them.
pub fn measure() -> std::io::Result<Observation> {
    Ok(Observation {
        // Neither arm pins, so neither can refuse -- the `?`s below propagate a
        // failure that no caller of this function can currently produce. Said
        // plainly because the previous note here contrasted the calibration
        // with the strategies as though only the first were unpinned, which
        // would have sent the next reader looking for a refusal path in the
        // second.
        calibration: median("shipping spsc", || Ok(time_real_spsc()))?,
        strategies: [Strategy::Baseline, Strategy::Cached, Strategy::Warmed]
            .into_iter()
            .map(|strategy| median(strategy.label(), || time_model(strategy)))
            .collect::<std::io::Result<Vec<_>>>()?,
    })
}

/// One timed pass, with the shared-read counts that pass performed.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// Which NUMA node held the ring, when that could be arranged.
    ///
    /// None means no placement was requested, or one was requested and could
    /// not be achieved -- recorded rather than assumed, because a hop measured
    /// with the data somewhere unknown is not a measurement of that hop.
    pub memory_node: Option<u32>,
    /// Wall-clock nanoseconds for the whole pass.
    pub nanos: f64,
    /// How many times the consumer read the producer's shared position.
    pub consumer_refreshes: u64,
    /// How many times the producer read the consumer's shared position.
    pub producer_refreshes: u64,
}

fn median(
    label: &'static str,
    mut timer: impl FnMut() -> std::io::Result<Sample>,
) -> std::io::Result<Run> {
    // One untimed pass: first touch of a fresh allocation faults pages in, and
    // that belongs to the allocator rather than to the ring.
    let _ = timer()?;

    let mut samples: Vec<Sample> = (0..REPETITIONS)
        .map(|_| timer())
        .collect::<std::io::Result<Vec<_>>>()?;
    samples.sort_by(|left, right| left.nanos.total_cmp(&right.nanos));
    let sample = samples[REPETITIONS / 2];

    Ok(Run {
        label,
        nanos_per_item: sample.nanos / ITEMS as f64,
        items_per_second: ITEMS as f64 / (sample.nanos / 1e9),
        consumer_refreshes: sample.consumer_refreshes,
        producer_refreshes: sample.producer_refreshes,
    })
}

/// The shipping queue, driven the same way the model is.
fn time_real_spsc() -> Sample {
    let (tx, rx) = spsc::bounded::<u64>(CAPACITY).expect("a valid capacity");
    let started = Instant::now();
    // The producer handle is deliberately not `Sync`, so it cannot be borrowed
    // into a scoped thread -- it has to move. That is the single-producer
    // guarantee being enforced by the compiler, and it is why this reads
    // differently from the model below.
    let producer = thread::spawn(move || {
        for item in 0..ITEMS as u64 {
            let mut item = item;
            while let Err(error) = tx.push(item) {
                item = error.into_inner();
                std::hint::spin_loop();
            }
        }
    });
    let mut taken = 0;
    while taken < ITEMS {
        if rx.pop().is_ok() {
            taken += 1;
        } else {
            std::hint::spin_loop();
        }
    }
    let elapsed = started.elapsed().as_nanos() as f64;
    producer.join().expect("the producer must not panic");
    Sample {
        nanos: elapsed,
        // The shipping queue caches nothing, so both sides read the shared line
        // at least once per item. These are floors, not exact counts: `push`
        // also consults `reserved` and the depth metric, and `pop` re-reads on
        // a retry.
        consumer_refreshes: ITEMS as u64,
        producer_refreshes: ITEMS as u64,
        // The calibration runs the shipping queue, which allocates its own ring
        // wherever it likes: this path has no placement to report.
        memory_node: None,
    }
}

/// Pads a value onto its own cache line, as `spsc` does.
#[repr(align(128))]
struct CacheAligned<T>(T);

/// A minimal SPSC ring, structurally identical to `spsc`'s.
struct Ring {
    slots: Slots,
    mask: usize,
    capacity: usize,
    head: CacheAligned<AtomicUsize>,
    tail: CacheAligned<AtomicUsize>,
}

// SAFETY: the two positions partition the slots between the threads exactly as
// `spsc` does -- a slot in `[head, tail)` belongs to the consumer, one outside
// it to the producer -- and each side publishes its position with a release
// store the other acquires.
unsafe impl Sync for Ring {}

impl Ring {
    /// Build a ring whose slots live on a chosen NUMA node.
    ///
    /// `None` allocates without a preference, which is the honest behaviour on
    /// a machine with one node and the only option when the placement fails.
    fn new_on(capacity: usize, node: Option<u32>) -> Self {
        Self {
            slots: Slots::on_node(capacity, node),
            mask: capacity - 1,
            capacity,
            head: CacheAligned(AtomicUsize::new(0)),
            tail: CacheAligned(AtomicUsize::new(0)),
        }
    }

    /// The NUMA node the slots were **observed** on, or `None` if unknown.
    fn memory_node(&self) -> Option<u32> {
        self.slots.node
    }
}

/// Bit layout of `PSAPI_WORKING_SET_EX_BLOCK`, which `windows-sys` exposes as
/// an opaque `usize` because the SDK declares it as bitfields.
///
/// Changing any value here is a breaking change: these describe an operating
/// system structure, not a choice this crate is free to make.
///
/// # Widths are declared, offsets are derived
///
/// Every offset is computed from the widths of the fields before it, rather
/// than written down. That is what makes the layout testable at all. `Node`
/// reads zero on every host available to this workspace whether or not its
/// offset is right, so a wrong `NODE_SHIFT` is invisible here and would first
/// show up as nonsense node numbers on a multi-socket machine -- the one
/// machine whose answer this tool exists to collect, and the one place nobody
/// can check the result against anything.
///
/// `Win32Protection` sits in the same run of bits and holds a value the caller
/// chose, so a test can decode it and know whether it is right. Deriving
/// `NODE_SHIFT` from the same widths makes that check carry: any error in the
/// run of fields moves both, and the test sees it.
mod working_set {
    /// `Valid`, which is set when the page is resident.
    const VALID_BITS: u32 = 1;
    /// `ShareCount`.
    const SHARE_COUNT_BITS: u32 = 3;
    /// `Win32Protection`.
    const PROTECTION_BITS: u32 = 11;
    /// `Shared`, which sits between the protection and the node.
    const SHARED_BITS: u32 = 1;
    /// `Node`.
    const NODE_BITS: u32 = 6;

    /// Set when the page is resident, and so when the rest is meaningful.
    pub const VALID: usize = (1 << VALID_BITS) - 1;
    /// Offset of the `Win32Protection` field.
    pub const PROTECTION_SHIFT: u32 = VALID_BITS + SHARE_COUNT_BITS;
    /// Width of the `Win32Protection` field, as a mask.
    ///
    /// Only the tests decode a protection -- the crate already knows what it
    /// asked for -- so this exists solely to make the layout falsifiable.
    #[cfg(test)]
    pub const PROTECTION_MASK: usize = (1 << PROTECTION_BITS) - 1;
    /// Offset of the `Shared` field, which is the bit immediately below
    /// `Node` and so the thing that pins `Node`'s position.
    pub const SHARED_SHIFT: u32 = PROTECTION_SHIFT + PROTECTION_BITS;
    /// Offset of the `Node` field.
    pub const NODE_SHIFT: u32 = SHARED_SHIFT + SHARED_BITS;
    /// Width of the `Node` field, as a mask.
    pub const NODE_MASK: usize = (1 << NODE_BITS) - 1;

    /// Where the SDK says `Node` begins.
    ///
    /// A tripwire, and deliberately not presented as verification. The tests
    /// pin every width below `Node` against values the operating system
    /// reports, but `SHARED_BITS` is invisible to them: widening it moves
    /// `Node` alone, and detecting that needs a page on a **non-zero node**,
    /// which no machine available to this workspace can produce. Sabotage
    /// confirms the gap rather than assuming it.
    ///
    /// So this restates the documented total and fails the build if the derived
    /// offset drifts from it. It cannot tell anyone whether the SDK is being
    /// read correctly -- only that nobody has changed the reading by accident.
    const DOCUMENTED_NODE_SHIFT: u32 = 16;
    const _: () = assert!(NODE_SHIFT == DOCUMENTED_NODE_SHIFT);
}

/// The ring's slot storage, and the NUMA node its pages turned out to be on.
///
/// # Why this is not simply a `Box<[UnsafeCell<u64>]>`
///
/// Placing memory on a chosen node means owning the pages. An earlier version
/// tried to avoid that by relying on **first touch** -- Windows backs a page
/// with physical memory from the node of whichever thread first accesses it --
/// and touching the slots from a thread pinned to the target node.
///
/// **It did not work, and it reported success anyway.** `Vec::resize_with`
/// writes every element as it builds the vector, on the unpinned thread that
/// called it, so the pages were already faulted in before the pinned thread ran
/// and its writes were second touches. An 8 KiB request is served from an
/// already-committed heap segment besides, so there was no first touch left to
/// take. `memory_node` was then set from "pinning succeeded" rather than from
/// anything about the memory -- precisely the lie the field exists to prevent.
/// Every hop row on a multi-socket machine would have claimed a placement that
/// never happened, and the two rows per hop would have been one configuration
/// measured twice, with the difference between them read as interconnect
/// asymmetry.
///
/// So the pages come from `VirtualAllocExNuma`, which asks for a node directly,
/// and the node is then **read back** rather than assumed.
struct Slots {
    /// Start of the slot array.
    ptr: *mut Slot,
    /// Number of slots.
    len: usize,
    /// How the storage was obtained, which decides how it is released.
    origin: Origin,
    /// The node the pages were observed on, never the one requested.
    ///
    /// `None` means unknown: no node was asked for, the placement failed, or
    /// the query could not answer. It never means "assume it worked".
    node: Option<u32>,
}

/// One slot in the ring.
///
/// **Named once because it is stated three times**: the number of bytes to
/// allocate, the pointer type the slots are read through, and the element type
/// the heap path builds. An earlier version spelled those out independently and
/// they disagreed -- `UnsafeCell::new(0)` with nothing to constrain the literal
/// inferred `i32`, `cast::<UnsafeCell<u64>>()` accepted the mismatch without
/// complaint because a pointer cast reinterprets rather than checks, and a
/// 4 KiB allocation was then read as 8 KiB. That is a heap overrun that
/// compiles, passes a length check, and corrupts memory a page later. Deriving
/// all three from one name makes the disagreement unrepresentable.
type Slot = UnsafeCell<u64>;

/// Where a [`Slots`] allocation came from, and therefore how it is freed.
enum Origin {
    /// `VirtualAllocExNuma`, released with `VirtualFree`.
    Numa,
    /// The ordinary allocator, released by reconstituting the `Box`.
    Heap,
}

impl Slots {
    /// Allocate `capacity` slots, on `node` when one is asked for.
    ///
    /// Falls back to the ordinary allocator when no node is requested or the
    /// placement fails, recording `None` for the node in both cases.
    fn on_node(capacity: usize, node: Option<u32>) -> Self {
        node.and_then(|node| Self::on_numa_node(capacity, node))
            .unwrap_or_else(|| Self::on_heap(capacity))
    }

    /// Slots from the ordinary allocator, whose node is not chosen or known.
    fn on_heap(capacity: usize) -> Self {
        // The annotation is load-bearing, not decoration: it is what fixes the
        // literal's type. Without it the element type is decided by inference,
        // and the only other mention is a pointer cast, which cannot disagree
        // out loud.
        let mut slots: Vec<Slot> = Vec::with_capacity(capacity);
        slots.resize_with(capacity, || Slot::new(0));
        let slots: Box<[Slot]> = slots.into_boxed_slice();
        let len = slots.len();
        Self {
            ptr: Box::into_raw(slots).cast::<Slot>(),
            len,
            origin: Origin::Heap,
            node: None,
        }
    }

    /// Slots on `node`, or `None` if the system would not place them there.
    fn on_numa_node(capacity: usize, node: u32) -> Option<Self> {
        let bytes = capacity.checked_mul(size_of::<Slot>())?;
        // SAFETY: a null base asks the system to choose the address. The
        // returned region is owned by this `Slots` and freed in `drop`.
        let base = unsafe {
            VirtualAllocExNuma(
                GetCurrentProcess(),
                ptr::null(),
                bytes,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
                node,
            )
        };
        if base.is_null() {
            // No placement was made, so none is recorded.
            //
            // **Do not read a non-null return as "the request was honoured".**
            // The documentation says an out-of-range node fails with
            // `ERROR_INVALID_PARAMETER`; measured on a single-node host, asking
            // for node `u32::MAX` *succeeded* and returned pages on node 0. So
            // success here means only that memory was obtained, and the node it
            // came from is settled by the observation below rather than by this
            // call. Trusting the return value would have reproduced exactly the
            // lie this rewrite exists to remove.
            return None;
        }

        // Fault every page in now. Committed pages are demand-zero, so until
        // something writes to them no physical page has been drawn from the
        // preferred node -- and the query below would have nothing to report.
        // Doing it here also keeps the page faults out of the timed run.
        let slots = base.cast::<Slot>();
        for index in 0..capacity {
            // SAFETY: `index < capacity`, and the region was sized as
            // `capacity * size_of::<Slot>()` from the same name.
            unsafe { slots.add(index).write(Slot::new(0)) };
        }

        Some(Self {
            ptr: slots,
            len: capacity,
            origin: Origin::Numa,
            node: observed_node_of_region(base, bytes),
        })
    }
}

impl Drop for Slots {
    fn drop(&mut self) {
        match self.origin {
            // SAFETY: `ptr` is the base `VirtualAllocExNuma` returned, and
            // `MEM_RELEASE` requires a size of zero.
            Origin::Numa => unsafe {
                VirtualFree(self.ptr.cast::<c_void>(), 0, MEM_RELEASE);
            },
            // SAFETY: reconstitutes exactly the box `on_heap` leaked.
            Origin::Heap => unsafe {
                drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
                    self.ptr, self.len,
                )));
            },
        }
    }
}

impl core::ops::Deref for Slots {
    type Target = [Slot];

    fn deref(&self) -> &Self::Target {
        // SAFETY: `ptr` and `len` describe one live allocation owned by `self`,
        // initialised by whichever constructor produced it.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

/// Which NUMA node an entire region is on, or `None` if it is not on one node.
///
/// **A region is not a page, and this is the difference.** `VirtualAllocExNuma`
/// expresses a *preference*, and physical pages are drawn one at a time as they
/// fault; the ring is 8 KiB at the default capacity, so it spans several pages
/// and the node is only guaranteed uniform if it is checked to be. Asking about
/// the base address alone would describe the first page and label the whole
/// measurement with it.
///
/// A region whose pages disagree has no single node, so it reports `None`
/// rather than picking one. That is the same rule the rest of this field
/// follows: unknown is a real answer, and a guess dressed as a measurement is
/// not.
fn observed_node_of_region(base: *mut c_void, bytes: usize) -> Option<u32> {
    let page = page_size();
    let mut node = None;
    let mut offset = 0;
    while offset < bytes {
        // SAFETY: `offset < bytes`, and the region spans `bytes` from `base`.
        let page_node = observed_node(unsafe { base.byte_add(offset) })?;
        match node {
            None => node = Some(page_node),
            Some(first) if first == page_node => {}
            // Split across nodes. Not an error -- the ring works -- but it is
            // not a measurement of a hop to any one node either.
            Some(_) => return None,
        }
        offset += page;
    }
    node
}

/// The system's page granularity, which is what a NUMA node is assigned by.
fn page_size() -> usize {
    // SAFETY: writes a fully owned `SYSTEM_INFO`, which is plain data.
    let mut info = unsafe { core::mem::zeroed::<SYSTEM_INFO>() };
    unsafe { GetSystemInfo(&raw mut info) };
    // Zero would loop forever below; the call cannot return it, but the loop
    // must not depend on that.
    match usize::try_from(info.dwPageSize) {
        Ok(size) if size > 0 => size,
        _ => 4096,
    }
}

/// Which NUMA node the page at `address` is actually on.
///
/// This is the difference between a record that reports a placement and one
/// that reports a *request*. The page must already be resident, which is why
/// the caller faults it in first.
fn observed_node(address: *mut c_void) -> Option<u32> {
    let flags = working_set_flags(address)?;
    if flags & working_set::VALID == 0 {
        // Not resident, so the node field means nothing. Unknown, not zero.
        return None;
    }
    u32::try_from((flags >> working_set::NODE_SHIFT) & working_set::NODE_MASK).ok()
}

/// The raw `PSAPI_WORKING_SET_EX_BLOCK` bits for the page at `address`.
///
/// Separate from [`observed_node`] so a test can check the layout against a
/// field whose value it already knows, rather than against the node field,
/// which reads zero on this workspace's hardware whether or not the offsets are
/// right.
fn working_set_flags(address: *mut c_void) -> Option<usize> {
    let mut info = PSAPI_WORKING_SET_EX_INFORMATION {
        VirtualAddress: address,
        // SAFETY: an all-zero block is the documented input state; the call
        // fills it in.
        VirtualAttributes: unsafe { core::mem::zeroed() },
    };
    let size = u32::try_from(size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()).ok()?;

    // SAFETY: `info` is one correctly sized entry, and the pseudo-handle from
    // `GetCurrentProcess` carries every access this needs.
    let queried = unsafe {
        QueryWorkingSetEx(
            GetCurrentProcess(),
            ptr::from_mut(&mut info).cast::<c_void>(),
            size,
        )
    };
    if queried == 0 {
        return None;
    }

    // SAFETY: `Flags` is the union's integer view of the same bits.
    Some(unsafe { info.VirtualAttributes.Flags })
}

fn time_model(strategy: Strategy) -> std::io::Result<Sample> {
    time_model_on(strategy, None, None)
}

/// One run of the model, optionally with each side pinned to a chosen
/// processor.
///
/// The affinity arguments exist so a caller can ask where the two threads run
/// rather than accept wherever the scheduler puts them. That turned out to
/// matter: this probe's headline result inverted between two hosts, and the
/// mechanism -- how deeply the two sides batch -- is a property of how they are
/// placed relative to each other, which an unpinned run leaves to chance and
/// cannot report.
///
/// `None` leaves a side unconstrained, which is what the unpinned entry points
/// pass and is deliberately not the same thing as pinning it to every
/// processor: an unconstrained thread can migrate mid-run.
///
/// # Errors
///
/// Refuses, rather than measuring, when either side cannot be confined to the
/// processor it was given -- a process restricted by a job object, a container
/// or `start /affinity` being the usual cause. An unpinned run would produce a
/// plausible number that answers a different question, so the refusal is the
/// result; returning it lets the caller report it rather than die on it.
pub fn time_model_on(
    strategy: Strategy,
    producer_cpu: Option<(u16, u8)>,
    consumer_cpu: Option<(u16, u8)>,
) -> std::io::Result<Sample> {
    time_model_placed(strategy, producer_cpu, consumer_cpu, None)
}

/// As [`time_model_on`], and also chooses which NUMA node holds the ring.
///
/// The third position. With the memory on the producer's node the producer
/// writes locally and the consumer reads remotely; moving it to the consumer's
/// node reverses exactly that, and those are different costs rather than two
/// samples of one.
///
/// # Errors
///
/// As [`time_model_on`].
pub fn time_model_placed(
    strategy: Strategy,
    producer_cpu: Option<(u16, u8)>,
    consumer_cpu: Option<(u16, u8)>,
    memory_node: Option<u32>,
) -> std::io::Result<Sample> {
    let ring = Ring::new_on(CAPACITY, memory_node);
    let placed_on = ring.memory_node();

    // **Pinned before anything is spawned.** `pin_current_thread` refuses on
    // failure, and this used to run *after* the producer was already started:
    // the failure then reached `thread::scope`'s cleanup, which waits for a
    // producer that is itself blocked forever on a ring nobody is draining. A
    // failure that should stop the run hung it instead. Nothing is running yet
    // here, so the refusal simply returns.
    let _consumer_pinned = pin_current_thread(consumer_cpu)?;

    // Outside the scope so it outlives every borrow the spawned thread takes.
    let producer_pin = AtomicU8::new(PIN_PENDING);

    let started = Instant::now();
    let (consumer_refreshes, producer_refreshes) =
        thread::scope(|scope| -> std::io::Result<(u64, u64)> {
            let shared = &ring;
            let producer_pin = &producer_pin;
            let producer = scope.spawn(move || -> std::io::Result<u64> {
                // Armed *before* the pin attempt, so leaving it without success
                // still publishes an answer -- by an early return now, and by an
                // unwind out of anything below. Without that the consumer waits
                // on a thread that has already finished.
                let signal = PinSignal(producer_pin);
                // Bound, not discarded: an unbound guard drops at the end of its
                // own statement, which would unpin the thread immediately and
                // measure the scheduler's choice while claiming to measure this
                // processor. `#[must_use]` makes that mistake a warning.
                let _pinned = pin_current_thread(producer_cpu)?;
                producer_pin.store(PIN_READY, Ordering::Release);
                // Its work is done; dropping it now cannot overwrite `PIN_READY`.
                drop(signal);
                Ok(produce(shared, strategy))
            });

            // **Neither side enters the transfer until both pins are settled.**
            // Spinning rather than parking because the wait is a pin call long,
            // and because this thread is already pinned and must not be handed to
            // another processor by a blocking primitive.
            while producer_pin.load(Ordering::Acquire) == PIN_PENDING {
                std::hint::spin_loop();
            }

            // On failure `consume` is skipped entirely: it would spin forever on
            // items no living producer will write. `join` then surfaces the
            // producer's refusal, which is the outcome the caller should see.
            let consumer_refreshes = if producer_pin.load(Ordering::Acquire) == PIN_READY {
                consume(&ring, strategy)
            } else {
                0
            };
            let producer_refreshes = producer.join().expect("the producer must not panic")?;
            Ok((consumer_refreshes, producer_refreshes))
        })?;
    Ok(Sample {
        nanos: started.elapsed().as_nanos() as f64,
        consumer_refreshes,
        producer_refreshes,
        memory_node: placed_on,
    })
}

/// The producer has not yet reached the end of its pin attempt.
const PIN_PENDING: u8 = 0;
/// The producer is pinned and has begun producing.
const PIN_READY: u8 = 1;
/// The producer left its pin attempt without succeeding, so no data is coming.
const PIN_FAILED: u8 = 2;

/// Publishes [`PIN_FAILED`] if the producer leaves without reporting success.
///
/// **The point is the path that skips the success store, not the success path.**
/// A plain store after `pin_current_thread` never runs when that call refuses --
/// the `?` returns first -- and the consumer would then wait on a producer that
/// has already finished. A guard armed before the attempt runs on every exit
/// from the closure, early return and unwind alike, so the wait always ends.
struct PinSignal<'a>(&'a AtomicU8);

impl Drop for PinSignal<'_> {
    fn drop(&mut self) {
        // Only moves `PENDING` on, so dropping this after a successful
        // `PIN_READY` store is a no-op rather than a lost signal.
        let _ = self.0.compare_exchange(
            PIN_PENDING,
            PIN_FAILED,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }
}

/// Confine the calling thread to one logical processor, named by group.
///
/// **The returned guard restores the previous affinity and must be held for as
/// long as the pinning is wanted.** Dropping it immediately -- by discarding
/// the return value -- unpins the thread at once, so the work that follows
/// measures wherever the scheduler puts it while the row claims a processor.
///
/// Refuses rather than warns on failure. A silently unpinned thread would turn
/// a placement experiment into a measurement of the scheduler's preferences,
/// and the run would still print a confident number -- the same failure mode as
/// a probe that asserts its conclusion. The refusal is returned rather than
/// raised, so the caller can render it into a report instead of dying on it;
/// what it must never do is continue.
///
/// # Why not `SetThreadAffinityMask`
///
/// Its mask is interpreted **within the caller's current group**, so it cannot
/// name a processor in another one. On a machine with more than 64 logical
/// processors that is not a matter of widening the mask; the call has no way to
/// express the target at all. `SetThreadGroupAffinity` takes the group
/// explicitly, and is the only way to pin across the whole machine.
fn pin_current_thread(cpu: Option<(u16, u8)>) -> std::io::Result<AffinityGuard> {
    let Some((group, number)) = cpu else {
        return Ok(AffinityGuard { previous: None });
    };
    // **The predicate lives in `is_nameable_in_a_mask`, not here.** It is also
    // asked by `core_affinity::check_group_support` over every discovered
    // processor, and when the two were separate expressions, converting this one
    // from a panic to a refusal left that one deciding the outcome -- so the fix
    // changed nothing a binary could reach. One definition cannot be
    // half-converted.
    //
    // An error rather than an assert, and for the same reason the OS failure
    // below is one: a group affinity mask is a `usize`, so on a 32-bit target a
    // processor numbered 32 or above cannot be named at all. That is a limit of
    // the mask, not a caller mistake, and it is the condition the pinning tests
    // use because it is the only one reachable deterministically on an ordinary
    // host. Leaving it a panic would have left the fallible path with no test
    // that could reach it.
    if !is_nameable_in_a_mask(number) {
        return Err(refusal(
            group,
            number,
            &format!(
                "a group affinity mask on this target holds {} processors, so \
                 processor {number} cannot be named in one",
                mask_width()
            ),
            MASK_TOO_NARROW,
        ));
    }

    let affinity = GROUP_AFFINITY {
        Mask: 1_usize << number,
        Group: group,
        Reserved: [0; 3],
    };
    // The previous affinity is captured, not discarded. Passing null here left
    // the calling thread pinned after the sample finished, and the next sample
    // then allocated its ring while still confined to the *previous* sample's
    // consumer -- so the rows that report "ring left where it fell" were
    // quietly biased toward whichever processor was last measured. It also made
    // the public timing helpers permanently re-affinitise their caller, which
    // is not a thing a measurement function may do to the program that called
    // it.
    //
    // SAFETY: `affinity` is a fully initialised `GROUP_AFFINITY` naming one
    // processor the caller took from the discovered topology, and `previous` is
    // a writable `GROUP_AFFINITY` this call fills in.
    let mut previous = GROUP_AFFINITY {
        Mask: 0,
        Group: 0,
        Reserved: [0; 3],
    };
    let ok = unsafe { SetThreadGroupAffinity(GetCurrentThread(), &affinity, &raw mut previous) };
    if ok == 0 {
        return Err(refusal(
            group,
            number,
            &std::io::Error::last_os_error().to_string(),
            ENVIRONMENT_REFUSED,
        ));
    }

    Ok(AffinityGuard {
        previous: Some(previous),
    })
}

/// Whether a processor number can be named in a group affinity mask.
///
/// **One definition, because the split between two is exactly how a fix came to
/// be measured as working while the binary's behaviour was unchanged.** This
/// predicate lived inline in `pin_current_thread` and again in
/// `core_affinity::check_group_support`, which runs over every discovered
/// processor *before* any pin. Converting only the first from a panic to a
/// refusal changed nothing a binary could reach: the run still died at the
/// second, with the banner, the heading and nothing else.
///
/// Both sites were consistent before that change and neither was wrong. The
/// hazard is **partial conversion** -- and a shared predicate makes the two
/// impossible to get out of step, which is stronger than remembering to grep.
#[must_use]
pub fn is_nameable_in_a_mask(number: u8) -> bool {
    u32::from(number) < usize::BITS
}

/// How many processors a group affinity mask can name on this target.
///
/// Reported by both refusals, so the reader is told the actual bound rather
/// than being left to infer it from a pointer width.
#[must_use]
pub fn mask_width() -> u32 {
    usize::BITS
}

/// The refusal a failed pin carries, as an error rather than a panic.
///
/// **The decision to stop is unchanged; only the channel is.** The message
/// below is the one this helper has always produced, and the paragraph about
/// measuring without pinning is still the argument for refusing. What changed
/// is that a panic bypassed the probe's report sink: stdout carried a banner
/// and nothing else, so a survey mining it could not tell a host that declined
/// to be measured from a job that died for an unrelated reason. Returned as an
/// error, the same words reach the report, and the refusal becomes an
/// observation the fleet can count.
///
/// **`explanation` is a parameter because the two causes are opposites, and one
/// message for both told a reader the wrong thing.** A `SetThreadGroupAffinity`
/// failure names a processor the topology reported, so something about the
/// environment is unexpected and a bug report is welcome. A number too wide for
/// an affinity mask is the exact reverse -- a limit of this build on this
/// target -- and the shared text told that reader the machine was misbehaving
/// and invited a report about a documented limitation.
///
/// A raw string rather than an escaped-continuation one: `cargo fmt` reindents
/// a multi-line string literal and the backslash continuations then swallow the
/// blank lines, which turns a carefully laid-out message into one paragraph.
/// This is the message a stranger sees when the tool gives up, so its shape
/// matters.
fn refusal(group: u16, number: u8, cause: &str, explanation: &str) -> std::io::Error {
    std::io::Error::other(format!(
        r"
This run is stopping, and no measurement was taken.

Could not confine a thread to processor {number} in group {group}:
  {cause}

{explanation}

The run stops rather than measuring without pinning. An unpinned thread
measures wherever the scheduler happened to put it, which would produce a
plausible number that answers a different question, and nothing in the
output would say so.
"
    ))
}

/// Why a `SetThreadGroupAffinity` failure is worth reporting.
///
/// The processor came from the machine's own topology, so the environment is
/// doing something the topology did not describe.
const ENVIRONMENT_REFUSED: &str =
    "That processor was reported by this machine's own topology, so this is
unexpected rather than a limit of the tool. A process restricted to a
subset of processors -- by a job object, a container, or a `start /affinity`
-- is the usual cause.

Reporting this is genuinely useful: please include this message.";

/// Why a processor too wide for an affinity mask is not worth reporting.
///
/// The opposite case, and it must not borrow the sentence above: nothing is
/// wrong with the host, and inviting a bug report for a documented limit of the
/// tool wastes the reader's time and ours.
const MASK_TOO_NARROW: &str =
    "A group affinity mask is one machine word wide, so this is a limit of this
build on this target rather than a fault in the machine. The host is fine;
this build cannot name that processor.";

/// This thread's group affinity as the system currently reports it.
///
/// Used by the tests that check the pin is undone. Reading it back from the
/// operating system rather than trusting the value the guard stored is the
/// point: a guard that recorded the right thing and restored nothing would
/// otherwise pass.
#[cfg(test)]
fn current_affinity() -> Option<GROUP_AFFINITY> {
    let mut affinity = GROUP_AFFINITY {
        Mask: 0,
        Group: 0,
        Reserved: [0; 3],
    };
    // SAFETY: writes one fully owned `GROUP_AFFINITY`.
    let ok = unsafe { GetThreadGroupAffinity(GetCurrentThread(), &raw mut affinity) };
    (ok != 0).then_some(affinity)
}

/// Puts the calling thread's affinity back when it goes out of scope.
///
/// A guard rather than a call at the end of the timed section, so an early
/// return or an unwind restores it too: either one between pinning and
/// restoring would otherwise leave the thread confined for the rest of the
/// process, and this crate's pinning failure path returns early by design.
#[must_use = "the thread is unpinned as soon as this guard is dropped"]
struct AffinityGuard {
    /// What to restore, or `None` when nothing was changed.
    previous: Option<GROUP_AFFINITY>,
}

impl Drop for AffinityGuard {
    fn drop(&mut self) {
        let Some(previous) = self.previous else {
            return;
        };
        // SAFETY: `previous` is the affinity this thread had, as reported by
        // the call that replaced it.
        let ok = unsafe { SetThreadGroupAffinity(GetCurrentThread(), &previous, ptr::null_mut()) };

        // Failing to restore silently is the defect this type exists to remove,
        // so it is not swallowed -- but panicking while already unwinding would
        // abort the process and destroy the original failure's message, which
        // is the one worth reading.
        assert!(
            ok != 0 || std::thread::panicking(),
            "could not restore the thread's affinity: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Fills the ring, returning how many times it read the consumer's position.
fn produce(ring: &Ring, strategy: Strategy) -> u64 {
    // The producer's local copy of the consumer's position. Plain, not atomic:
    // it is this thread's alone, and it is only ever a conservative
    // under-estimate of how much room there is.
    let mut cached_head = 0_usize;
    let mut refreshes = 0_u64;

    for item in 0..ITEMS as u64 {
        loop {
            // The producer owns `tail`, so this never leaves its own core.
            let tail = ring.tail.0.load(Ordering::Relaxed);

            let full = match strategy {
                Strategy::Baseline => {
                    refreshes += 1;
                    tail.wrapping_sub(ring.head.0.load(Ordering::Acquire)) == ring.capacity
                }
                Strategy::Warmed => {
                    refreshes += 1;
                    // Discarded: warms the line, and then the authoritative
                    // load happens anyway. `black_box` stops the compiler from
                    // noticing the first load is dead and removing it.
                    black_box(ring.head.0.load(Ordering::Relaxed));
                    tail.wrapping_sub(ring.head.0.load(Ordering::Acquire)) == ring.capacity
                }
                Strategy::Cached => {
                    // The shared line is touched only when the cached copy says
                    // there is no room, which is the whole optimisation.
                    if tail.wrapping_sub(cached_head) == ring.capacity {
                        refreshes += 1;
                        cached_head = ring.head.0.load(Ordering::Acquire);
                    }
                    tail.wrapping_sub(cached_head) == ring.capacity
                }
            };

            if full {
                std::hint::spin_loop();
                continue;
            }

            // SAFETY: `tail` is outside `[head, tail)`, so this slot belongs to
            // the producer and no other thread reads it before the release
            // store below publishes it.
            unsafe {
                *ring.slots[tail & ring.mask].get() = item;
            }
            ring.tail.0.store(tail.wrapping_add(1), Ordering::Release);
            break;
        }
    }

    refreshes
}

/// Drains the ring, returning how many times it read the producer's position.
fn consume(ring: &Ring, strategy: Strategy) -> u64 {
    // The consumer's local copy of the producer's position; see `produce`.
    let mut cached_tail = 0_usize;
    let mut taken = 0_usize;
    let mut refreshes = 0_u64;

    while taken < ITEMS {
        let head = ring.head.0.load(Ordering::Relaxed);

        let empty = match strategy {
            Strategy::Baseline => {
                refreshes += 1;
                head == ring.tail.0.load(Ordering::Acquire)
            }
            Strategy::Warmed => {
                refreshes += 1;
                black_box(ring.tail.0.load(Ordering::Relaxed));
                head == ring.tail.0.load(Ordering::Acquire)
            }
            Strategy::Cached => {
                // One acquire load of `tail` per *batch*: everything that
                // snapshot made visible is then drained with no shared reads at
                // all. This is the load the technique is named for.
                if head == cached_tail {
                    refreshes += 1;
                    cached_tail = ring.tail.0.load(Ordering::Acquire);
                }
                head == cached_tail
            }
        };

        if empty {
            std::hint::spin_loop();
            continue;
        }

        // SAFETY: `head` is in `[head, tail)`, so the producer wrote this slot
        // and released it; the acquire above makes that write visible here.
        let item = unsafe { *ring.slots[head & ring.mask].get() };
        black_box(item);
        ring.head.0.store(head.wrapping_add(1), Ordering::Release);
        taken += 1;
    }

    refreshes
}
