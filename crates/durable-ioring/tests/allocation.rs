// Copyright (c) 2026 Mike Grier
//! How many allocations the core makes in each phase of its work ([DI-23.1], [DI-D-97]).
//!
//! The goal is a core that allocates everything it needs at construction and nothing after. This
//! measures how far it is from that: a counting global allocator, and a scenario over the in-memory
//! backend split into phases, each counting only the allocations made inside calls into the core.
//! The test's own buffers, the backend's work and the test's bookkeeping are not counted.
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
//! [DI-D-97]: ../DESIGN-NOTES.md

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fmt::Write as _;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use durable_ioring::backend::{IoBackend, SubmissionData};
use durable_ioring::memory::{
    Fault, FaultScript, MemoryBackend, MemoryTarget, ScriptOp, When, scripted_fault,
};
use durable_ioring::{
    CoreActions, DioringIds, DurabilityRequest, DurableIoCore, Entry, Epoch, ImportScope, IoTarget,
    IoTargetKey, Lineage, NoDiagnostics, ReadOptions, Resolution, StampClock, WriteOptions,
};

thread_local! {
    /// Whether this thread is inside a call into the core.
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    /// The allocations counted on this thread since the phase began.
    static COUNTED: Cell<u64> = const { Cell::new(0) };
}

/// The system allocator, counting each allocation, zeroed allocation and reallocation made while
/// this thread is counting. Freeing is not counted: the question is whether the core asks for
/// memory, not whether it gives it back.
struct Counting;

fn note() {
    // `try_with`, not `with`: an allocator must not panic. These thread locals have no destructor,
    // so this is only defensive.
    let _ = COUNTING.try_with(|counting| {
        if counting.get() {
            let _ = COUNTED.try_with(|counted| counted.set(counted.get() + 1));
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

/// End the current phase: what it counted, and a fresh count for the next.
fn take() -> u64 {
    COUNTED.with(|counted| counted.replace(0))
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
    phases: Vec<(&'static str, u64)>,
}

impl Rig {
    /// Build the core over `backend`, with `KEY` and `OTHER` attached: the first phase.
    fn new(mut backend: MemoryBackend) -> Self {
        let diagnostics = Arc::new(NoDiagnostics);
        let mut core = counted(|| Core::new(Ticks::default(), diagnostics));
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
        let count = take();
        self.phases.push((name, count));
    }

    /// Push what the core asks for, handing refusals back to it.
    fn act(&mut self, actions: CoreActions<V, Vec<u8>>) {
        let mut pending = vec![actions];
        while let Some(actions) = pending.pop() {
            for submission in actions.submit {
                if let Err(refused) = self.backend.push(submission) {
                    let core = &mut self.core;
                    pending.push(counted(|| core.refused(refused)));
                }
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
fn clean() -> Vec<(&'static str, u64)> {
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

    let minted = counted(|| rig.core.mint_lineage(None));
    rig.phase("a lineage minted");
    rig.round(minted, 1);
    rig.run();
    rig.seal(minted, 1);
    rig.run();
    rig.drain();
    rig.phase("the minted lineage written and sealed");
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
    rig.seal(default, 5);
    rig.run();
    rig.drain();
    rig.phase("the added target written and sealed");
    let core = &mut rig.core;
    counted(|| core.remove_target(ADDED)).expect("nothing holds it");
    rig.phase("the added target removed");

    let core = &rig.core;
    drop(counted(|| core.lineages()));
    rig.phase("lineages queried");

    let Rig {
        core, mut phases, ..
    } = rig;
    drop(counted(|| core.close()));
    phases.push(("close", take()));
    phases
}

/// The failure paths: a commit that fails, the inventory queried, a heal taking effect, an import,
/// and an abandon.
fn failures() -> Vec<(&'static str, u64)> {
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

    rig.round(default, 2);
    rig.run();
    rig.drain();
    rig.phase("writes re-issued");
    let core = &mut rig.core;
    let actions = counted(|| core.resolve(vec![(token, Resolution::Heal)])).expect("a live token");
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
    let actions =
        counted(|| core.resolve(vec![(token, Resolution::Abandon)])).expect("a live token");
    rig.act(actions);
    rig.drain();
    rig.phase("an abandon resolved");

    rig.phases
}

/// The counts each phase made when DI-23.1 was written. A change that moves one updates its table,
/// and its commit says why.
const CLEAN: &[(&str, u64)] = &[
    ("construction, with two targets", 6),
    ("writes, first round", 12),
    ("write completions, first round", 19),
    ("pops, first round", 0),
    ("writes, second round", 2),
    ("write completions, second round", 16),
    ("pops, second round", 0),
    ("reads", 0),
    ("read completions and pops", 0),
    ("a seal covering writes", 5),
    ("commit completions", 9),
    ("pops after the seal", 0),
    ("a seal covering no writes", 2),
    ("completions and pops after the empty seal", 0),
    ("gated writes", 13),
    ("the gate's epoch written", 18),
    ("the gate's epoch sealed, releasing the gated writes", 19),
    ("the gated writes sealed", 7),
    ("a lineage minted", 0),
    ("the minted lineage written and sealed", 32),
    ("the minted lineage ended", 2),
    ("a target added", 1),
    ("the added target written and sealed", 8),
    ("the added target removed", 0),
    ("lineages queried", 1),
    ("close", 1),
];
/// As [`CLEAN`], for the failure paths.
const FAILURES: &[(&str, u64)] = &[
    ("construction, with two targets", 6),
    ("writes before the failure", 31),
    ("a commit failure observed", 22),
    ("failures queried", 2),
    ("writes re-issued", 19),
    ("a heal resolved", 4),
    ("the heal taking effect at the next seal", 11),
    ("writes before the import", 18),
    ("a failure imported", 9),
    ("an abandon resolved", 4),
];
/// The measured table, written so it can be pasted over the expected one.
fn table(name: &str, phases: &[(&'static str, u64)]) -> String {
    let mut shown = format!("const {name}: &[(&str, u64)] = &[\n");
    for (phase, count) in phases {
        let _ = writeln!(shown, "    ({phase:?}, {count}),");
    }
    shown.push_str("];");
    shown
}

#[test]
fn the_clean_paths_allocate_as_recorded() {
    let measured = clean();
    assert_eq!(measured, clean(), "the counts are the same on a second run");
    assert!(
        measured == CLEAN,
        "the clean paths' allocations changed:\n{}",
        table("CLEAN", &measured)
    );
}

#[test]
fn the_failure_paths_allocate_as_recorded() {
    let measured = failures();
    assert_eq!(
        measured,
        failures(),
        "the counts are the same on a second run"
    );
    assert!(
        measured == FAILURES,
        "the failure paths' allocations changed:\n{}",
        table("FAILURES", &measured)
    );
}

#[test]
fn the_counter_counts_inside_a_counted_call_and_nowhere_else() {
    take();
    let outside = vec![1_u8; 32];
    assert_eq!(
        take(),
        0,
        "an allocation outside a counted call is not counted"
    );
    let inside = counted(|| vec![2_u8; 32]);
    let grown = counted(|| {
        let mut grown = Vec::<u8>::with_capacity(1);
        grown.extend_from_slice(&[3; 64]);
        grown
    });
    assert_eq!(
        take(),
        3,
        "an allocation and a reallocation inside are each counted"
    );
    drop((outside, inside, grown));
    assert_eq!(take(), 0, "freeing is not counted");
}
