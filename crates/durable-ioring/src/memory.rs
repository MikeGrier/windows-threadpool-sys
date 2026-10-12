// Copyright (c) 2026 Mike Grier
//! An in-memory backend and target (DI-D-50 P13, DI-D-52): a deterministic backend, holding each
//! target's bytes in memory, for testing the core and driving it without a device.
//!
//! It faults on nothing of its own accord; `DI-11.1` adds the fault script. A submission is executed
//! when it is submitted, in push order, and its completion is ready at once. A commit makes the
//! target's current bytes its durable bytes, so a test can read what a commit warranted.
//!
//! **Refusals.** A push is refused with [`RefusalCause::SubmitNeeded`] when the submission queue is
//! full, and with [`RefusalCause::DrainNeeded`] when as many submissions are queued or completed and
//! not yet popped as the completion queue holds -- so a completion always has room -- and with
//! [`RefusalCause::SubmitAndDrainNeeded`] when both hold.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::num::NonZeroU32;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use crate::backend::{
    Completion, DuplicateTarget, IoBackend, RefusalCause, Refused, RegisteredRegion, Submission,
    SubmissionData, SubmissionOp,
};
use crate::io_target::{Capabilities, Geometry, IoTarget, WarrantyClass};
use crate::pump::Waitable;
use crate::types::{FlushDomain, IoFault, IoTargetKey};

#[cfg(test)]
mod tests;

/// The source every fault the in-memory backend reports names.
pub const MEMORY: &str = "memory";

fn memory_fault(message: &str) -> IoFault {
    IoFault {
        source: MEMORY,
        code: None,
        error: Arc::new(io::Error::other(message.to_owned())),
    }
}

/// A target whose bytes are held in memory: what was written, and what the last successful commit
/// warranted. It states what it is built with: no flush domains, [`WarrantyClass::Complete`],
/// [`Geometry::UNALIGNED`] and [`Capabilities::NONE`] unless set.
#[derive(Clone, Debug)]
pub struct MemoryTarget {
    contents: Vec<u8>,
    durable: Vec<u8>,
    domains: Vec<FlushDomain>,
    warranty: WarrantyClass,
    geometry: Geometry,
    capabilities: Capabilities,
}

impl Default for MemoryTarget {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryTarget {
    /// An empty target.
    #[must_use]
    pub fn new() -> Self {
        Self {
            contents: Vec::new(),
            durable: Vec::new(),
            domains: Vec::new(),
            warranty: WarrantyClass::Complete,
            geometry: Geometry::UNALIGNED,
            capabilities: Capabilities::NONE,
        }
    }

    /// Declare its flush domains.
    #[must_use]
    pub fn with_domains(mut self, domains: impl IntoIterator<Item = FlushDomain>) -> Self {
        self.domains = domains.into_iter().collect();
        self
    }

    /// State its warranty class.
    #[must_use]
    pub fn with_warranty(mut self, warranty: WarrantyClass) -> Self {
        self.warranty = warranty;
        self
    }

    /// State its geometry, which the backend enforces.
    #[must_use]
    pub fn with_geometry(mut self, geometry: Geometry) -> Self {
        self.geometry = geometry;
        self
    }

    /// State its capabilities.
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Its bytes as written: every byte up to the end of the furthest write, zero where nothing
    /// was written.
    #[must_use]
    pub fn contents(&self) -> &[u8] {
        &self.contents
    }

    /// Its bytes as of its last successful commit.
    #[must_use]
    pub fn durable_contents(&self) -> &[u8] {
        &self.durable
    }

    /// Write `bytes` at `offset`, extending with zeros as needed.
    fn write(&mut self, offset: usize, bytes: &[u8]) {
        let end = offset + bytes.len();
        if self.contents.len() < end {
            self.contents.resize(end, 0);
        }
        self.contents[offset..end].copy_from_slice(bytes);
    }

    /// Read into `into` from `offset`; the count read, short past the end.
    fn read(&self, offset: usize, into: &mut [u8]) -> usize {
        let available = self.contents.get(offset..).unwrap_or_default();
        let count = into.len().min(available.len());
        into[..count].copy_from_slice(&available[..count]);
        count
    }
}

impl IoTarget for MemoryTarget {
    fn flush_domains(&self) -> &[FlushDomain] {
        &self.domains
    }

    fn warranty(&self) -> WarrantyClass {
        self.warranty
    }

    fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }
}

/// The in-memory backend's notification: a count of its raisings, which a thread can wait to see
/// pass a value it has seen. Clones share one count, and each keeps its own record of how many
/// raisings its own waits have seen, which starts at none -- so a raising before a notification's
/// first wait is seen by it, as the Windows ring's setup signal makes one that was raised before
/// the wait began seen.
#[derive(Clone, Debug, Default)]
pub struct MemoryNotification {
    count: Arc<(Mutex<u64>, Condvar)>,
    /// The count at the end of the last [`Waitable::wait`] that saw a raising.
    seen: u64,
}

impl MemoryNotification {
    /// How many times it has been raised.
    #[must_use]
    pub fn raised(&self) -> u64 {
        *self.count.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait up to `timeout` for it to have been raised more than `seen` times; the count then.
    #[must_use]
    pub fn wait_past(&self, seen: u64, timeout: Duration) -> u64 {
        self.count_past(seen, Some(timeout))
    }

    /// [`wait_past`](Self::wait_past), with no limit for `None`.
    fn count_past(&self, seen: u64, timeout: Option<Duration>) -> u64 {
        let (count, raised) = &*self.count;
        let guard = count.lock().unwrap_or_else(PoisonError::into_inner);
        let guard = match timeout {
            Some(timeout) => {
                raised
                    .wait_timeout_while(guard, timeout, |count| *count <= seen)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            None => raised
                .wait_while(guard, |count| *count <= seen)
                .unwrap_or_else(PoisonError::into_inner),
        };
        *guard
    }

    fn raise(&self) {
        let (count, raised) = &*self.count;
        *count.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        raised.notify_all();
    }
}

impl Waitable for MemoryNotification {
    fn wait(&mut self, timeout: Option<Duration>) -> io::Result<bool> {
        let count = self.count_past(self.seen, timeout);
        let raised = count > self.seen;
        self.seen = count;
        Ok(raised)
    }
}

/// The in-memory backend. Its buffers are `Vec<u8>`: a write's length is its buffer's, and a
/// read's the most it transfers.
#[derive(Debug)]
pub struct MemoryBackend {
    targets: HashMap<IoTargetKey, MemoryTarget>,
    registered: Vec<Vec<u8>>,
    queued: VecDeque<Submission<Vec<u8>>>,
    completed: VecDeque<Completion<Vec<u8>>>,
    submission_queue_size: usize,
    completion_queue_size: usize,
    notification: MemoryNotification,
}

impl MemoryBackend {
    /// A backend whose submission queue holds `submission_queue_size` submissions and whose
    /// completion queue holds `completion_queue_size` completions, with no registered buffers.
    #[must_use]
    pub fn new(submission_queue_size: NonZeroU32, completion_queue_size: NonZeroU32) -> Self {
        Self {
            targets: HashMap::new(),
            registered: Vec::new(),
            queued: VecDeque::new(),
            completed: VecDeque::new(),
            submission_queue_size: size(submission_queue_size),
            completion_queue_size: size(completion_queue_size),
            notification: MemoryNotification::default(),
        }
    }

    /// Register `buffers`, in order, replacing any registered before.
    #[must_use]
    pub fn with_registered(mut self, buffers: Vec<Vec<u8>>) -> Self {
        self.registered = buffers;
        self
    }

    /// Registered buffer `index`, if there is one.
    #[must_use]
    pub fn registered(&self, index: u32) -> Option<&[u8]> {
        self.registered.get(position(index)).map(Vec::as_slice)
    }

    /// Registered buffer `index`, writable, if there is one.
    pub fn registered_mut(&mut self, index: u32) -> Option<&mut [u8]> {
        self.registered
            .get_mut(position(index))
            .map(Vec::as_mut_slice)
    }

    /// The target attached under `key`, if there is one.
    #[must_use]
    pub fn target(&self, key: IoTargetKey) -> Option<&MemoryTarget> {
        self.targets.get(&key)
    }

    /// How many submissions are queued, not yet submitted.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    /// How many completions are ready, not yet popped.
    #[must_use]
    pub fn completed(&self) -> usize {
        self.completed.len()
    }

    /// Why `submission` cannot be queued, if it cannot: what [`IoBackend::check`] answers, then the
    /// queues.
    fn refusal(&self, submission: &Submission<Vec<u8>>) -> Option<RefusalCause> {
        if let Err(cause) = self.admits(submission.target, &submission.op) {
            return Some(cause);
        }
        let submit = self.queued.len() >= self.submission_queue_size;
        let drain = self.queued.len() + self.completed.len() >= self.completion_queue_size;
        match (submit, drain) {
            (true, true) => Some(RefusalCause::SubmitAndDrainNeeded),
            (true, false) => Some(RefusalCause::SubmitNeeded),
            (false, true) => Some(RefusalCause::DrainNeeded),
            (false, false) => None,
        }
    }

    /// Why `op` on `target` cannot be queued whatever the queues hold, if it cannot.
    fn admits(&self, target: IoTargetKey, op: &SubmissionOp<Vec<u8>>) -> Result<(), RefusalCause> {
        let Some(target) = self.targets.get(&target) else {
            return Err(RefusalCause::UnknownTarget);
        };
        let shape = match op {
            SubmissionOp::Write { offset, data, .. } | SubmissionOp::Read { offset, data } => {
                Some((*offset, data))
            }
            SubmissionOp::Commit => None,
        };
        if let Some((offset, data)) = shape {
            let (address, length) = match data {
                SubmissionData::Owned(buffer) => (buffer.as_ptr() as usize, buffer.len()),
                SubmissionData::Registered(region) => {
                    if self.registered.is_empty() {
                        return Err(RefusalCause::Unsupported);
                    }
                    match self.region(*region) {
                        Some(bytes) => (bytes.as_ptr() as usize, bytes.len()),
                        None => return Err(RefusalCause::Invalid),
                    }
                }
            };
            let geometry = target.geometry;
            let fits = u32::try_from(length).is_ok()
                && usize::try_from(offset).is_ok_and(|start| start.checked_add(length).is_some());
            if !fits || !geometry.admits(offset, length as u64) || !geometry.admits_buffer(address)
            {
                return Err(RefusalCause::Invalid);
            }
        }
        Ok(())
    }

    /// The bytes `region` names, if it lies within a registered buffer.
    fn region(&self, region: RegisteredRegion) -> Option<&[u8]> {
        let start = position(region.offset);
        let end = start.checked_add(position(region.length))?;
        self.registered.get(position(region.index))?.get(start..end)
    }

    /// Execute `submission` against its target.
    fn execute(&mut self, submission: Submission<Vec<u8>>) -> Completion<Vec<u8>> {
        let Submission { token, target, op } = submission;
        let Some(target) = self.targets.get_mut(&target) else {
            let buffer = match op {
                SubmissionOp::Write {
                    data: SubmissionData::Owned(buffer),
                    ..
                }
                | SubmissionOp::Read {
                    data: SubmissionData::Owned(buffer),
                    ..
                } => Some(buffer),
                _ => None,
            };
            return Completion {
                token,
                result: Err(memory_fault("the target was detached")),
                buffer,
            };
        };
        let (result, buffer) = match op {
            SubmissionOp::Write { offset, data, .. } => {
                let offset = position_u64(offset);
                match data {
                    SubmissionData::Owned(buffer) => {
                        target.write(offset, &buffer);
                        (buffer.len(), Some(buffer))
                    }
                    SubmissionData::Registered(region) => {
                        let start = position(region.offset);
                        let bytes = &self.registered[position(region.index)]
                            [start..start + position(region.length)];
                        target.write(offset, bytes);
                        (bytes.len(), None)
                    }
                }
            }
            SubmissionOp::Read { offset, data } => {
                let offset = position_u64(offset);
                match data {
                    SubmissionData::Owned(mut buffer) => {
                        (target.read(offset, &mut buffer), Some(buffer))
                    }
                    SubmissionData::Registered(region) => {
                        let start = position(region.offset);
                        let bytes = &mut self.registered[position(region.index)]
                            [start..start + position(region.length)];
                        (target.read(offset, bytes), None)
                    }
                }
            }
            SubmissionOp::Commit => {
                target.durable.clone_from(&target.contents);
                (0, None)
            }
        };
        Completion {
            token,
            result: Ok(u32::try_from(result).expect("push refused any length beyond a u32")),
            buffer,
        }
    }
}

/// A queue size as a count.
fn size(size: NonZeroU32) -> usize {
    position(size.get())
}

/// A `u32` position or length as an index.
fn position(value: u32) -> usize {
    usize::try_from(value).expect("a u32 fits in a usize on every supported target")
}

/// A `u64` offset as an index; push refused one that does not fit.
fn position_u64(value: u64) -> usize {
    usize::try_from(value).expect("push refused an offset beyond a usize")
}

impl IoBackend for MemoryBackend {
    type Target = MemoryTarget;
    type Buffer = Vec<u8>;
    type Notification = MemoryNotification;

    fn attach(
        &mut self,
        key: IoTargetKey,
        target: MemoryTarget,
    ) -> Result<(), DuplicateTarget<MemoryTarget>> {
        if self.targets.contains_key(&key) {
            return Err(DuplicateTarget { key, target });
        }
        self.targets.insert(key, target);
        Ok(())
    }

    fn detach(&mut self, key: IoTargetKey) -> Option<MemoryTarget> {
        self.targets.remove(&key)
    }

    fn registered_buffers(&self) -> u32 {
        u32::try_from(self.registered.len()).unwrap_or(u32::MAX)
    }

    fn check(&self, target: IoTargetKey, op: &SubmissionOp<Vec<u8>>) -> Result<(), RefusalCause> {
        self.admits(target, op)
    }

    fn push(&mut self, submission: Submission<Vec<u8>>) -> Result<(), Refused<Vec<u8>>> {
        if let Some(cause) = self.refusal(&submission) {
            return Err(Refused { cause, submission });
        }
        self.queued.push_back(submission);
        Ok(())
    }

    fn submit(&mut self) -> Result<(), RefusalCause> {
        let was_empty = self.completed.is_empty();
        while let Some(submission) = self.queued.pop_front() {
            let completion = self.execute(submission);
            self.completed.push_back(completion);
        }
        if was_empty && !self.completed.is_empty() {
            self.notification.raise();
        }
        Ok(())
    }

    fn pop(&mut self) -> Result<Option<Completion<Vec<u8>>>, IoFault> {
        Ok(self.completed.pop_front())
    }

    fn notification(&self) -> Result<MemoryNotification, IoFault> {
        Ok(self.notification.clone())
    }
}
