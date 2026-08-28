// Copyright (c) Mike Grier.

//! Does associating a handle with a completion port foreclose `IoRing` use of
//! it?
//!
//! This is the evidence for `windows-namespace-request-sys` returning an opened
//! handle **plain and unassociated**: if the association is irreversible, then
//! making it on a caller's behalf silently removes a capability, so the choice
//! belongs to a layer that knows the handle's destination.
//!
//! # Why this was split out of the rest of the migration
//!
//! The original probe exists in two versions that **disagree**, and the
//! disagreement is instructive rather than incidental.
//!
//! The first declared "COEXIST" on seeing a completion arrive on the ring --
//! while that completion's result code was `ERROR_INVALID_PARAMETER` and its
//! byte count was zero. It checked *where the completion arrived* and not
//! *whether the operation succeeded*, so it read a clean failure as a success.
//! Establishing which reading is right is fresh measurement rather than a port,
//! which is why it is its own item.
//!
//! # What this version checks, and why each part is load-bearing
//!
//! A read passes only when **all three** hold: the result code is success, the
//! byte count is the full length, and the buffer actually contains the fill
//! byte. Any one alone can be satisfied by a read that did nothing -- a
//! zero-filled buffer looks identical to an unread one unless the fixture is
//! filled with something else.
//!
//! Two negative controls make a failure attributable:
//!
//! - the **same read on a non-associated handle**, so a failure can be blamed
//!   on the association rather than on the probe;
//! - the associated handle still working **through the port**, so a failure
//!   means the `IoRing` path specifically was refused rather than the handle
//!   being broken outright.
//!
//! # `CreateThreadpoolIo` is measured too, and matters more
//!
//! Raw `CreateIoCompletionPort` is the textbook case, but this workspace
//! reaches completion ports through `CreateThreadpoolIo`. If that poisons a
//! handle the same way, the consequence lands on
//! [windows-threadpool-sys](https://docs.rs/windows-threadpool-sys)'s own users,
//! so it is measured rather than assumed to follow.
//!
//! Migrated from the throwaway `ctx-probe` spike (Probe D, corrected version).
//!
//! # Tier: ignored
//!
//! Needs a recent Windows build with `IoRing`, so it is environment-dependent.

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::IO::{CreateIoCompletionPort, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CreateThreadpoolIo, PTP_CALLBACK_INSTANCE, PTP_IO, TP_CALLBACK_ENVIRON_V3,
};

use crate::ioring::{FILL, FIXTURE_LEN, Fixture, IoRingSupport, Ring, read_raw_handle};

/// The completion key the port is created with. Named rather than written as a
/// bare literal, since it is checked on the way back out.
const COMPLETION_KEY: usize = 0xD;

/// One `IoRing` read attempt, judged on every field that can lie on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadAttempt {
    /// The completion's result code.
    pub result_code: i32,
    /// The byte count the completion reported.
    pub bytes: usize,
    /// The first byte actually landed in the buffer.
    pub first_byte: u8,
}

impl ReadAttempt {
    /// The read genuinely happened.
    ///
    /// All three conditions, deliberately. The first version of this probe
    /// checked none of them and concluded the opposite of the truth.
    #[must_use]
    pub fn succeeded(self) -> bool {
        self.result_code >= 0 && self.bytes == FIXTURE_LEN && self.first_byte == FILL
    }
}

/// What the whole probe observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionPortFinding {
    /// A read on a handle with no association at all.
    ///
    /// The negative control. If this fails, nothing else can be concluded --
    /// the probe is broken rather than the platform answering.
    pub control_unassociated: ReadAttempt,
    /// A read after `CreateIoCompletionPort`.
    pub after_iocp_association: ReadAttempt,
    /// A read on a third handle *before* it is associated.
    pub before_late_association: ReadAttempt,
    /// The same handle after it is associated.
    pub after_late_association: ReadAttempt,
    /// Whether the associated handle still completed an overlapped read
    /// through the port.
    ///
    /// The second control: it separates "the `IoRing` path is refused" from
    /// "the handle is broken".
    pub port_still_works: bool,
    /// A read before `CreateThreadpoolIo`.
    pub before_threadpool_io: ReadAttempt,
    /// A read after `CreateThreadpoolIo`.
    pub after_threadpool_io: ReadAttempt,
}

impl CompletionPortFinding {
    /// The probe's own controls held, so its result means something.
    #[must_use]
    pub fn is_valid(self) -> bool {
        self.control_unassociated.succeeded()
            && self.before_late_association.succeeded()
            && self.before_threadpool_io.succeeded()
    }

    /// Association **forecloses** `IoRing` use of the handle.
    ///
    /// The finding `windows-namespace-request-sys` rests on.
    #[must_use]
    pub fn association_forecloses_ioring(self) -> bool {
        self.is_valid()
            && !self.after_iocp_association.succeeded()
            && !self.after_late_association.succeeded()
    }

    /// `CreateThreadpoolIo` forecloses it the same way.
    ///
    /// Measured separately because it is the path this workspace actually uses.
    #[must_use]
    pub fn threadpool_io_forecloses_ioring(self) -> bool {
        self.is_valid() && !self.after_threadpool_io.succeeded()
    }
}

/// A callback that is never invoked, because no I/O is started through the
/// `TP_IO` this probe creates.
///
/// # Safety
///
/// Never called. It exists only because `CreateThreadpoolIo` requires one.
unsafe extern "system" fn unused_io_callback(
    _instance: PTP_CALLBACK_INSTANCE,
    _context: *mut c_void,
    _overlapped: *mut c_void,
    _result: u32,
    _bytes: usize,
    _io: PTP_IO,
) {
}

fn attempt(handle: HANDLE) -> ReadAttempt {
    let (result_code, bytes, first_byte) = read_raw_handle(handle);
    ReadAttempt {
        result_code,
        bytes,
        first_byte,
    }
}

/// Runs every case and reports what association did to each handle.
///
/// # Panics
///
/// Panics if a completion port cannot be created on a host that has a ring,
/// since that is a real failure rather than an absent platform feature.
#[must_use]
pub fn measure() -> IoRingSupport<CompletionPortFinding> {
    if Ring::new().is_none() {
        return IoRingSupport::Unavailable;
    }

    let fixture = Fixture::new("completion-port");

    // Case 1, the negative control: no association at all.
    let plain = fixture.open_overlapped();
    let control_unassociated = attempt(plain.as_raw_handle().cast::<c_void>());

    // Case 2: associate first, then try the ring.
    let associated = fixture.open_overlapped();
    let associated_raw = associated.as_raw_handle().cast::<c_void>();
    // SAFETY: `associated` is a live overlapped handle; a null existing port
    // creates a fresh one.
    let port =
        unsafe { CreateIoCompletionPort(associated_raw, std::ptr::null_mut(), COMPLETION_KEY, 0) };
    assert!(!port.is_null(), "create a completion port");
    let after_iocp_association = attempt(associated_raw);

    // Case 3: ring first (which must pass), then associate, then ring again.
    let late = fixture.open_overlapped();
    let late_raw = late.as_raw_handle().cast::<c_void>();
    let before_late_association = attempt(late_raw);
    // SAFETY: as above.
    let late_port =
        unsafe { CreateIoCompletionPort(late_raw, std::ptr::null_mut(), COMPLETION_KEY, 0) };
    assert!(!late_port.is_null(), "create the second completion port");
    let after_late_association = attempt(late_raw);

    // Case 4, the second control: is the associated handle still usable through
    // the port? If it is, only the ring path was refused.
    let port_still_works = read_through_port(associated_raw, port);

    // Case 5: does CreateThreadpoolIo poison a handle the same way? This is the
    // path this workspace actually uses, so it matters more than raw IOCP.
    let pooled = fixture.open_overlapped();
    let pooled_raw = pooled.as_raw_handle().cast::<c_void>();
    let before_threadpool_io = attempt(pooled_raw);
    // SAFETY: `pooled` is a live overlapped handle; the callback is never
    // invoked because no I/O is started through the returned TP_IO.
    let pooled_io = unsafe {
        CreateThreadpoolIo(
            pooled_raw,
            Some(unused_io_callback),
            std::ptr::null_mut(),
            std::ptr::null::<TP_CALLBACK_ENVIRON_V3>(),
        )
    };
    assert_ne!(pooled_io, 0, "create a threadpool I/O object");
    let after_threadpool_io = attempt(pooled_raw);

    // SAFETY: the port handles are owned here and closed once each.
    unsafe {
        CloseHandle(port);
        CloseHandle(late_port);
    }

    IoRingSupport::Measured(CompletionPortFinding {
        control_unassociated,
        after_iocp_association,
        before_late_association,
        after_late_association,
        port_still_works,
        before_threadpool_io,
        after_threadpool_io,
    })
}

/// How long each completion wait blocks before rechecking, rather than one
/// unbounded call -- the same discipline `windows-ioring-sys` uses for rundown.
const COMPLETION_POLL_MS: u32 = 5_000;

/// Issues an overlapped read and collects it from `port`.
///
/// # Why the failure path cancels and keeps waiting
///
/// `overlapped` has automatic storage and `buffer` is a heap allocation, and
/// both are freed when this returns. Once `ReadFile` reports `ERROR_IO_PENDING`
/// the kernel owns both until the operation completes, so returning on a
/// timeout would leave it writing into a freed stack frame and a freed heap
/// block -- and the caller closes the port immediately afterwards. There is no
/// safe early return: the operation is cancelled and then waited for.
fn read_through_port(handle: HANDLE, port: HANDLE) -> bool {
    use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::IO::{CancelIoEx, GetQueuedCompletionStatus};

    let mut buffer = vec![0_u8; FIXTURE_LEN];
    let mut overlapped = unsafe { std::mem::zeroed::<OVERLAPPED>() };

    // SAFETY: the handle is live and overlapped; the buffer and OVERLAPPED both
    // outlive the completion collected below -- see this function's comment.
    let started = unsafe {
        ReadFile(
            handle,
            buffer.as_mut_ptr(),
            u32::try_from(FIXTURE_LEN).expect("a small length fits a u32"),
            std::ptr::null_mut(),
            &raw mut overlapped,
        )
    };

    if started == 0 {
        // SAFETY: no preconditions.
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            // The read was refused outright, so nothing is in flight and
            // nothing owns the buffer. This is the measured negative.
            return false;
        }
    }

    let mut bytes = 0_u32;
    let mut key = 0_usize;
    let mut completed: *mut OVERLAPPED = std::ptr::null_mut();

    // SAFETY: the port is live and every out-parameter is writable.
    let dequeued = unsafe {
        GetQueuedCompletionStatus(
            port,
            &raw mut bytes,
            &raw mut key,
            &raw mut completed,
            COMPLETION_POLL_MS,
        )
    };

    if completed.is_null() {
        // Nothing was dequeued, so the read is still outstanding and still
        // owns the buffer. Cancel it, then wait for the completion that
        // cancelling guarantees -- rechecking rather than waiting unbounded,
        // but never giving up, because giving up is what corrupts memory.
        // SAFETY: `handle` is live and `overlapped` names an operation on it.
        unsafe { CancelIoEx(handle, &raw mut overlapped) };

        while completed.is_null() {
            // SAFETY: as above.
            unsafe {
                GetQueuedCompletionStatus(
                    port,
                    &raw mut bytes,
                    &raw mut key,
                    &raw mut completed,
                    COMPLETION_POLL_MS,
                )
            };
        }

        return false;
    }

    dequeued != 0 && bytes as usize == FIXTURE_LEN && key == COMPLETION_KEY && buffer[0] == FILL
}
