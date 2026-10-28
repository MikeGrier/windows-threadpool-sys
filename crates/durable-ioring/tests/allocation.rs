// Copyright (c) 2026 Mike Grier
//! How many allocations the core makes in each phase of its work, and which call made each
//! ([DI-23.1], [DI-23.2], [DI-D-97], [DI-D-104]).
//!
//! The goal is a core that allocates everything it needs at construction and nothing after, and
//! this holds it to that: a counting global allocator, and a scenario over the in-memory backend
//! split into phases, each counting only the allocations made inside calls into the core. The
//! test's own buffers and bookkeeping are not counted; the backend's work is, inside the calls the
//! core makes to it, so a backend that grows a queue as it fills fails here too.
//!
//! Every allocation is counted under the tag the core set for the call that made it. Three rules
//! hold in every phase: no counted allocation is untagged, no tag is seen from two places in the
//! source, and the tags add up to the phase's total.
//!
//! **Every phase after construction allocates nothing** (DI-23.12.9), and any that does fails the
//! test, naming it. Construction's allocations are recorded, by phase and by tag: a change that
//! moves one fails here and prints the new table, so a count going down is seen as clearly as one
//! going up.
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use durable_ioring::alloc_tags::{self, Site, Tag};
use durable_ioring::backend::{IoBackend, SubmissionData};
use durable_ioring::driver::{Answer, DomainCall};
use durable_ioring::memory::{
    Fault, FaultScript, MemoryBackend, MemoryTarget, ScriptOp, When, scripted_fault,
};
use durable_ioring::{
    Capacity, ConstructionError, CoreActions, DioringIds, DomainOutcome, DurabilityRequest,
    DurableIoCore, Entry, Epoch, FailureCause, FailureObserver, FlushDomain, ImportScope, IoTarget,
    IoTargetKey, Lineage, Marking, NoDiagnostics, PushRefusal, ReadOptions, RequestBuf, Resolution,
    SealError, StampClock, SuspectWrite, WriteOptions,
};

/// A failure observer that only counts what it is told, so it allocates nothing itself.
#[derive(Default)]
struct TallyObserver(AtomicU64);

impl FailureObserver<V> for TallyObserver {
    fn failed(&self, _: <V as durable_ioring::Identities>::FailureId, _: &FailureCause<V>, _: u64) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn suspected(&self, _: <V as durable_ioring::Identities>::FailureId, _: SuspectWrite<V>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn marked(&self, _: <V as durable_ioring::Identities>::FailureId, _: &Marking<V>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

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
/// Where the scenario's provider answers: each answer is kept, in room made before anything is
/// counted, for the test to give the core.
#[derive(Default)]
struct Answers {
    given: Mutex<Vec<(Lineage, u64, FlushDomain, DomainOutcome)>>,
}

impl Answers {
    /// With room for the answers one round makes.
    fn new() -> Arc<Self> {
        let answers = Self::default();
        answers
            .given
            .lock()
            .expect("the answers")
            .reserve(CAPACITY.domains);
        Arc::new(answers)
    }
}

impl Answer for Answers {
    fn issued(&self) {}

    fn answer(&self, call: &DomainCall, outcome: DomainOutcome) {
        self.given.lock().expect("the answers").push((
            call.lineage,
            call.seal,
            call.name.clone(),
            outcome,
        ));
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
    /// The core's failure observer, to show it was told.
    told: Arc<TallyObserver>,
    /// The buffer the provider's requests are drained into, made with a core that has a provider.
    requests: Option<RequestBuf<V>>,
}

impl Rig {
    /// Build the core over `backend`, with `KEY` and `OTHER` attached: the first phase.
    fn new(backend: MemoryBackend) -> Self {
        Self::built(backend, None, "construction, with two targets")
    }

    /// As [`new`](Self::new), for a core whose provider serves `domains`, which `KEY` declares, and
    /// with the buffer the provider's requests are drained into.
    fn with_provider(backend: MemoryBackend, domains: Vec<FlushDomain>) -> Self {
        Self::built(
            backend,
            Some(domains),
            "construction, with a provider and its request buffer",
        )
    }

    fn built(
        mut backend: MemoryBackend,
        provider: Option<Vec<FlushDomain>>,
        first: &'static str,
    ) -> Self {
        let diagnostics = Arc::new(NoDiagnostics);
        let served = provider.clone().unwrap_or_default();
        // Made before anything is counted: what is measured is the core telling it, which the
        // failure paths do (DI-D-113).
        let told = Arc::new(TallyObserver::default());
        let observer: Arc<dyn FailureObserver<V>> = told.clone();
        let (mut core, requests) = counted(|| {
            let (core, requests) = match provider {
                Some(domains) => {
                    let core =
                        Core::with_provider(Ticks::default(), diagnostics, CAPACITY, domains)?;
                    let requests = core.request_buf()?;
                    (core, Some(requests))
                }
                None => (Core::new(Ticks::default(), diagnostics, CAPACITY)?, None),
            };
            Ok::<_, ConstructionError>((core.observed_by(observer), requests))
        })
        .expect("a valid capacity");
        for key in [KEY, OTHER] {
            let target = if key == KEY {
                MemoryTarget::new().with_domains(served.iter().cloned())
            } else {
                MemoryTarget::new()
            };
            let description = target.describe();
            counted(|| core.add_target(key, description)).expect("a new key");
            backend.attach(key, target).expect("a new key");
        }
        let mut rig = Self {
            core,
            backend,
            phases: Vec::new(),
            told,
            requests,
        };
        rig.phase(first);
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

    /// Drain the provider's requests into the buffer made with the core, as the driver does with
    /// the engine's lock held.
    fn drain_requests(&mut self) {
        let buf = self.requests.as_mut().expect("a core with a provider");
        let core = &mut self.core;
        counted(|| core.drain_requests(buf));
    }

    /// Lend each drained request to the provider, which answers every domain of it with success, as
    /// a provider does inside its call: the answers land in `answers`.
    fn lend_requests(&mut self, home: &Weak<dyn Answer>) {
        let buf = self.requests.as_mut().expect("a core with a provider");
        counted(|| {
            for request in buf.requests(home) {
                for part in request.domains() {
                    for file in part.files {
                        assert!(!file.writes.is_empty(), "a file is named for its writes");
                    }
                    part.completion.succeed();
                }
            }
        });
        buf.clear();
    }

    /// Give the core each answer the provider made.
    fn provide(&mut self, answers: &Answers) {
        let given: Vec<_> = answers
            .given
            .lock()
            .expect("the answers")
            .drain(..)
            .collect();
        for (lineage, seal, domain, outcome) in given {
            let core = &mut self.core;
            let actions = counted(|| core.provided(lineage, seal, &domain, outcome));
            self.act(actions);
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

    /// A write the core refuses, as the refusal.
    fn write_refused(
        &mut self,
        key: IoTargetKey,
        epoch: Epoch<V>,
        options: WriteOptions<V>,
    ) -> PushRefusal<V> {
        let data = SubmissionData::Owned(vec![0x5A; 8]);
        let (core, backend) = (&mut self.core, &mut self.backend);
        counted(|| core.write(key, 0, data, epoch, "write", options, backend))
            .expect_err("refused")
            .reason
    }

    /// A seal the core refuses, as the refusal.
    fn seal_refused(&mut self, lineage: Lineage, through: u64) -> SealError<V> {
        let core = &mut self.core;
        counted(|| core.make_durable_through(Epoch::new(lineage, through))).expect_err("refused")
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

    // A gate cycle across two lineages: a write at `a:4` waiting for `b:9`, and then what would
    // close the cycle -- a write at `b:9` gated on `a:4`, and a seal of `a` through 5.
    let (a, b) = {
        let core = &mut rig.core;
        let a = counted(|| core.mint_lineage(None)).expect("room for a lineage");
        let b = counted(|| core.mint_lineage(None)).expect("room for a lineage");
        (a, b)
    };
    rig.phase("two lineages minted");
    let waits_for_b = WriteOptions::new().gate(Epoch::new(b, 9));
    rig.write(KEY, Epoch::new(a, 4), 0, waits_for_b);
    rig.phase("a gated write held");
    let closing = WriteOptions::new().gate(Epoch::new(a, 4));
    let refusal = rig.write_refused(KEY, Epoch::new(b, 9), closing);
    assert!(matches!(refusal, PushRefusal::GateCycle), "{refusal:?}");
    rig.phase("a gated push refused for a cycle");
    let waits_for_a = WriteOptions::new().gate(Epoch::new(a, 3));
    rig.write(KEY, Epoch::new(b, 9), 8, waits_for_a);
    rig.phase("a second gated write held");
    let refusal = rig.seal_refused(a, 5);
    assert_eq!(refusal, SealError::GateCycle);
    rig.phase("a seal refused for a cycle");
    // Sealing lower first releases the gates that were in the way.
    assert_eq!(rig.seal(a, 3), DurabilityRequest::Submitted);
    rig.run();
    rig.drain();
    rig.phase("the lower seal releasing a gated write");
    assert_eq!(rig.seal(b, 9), DurabilityRequest::Submitted);
    rig.run();
    rig.drain();
    rig.phase("the other lineage sealed, releasing the first gated write");
    assert_eq!(rig.seal(a, 5), DurabilityRequest::Submitted);
    rig.run();
    rig.drain();
    rig.phase("the first lineage sealed through its held write");
    for lineage in [a, b] {
        let core = &mut rig.core;
        let actions = counted(|| core.end_lineage(lineage)).expect("a live lineage");
        rig.act(actions);
    }
    rig.run();
    rig.drain();
    rig.phase("two lineages ended");

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

/// The failure paths: a commit that fails, the inventory queried, a heal taking effect, an import,
/// and an abandon.
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
    let mut listed = 0_usize;
    counted(|| core.failures(|_| listed += 1));
    assert_eq!(listed, 1, "the one failure");
    rig.phase("failures queried");

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
    assert!(
        rig.told.0.load(Ordering::Relaxed) >= 4,
        "the observer was told of both failures, a suspect write and a marking"
    );

    rig.phases
}

/// A round of writes, and a seal covering them that asks the provider about `KEY`'s: the writes
/// completing, the seal, its request drained into the buffer made at construction, lent to the
/// provider, answered, the answer given to the core, and the seal's commit completing. `names` are
/// the phases, in that order.
fn provider_round(
    rig: &mut Rig,
    answers: &Answers,
    home: &Weak<dyn Answer>,
    lineage: Lineage,
    epoch: u64,
    names: [&'static str; 7],
) {
    rig.round(lineage, epoch);
    rig.phase(names[0]);
    rig.run();
    rig.drain();
    rig.phase(names[1]);
    rig.seal(lineage, epoch);
    rig.phase(names[2]);
    rig.drain_requests();
    rig.phase(names[3]);
    rig.lend_requests(home);
    rig.phase(names[4]);
    rig.provide(answers);
    rig.phase(names[5]);
    rig.run();
    rig.drain();
    rig.phase(names[6]);
}

/// The provider's path: seals that ask it, their requests drained into the buffer made at
/// construction, lent to the provider and answered, and the answers given back to the core; and a
/// seal whose lineage ended before its request was drained (DI-D-115).
fn providers() -> Vec<Measured> {
    let log = FlushDomain::new(b"log".to_vec());
    let mut rig = Rig::with_provider(MemoryBackend::new(nz(64), nz(128)), vec![log]);
    let default = rig.core.default_lineage();
    // Made before the count: where the provider's answers go is the driver's.
    let answers = Answers::new();
    let strong: Arc<dyn Answer> = answers.clone();
    let home: Weak<dyn Answer> = Arc::downgrade(&strong);

    provider_round(
        &mut rig,
        &answers,
        &home,
        default,
        1,
        [
            "provider writes, first round",
            "provider write completions, first round",
            "a seal asking the provider, first round",
            "the request drained, first round",
            "the request lent and answered, first round",
            "the answer given to the core, first round",
            "commit completions and pops, first round",
        ],
    );
    provider_round(
        &mut rig,
        &answers,
        &home,
        default,
        2,
        [
            "provider writes, second round",
            "provider write completions, second round",
            "a seal asking the provider, second round",
            "the request drained, second round",
            "the request lent and answered, second round",
            "the answer given to the core, second round",
            "commit completions and pops, second round",
        ],
    );

    let description = Some("the minted lineage".to_owned());
    let minted = counted(|| rig.core.mint_lineage(description)).expect("room for a lineage");
    rig.phase("a lineage minted for the provider");
    rig.round(minted, 1);
    rig.run();
    rig.drain();
    rig.phase("the minted lineage written");
    rig.seal(minted, 1);
    let core = &mut rig.core;
    let actions = counted(|| core.end_lineage(minted)).expect("a live lineage");
    rig.act(actions);
    rig.phase("the minted lineage sealed and ended before its request was drained");
    rig.drain_requests();
    rig.phase("the ended lineage's request drained");
    rig.lend_requests(&home);
    rig.phase("the ended lineage's request lent and answered");
    rig.provide(&answers);
    rig.run();
    rig.drain();
    rig.phase("the ended lineage's answer given and its commit completed");

    let Rig {
        core, mut phases, ..
    } = rig;
    counted(|| core.close(|_| {}));
    phases.push(take("close"));
    phases
}

/// The allocations each phase made, as last recorded. A change that moves one updates this table and
/// its tags', and its commit says why.
const CLEAN: &[(&str, u64)] = &[("construction, with two targets", 39)];

/// As [`CLEAN`], for the failure paths.
const FAILURES: &[(&str, u64)] = &[("construction, with two targets", 39)];

/// As [`CLEAN`], for the provider's path.
const PROVIDERS: &[(&str, u64)] = &[("construction, with a provider and its request buffer", 51)];

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
    ("construction, with two targets", "durability::new::kept", 1),
    (
        "construction, with two targets",
        "durability::new::abandoned",
        1,
    ),
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
    ("construction, with two targets", "durability::new::kept", 1),
    (
        "construction, with two targets",
        "durability::new::abandoned",
        1,
    ),
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
];

/// As [`CLEAN_TAGS`], for the provider's path.
const PROVIDERS_TAGS: &[(&str, &str, u64)] = &[
    (
        "construction, with a provider and its request buffer",
        "domains::first_repeat::seen",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::lineages",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::writes",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::seals",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::in_flight_by_epoch",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::held_by_epoch",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::gated",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::held",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::spans",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::window",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::seal_domains",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::flush_pairs",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::served_names",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::failures",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::kept",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::abandoned",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::tokens",
        2,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::free_tokens",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::lists",
        2,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::due",
        4,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::scratch_live",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::scratch_covered",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::scratch_work",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::scratch_bounds",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::scratch_touched",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::scratch_pairs",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "durability::new::served",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::targets",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::domains",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::in_flight",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::per_target",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::held",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::unpushed",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::outgoing",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::requests",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::queue",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::new::served",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "domains::intern_all::id",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "domains::intern_all::shared",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::with_provider::named",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::with_provider::served_names",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "io_core::with_provider::names",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "request_buf::new::requests",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "request_buf::new::parts",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "request_buf::new::targets",
        1,
    ),
    (
        "construction, with a provider and its request buffer",
        "request_buf::new::ids",
        1,
    ),
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

/// The phase every scenario starts with, the only one that may allocate, and so the only one whose
/// counts are recorded (DI-23.12.9).
fn construction(measured: &[Measured]) -> &[Measured] {
    &measured[..1]
}

/// The rules every phase keeps: no allocation untagged, no tag seen from two places, and the tags
/// adding up to the total; and that no phase after construction allocates at all.
fn broken_rules(measured: &[Measured]) -> Vec<String> {
    let mut broken = Vec::new();
    for m in &measured[1..] {
        if m.total > 0 {
            broken.push(format!(
                "{}: {} allocations after construction",
                m.phase, m.total
            ));
        }
    }
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
    let made = construction(&measured);
    assert!(
        totals(made) == expected.0 && by_tag(made) == expected.1,
        "construction's allocations changed:\n{}",
        tables(names.0, names.1, made)
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
fn the_provider_path_allocates_as_recorded() {
    check(
        providers,
        (PROVIDERS, PROVIDERS_TAGS),
        ("PROVIDERS", "PROVIDERS_TAGS"),
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
