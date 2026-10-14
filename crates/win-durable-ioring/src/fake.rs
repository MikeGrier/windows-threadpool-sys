// Copyright (c) 2026 Mike Grier
//! The least implementation of the contract that the oracle's readiness check and the Model A
//! front end can run against in tests: a queue and a signal, with an operation that completes the
//! moment it is pushed. Each entry's context is its sequence number, so delivery order is
//! observable.

use std::collections::VecDeque;
use std::io;

use win_sync_sys::{Event, ResetMode};

use crate::memory::MemoryTarget;
use durable_ioring::driver::InstanceId;
use durable_ioring::{
    AddTargetError, DurabilityRequest, EndLineageError, EndLineageRefusal, Entry, Epoch,
    EpochState, FailureInfo, ImportScope, Imported, IoTargetKey, Leftovers, LineageInfo,
    OpCompletion, OpKind, Outcome, ReadOptions, RemoveTargetError, Resolution, ResolveError,
    RetireLineageError, RetireLineageRefusal, SealError, Tag, UnknownLineage, WriteOptions,
};
use durable_ioring::{DurableRing, EntryOf, PushResult};
use durable_ioring::{FailureId, FailureToken, Lineage, LineageHandle, OpId};

type V = crate::WindowsIds<u64>;

/// How the fake sets its readiness signal.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Signals {
    /// On every empty-to-non-empty transition, as DI-D-28 requires.
    OnEveryTransition,
    /// Never.
    Never,
    /// On the first transition only.
    OnceOnly,
}

pub(crate) struct Fake {
    instance: InstanceId,
    /// Its one lineage, which reports to no instance.
    default: LineageHandle,
    pub(crate) signal: Event,
    signals: Signals,
    queue: VecDeque<EntryOf<Self>>,
    next: u64,
    transitions: u32,
    /// `readiness()` fails, as duplicating a handle can.
    pub(crate) readiness_fails: bool,
    /// `pop` fails, as an instance's does when its doorbell cannot issue or its ring cannot be read.
    pub(crate) pop_fails: bool,
}

impl Fake {
    pub(crate) fn new(signals: Signals) -> Self {
        let instance = InstanceId::next();
        Self {
            instance,
            default: LineageHandle::detached(Lineage { instance, seq: 0 }),
            signal: Event::new(ResetMode::Auto, false).expect("create the signal"),
            signals,
            queue: VecDeque::new(),
            next: 0,
            transitions: 0,
            readiness_fails: false,
            pop_fails: false,
        }
    }

    /// Complete one operation: its entry becomes poppable, then the signal is set as `signals`
    /// says.
    pub(crate) fn complete_one(&mut self) {
        let id = OpId {
            instance: self.instance,
            seq: self.next,
        };
        let context = u32::try_from(self.next).expect("a test completes fewer than 2^32");
        self.next += 1;
        let was_empty = self.queue.is_empty();
        self.queue.push_back(Entry::Op(OpCompletion {
            id,
            kind: OpKind::Read,
            outcome: Outcome::Transferred(4),
            buffer: None,
            context,
        }));
        if was_empty {
            self.transitions += 1;
            let set = match self.signals {
                Signals::OnEveryTransition => true,
                Signals::Never => false,
                Signals::OnceOnly => self.transitions == 1,
            };
            if set {
                self.signal.set().expect("set the signal");
            }
        }
    }
}

impl DurableRing for Fake {
    type Ids = V;
    type Buffer = Vec<u8>;
    type Context = u32;
    type Readiness = Event;
    type Target = MemoryTarget;

    fn readiness(&mut self) -> io::Result<Event> {
        if self.readiness_fails {
            return Err(io::Error::other("the fake refuses to duplicate its signal"));
        }
        self.signal.try_clone()
    }

    fn add_target(
        &mut self,
        _key: IoTargetKey,
        _target: MemoryTarget,
    ) -> Result<(), AddTargetError<MemoryTarget>> {
        Ok(())
    }

    fn remove_target(&mut self, key: IoTargetKey) -> Result<MemoryTarget, RemoveTargetError<V>> {
        Err(RemoveTargetError::UnknownKey(key))
    }

    fn default_lineage(&self) -> LineageHandle {
        self.default.clone()
    }

    fn mint_lineage(&mut self, _description: Option<String>) -> LineageHandle {
        unimplemented!("the fake has only its default lineage")
    }

    fn lineages(&self) -> Vec<LineageInfo<V>> {
        Vec::new()
    }

    fn end_lineage(&mut self, handle: LineageHandle) -> Result<(), EndLineageError<V>> {
        Err(EndLineageError {
            reason: EndLineageRefusal::Default,
            handle,
        })
    }

    fn retire_lineage(&mut self, handle: LineageHandle) -> Result<(), RetireLineageError<V>> {
        Err(RetireLineageError {
            reason: RetireLineageRefusal::Default,
            handle,
        })
    }

    fn write_with(
        &mut self,
        _file: IoTargetKey,
        _offset: u64,
        _buffer: Vec<u8>,
        _tag: Tag<'_, V>,
        _context: u32,
        _options: WriteOptions<V>,
    ) -> PushResult<Self> {
        unimplemented!("tests complete the fake's operations with complete_one")
    }

    fn read_with(
        &mut self,
        _file: IoTargetKey,
        _offset: u64,
        _buffer: Vec<u8>,
        _context: u32,
        _options: ReadOptions<V>,
    ) -> PushResult<Self> {
        unimplemented!("tests complete the fake's operations with complete_one")
    }

    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>> {
        if self.pop_fails {
            return Err(io::Error::other("the fake's pop was told to fail"));
        }
        Ok(self.queue.pop_front())
    }

    fn make_durable_through(
        &mut self,
        _through: Tag<'_, V>,
    ) -> Result<DurabilityRequest<V>, SealError<V>> {
        unimplemented!("the fake makes nothing durable")
    }

    fn durable_through(&self, _lineage: Lineage) -> Result<Option<u64>, UnknownLineage<V>> {
        Ok(None)
    }

    fn sealed_through(&self, _lineage: Lineage) -> Result<Option<u64>, UnknownLineage<V>> {
        Ok(None)
    }

    fn epoch_state(&self, _epoch: Epoch<V>) -> Result<EpochState<V>, UnknownLineage<V>> {
        Ok(EpochState::Open)
    }

    fn resolve(&mut self, _items: Vec<(FailureToken, Resolution)>) -> Result<(), ResolveError<V>> {
        unimplemented!("the fake observes no failures")
    }

    fn import_failure(&mut self, _scope: ImportScope<V>) -> Result<Imported<V>, UnknownLineage<V>> {
        unimplemented!("the fake observes no failures")
    }

    fn failures(&self) -> Vec<FailureInfo<V>> {
        Vec::new()
    }

    fn take_token(&mut self, _failure: FailureId) -> Option<FailureToken> {
        None
    }

    fn close(self) -> Leftovers<V, Vec<u8>, u32> {
        Leftovers {
            held: Vec::new(),
            failures: Vec::new(),
            lineages: Vec::new(),
        }
    }
}
