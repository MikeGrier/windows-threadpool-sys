// Copyright (c) 2026 Mike Grier
//! The reading engine: it drains the watcher's SQ, reads what each attention announced through an
//! I/O ring into buffers from the pool, and writes each chunk to the reading layer's SQ as its read
//! completes (TW-D-15). It is driven synchronously: [`ReadEngine::run`] does what it can without
//! waiting and says why it stopped, and the next run continues from there.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the reading layer (M6) drives it")
)]

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::num::NonZeroU32;
use std::os::windows::io::{AsRawHandle, RawHandle};

use windows_ioring_sys::{Batch, IoRing, IoRingErrorExt, Op, Owned, PushOptions};
use windows_overlapped_io_sys::EndpointObserver;
use windows_sys::Win32::Foundation::ERROR_HANDLE_EOF;
use wtf_string::Wtf16Str;

use crate::pool::{BufferPool, PoolBuffer};
use crate::read_plan::{Plan, Reads, plan};
use crate::read_sq::{Data, ReadEntry, ReadSq};
use crate::sq::{Attention, PushFailure, SqEntry, Terminal};
use crate::{CreateError, Diagnostic, DiagnosticKind, HeldFile, OpenError, Registration, Watcher};

#[cfg(test)]
mod tests;

/// The top sixteen bits of an `HRESULT` made from a Win32 code: severity 1, facility 7.
const HRESULT_FROM_WIN32_PREFIX: u32 = 0x8007_0000;

/// The bits of such an `HRESULT` that carry the Win32 code.
const HRESULT_CODE_MASK: u32 = 0xFFFF;

/// The value a registration is made with in the watcher the engine owns: the caller's `T`, and
/// what a read of the file needs, taken when the file is opened so that no attention can arrive
/// without it.
#[derive(Clone, Debug)]
pub(crate) struct Tracked<T> {
    value: T,
    registration: u64,
    // The handle's value, which is a number. A read addresses it, and `keepalive` is what makes
    // that sound: the handle stays open while any clone of this does.
    file: usize,
    keepalive: EndpointObserver,
}

impl<T> Tracked<T> {
    fn new(value: T, file: &HeldFile, registration: u64) -> Self {
        let endpoint = file.endpoint_handle();
        Self {
            value,
            registration,
            file: endpoint.as_raw_handle().addr(),
            keepalive: endpoint.observer(),
        }
    }
}

/// What a read of one registration's file needs, split from the attention that carried it.
struct Target {
    registration: u64,
    file: usize,
    keepalive: EndpointObserver,
}

/// The watcher's entry as the planner takes it, and the target of the attention if it was one.
fn split<T>(entry: SqEntry<Tracked<T>>) -> (SqEntry<T>, Option<Target>) {
    match entry {
        SqEntry::Attention(Attention {
            value,
            last_known,
            current,
        }) => {
            let Tracked {
                value,
                registration,
                file,
                keepalive,
            } = value;
            (
                SqEntry::Attention(Attention {
                    value,
                    last_known,
                    current,
                }),
                Some(Target {
                    registration,
                    file,
                    keepalive,
                }),
            )
        }
        SqEntry::Terminal(Terminal {
            value,
            win32_code,
            last_known,
        }) => (
            SqEntry::Terminal(Terminal {
                value: value.value,
                win32_code,
                last_known,
            }),
            None,
        ),
        SqEntry::PollDone => (SqEntry::PollDone, None),
    }
}

/// Why the engine could not be made.
#[derive(Debug)]
pub(crate) enum EngineError {
    /// The reading layer's SQ could not be made.
    Sq(CreateError),
    /// The ring could not be made, or cannot read.
    Ring(io::Error),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sq(error) => write!(f, "the reading layer's SQ could not be made: {error}"),
            Self::Ring(error) => write!(f, "the reading engine's ring could not be made: {error}"),
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sq(error) => Some(error),
            Self::Ring(error) => Some(error),
        }
    }
}

/// Why a run stopped. Each stop is without waiting, and the next run continues from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    /// Nothing is waiting on the engine: the watcher's SQ is drained and nothing is undelivered.
    /// Reads may still be in flight, and their completions are not yet there to pop.
    Idle,
    /// A read is planned and every buffer is lent. A buffer returns when its chunk drops.
    PoolEmpty,
    /// A read is planned and the ring cannot take it: its SQ refused after a submit, or reads in
    /// flight fill its completion queue. A read completing, and being popped, makes room.
    RingFull,
    /// The reading layer's SQ is full. The client making room, and ringing back, continues it.
    ClientFull,
}

/// The reads of one poll, which is done once every one of them has been delivered or settled.
#[derive(Clone, Copy, Debug)]
struct Group {
    // Reads submitted and not yet settled.
    outstanding: usize,
    // Poll-done has been taken from the watcher: no further reads join this group.
    closed: bool,
}

/// The polls whose poll-done has not been delivered, oldest first.
#[derive(Debug, Default)]
struct Groups {
    // The id of the front group; ids only grow, so a read names its group by it.
    first: u64,
    list: VecDeque<Group>,
}

impl Groups {
    /// The group reads are being planned into, made when the newest is closed or there is none.
    fn open(&mut self) -> u64 {
        if self.list.back().is_none_or(|group| group.closed) {
            self.list.push_back(Group {
                outstanding: 0,
                closed: false,
            });
        }
        self.first + u64::try_from(self.list.len() - 1).expect("a count fits u64")
    }

    fn get(&mut self, id: u64) -> &mut Group {
        let index = usize::try_from(id - self.first).expect("a count fits usize");
        &mut self.list[index]
    }

    fn started(&mut self, id: u64) {
        self.get(id).outstanding += 1;
    }

    fn settled(&mut self, id: u64) {
        self.get(id).outstanding -= 1;
    }

    /// Poll-done has come from the watcher: close the group its reads joined, or make a closed one
    /// for a poll that had none, so its poll-done is delivered in its place.
    fn close_open(&mut self) {
        match self.list.back_mut() {
            Some(group) if !group.closed => group.closed = true,
            _ => self.list.push_back(Group {
                outstanding: 0,
                closed: true,
            }),
        }
    }

    /// Whether the oldest poll's poll-done can be delivered: it is closed and every one of its
    /// reads is settled.
    fn ready(&self) -> bool {
        self.list
            .front()
            .is_some_and(|group| group.closed && group.outstanding == 0)
    }

    fn pop_ready(&mut self) {
        debug_assert!(self.ready());
        self.list.pop_front();
        self.first += 1;
    }
}

/// What the ring holds for a read, and hands back with its completion.
struct Pending<T> {
    registration: u64,
    value: T,
    offset: u64,
    len: u32,
    group: u64,
    // Keeps the handle the read addresses open until this drops, which is when the completion is
    // popped (TW-D-10).
    _keepalive: EndpointObserver,
}

/// The attention being read: its reads are taken from the plan as they are submitted.
struct Current<T> {
    reads: Reads<T>,
    target: Target,
    group: u64,
}

/// How a pass at submitting the current plan's reads ended.
enum Submitted {
    /// Every read of the plan is submitted.
    Done,
    Blocked(Stop),
}

/// What taking the next piece of work did.
enum Advance {
    Progress,
    Blocked(Stop),
    /// The watcher's SQ is empty and no plan is current.
    Drained,
}

/// Why one batch of pushes ended.
enum Ended {
    Planned,
    Stopped(Stop),
    RingSqFull,
    Refused(io::Error),
}

/// The reading engine over a watcher: see the module documentation.
///
/// It owns the watcher, the pool, the ring and the reading layer's SQ. Files are registered
/// through [`register`](Self::register), which is what gives each attention the means to read.
pub(crate) struct ReadEngine<T: Clone> {
    ring: IoRing<PoolBuffer, Pending<T>, Owned>,
    // Reads the ring holds, queued or in flight or completed and not popped, never exceed this.
    cq_size: usize,
    sq: ReadSq<T>,
    pool: BufferPool,
    buffer_len: NonZeroU32,
    watcher: Watcher<Tracked<T>>,
    current: Option<Current<T>>,
    // The one entry the client's SQ refused. While it is held nothing else is taken, so entries
    // reach the client in the order they were made.
    undelivered: Option<ReadEntry<T>>,
    groups: Groups,
    // A push is queued in the ring that no submit has run for.
    unsubmitted: bool,
}

impl<T: Clone> ReadEngine<T> {
    /// An engine reading what `watcher` announces into buffers from `pool`, delivering on an SQ of
    /// `client_sq_capacity` entries, through a ring of the given queue sizes.
    ///
    /// # Errors
    ///
    /// [`EngineError::Sq`] for a capacity the SQ cannot have or a doorbell that cannot be made;
    /// [`EngineError::Ring`] when the ring cannot be made or cannot read.
    pub(crate) fn new(
        watcher: Watcher<Tracked<T>>,
        pool: BufferPool,
        client_sq_capacity: usize,
        ring_sq_size: u32,
        ring_cq_size: u32,
    ) -> Result<Self, EngineError> {
        let sq = ReadSq::new(client_sq_capacity).map_err(EngineError::Sq)?;
        let ring = IoRing::<PoolBuffer, Pending<T>>::with_inventory(ring_sq_size, ring_cq_size)
            .map_err(EngineError::Ring)?;
        if !ring.supports(Op::Read) {
            return Err(EngineError::Ring(io::Error::new(
                io::ErrorKind::Unsupported,
                "this ring does not support reads",
            )));
        }
        let cq_size = usize::try_from(
            ring.info()
                .map_err(EngineError::Ring)?
                .completion_queue_size,
        )
        .expect("a u32 fits usize");
        let ring = ring
            .into_owned()
            .unwrap_or_else(|_| unreachable!("a new ring has nothing in flight"));
        // The pool's lengths are 1 to `u32::MAX`, which is what a read's length is.
        let buffer_len = NonZeroU32::new(
            u32::try_from(pool.buffer_len()).expect("a pool's buffers are at most u32::MAX"),
        )
        .expect("a pool's buffers are at least one byte");
        Ok(Self {
            ring,
            cq_size,
            sq,
            pool,
            buffer_len,
            watcher,
            current: None,
            undelivered: None,
            groups: Groups::default(),
            unsubmitted: false,
        })
    }

    /// Register the file at `path`, as [`Watcher::register`] does, with what a read of it needs
    /// taken from the open file. The registration's value is a [`Tracked`], which is what the
    /// watcher is of.
    ///
    /// # Errors
    ///
    /// As [`Watcher::register`].
    pub(crate) fn register(
        &self,
        path: &Wtf16Str,
        offset: u64,
        value: T,
    ) -> Result<Registration<Tracked<T>>, OpenError> {
        self.watcher
            .register_with(path, offset, |file, id| Tracked::new(value, file, id))
    }

    /// The watcher the engine drains, for polling it and for what it reports about itself.
    pub(crate) fn watcher(&self) -> &Watcher<Tracked<T>> {
        &self.watcher
    }

    /// The reading layer's SQ: what the client drains, and the doorbells it waits and rings on.
    pub(crate) fn read_sq(&self) -> &ReadSq<T> {
        &self.sq
    }

    /// The pool the reads' buffers come from.
    pub(crate) fn pool(&self) -> &BufferPool {
        &self.pool
    }

    /// How many reads the ring holds: queued, in flight, or completed and not yet popped.
    pub(crate) fn in_flight(&self) -> usize {
        self.ring.outstanding()
    }

    /// Do everything that can be done without waiting, and say why it stopped.
    ///
    /// The resume doorbell and the pool's are reset first, so a ring that lands while this runs
    /// is not lost (TW-D-6). The reading layer's doorbell is rung once at the end if entries were
    /// written, or if the stop is a full SQ.
    ///
    /// # Errors
    ///
    /// An error from the ring other than a refused push the engine answers itself. State is
    /// consistent after one, and a later run continues.
    pub(crate) fn run(&mut self) -> io::Result<Stop> {
        self.sq.reset_resume();
        self.pool.reset_doorbell();
        let mut wrote = false;
        let stop = self.run_until_stopped(&mut wrote);
        if wrote || matches!(stop, Ok(Stop::ClientFull)) {
            self.sq.ring_doorbell();
        }
        stop
    }

    /// Block until a read has completed, or `timeout_ms` has passed, for a synchronous driver with
    /// nothing else to do. Returns at once when the ring holds nothing.
    ///
    /// # Errors
    ///
    /// Any error from the ring's submit.
    pub(crate) fn wait_for_completion(&mut self, timeout_ms: u32) -> io::Result<()> {
        if self.ring.outstanding() == 0 {
            return Ok(());
        }
        Batch::new(&mut self.ring).submit_and_wait(1, timeout_ms)?;
        self.unsubmitted = false;
        Ok(())
    }

    fn run_until_stopped(&mut self, wrote: &mut bool) -> io::Result<Stop> {
        self.submit_queued()?;
        loop {
            if !self.deliver(wrote) {
                return Ok(Stop::ClientFull);
            }
            if self.settle_completion()? {
                continue;
            }
            let stop = match self.advance()? {
                Advance::Progress => continue,
                Advance::Blocked(stop) => stop,
                Advance::Drained => Stop::Idle,
            };
            // What the stop waited for may have completed while this ran.
            if self.settle_completion()? {
                continue;
            }
            return Ok(stop);
        }
    }

    /// Write what is waiting for the client: the entry its SQ refused, then the poll-done of every
    /// poll whose reads are all delivered. Returns whether it all went; a refusal stops it.
    fn deliver(&mut self, wrote: &mut bool) -> bool {
        if let Some(entry) = self.undelivered.take() {
            match self.sq.push(entry) {
                Ok(()) => *wrote = true,
                Err(PushFailure::Full(entry)) => {
                    self.undelivered = Some(entry);
                    return false;
                }
                Err(PushFailure::Closed(_)) => unreachable!("the engine holds the SQ's reader"),
            }
        }
        while self.groups.ready() {
            match self.sq.push(ReadEntry::PollDone) {
                Ok(()) => {
                    self.groups.pop_ready();
                    *wrote = true;
                }
                Err(PushFailure::Full(_)) => return false,
                Err(PushFailure::Closed(_)) => unreachable!("the engine holds the SQ's reader"),
            }
        }
        true
    }

    /// Pop one completion, if there is one, and settle its read. Returns whether there was one.
    fn settle_completion(&mut self) -> io::Result<bool> {
        let Some((completion, buffer, pending)) = self.ring.try_pop()? else {
            return Ok(false);
        };
        let buffer = buffer.expect("a read holds its buffer");
        self.finish_read(buffer, pending, completion.result());
        Ok(true)
    }

    /// Settle a read from what it returned. Bytes become the one entry awaiting delivery; a read
    /// that returned none, or failed, returns its buffer and delivers nothing.
    ///
    /// A short read is delivered as the bytes it read, labelled with that range: the rest of what
    /// was announced is not read again, since a file that shrank under the read is the next poll's
    /// shrink. End of file is not a failure and reads nothing. Any other failure is reported to the
    /// watcher's diagnostic sink, which is the one place a client can learn that bytes it was told
    /// of will not arrive.
    fn finish_read(&mut self, buffer: PoolBuffer, pending: Pending<T>, result: io::Result<usize>) {
        assert!(
            self.undelivered.is_none(),
            "a completion is settled only once the entry before it is delivered"
        );
        self.groups.settled(pending.group);
        let read = match result {
            Ok(read) => read,
            Err(error) => match win32_code(&error) {
                Some(ERROR_HANDLE_EOF) => 0,
                code => {
                    self.watcher
                        .diagnose(DiagnosticKind::ReadFailed, || Diagnostic::ReadFailed {
                            registration: pending.registration,
                            offset: pending.offset,
                            len: pending.len,
                            win32_code: code,
                        });
                    return;
                }
            },
        };
        if read == 0 {
            return;
        }
        self.undelivered = Some(ReadEntry::Data(Data::new(
            pending.value,
            pending.offset,
            buffer.into_chunk(read),
        )));
    }

    /// Take the next piece of work: submit the current plan's reads, or take the next entry from
    /// the watcher and plan it.
    fn advance(&mut self) -> io::Result<Advance> {
        if self.current.is_some() {
            return Ok(match self.submit_reads()? {
                Submitted::Done => {
                    self.current = None;
                    Advance::Progress
                }
                Submitted::Blocked(stop) => Advance::Blocked(stop),
            });
        }
        let Some(entry) = self.watcher.drain().next() else {
            return Ok(Advance::Drained);
        };
        let (entry, target) = split(entry);
        match plan(entry, self.buffer_len) {
            Plan::Reads(reads) => {
                let target = target.expect("an attention carries its target");
                let group = self.groups.open();
                self.current = Some(Current {
                    reads,
                    target,
                    group,
                });
            }
            Plan::Deliver(entry) => self.undelivered = Some(entry),
            Plan::PollDone => self.groups.close_open(),
        }
        Ok(Advance::Progress)
    }

    /// Submit the current plan's reads until it is done or something stops it, a batch at a time:
    /// the ring's SQ is drained by a submit, so one that refuses a push is submitted and the push
    /// goes on, and only a ring that refuses with nothing queued stops it.
    fn submit_reads(&mut self) -> io::Result<Submitted> {
        let Self {
            ring,
            pool,
            current,
            groups,
            unsubmitted,
            cq_size,
            ..
        } = self;
        let current = current.as_mut().expect("reads are planned");
        loop {
            // Everything the ring holds counts, so the completion queue cannot overflow.
            let mut outstanding = ring.outstanding();
            let mut batch = Batch::new(ring);
            let mut pushed = 0_usize;
            let ended = loop {
                let Some(read) = current.reads.front() else {
                    break Ended::Planned;
                };
                if outstanding >= *cq_size {
                    break Ended::Stopped(Stop::RingFull);
                }
                let Some(mut buffer) = pool.take() else {
                    break Ended::Stopped(Stop::PoolEmpty);
                };
                buffer.limit_to(usize::try_from(read.len).expect("a u32 fits usize"));
                let extra = Pending {
                    registration: current.target.registration,
                    value: current.reads.value().clone(),
                    offset: read.offset,
                    len: read.len,
                    group: current.group,
                    _keepalive: current.target.keepalive.clone(),
                };
                let file: RawHandle = std::ptr::without_provenance_mut(current.target.file);
                // SAFETY: `file` is an open handle, opened read-only and overlapped, that stays
                // open until the completion is popped: `extra`, which the ring holds until then,
                // holds an observer of the same handle (`Tracked::new` took the two from one
                // `HeldFile`), and a ring that cannot run down leaks what it holds rather than
                // freeing it.
                let pushed_read = unsafe {
                    batch.read_raw_owned(file, buffer, extra, read.offset, PushOptions::new())
                };
                match pushed_read {
                    Ok(_) => {
                        current.reads.next();
                        groups.started(current.group);
                        outstanding += 1;
                        pushed += 1;
                    }
                    // The buffer and the sidecar come back in the refusal and are dropped, the
                    // buffer to the pool.
                    Err(refused) if refused.error.is_submission_queue_full() => {
                        break Ended::RingSqFull;
                    }
                    Err(refused) => break Ended::Refused(refused.into_error()),
                }
            };
            if pushed == 0 {
                batch.leave_queued();
            } else {
                *unsubmitted = true;
                batch.submit()?;
                *unsubmitted = false;
            }
            match ended {
                Ended::Planned => return Ok(Submitted::Done),
                Ended::Stopped(stop) => return Ok(Submitted::Blocked(stop)),
                // The submit made room.
                Ended::RingSqFull if pushed > 0 => {}
                Ended::RingSqFull => return Ok(Submitted::Blocked(Stop::RingFull)),
                Ended::Refused(error) => return Err(error),
            }
        }
    }

    /// Run a submit that an earlier one failed, so what it left queued is not forgotten. Untested:
    /// `SubmitIoRing` cannot be made to fail on demand, so no test reaches the failure this answers.
    fn submit_queued(&mut self) -> io::Result<()> {
        if self.unsubmitted {
            Batch::new(&mut self.ring).submit()?;
            self.unsubmitted = false;
        }
        Ok(())
    }
}

/// The Win32 code an error carries: its own, or the one inside the `HRESULT` the ring reported.
fn win32_code(error: &io::Error) -> Option<u32> {
    if let Some(code) = error.raw_os_error() {
        return Some(code.cast_unsigned());
    }
    let hresult = error.as_ioring_error()?.code().cast_unsigned();
    (hresult & !HRESULT_CODE_MASK == HRESULT_FROM_WIN32_PREFIX)
        .then_some(hresult & HRESULT_CODE_MASK)
}
