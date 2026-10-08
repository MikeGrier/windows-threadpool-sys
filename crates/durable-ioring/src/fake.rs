// Copyright (c) 2026 Mike Grier
//! The least implementation of the contract that the oracle's readiness check and the Model A
//! front end can run against in tests: a queue and a signal, with an operation that completes the
//! moment it is pushed. Each entry's context is its sequence number, so delivery order is
//! observable.

use std::collections::VecDeque;
use std::io;

use win_shared_os_owned_handle::SharedHandle;
use win_sync_sys::{Event, ResetMode};

use crate::contract::{DurableRing, EntryOf, PushResult};
use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::types::{
    AddFileError, Entry, Epoch, FileKey, FileOptions, LineageInfo, OpCompletion, OpKind, Outcome,
    ReadOptions, WriteOptions,
};

type V = DioringIds<u64>;

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
    pub(crate) signal: Event,
    signals: Signals,
    queue: VecDeque<EntryOf<Self>>,
    next: u64,
    transitions: u32,
    /// `readiness()` fails, as duplicating a handle can.
    pub(crate) readiness_fails: bool,
}

impl Fake {
    pub(crate) fn new(signals: Signals) -> Self {
        Self {
            instance: InstanceId::next(),
            signal: Event::new(ResetMode::Auto, false).expect("create the signal"),
            signals,
            queue: VecDeque::new(),
            next: 0,
            transitions: 0,
            readiness_fails: false,
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

    fn readiness(&mut self) -> io::Result<Event> {
        if self.readiness_fails {
            return Err(io::Error::other("the fake refuses to duplicate its signal"));
        }
        self.signal.try_clone()
    }

    fn add_file_with(
        &mut self,
        _key: FileKey,
        _file: SharedHandle,
        _options: FileOptions,
    ) -> Result<(), AddFileError> {
        Ok(())
    }

    fn default_lineage(&self) -> Lineage {
        Lineage {
            instance: self.instance,
            seq: 0,
        }
    }

    fn lineages(&self) -> Vec<LineageInfo<V>> {
        Vec::new()
    }

    fn write_with(
        &mut self,
        _file: FileKey,
        _offset: u64,
        _buffer: Vec<u8>,
        _epoch: Epoch<V>,
        _context: u32,
        _options: WriteOptions<V>,
    ) -> PushResult<Self> {
        unimplemented!("tests complete the fake's operations with complete_one")
    }

    fn read_with(
        &mut self,
        _file: FileKey,
        _offset: u64,
        _buffer: Vec<u8>,
        _context: u32,
        _options: ReadOptions<V>,
    ) -> PushResult<Self> {
        unimplemented!("tests complete the fake's operations with complete_one")
    }

    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>> {
        Ok(self.queue.pop_front())
    }
}
