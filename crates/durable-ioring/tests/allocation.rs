// Copyright (c) 2026 Mike Grier
//! How many allocations the core makes in each phase of its work, and which call made each
//! ([DI-23.1], [DI-23.2], [DI-D-97], [DI-D-104]).
//!
//! The goal is a core that allocates everything it needs at construction and nothing after. This
//! measures how far it is from that: a counting global allocator, and a scenario over the in-memory
//! backend split into phases, each counting only the allocations made inside calls into the core.
//! The test's own buffers, the backend's work and the test's bookkeeping are not counted.
//!
//! Every allocation is counted under the tag the core set for the call that made it. Three rules
//! hold in every phase: no counted allocation is untagged, no tag is seen from two places in the
//! source, and the tags add up to the phase's total.
//!
//! The expected tables are today's counts, not a target. A change that moves a count fails here and
//! prints the new table, so a count going down is seen as clearly as one going up. Each item of
//! DI-M23 lowers the counts it addresses, and a phase whose count reaches zero is held there.
//!
//! Counting is per thread, so the tests the harness runs in parallel are not counted. An allocation
//! the core made on another thread would be missed; it makes none, since it runs only inside the
//! calls made to it.
//!
//! [DI-23.1]: ../CHECKLIST.md
//! [DI-23.2]: ../CHECKLIST.md
//! [DI-D-97]: ../DESIGN-NOTES.md
//! [DI-D-104]: ../DESIGN-NOTES.md

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::num::NonZeroU32;
use std::panic::Location;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use durable_ioring::alloc_tags::{self, Site, Tag};
use durable_ioring::backend::{IoBackend, SubmissionData};
use durable_ioring::memory::{
    Fault, FaultScript, MemoryBackend, MemoryTarget, ScriptOp, When, scripted_fault,
};
use durable_ioring::{
    Capacity, CoreActions, DioringIds, DurabilityRequest, DurableIoCore, Entry, Epoch, ImportScope,
    IoTarget, IoTargetKey, Lineage, NoDiagnostics, ReadOptions, Resolution, StampClock,
    WriteOptions,
};

thread_local! {
    /// Whether this thread is inside a call into the core.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    /// What this thread has counted since the phase began.
    static COUNTS: RefCell<Counts> = const { RefCell::new(Counts::EMPTY) };
}

/// The most distinct tags one phase can count. Fixed, so counting never allocates.
const SLOTS: usize = 256;

/// What the core is built with: more than any phase here reaches.
const CAPACITY: Capacity = Capacity {
    operations: 64,
    lineages: 8,
    imports: 4,
    targets: 4,
    domains: 4,
};

/// One tag's count in a phase, and where it was seen.
#[derive(Clone, Copy)]
struct Slot {
    tag: Option<Tag>,
    location: Option<&'static Location<'static>>,
    count: u64,
    /// A second place the same tag was seen, which breaks the rule that a tag names one call.
    conflict: Option<&'static Location<'static>>,
}

impl Slot {
    const EMPTY: Self = Self {
        tag: None,
        location: None,
        count: 0,
        conflict: None,
    };
}

/// What a thread counted in a phase, held in fixed storage so counting never allocates.
struct Counts {
    total: u64,
    untagged: u64,
    /// Set if more distinct tags were seen than there are slots.
    overflowed: bool,
    slots: [Slot; SLOTS],
}

impl Counts {
    const EMPTY: Self = Self {
        total: 0,
        untagged: 0,
        overflowed: false,
        slots: [Slot::EMPTY; SLOTS],
    };

    fn note(&mut self, site: Option<Site>) {
        self.total += 1;
        let Some(site) = site else {
            self.untagged += 1;
            return;
        };
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.tag.is_none_or(|tag| tag == site.tag));
        let Some(slot) = slot else {
            self.overflowed = true;
            return;
        };
        match slot.location {
            None => {
                slot.tag = Some(site.tag);
                slot.location = Some(site.location);
            }
            Some(seen) if !same_place(seen, site.location) => slot.conflict = Some(site.location),
            Some(_) => {}
        }
        slot.count += 1;
    }
}

fn same_place(a: &Location<'_>, b: &Location<'_>) -> bool {
    a.file() == b.file() && a.line() == b.line() && a.column() == b.column()
}

/// The system allocator, counting each allocation, zeroed allocation and reallocation made while
/// this thread is counting, under the tag the core set for the call that made it. Freeing is not
/// counted: the question is whether the core asks for memory, not whether it gives it back.
struct Counting;

fn note() {
    // `try_with` and `try_borrow_mut`, not `with` and `borrow_mut`: an allocator must not panic.
    // These thread locals have no destructor and nothing here allocates, so this is only defensive.
    let _ = COUNTING.try_with(|counting| {
        if counting.get() {
            let site = alloc_tags::current();
            let _ = COUNTS.try_with(|counts| {
                if let Ok(mut counts) = counts.try_borrow_mut() {
                    counts.note(site);
                }
            });
        }
    });
}

// SAFETY: every method forwards to `System` with the caller's arguments unchanged, so this
// allocator upholds exactly the contract `System` does. Counting touches only const-initialised
// thread locals, which neither allocate nor unwind.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's contract for `alloc` is passed on unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's contract for `alloc_zeroed` is passed on unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: the caller's contract for `realloc` is passed on unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract for `dealloc` is passed on unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Run `call`, counting the allocations it makes into the current phase.
fn counted<R>(call: impl FnOnce() -> R) -> R {
    COUNTING.with(|counting| counting.set(true));
    let result = call();
    COUNTING.with(|counting| counting.set(false));
    result
}

/// What one phase counted.
struct Measured {
    phase: &'static str,
    total: u64,
    untagged: u64,
    overflowed: bool,
    /// Each tag's count, in the order the tags were first seen.
    tags: Vec<(&'static str, u64)>,
    /// Each tag seen from two places, and the two places.
    conflicts: Vec<String>,
}

/// End the current phase, `phase`: what it counted, and fresh counts for the next. Called outside
/// any counted call, so what it allocates is not counted.
fn take(phase: &'static str) -> Measured {
    let counts = COUNTS.with(|counts| counts.replace(Counts::EMPTY));
    let used = counts.slots.iter().filter(|slot| slot.tag.is_some());
    Measured {
        phase,
        total: counts.total,
        untagged: counts.untagged,
        overflowed: counts.overflowed,
        tags: used
            .clone()
            .map(|slot| (slot.tag.expect("used").name(), slot.count))
            .collect(),
        conflicts: used
            .filter_map(|slot| {
                let (first, second) = (slot.location?, slot.conflict?);
                Some(format!("{} at {first} and at {second}", slot.tag?.name()))
            })
            .collect(),
    }
}
/// A clock that counts its readings, with no allocation behind it.
#[derive(Default)]
struct Ticks(AtomicU64);

impl StampClock for Ticks {
    type Instant = u64;

    fn now(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

type V = DioringIds<u64, u64>;
type Core = DurableIoCore<u64, Vec<u8>, &'static str, Ticks>;
type CoreEntry = Entry<V, Vec<u8>, &'static str>;

const KEY: IoTargetKey = IoTargetKey(1);
const OTHER: IoTargetKey = IoTargetKey(2);
/// A third target, added and removed in the middle of the scenario.
const ADDED: IoTargetKey = IoTargetKey(3);
/// Writes in a round: enough that a container growing with them grows more than once.
const ROUND: u64 = 16;

fn nz(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("non-zero")
}

/// The target a write or read of a round's `n`th goes to: `KEY` and `OTHER` in turn.
fn alternate(n: u64) -> IoTargetKey {
    if n.is_multiple_of(2) { KEY } else { OTHER }
}

/// A core and a backend, with the driver's part played here, and the phases measured so far.
struct Rig {
    core: Core,
    backend: MemoryBackend,
    phases: Vec<Measured>,
}

impl Rig {
    /// Build the core over `backend`, with `KEY` and `OTHER` attached: the first phase.
    fn new(mut backend: MemoryBackend) -> Self {
        let diagnostics = Arc::new(NoDiagnostics);
        let mut core = counted(|| Core::new(Ticks::default(), diagnostics, CAPACITY))
            .expect("a valid capacity");
        for key in [KEY, OTHER] {
            let target = MemoryTarget::new();
            let description = target.describe();
            counted(|| core.add_target(key, description)).expect("a new key");
            backend.attach(key, target).expect("a new key");
        }
        let mut rig = Self {
            core,
            backend,
            phases: Vec::new(),
        };
        rig.phase("construction, with two targets");
        rig
    }

    /// End the current phase, recording what it counted.
    fn phase(&mut self, name: &'static str) {
        self.phases.push(take(name));
    }

    /// Push what the core asks for, handing refusals back to it, as the engine does.
    fn act(&mut self, _: CoreActions) {
        loop {
            let core = &mut self.core;
            let Some(submission) = counted(|| core.next_submission()) else {
                return;
            };
            if let Err(refused) = self.backend.push(submission) {
                let core = &mut self.core;
                let _ = counted(|| core.refused(refused));
            }
        }
    }

    /// Submit, and feed every completion back, until nothing is queued.
    fn run(&mut self) {
        loop {
            self.backend
                .submit()
                .expect("the memory backend's submit never fails");
            let mut any = false;
            while let Some(completion) = self.backend.pop().expect("never fails") {
                any = true;
                let core = &mut self.core;
                let actions = counted(|| core.complete(completion));
                self.act(actions);
            }
            if !any && self.backend.queued() == 0 {
                return;
            }
        }
    }

    /// Pop every entry.
    fn drain(&mut self) -> Vec<CoreEntry> {
        let mut entries = Vec::new();
        loop {
            let core = &mut self.core;
            let Some(entry) = counted(|| core.pop()) else {
                return entries;
            };
            entries.push(entry);
        }
    }

    fn write(&mut self, key: IoTargetKey, epoch: Epoch<V>, offset: u64, options: WriteOptions<V>) {
        let data = SubmissionData::Owned(vec![0x5A; 8]);
        let (core, backend) = (&mut self.core, &mut self.backend);
        counted(|| core.write(key, offset, data, epoch, "write", options, backend))
            .expect("accepted");
    }

    fn read(&mut self, key: IoTargetKey, offset: u64) {
        let data = SubmissionData::Owned(vec![0; 8]);
        let (core, backend) = (&mut self.core, &mut self.backend);
        counted(|| core.read(key, offset, data, "read", ReadOptions::new(), backend))
            .expect("accepted");
    }

    /// A round of writes in `lineage`'s `epoch`, to `KEY` and `OTHER` in turn.
    fn round(&mut self, lineage: Lineage, epoch: u64) {
        for n in 0..ROUND {
            self.write(
                alternate(n),
                Epoch::new(lineage, epoch),
                n * 8,
                WriteOptions::new(),
            );
        }
    }

    fn seal(&mut self, lineage: Lineage, through: u64) -> DurabilityRequest<V> {
        let core = &mut self.core;
        let (answer, actions) = counted(|| core.make_durable_through(Epoch::new(lineage, through)))
            .expect("a live lineage");
        self.act(actions);
        answer
    }
}

/// The clean paths: writes and reads, gated writes, a seal that covers writes and one that covers
/// none, a lineage minted and ended, a target added and removed, the queries, and close.
fn clean() -> Vec<Measured> {
    let mut rig = Rig::new(MemoryBackend::new(nz(64), nz(128)));
    let default = rig.core.default_lineage();

    for (writes, completions, pops) in [
        (
            "writes, first round",
            "write completions, first round",
            "pops, first round",
        ),
        (
            "writes, second round",
            "write completions, second round",
            "pops, second round",
        ),
    ] {
        rig.round(default, 1);
        rig.phase(writes);
        rig.run();
        rig.phase(completions);
        rig.drain();
        rig.phase(pops);
    }

    for n in 0..ROUND {
        rig.read(alternate(n), n * 8);
    }
    rig.phase("reads");
    rig.run();
    rig.drain();
    rig.phase("read completions and pops");

    assert_eq!(rig.seal(default, 1), DurabilityRequest::Submitted);
    rig.phase("a seal covering writes");
    rig.run();
    rig.phase("commit completions");
    rig.drain();
    rig.phase("pops after the seal");

    assert_eq!(rig.seal(default, 2), DurabilityRequest::Submitted);
    rig.phase("a seal covering no writes");
    rig.run();
    rig.drain();
    rig.phase("completions and pops after the empty seal");

    let gate = Epoch::new(default, 3);
    for n in 0..4 {
        rig.write(
            KEY,
            Epoch::new(default, 4),
            n * 8,
            WriteOptions::new().gate(gate),
        );
    }
    rig.phase("gated writes");
    rig.round(default, 3);
    rig.run();
    rig.drain();
    rig.phase("the gate's epoch written");
    rig.seal(default, 3);
    rig.run();
    rig.drain();
    rig.phase("the gate's epoch sealed, releasing the gated writes");
    rig.seal(default, 4);
    rig.run();
    rig.drain();
    rig.phase("the gated writes sealed");

    // The description is the consumer's, made before the count: keeping it copies nothing.
    let description = Some("the minted lineage".to_owned());
    let minted = counted(|| rig.core.mint_lineage(description)).expect("room for a lineage");
    rig.phase("a lineage minted");
    rig.round(minted, 1);
    rig.run();
    rig.drain();
    rig.phase("the minted lineage written");
    let core = &mut rig.core;
    counted(|| core.retire_lineage(minted)).expect_err("uncovered writes");
    let core = &rig.core;
    counted(|| core.lineage_holds(minted, drop));
    rig.phase("a busy lineage refused, and its holds named");
    rig.seal(minted, 1);
    rig.run();
    rig.drain();
    rig.phase("the minted lineage sealed");
    let core = &mut rig.core;
    let actions = counted(|| core.end_lineage(minted)).expect("a live lineage");
    rig.act(actions);
    rig.run();
    rig.drain();
    rig.phase("the minted lineage ended");

    let target = MemoryTarget::new();
    let description = target.describe();
    let core = &mut rig.core;
    counted(|| core.add_target(ADDED, description)).expect("a new key");
    rig.backend.attach(ADDED, target).expect("a new key");
    rig.phase("a target added");
    rig.write(ADDED, Epoch::new(default, 5), 0, WriteOptions::new());
    rig.run();
    rig.drain();
    rig.phase("the added target written");
    let core = &mut rig.core;
    counted(|| core.remove_target(ADDED)).expect_err("an uncovered write");
    let core = &rig.core;
    counted(|| core.target_holds(ADDED, drop));
    rig.phase("a busy target refused, and its holds named");
    rig.seal(default, 5);
    rig.run();
    rig.drain();
    rig.phase("the added target sealed");
    let core = &mut rig.core;
    counted(|| core.remove_target(ADDED)).expect("nothing holds it");
    rig.phase("the added target removed");

    let core = &rig.core;
    counted(|| core.lineages(|_| {}));
    rig.phase("lineages queried");

    let Rig {
        core, mut phases, ..
    } = rig;
    counted(|| core.close(|_| {}));
    phases.push(take("close"));
    phases
}

/// The failure paths: a commit that fails, the inventory queried, its suspect set and markings read,
/// a heal taking effect, an import, and an abandon.
fn failures() -> Vec<Measured> {
    let script = FaultScript::new().on(
        When::any().op(ScriptOp::Commit).target(KEY).nth(1),
        Fault::Fail(scripted_fault("the flush failed")),
    );
    let mut rig = Rig::new(MemoryBackend::new(nz(64), nz(128)).with_script(script));
    let default = rig.core.default_lineage();

    rig.round(default, 1);
    rig.run();
    rig.drain();
    rig.phase("writes before the failure");
    rig.seal(default, 1);
    rig.run();
    let entries = rig.drain();
    rig.phase("a commit failure observed");
    let token = entries
        .into_iter()
        .find_map(|entry| match entry {
            Entry::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .expect("the failed commit's entry");

    let core = &rig.core;
    let inventory = counted(|| core.failures());
    assert_eq!(inventory.len(), 1, "the one failure");
    drop(inventory);
    rig.phase("failures queried");
    let core = &rig.core;
    let (mut suspects, mut markings) = (0_usize, 0_usize);
    counted(|| {
        core.failure_suspects(token.id(), |_| suspects += 1)
            .expect("unresolved");
        core.failure_markings(token.id(), |_| markings += 1)
            .expect("unresolved");
    });
    assert!(suspects > 0, "the failed commit's writes are suspect");
    rig.phase("a failure's suspects and markings read");

    rig.round(default, 2);
    rig.run();
    rig.drain();
    rig.phase("writes re-issued");
    let core = &mut rig.core;
    let items = vec![(token, Resolution::Heal)];
    let actions = counted(|| core.resolve(items)).expect("a live token");
    rig.act(actions);
    rig.phase("a heal resolved");
    rig.seal(default, 2);
    rig.run();
    rig.drain();
    rig.phase("the heal taking effect at the next seal");

    rig.round(default, 3);
    rig.run();
    rig.drain();
    rig.phase("writes before the import");
    let core = &mut rig.core;
    let (imported, actions) = counted(|| core.import_failure(ImportScope::All)).expect("in scope");
    rig.act(actions);
    let entries = rig.drain();
    rig.phase("a failure imported");
    let token = entries
        .into_iter()
        .find_map(|entry| match entry {
            Entry::Failed(failed) if failed.id == imported.failure => Some(failed.token),
            _ => None,
        })
        .expect("the import's entry");
    let core = &mut rig.core;
    let items = vec![(token, Resolution::Abandon)];
    let actions = counted(|| core.resolve(items)).expect("a live token");
    rig.act(actions);
    rig.drain();
    rig.phase("an abandon resolved");

    rig.phases
}

/// The allocations each phase made, as last recorded. A change that moves one updates this table and
/// its tags', and its commit says why.
const CLEAN: &[(&str, u64)] = &[
    ("construction, with two targets", 40),
    ("writes, first round", 3),
    ("write completions, first round", 0),
    ("pops, first round", 0),
    ("writes, second round", 0),
    ("write completions, second round", 0),
    ("pops, second round", 0),
    ("reads", 0),
    ("read completions and pops", 0),
    ("a seal covering writes", 0),
    ("commit completions", 0),
    ("pops after the seal", 0),
    ("a seal covering no writes", 0),
    ("completions and pops after the empty seal", 0),
    ("gated writes", 0),
    ("the gate's epoch written", 0),
    ("the gate's epoch sealed, releasing the gated writes", 0),
    ("the gated writes sealed", 0),
    ("a lineage minted", 0),
    ("the minted lineage written", 0),
    ("a busy lineage refused, and its holds named", 0),
    ("the minted lineage sealed", 0),
    ("the minted lineage ended", 0),
    ("a target added", 0),
    ("the added target written", 0),
    ("a busy target refused, and its holds named", 0),
    ("the added target sealed", 0),
    ("the added target removed", 0),
    ("lineages queried", 0),
    ("close", 0),
];

/// As [`CLEAN`], for the failure paths.
const FAILURES: &[(&str, u64)] = &[
    ("construction, with two targets", 40),
    ("writes before the failure", 3),
    ("a commit failure observed", 0),
    ("failures queried", 1),
    ("a failure's suspects and markings read", 0),
    ("writes re-issued", 0),
    ("a heal resolved", 0),
    ("the heal taking effect at the next seal", 0),
    ("writes before the import", 0),
    ("a failure imported", 0),
    ("an abandon resolved", 1),
];

/// Each phase's allocations by tag, as last recorded, in the order the tags were first seen.
const CLEAN_TAGS: &[(&str, &str, u64)] = &[
    (
        "construction, with two targets",
        "durability::new::lineages",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::writes",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::seals",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::in_flight_by_epoch",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::held_by_epoch",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::gated",
        1,
    ),
    ("construction, with two targets", "durability::new::held", 1),
    (
        "construction, with two targets",
        "durability::new::spans",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::window",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::flush_pairs",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::served_names",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::failures",
        1,
    ),
    ("construction, with two targets", "durability::new::kept", 2),
    (
        "construction, with two targets",
        "durability::new::tokens",
        2,
    ),
    (
        "construction, with two targets",
        "durability::new::free_tokens",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::lists",
        2,
    ),
    ("construction, with two targets", "durability::new::due", 4),
    (
        "construction, with two targets",
        "durability::new::scratch_live",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_covered",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_work",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_bounds",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_steps",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_pairs",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::served",
        1,
    ),
    ("construction, with two targets", "io_core::new::targets", 1),
    ("construction, with two targets", "io_core::new::domains", 1),
    (
        "construction, with two targets",
        "io_core::new::in_flight",
        1,
    ),
    (
        "construction, with two targets",
        "io_core::new::per_target",
        1,
    ),
    ("construction, with two targets", "io_core::new::held", 1),
    (
        "construction, with two targets",
        "io_core::new::unpushed",
        1,
    ),
    (
        "construction, with two targets",
        "io_core::new::outgoing",
        1,
    ),
    (
        "construction, with two targets",
        "io_core::new::requests",
        1,
    ),
    ("construction, with two targets", "io_core::new::queue", 1),
    ("construction, with two targets", "io_core::new::served", 1),
    ("writes, first round", "io_core::issue::backend_push", 3),
];

/// As [`CLEAN_TAGS`], for the failure paths.
const FAILURE_TAGS: &[(&str, &str, u64)] = &[
    (
        "construction, with two targets",
        "durability::new::lineages",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::writes",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::seals",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::in_flight_by_epoch",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::held_by_epoch",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::gated",
        1,
    ),
    ("construction, with two targets", "durability::new::held", 1),
    (
        "construction, with two targets",
        "durability::new::spans",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::window",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::flush_pairs",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::served_names",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::failures",
        1,
    ),
    ("construction, with two targets", "durability::new::kept", 2),
    (
        "construction, with two targets",
        "durability::new::tokens",
        2,
    ),
    (
        "construction, with two targets",
        "durability::new::free_tokens",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::lists",
        2,
    ),
    ("construction, with two targets", "durability::new::due", 4),
    (
        "construction, with two targets",
        "durability::new::scratch_live",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_covered",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_work",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_bounds",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_steps",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::scratch_pairs",
        1,
    ),
    (
        "construction, with two targets",
        "durability::new::served",
        1,
    ),
    ("construction, with two targets", "io_core::new::targets", 1),
    ("construction, with two targets", "io_core::new::domains", 1),
    (
        "construction, with two targets",
        "io_core::new::in_flight",
        1,
    ),
    (
        "construction, with two targets",
        "io_core::new::per_target",
        1,
    ),
    ("construction, with two targets", "io_core::new::held", 1),
    (
        "construction, with two targets",
        "io_core::new::unpushed",
        1,
    ),
    (
        "construction, with two targets",
        "io_core::new::outgoing",
        1,
    ),
    (
        "construction, with two targets",
        "io_core::new::requests",
        1,
    ),
    ("construction, with two targets", "io_core::new::queue", 1),
    ("construction, with two targets", "io_core::new::served", 1),
    (
        "writes before the failure",
        "io_core::issue::backend_push",
        3,
    ),
    ("failures queried", "durability::failures::infos", 1),
    ("an abandon resolved", "durability::resolve::abandoned", 1),
];

/// Each phase's total.
type Totals = &'static [(&'static str, u64)];

/// Each phase's count for each tag.
type ByTag = &'static [(&'static str, &'static str, u64)];

/// Each phase's total.
fn totals(measured: &[Measured]) -> Vec<(&'static str, u64)> {
    measured.iter().map(|m| (m.phase, m.total)).collect()
}

/// Each phase's count for each tag.
fn by_tag(measured: &[Measured]) -> Vec<(&'static str, &'static str, u64)> {
    measured
        .iter()
        .flat_map(|m| m.tags.iter().map(|&(tag, count)| (m.phase, tag, count)))
        .collect()
}

/// The measured tables, written so they can be pasted over the expected ones.
fn tables(name: &str, tags_name: &str, measured: &[Measured]) -> String {
    let mut shown = format!("const {name}: &[(&str, u64)] = &[\n");
    for (phase, count) in totals(measured) {
        let _ = writeln!(shown, "    ({phase:?}, {count}),");
    }
    let _ = write!(
        shown,
        "];\n\nconst {tags_name}: &[(&str, &str, u64)] = &[\n"
    );
    for (phase, tag, count) in by_tag(measured) {
        let _ = writeln!(shown, "    ({phase:?}, {tag:?}, {count}),");
    }
    shown.push_str("];");
    shown
}

/// The rules every phase keeps: no allocation untagged, no tag seen from two places, and the tags
/// adding up to the total.
fn broken_rules(measured: &[Measured]) -> Vec<String> {
    let mut broken = Vec::new();
    for m in measured {
        if m.untagged > 0 {
            broken.push(format!("{}: {} untagged", m.phase, m.untagged));
        }
        if m.overflowed {
            broken.push(format!("{}: more tags than {SLOTS} slots", m.phase));
        }
        for conflict in &m.conflicts {
            broken.push(format!("{}: {conflict}", m.phase));
        }
        let tagged: u64 = m.tags.iter().map(|&(_, count)| count).sum();
        if tagged + m.untagged != m.total {
            broken.push(format!(
                "{}: tags add up to {tagged} of {}",
                m.phase, m.total
            ));
        }
    }
    broken
}

fn check(run: fn() -> Vec<Measured>, expected: (Totals, ByTag), names: (&str, &str)) {
    let measured = run();
    let again = run();
    assert_eq!(
        totals(&measured),
        totals(&again),
        "the same totals on a second run"
    );
    assert_eq!(
        by_tag(&measured),
        by_tag(&again),
        "the same tags on a second run"
    );
    let broken = broken_rules(&measured);
    assert!(
        broken.is_empty(),
        "allocation tags broken:\n{}",
        broken.join("\n")
    );
    assert!(
        totals(&measured) == expected.0 && by_tag(&measured) == expected.1,
        "allocations changed:\n{}",
        tables(names.0, names.1, &measured)
    );
}

#[test]
fn the_clean_paths_allocate_as_recorded() {
    check(clean, (CLEAN, CLEAN_TAGS), ("CLEAN", "CLEAN_TAGS"));
}

#[test]
fn the_failure_paths_allocate_as_recorded() {
    check(
        failures,
        (FAILURES, FAILURE_TAGS),
        ("FAILURES", "FAILURE_TAGS"),
    );
}

#[test]
fn the_counter_counts_inside_a_counted_call_and_nowhere_else() {
    take("before");
    let outside = vec![1_u8; 32];
    assert_eq!(
        take("outside").total,
        0,
        "an allocation outside a counted call is not counted"
    );
    let inside = counted(|| vec![2_u8; 32]);
    let grown = counted(|| {
        let mut grown = Vec::<u8>::with_capacity(1);
        grown.extend_from_slice(&[3; 64]);
        grown
    });
    let measured = take("inside");
    assert_eq!(
        measured.total, 3,
        "an allocation and a reallocation inside are each counted"
    );
    assert_eq!(measured.untagged, 3, "with no tag set, each is untagged");
    assert_eq!(
        broken_rules(&[measured]),
        ["inside: 3 untagged"],
        "and the rules report it"
    );
    drop((outside, inside, grown));
    assert_eq!(take("freed").total, 0, "freeing is not counted");
}
