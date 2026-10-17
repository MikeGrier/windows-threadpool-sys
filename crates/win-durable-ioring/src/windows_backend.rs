// Copyright (c) 2026 Mike Grier
//! The Windows backend (DI-D-50 P10, DI-D-52): [`WindowsBackend`], an [`IoBackend`] over
//! `windows-ioring-sys`' `IoRing`, and [`Win32FileTarget`], a target backed by a Win32 file.
//!
//! **Threadless.** The backend has no threads of its own. Its notification is the ring's
//! completion event; who waits on it and drains is the driver's delivery (DI-D-52).
//!
//! **Queued at push, issued at the doorbell.** A push leaves its entry in the ring's submission
//! queue (a batch left queued, the ring crate's D-90, not submitted as it drops, its D-5), and
//! [`IoBackend::submit`] issues what is queued and reports why it could not: the engine rings it
//! once after the pushes it makes, so a batch of pushes is one submission and a failure to submit
//! is seen where the doorbell is rung (DI-7+.2).
//!
//! **Files, volumes and disks.** A [`Win32FileTarget`] is made from an [`OpenedFile`], a file this
//! crate opened and so knows the buffering mode of, from an [`OpenedVolume`], a raw volume it opened
//! and found the disks and sector size of, or from an [`OpenedDisk`], a whole disk it opened by
//! number, and from nothing else (DI-D-60, DI-D-61, DI-D-62).
//!
//! **Targets.** A target given at construction is registered with the ring, which holds it for the
//! ring's life (`windows-ioring-sys` D-81) and addresses it by index; one attached later is
//! addressed through its `SharedHandle`, guarded per operation. Registered buffers are given at
//! construction too.
//!
//! **Refusals** (DI-D-52), of a push or of a submission, carry the [`RefusalCause`] the ring's
//! report maps to; and a write the ring completes short completes with a fault instead, under the
//! `Fault` policy (DI-D-50).

use std::collections::HashMap;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::sync::Arc;
use std::time::Duration;

use win_sync_sys::{Access, duplicate_handle_with_access};
use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, IoBuf, IoBufMut, IoRing, IoRingErrorExt, OperationId, Owned,
    PushOptions, PushRefused, RegisteredBuffers, RegisteredFile, RegisteredSpan, RingCondition,
    SharedHandle,
};

use crate::dioring::{register_buffers, register_files};
use crate::error_code::{WINDOWS_IORING, windows_fault};
use durable_ioring::IoTarget;
use durable_ioring::backend::{
    Completion, DuplicateTarget, IoBackend, RefusalCause, Refused, RegisteredRegion, Submission,
    SubmissionData, SubmissionOp, SubmissionToken,
};
use durable_ioring::{IoFault, IoTargetKey, WriteCaching};

mod file_rights;
mod file_target;
mod opened_disk;
mod opened_file;
mod opened_volume;
pub(crate) mod pool;
mod share_mode;
pub(crate) mod signalled;
mod wait;

pub use wait::CompletionEvent;

// The markers are the core's (DI-D-66); this module states what each asks of Win32.
pub use durable_ioring::{
    CanRead, CanWrite, DataAccess, HasData, NoData, ReadOnly, ReadWrite, WriteOnly,
};
pub use file_rights::FileRights;
pub use file_target::Win32FileTarget;
pub use opened_disk::{DiskPath, OpenedDisk};
pub use opened_file::{
    Buffering, Cached, FileOptions, OpenedFile, RemoteDevice, SharingViolation, Unbuffered,
};
pub use opened_volume::{
    OpenedVolume, VolumeOptions, VolumePath, VolumePathError, disk_flush_domain,
};
pub use share_mode::ShareMode;

#[cfg(test)]
mod tests;

/// A completion's count, on the Windows path. A target completes a write in full or with a fault
/// (DI-D-50), and the I/O ring does not complete a file or volume write short, so a short count is
/// a fault here -- the `Fault` policy -- with no code, since no layer gave one. A read may complete
/// short.
pub(crate) fn transfer(
    result: io::Result<usize>,
    write: bool,
    requested: u32,
) -> Result<u32, IoFault> {
    match result {
        // A transfer is never longer than the `u32` length it was pushed with.
        Ok(transferred) => {
            let transferred = u32::try_from(transferred).unwrap_or(u32::MAX);
            if write && transferred < requested {
                Err(IoFault {
                    source: WINDOWS_IORING,
                    code: None,
                    error: Arc::new(io::Error::other(format!(
                        "the write completed short: {transferred} of {requested} bytes"
                    ))),
                })
            } else {
                Ok(transferred)
            }
        }
        Err(error) => Err(windows_fault(error)),
    }
}

/// Why the ring refused a push or a submission, as the caller acts on it (DI-D-52). A full
/// submission queue needs a submit; a full completion queue a drain; a submission already running,
/// a retry; a corrupt ring is unusable. The ring's own rejections carry an `io::ErrorKind`: an
/// operation it does not support is `Unsupported`, and a malformed one -- a buffer longer than a
/// `u32`, a registration of another ring -- is `Invalid`. Anything else is `Unknown`, with the
/// report.
pub(crate) fn refusal_cause(error: io::Error) -> RefusalCause {
    match error.ring_condition() {
        Some(RingCondition::SubmissionQueueFull) => RefusalCause::SubmitNeeded,
        Some(RingCondition::CompletionQueueTooFull) => RefusalCause::DrainNeeded,
        Some(RingCondition::SubmitInProgress) => RefusalCause::Busy,
        Some(RingCondition::Corrupt) => RefusalCause::Unusable(windows_fault(error)),
        Some(_) => RefusalCause::Unknown(windows_fault(error)),
        None => match error.kind() {
            io::ErrorKind::Unsupported => RefusalCause::Unsupported,
            io::ErrorKind::InvalidInput => RefusalCause::Invalid,
            _ => RefusalCause::Unknown(windows_fault(error)),
        },
    }
}

/// What the ring carries for each operation, and hands back with its completion.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sidecar {
    token: SubmissionToken,
    target: IoTargetKey,
    kind: Kind,
}

/// What an operation was, as its completion is read.
#[derive(Clone, Copy, Debug)]
enum Kind {
    /// A write of this many bytes, which completes in full or with a fault.
    Write(u32),
    Read,
    Commit,
}

/// A target and how the ring addresses it.
struct Slot {
    target: Win32FileTarget,
    /// Its registration, for a target given at construction.
    registered: Option<RegisteredFile>,
}

/// A test hook called with a ring as soon as it exists (DI-7.2).
#[cfg(test)]
pub(crate) type RingHook<B> = Box<dyn FnOnce(&IoRing<B, Sidecar>)>;

/// What construction needs beyond [`WindowsBackendSetup`], passed down explicitly from
/// [`Dioring::with_construction`](crate::dioring::Dioring::with_construction) (DI-7.2): never a
/// thread-local or a global, since the ring is created and registered before its consumer can
/// name it, and a responder installed by thread would miss a pool thread's calls (the ring
/// crate's D-85).
pub(crate) struct Construction<B> {
    /// How long construction waits for each registration's completion.
    pub(crate) registration_bound: Duration,
    /// Called with the ring as soon as it exists, before anything is registered on it: a test
    /// registers a responder here, which is the only moment that reaches the registration calls.
    #[cfg(test)]
    pub(crate) on_ring: Option<RingHook<B>>,
    /// `B` is named only by the test hook.
    ring: std::marker::PhantomData<fn(B)>,
}

impl<B> Default for Construction<B> {
    /// What every instance outside a test is built with.
    fn default() -> Self {
        Self {
            registration_bound: crate::dioring::REGISTRATION_BOUND,
            #[cfg(test)]
            on_ring: None,
            ring: std::marker::PhantomData,
        }
    }
}

#[cfg(test)]
impl<B> Construction<B> {
    /// A construction whose ring answers through the ring crate's safe call-fault responder,
    /// registered as the ring is created, waiting `registration_bound` for each registration. The
    /// control arms it from any thread; nothing is armed to begin with.
    pub(crate) fn faulted(
        registration_bound: Duration,
    ) -> (Self, windows_ioring_sys::sys::CallFaultControl) {
        let (faults, control) = windows_ioring_sys::sys::call_faults();
        let on_ring: RingHook<B> = Box::new(move |ring| {
            let _ = windows_ioring_sys::sys::register(ring, Box::new(faults));
        });
        let construction = Self {
            registration_bound,
            on_ring: Some(on_ring),
            ring: std::marker::PhantomData,
        };
        (construction, control)
    }
}

/// What [`WindowsBackend::new`] takes.
pub struct WindowsBackendSetup<R> {
    /// Submission queue entries.
    pub submission_queue_size: u32,
    /// Completion queue entries.
    pub completion_queue_size: u32,
    /// Targets registered with the ring, under their keys, in order.
    pub targets: Vec<(IoTargetKey, Win32FileTarget)>,
    /// Buffers registered with the ring, in order; a [`RegisteredRegion`] names one by position.
    pub buffers: Vec<R>,
}

/// Why [`WindowsBackend::new`] refused.
#[derive(Debug)]
pub enum WindowsBackendRefusal {
    /// Two targets share a key.
    DuplicateKey(IoTargetKey),
    /// Creating the ring, registering, or attaching its completion event failed.
    Ring(io::Error),
}

/// A refused construction, handing back what it was given: the targets always, and the buffers
/// unless the kernel was given them and could not be shown to have given them back.
pub struct WindowsBackendError<R> {
    /// Why.
    pub reason: WindowsBackendRefusal,
    /// The targets.
    pub targets: Vec<(IoTargetKey, Win32FileTarget)>,
    /// The buffers, or `None`.
    pub buffers: Option<Vec<R>>,
}

impl<R> std::fmt::Debug for WindowsBackendError<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsBackendError")
            .field("reason", &self.reason)
            .field("targets", &self.targets.len())
            .field("buffers", &self.buffers.as_ref().map(Vec::len))
            .finish()
    }
}

/// The Windows backend: an `IoRing` and the targets it runs I/O for. `B` is the owned buffer type
/// and `R` the registered buffers'.
///
/// **Write-only buffer types** (DI-D-17): an owned write needs only `B: IoBuf`, so a type such as `Arc<[u8]>` can be written. An owned read
/// needs `B: IoBufMut`, and is pushed through a push made where that bound holds: [`new`](Self::new)
/// makes it, [`without_owned_reads`](Self::without_owned_reads) does not, and an owned read on a
/// backend without it is refused as [`RefusalCause::Unsupported`].
pub struct WindowsBackend<B: IoBuf, R: IoBufMut = Vec<u8>> {
    /// Declared before `registered`: dropping the ring runs down every operation, so it ends before
    /// the buffers the kernel may still be using.
    ring: IoRing<B, Sidecar, Owned>,
    registered: Option<RegisteredBuffers<R>>,
    targets: HashMap<IoTargetKey, Slot>,
    /// How an owned read is pushed, made where `B: IoBufMut` holds; `None` refuses owned reads.
    owned_read: Option<OwnedRead<B>>,
    /// The ring's completion event, duplicated for each notification handed out.
    event: OwnedHandle,
    /// Something is queued that no submit has issued: pushed since the last one that succeeded.
    queued: bool,
    /// Failures armed by the fault seam: the next commit of the target to complete reports the
    /// Win32 error instead of what the kernel did.
    #[cfg(feature = "fault-injection")]
    injected: Vec<(IoTargetKey, u32)>,
}

impl<B: IoBufMut, R: IoBufMut> WindowsBackend<B, R> {
    /// A ring with `setup`'s queue sizes, its targets and buffers registered, and its completion
    /// event attached, which reads into owned buffers as well as writing from them.
    ///
    /// # Errors
    ///
    /// [`WindowsBackendError`], handing back the targets and, where it can, the buffers.
    pub fn new(setup: WindowsBackendSetup<R>) -> Result<Self, WindowsBackendError<R>> {
        let mut backend = Self::without_owned_reads(setup)?;
        backend.enable_owned_reads();
        Ok(backend)
    }

    /// Read into owned buffers from now on, through a push made here, where `B: IoBufMut` holds.
    pub(crate) fn enable_owned_reads(&mut self) {
        self.owned_read = Some(push_owned_read::<B>);
    }
}

impl<B: IoBuf, R: IoBufMut> WindowsBackend<B, R> {
    /// As [`new`](WindowsBackend::new), for a buffer type that may be only `IoBuf`: it writes from
    /// owned buffers and reads into registered ones, and refuses an owned read as
    /// [`RefusalCause::Unsupported`] (DI-D-17).
    ///
    /// # Errors
    ///
    /// [`WindowsBackendError`], handing back the targets and, where it can, the buffers.
    pub fn without_owned_reads(
        setup: WindowsBackendSetup<R>,
    ) -> Result<Self, WindowsBackendError<R>> {
        Self::construct(setup, Construction::default())
    }

    /// [`without_owned_reads`](Self::without_owned_reads), as `construction` says (DI-7.2).
    ///
    /// # Errors
    ///
    /// As [`without_owned_reads`](Self::without_owned_reads).
    pub(crate) fn construct(
        setup: WindowsBackendSetup<R>,
        construction: Construction<B>,
    ) -> Result<Self, WindowsBackendError<R>> {
        let Construction {
            registration_bound,
            #[cfg(test)]
            on_ring,
            ..
        } = construction;
        let WindowsBackendSetup {
            submission_queue_size,
            completion_queue_size,
            targets,
            buffers,
        } = setup;
        if let Some(key) = durable_ioring::driver::first_repeat(targets.iter().map(|(key, _)| *key))
        {
            return Err(WindowsBackendError {
                reason: WindowsBackendRefusal::DuplicateKey(key),
                targets,
                buffers: Some(buffers),
            });
        }
        let fail = |error, targets, buffers| WindowsBackendError {
            reason: WindowsBackendRefusal::Ring(error),
            targets,
            buffers,
        };
        let mut ring = match IoRing::with_inventory(submission_queue_size, completion_queue_size) {
            Ok(ring) => ring,
            Err(error) => return Err(fail(error, targets, Some(buffers))),
        };
        #[cfg(test)]
        if let Some(on_ring) = on_ring {
            on_ring(&ring);
        }
        let handles = targets
            .iter()
            .map(|(_, target)| target.handle().clone())
            .collect();
        let files = match register_files(&mut ring, handles, registration_bound) {
            Ok(files) => files,
            Err(error) => return Err(fail(error, targets, Some(buffers))),
        };
        let registered = match register_buffers(&mut ring, buffers, registration_bound) {
            Ok(registered) => registered,
            Err((error, buffers)) => return Err(fail(error, targets, buffers)),
        };
        let event = match ring.completion_event() {
            Ok(event) => event,
            Err(error) => return Err(fail(error, targets, None)),
        };
        // Registrations were the last operations queued that the ring holds nothing for, and each
        // was waited on, so none is in flight. If one is, this crate left it: an inconsistency, not
        // an outcome to report (DI-D-59).
        let ring = ring.into_owned().unwrap_or_else(|_| {
            panic!(
                "a registration was left in flight on the backend's ring: it cannot be sealed \
                 against operations the ring holds nothing for"
            )
        });
        let slots = targets
            .into_iter()
            .enumerate()
            .map(|(i, (key, target))| {
                let index = files
                    .as_ref()
                    .and_then(|files| files.get(u32::try_from(i).ok()?))
                    .expect("every target given at construction was registered, in order");
                (
                    key,
                    Slot {
                        target,
                        registered: Some(index),
                    },
                )
            })
            .collect();
        Ok(Self {
            ring,
            registered,
            targets: slots,
            owned_read: None,
            event,
            queued: false,
            #[cfg(feature = "fault-injection")]
            injected: Vec::new(),
        })
    }

    /// A thread-pool wake over this backend's ring (DI-D-53): `on_wake` is called on a pool thread
    /// whenever the ring's completions may have gone from none to some, and once at setup, and
    /// must drain this backend to empty each time. `env` binds the pool; `None` is the process's
    /// default. The ring crate's `CompletionWake` is the protocol; this backend only lends it the
    /// ring.
    ///
    /// # Errors
    ///
    /// As `CompletionWake::new`: the system does not support a ring completion event, or the
    /// wait could not be made or armed.
    pub(crate) fn wake(
        &mut self,
        on_wake: impl Fn() + Send + Sync + 'static,
        env: Option<&mut windows_threadpool_sys::callback_env::CallbackEnviron<'_>>,
    ) -> io::Result<windows_ioring_sys::CompletionWake> {
        windows_ioring_sys::CompletionWake::new(&mut self.ring, on_wake, env)
    }

    /// A handle to the ring's completion event that can wait on it and do nothing else: a
    /// duplicate with `SYNCHRONIZE` access alone, so the kernel refuses a set or a reset of it
    /// (DI-7+.1).
    ///
    /// # Errors
    ///
    /// The duplication's: the process cannot get another handle.
    pub(crate) fn completion_wait_handle(&self) -> io::Result<OwnedHandle> {
        duplicate_handle_with_access(&self.event, Access::SYNCHRONIZE)
    }

    /// Fail this backend's ring's submits on demand, whichever thread makes them (DI-7.1): a
    /// responder is registered for the ring, which its drop ends after the rundown, so the fault
    /// is cleared before the backend drops with an operation outstanding. Once per backend.
    #[cfg(test)]
    pub(crate) fn submit_faults(&self) -> windows_ioring_sys::sys::SubmitFaultControl {
        let (faults, control) = windows_ioring_sys::sys::submit_faults();
        let _ = windows_ioring_sys::sys::register(&self.ring, Box::new(faults));
        control
    }

    /// Fail any call of this backend's ring's the ring crate's seam carries, on demand (DI-7+.2):
    /// a pop that fails, for one. A ring has one responder, so this is instead of
    /// [`submit_faults`](Self::submit_faults) and not beside it.
    #[cfg(test)]
    pub(crate) fn call_faults(&self) -> windows_ioring_sys::sys::CallFaultControl {
        let (faults, control) = windows_ioring_sys::sys::call_faults();
        let _ = windows_ioring_sys::sys::register(&self.ring, Box::new(faults));
        control
    }

    /// The target under `key`, if there is one.
    #[must_use]
    pub fn target(&self, key: IoTargetKey) -> Option<&Win32FileTarget> {
        self.targets.get(&key).map(|slot| &slot.target)
    }

    /// The position of `key`'s target in the ring's file registration, for a target given at
    /// construction; `None` for one attached later, or no target.
    #[cfg(test)]
    pub(crate) fn registration_index(&self, key: IoTargetKey) -> Option<u32> {
        self.targets
            .get(&key)
            .and_then(|slot| slot.registered)
            .map(RegisteredFile::index)
    }

    /// Registered buffer `index`'s bytes.
    ///
    /// # Errors
    ///
    /// For a backend built without registered buffers, or an index out of range.
    pub fn registered_buffer(&mut self, index: u32) -> io::Result<&[u8]> {
        self.registration()?.get(index)
    }

    /// Registered buffer `index`'s bytes, writable.
    ///
    /// # Errors
    ///
    /// As [`registered_buffer`](Self::registered_buffer).
    pub fn registered_buffer_mut(&mut self, index: u32) -> io::Result<&mut [u8]> {
        self.registration()?.get_mut(index)
    }

    fn registration(&mut self) -> io::Result<&mut RegisteredBuffers<R>> {
        self.registered.as_mut().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "this backend was built without registered buffers",
            )
        })
    }

    /// Why `op` on `target` cannot be pushed, before the ring is asked: an unknown target, an
    /// operation its access does not cover (DI-D-66), an owned
    /// read on a backend without owned reads (DI-D-17), a registered region without a registration,
    /// or an operation its target's geometry does not admit. A registered region is checked for offset and length only; its buffer's address is
    /// the registration's, and whether it lies within the registration is the ring's own check at
    /// push, which [`IoBackend::check`] asks without pushing.
    fn admits(&self, target: IoTargetKey, op: &SubmissionOp<B>) -> Result<(), RefusalCause> {
        let Some(slot) = self.targets.get(&target) else {
            return Err(RefusalCause::UnknownTarget);
        };
        // What the target can do with its data, before the shape of the operation is looked at.
        let access = slot.target.access();
        let covered = match op {
            SubmissionOp::Write { .. } => access.can_write(),
            SubmissionOp::Read { .. } => access.can_read(),
            SubmissionOp::Commit => true,
        };
        if !covered {
            return Err(RefusalCause::AccessDenied);
        }
        if self.owned_read.is_none()
            && matches!(
                op,
                SubmissionOp::Read {
                    data: SubmissionData::Owned(_),
                    ..
                }
            )
        {
            return Err(RefusalCause::Unsupported);
        }
        let (offset, data) = match op {
            SubmissionOp::Write { offset, data, .. } | SubmissionOp::Read { offset, data } => {
                (*offset, data)
            }
            SubmissionOp::Commit => return Ok(()),
        };
        let geometry = slot.target.geometry();
        let admitted = match data {
            SubmissionData::Owned(buffer) => {
                let length = u64::try_from(buffer.bytes_len()).unwrap_or(u64::MAX);
                // The ring's own limit, which a push would refuse beyond only after the operation
                // had been held for its gate: asked here, it is refused at the call that gave it.
                u32::try_from(length).is_ok()
                    && geometry.admits(offset, length)
                    && geometry.admits_buffer(buffer.stable_ptr() as usize)
            }
            SubmissionData::Registered(region) => {
                if self.registered.is_none() {
                    return Err(RefusalCause::Unsupported);
                }
                geometry.admits(offset, u64::from(region.length))
            }
        };
        if admitted {
            Ok(())
        } else {
            Err(RefusalCause::Invalid)
        }
    }
}

/// Push one operation against `file`, a registered index or a shared handle, which `via` also
/// names for an owned read's push.
macro_rules! push_to {
    ($batch:expr, $file:expr, $via:expr, $read:expr, $registered:expr, $op:expr, $sidecar:expr) => {{
        let options = PushOptions::new();
        match $op {
            Op::Write {
                offset,
                data: SubmissionData::Owned(buffer),
                caching,
            } => $batch.write_owned($file, buffer, $sidecar, offset, options, caching),
            Op::Write {
                offset,
                data: SubmissionData::Registered(region),
                caching,
            } => $batch.write_registered_owned(
                $file,
                $registered.expect("refusal() checked for a registration"),
                span(region),
                $sidecar,
                offset,
                options,
                caching,
            ),
            Op::Read {
                offset,
                data: SubmissionData::Owned(buffer),
            } => $read.expect("admits() checked for an owned read")(
                &mut $batch,
                $via,
                buffer,
                $sidecar,
                offset,
            ),
            Op::Read {
                offset,
                data: SubmissionData::Registered(region),
            } => $batch.read_registered_owned(
                $file,
                $registered.expect("refusal() checked for a registration"),
                span(region),
                $sidecar,
                offset,
                options,
            ),
            Op::Commit => $batch.flush_owned(
                $file,
                $sidecar,
                FlushCoverage::Unordered,
                FlushMode::Default,
            ),
        }
    }};
}

/// What a push is made against, borrowed from a target's slot.
#[derive(Clone, Copy)]
enum Via<'a> {
    Registered(&'a RegisteredFile),
    Shared(&'a SharedHandle),
}

/// An owned read's push, made where `B: IoBufMut` holds and carried by a backend whose `B` may be
/// only `IoBuf` (DI-D-17).
type OwnedRead<B> = for<'r, 'v> fn(
    &mut Batch<'r, B, Sidecar, Owned>,
    Via<'v>,
    B,
    Sidecar,
    u64,
) -> Result<OperationId, PushRefused<B, Sidecar>>;

/// Push an owned read: the push [`WindowsBackend::new`] makes.
fn push_owned_read<B: IoBufMut>(
    batch: &mut Batch<'_, B, Sidecar, Owned>,
    via: Via<'_>,
    buffer: B,
    sidecar: Sidecar,
    offset: u64,
) -> Result<OperationId, PushRefused<B, Sidecar>> {
    let options = PushOptions::new();
    match via {
        Via::Registered(index) => batch.read_owned(index, buffer, sidecar, offset, options),
        Via::Shared(handle) => batch.read_owned(handle, buffer, sidecar, offset, options),
    }
}

/// An operation in the ring crate's caching terms.
enum Op<B> {
    Write {
        offset: u64,
        data: SubmissionData<B>,
        caching: windows_ioring_sys::WriteCaching,
    },
    Read {
        offset: u64,
        data: SubmissionData<B>,
    },
    Commit,
}

/// A region as the ring crate names it.
fn span(region: RegisteredRegion) -> RegisteredSpan {
    RegisteredSpan {
        buffer_index: region.index,
        offset: region.offset,
        len: region.length,
    }
}

/// The contract's caching choice, as the ring crate spells it.
fn ring_caching(caching: WriteCaching) -> windows_ioring_sys::WriteCaching {
    match caching {
        WriteCaching::Cached => windows_ioring_sys::WriteCaching::Cached,
        WriteCaching::WriteThrough => windows_ioring_sys::WriteCaching::WriteThrough,
    }
}

/// The submission a refused push was built from, with what the ring handed back.
fn rebuilt<B>(
    token: SubmissionToken,
    target: IoTargetKey,
    shape: Shape,
    payload: Option<B>,
) -> Submission<B> {
    let data = |region| match (payload, region) {
        (Some(buffer), _) => SubmissionData::Owned(buffer),
        (None, Some(region)) => SubmissionData::Registered(region),
        (None, None) => unreachable!("a refused owned push hands its buffer back"),
    };
    let op = match shape {
        Shape::Write {
            offset,
            region,
            caching,
        } => SubmissionOp::Write {
            offset,
            data: data(region),
            caching,
        },
        Shape::Read { offset, region } => SubmissionOp::Read {
            offset,
            data: data(region),
        },
        Shape::Commit => SubmissionOp::Commit,
    };
    Submission { token, target, op }
}

/// What a submission was, without its buffer, kept to rebuild it if the ring refuses it.
#[derive(Clone, Copy)]
enum Shape {
    Write {
        offset: u64,
        region: Option<RegisteredRegion>,
        caching: WriteCaching,
    },
    Read {
        offset: u64,
        region: Option<RegisteredRegion>,
    },
    Commit,
}

impl Shape {
    fn of<B>(op: &SubmissionOp<B>) -> Self {
        let region = |data: &SubmissionData<B>| match data {
            SubmissionData::Owned(_) => None,
            SubmissionData::Registered(region) => Some(*region),
        };
        match op {
            SubmissionOp::Write {
                offset,
                data,
                caching,
            } => Shape::Write {
                offset: *offset,
                region: region(data),
                caching: *caching,
            },
            SubmissionOp::Read { offset, data } => Shape::Read {
                offset: *offset,
                region: region(data),
            },
            SubmissionOp::Commit => Shape::Commit,
        }
    }
}

/// What an operation's completion is read as.
fn kind<B: IoBuf>(op: &SubmissionOp<B>) -> Kind {
    match op {
        SubmissionOp::Write { data, .. } => Kind::Write(match data {
            SubmissionData::Owned(buffer) => u32::try_from(buffer.bytes_len()).unwrap_or(u32::MAX),
            SubmissionData::Registered(region) => region.length,
        }),
        SubmissionOp::Read { .. } => Kind::Read,
        SubmissionOp::Commit => Kind::Commit,
    }
}

impl<B: IoBuf, R: IoBufMut> IoBackend for WindowsBackend<B, R> {
    type Target = Win32FileTarget;
    type Buffer = B;
    type Notification = CompletionEvent;

    fn attach(
        &mut self,
        key: IoTargetKey,
        target: Win32FileTarget,
    ) -> Result<(), DuplicateTarget<Win32FileTarget>> {
        if self.targets.contains_key(&key) {
            return Err(DuplicateTarget { key, target });
        }
        self.targets.insert(
            key,
            Slot {
                target,
                registered: None,
            },
        );
        Ok(())
    }

    fn detach(&mut self, key: IoTargetKey) -> Option<Win32FileTarget> {
        self.targets.remove(&key).map(|slot| slot.target)
    }

    fn registered_buffers(&self) -> u32 {
        self.registered.as_ref().map_or(0, RegisteredBuffers::len)
    }

    fn check(&self, target: IoTargetKey, op: &SubmissionOp<B>) -> Result<(), RefusalCause> {
        self.admits(target, op)?;
        // The span check the ring makes at push, asked without pushing (its M34.1), and read as a
        // push's refusal is.
        let region = match op {
            SubmissionOp::Write {
                data: SubmissionData::Registered(region),
                ..
            }
            | SubmissionOp::Read {
                data: SubmissionData::Registered(region),
                ..
            } => *region,
            _ => return Ok(()),
        };
        match &self.registered {
            Some(registered) => registered.check_span(span(region)).map_err(refusal_cause),
            None => Err(RefusalCause::Unsupported),
        }
    }

    fn push(&mut self, submission: Submission<B>) -> Result<(), Refused<B>> {
        if let Err(cause) = self.admits(submission.target, &submission.op) {
            return Err(Refused { cause, submission });
        }
        let Submission { token, target, op } = submission;
        let shape = Shape::of(&op);
        let sidecar = Sidecar {
            token,
            target,
            kind: kind(&op),
        };
        let op = match op {
            SubmissionOp::Write {
                offset,
                data,
                caching,
            } => Op::Write {
                offset,
                data,
                caching: ring_caching(caching),
            },
            SubmissionOp::Read { offset, data } => Op::Read { offset, data },
            SubmissionOp::Commit => Op::Commit,
        };
        let slot = &self.targets[&target];
        let registered = self.registered.as_ref();
        let read = self.owned_read;
        let mut batch = Batch::new(&mut self.ring);
        let pushed: Result<OperationId, PushRefused<B, Sidecar>> = match &slot.registered {
            Some(index) => {
                let via = Via::Registered(index);
                push_to!(batch, index, via, read, registered, op, sidecar)
            }
            None => {
                let handle = slot.target.handle();
                let via = Via::Shared(handle);
                push_to!(batch, handle, via, read, registered, op, sidecar)
            }
        };
        match pushed {
            Ok(_) => {
                // Queued, not submitted: the doorbell is `submit`'s, rung by the engine, so a
                // failure of it is seen by the caller that rings it and not lost in a drop.
                batch.leave_queued();
                self.queued = true;
                Ok(())
            }
            Err(refused) => {
                // Nothing of this push is queued, but an earlier one may be, which a drop would
                // submit and not report on.
                batch.leave_queued();
                let PushRefused { error, payload, .. } = refused;
                Err(Refused {
                    cause: refusal_cause(error),
                    submission: rebuilt(token, target, shape, payload),
                })
            }
        }
    }

    fn submit(&mut self) -> Result<(), RefusalCause> {
        if !self.queued {
            return Ok(());
        }
        match Batch::new(&mut self.ring).submit() {
            Ok(_) => {
                self.queued = false;
                Ok(())
            }
            Err(error) => Err(refusal_cause(error)),
        }
    }

    fn pop(&mut self) -> Result<Option<Completion<B>>, IoFault> {
        // The ring is an owned one, so it never hands back a completion it held nothing for: there
        // is no such case to report (DI-D-59).
        let Some((completion, buffer, sidecar)) = self.ring.try_pop().map_err(windows_fault)?
        else {
            return Ok(None);
        };
        #[cfg(feature = "fault-injection")]
        let completion = match sidecar.kind {
            Kind::Commit => match self
                .injected
                .iter()
                .position(|(target, _)| *target == sidecar.target)
            {
                Some(index) => {
                    let (_, code) = self.injected.remove(index);
                    completion
                        .with_injected_failure(windows_ioring_sys::InjectedFailure::Win32(code))
                }
                None => completion,
            },
            Kind::Write(_) | Kind::Read => completion,
        };
        let result = match sidecar.kind {
            Kind::Write(requested) => transfer(completion.result(), true, requested),
            Kind::Read => transfer(completion.result(), false, 0),
            Kind::Commit => completion.result().map(|_| 0).map_err(windows_fault),
        };
        Ok(Some(Completion {
            token: sidecar.token,
            result,
            buffer,
        }))
    }

    fn notification(&self) -> Result<CompletionEvent, IoFault> {
        self.event
            .try_clone()
            .map(CompletionEvent::new)
            .map_err(windows_fault)
    }
}

/// The fault seam (`DI-3.2.4`), on the backend: failures on demand, for tests. It transforms a
/// commit's real completion through `windows-ioring-sys`' own seam, so the commit still reaches the
/// kernel and only the answer changes.
#[cfg(feature = "fault-injection")]
impl<B: IoBuf, R: IoBufMut> WindowsBackend<B, R> {
    /// Make the next commit of `target` to complete report the Win32 error `code`, whatever the
    /// kernel answered. Armed failures for one target are used in the order armed.
    pub fn fail_next_commit(&mut self, target: IoTargetKey, code: u32) {
        self.injected.push((target, code));
    }
}
