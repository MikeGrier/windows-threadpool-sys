// Copyright (c) Mike Grier.

//! `IoRing` registration semantics and thread agnosticism.
//!
//! Two findings the pseudo-async namespace-plane design rests on.
//!
//! # Registration replaces; it does not append
//!
//! `windows-ioring-sys` asserts that `BuildIoRingRegisterFileHandles` replaces
//! the ring's whole file table rather than appending to it, and refuses a
//! second call outright on that basis. That assertion was recorded as
//! explicitly **unverified**. If registration in fact appended, the crate's
//! index bookkeeping would be wrong and its refusal a needless restriction.
//!
//! # An operation outlives the thread that submitted it
//!
//! Every thread in the proposed design is transient by construction, so a
//! thread-bound IRP would fail only under load -- the worst possible failure
//! mode to discover late.
//!
//! # Why this calls Win32 directly rather than using `windows-ioring-sys`
//!
//! **That crate refuses a second registration precisely because of the
//! assumption being measured.** Probing through its safe API would test the
//! guard rather than the platform, and would confirm our own belief by
//! consulting it -- the circularity this repository's contract-integrity rule
//! exists to prevent. So these go straight to the Win32 entry points.
//!
//! Migrated from the throwaway `ioring-probe` spike (Probes A, A2, and B).
//!
//! # Tier: ignored
//!
//! `IoRing` needs a recent Windows build, so these are environment-dependent
//! rather than universally assertable. Each reports
//! [`IoRingSupport::Unavailable`] rather than failing when the platform has no
//! ring, so an older host says "cannot measure" instead of "the finding is
//! false".

use std::ffi::c_void;
use std::io::Write;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, S_OK};
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingReadFile, BuildIoRingRegisterFileHandles, CloseIoRing, CreateIoRing, HIORING,
    IORING_BUFFER_REF, IORING_BUFFER_REF_0, IORING_CQE, IORING_CREATE_FLAGS, IORING_HANDLE_REF,
    IORING_HANDLE_REF_0, IORING_REF_RAW, IORING_REF_REGISTERED, IORING_VERSION_3,
    PopIoRingCompletion, SubmitIoRing,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_INBOUND};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};

/// How long a submit may wait for its completions.
const SUBMIT_TIMEOUT_MS: u32 = 5_000;

/// The byte every fixture is filled with.
///
/// A read that reports success must also return *this*, so a probe cannot pass
/// on a zero-filled buffer it never actually read into. That is the specific
/// mistake the first completion-port probe made.
pub(crate) const FILL: u8 = 0xAB;

/// How large each fixture is, and how much each probe read asks for.
pub(crate) const FIXTURE_LEN: usize = 4096;

/// How much the thread-agnosticism probe's read asks for.
///
/// Sized on its own rather than sharing [`FIXTURE_LEN`] because this probe is
/// the one that needs the read to still be outstanding when its submitting
/// thread ends -- see `measure_thread_agnosticism`.
pub(crate) const READ_LEN: u32 = 512;

/// Whether this host can answer these questions at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IoRingSupport<T> {
    /// The platform has no usable `IoRing`, so nothing was measured.
    ///
    /// Distinguished from a negative result on purpose: "we could not ask" and
    /// "the answer is no" are different facts, and conflating them is how a
    /// design note ends up citing a measurement that never ran.
    Unavailable,
    /// The measurement ran.
    Measured(T),
}

impl<T> IoRingSupport<T> {
    /// The measurement, if it ran.
    pub fn measured(self) -> Option<T> {
        match self {
            Self::Unavailable => None,
            Self::Measured(value) => Some(value),
        }
    }
}

/// An owned ring, closed on drop.
pub(crate) struct Ring(pub(crate) HIORING);

// SAFETY: moving a ring between threads is exactly what the thread-agnosticism
// probe measures. This impl makes the measurement expressible; the probe is the
// check, so nothing here rides on an assumption the probe has not tested.
unsafe impl Send for Ring {}

impl Ring {
    /// Creates a ring, or reports that the platform has none.
    pub(crate) fn new() -> Option<Self> {
        let mut handle: HIORING = std::ptr::null_mut();
        // SAFETY: `handle` is a writable destination and the remaining
        // arguments are plain values.
        let created = unsafe {
            CreateIoRing(
                IORING_VERSION_3,
                IORING_CREATE_FLAGS::default(),
                16,
                16,
                &raw mut handle,
            )
        };

        (created >= 0).then_some(Self(handle))
    }

    /// Submits, waiting for `operations` completions.
    pub(crate) fn submit_and_wait(&self, operations: u32) -> i32 {
        let mut submitted = 0_u32;
        // SAFETY: the ring is live and `submitted` is writable.
        unsafe { SubmitIoRing(self.0, operations, SUBMIT_TIMEOUT_MS, &raw mut submitted) }
    }

    /// Pops one completion, or `None` when the queue is empty.
    ///
    /// # Why this tests for `S_OK` rather than for success
    ///
    /// `PopIoRingCompletion` reports an **empty queue** with `S_FALSE`, which
    /// is a success code. Testing `>= 0`, the usual shape for an `HRESULT`,
    /// therefore treats "there was nothing to pop" as "here is a completion"
    /// and hands back the zeroed `IORING_CQE` the call left untouched -- whose
    /// `ResultCode` field is `0`, and so reads as a *successful operation*.
    ///
    /// That is not a hypothetical. This probe module previously reported that
    /// an `IoRing` operation survives its submitting thread on the strength of
    /// exactly such a phantom completion: the queue was empty, `pop` returned
    /// a zeroed CQE, and the probe read `ResultCode == 0` as success. The
    /// finding was recorded in DESIGN-NOTES.md before the confusion was found.
    /// A probe that cannot tell an empty queue from a successful operation
    /// cannot measure anything.
    pub(crate) fn pop(&self) -> Option<IORING_CQE> {
        let mut cqe = unsafe { std::mem::zeroed::<IORING_CQE>() };
        // SAFETY: the ring is live and `cqe` is writable.
        let popped = unsafe { PopIoRingCompletion(self.0, &raw mut cqe) };

        (popped == S_OK).then_some(cqe)
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        // SAFETY: the ring is live and closed exactly once.
        unsafe { CloseIoRing(self.0) };
    }
}

/// Whether this host has a usable `IoRing`.
#[must_use]
pub fn is_available() -> bool {
    Ring::new().is_some()
}

/// A temporary file the probes read from, removed on drop.
pub(crate) struct Fixture {
    path: PathBuf,
}

impl Fixture {
    pub(crate) fn new(label: &str) -> Self {
        // Unique per instance, not merely per label: two tests running
        // concurrently in one process would otherwise share a path, and the
        // second's write would hit a sharing violation against the first's open
        // handles. That failure looks like the platform refusing something,
        // which is exactly the wrong thing for a probe to report.
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);

        let path = std::env::temp_dir().join(format!(
            "windows-platform-probes-ioring-{}-{label}-{unique}.tmp",
            std::process::id()
        ));
        std::fs::write(&path, vec![FILL; FIXTURE_LEN]).expect("write the probe fixture");
        Self { path }
    }

    pub(crate) fn open(&self) -> std::fs::File {
        std::fs::File::open(&self.path).expect("open the probe fixture")
    }

    /// Opens the fixture with `FILE_FLAG_OVERLAPPED`.
    ///
    /// The completion-port probe needs this: a handle cannot be associated with
    /// a port unless it was opened for overlapped I/O, so a probe using an
    /// ordinary handle would be measuring a refusal it caused itself.
    pub(crate) fn open_overlapped(&self) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;

        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&self.path)
            .expect("open the probe fixture for overlapped use")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A pipe with nothing in it yet, and the writer that can fill it.
///
/// The thread-agnosticism probe needs an operation that is genuinely still
/// outstanding when its submitting thread ends. A read of a small cached temp
/// file is not: measured on this workspace's hardware, it completed inside
/// `SubmitIoRing` on 8 runs out of 8, so the operation had already finished
/// before the submitter returned and the run was evidence of nothing. That is
/// why the earlier version of this probe could not fail.
///
/// A pipe with nothing written to it cannot complete until we choose to write,
/// so the pending state is *controlled* rather than hoped for, and the probe
/// can distinguish "the IRP survived its submitter" from "the IRP was already
/// finished".
pub(crate) struct PipePair {
    /// The read end, opened for overlapped use so the read can go pending.
    server: OwnedHandle,
    /// The write end. Writing here is what lets the pending read complete.
    client: std::fs::File,
}

impl PipePair {
    pub(crate) fn new(label: &str) -> Self {
        // Unique per instance for the same reason `Fixture` is: two probes
        // running concurrently must not collide on a name, or one would
        // observe the other's pipe and report a platform behaviour that was
        // really a fixture collision.
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            r"\\.\pipe\windows-platform-probes-{}-{label}-{unique}",
            std::process::id()
        );
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

        // SAFETY: `wide` is NUL-terminated and every other argument is a plain
        // value; a null security descriptor requests the default.
        let raw = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                READ_LEN,
                READ_LEN,
                0,
                std::ptr::null(),
            )
        };
        assert!(
            !raw.is_null() && raw != INVALID_HANDLE_VALUE,
            "create the probe pipe"
        );

        // SAFETY: `raw` is a fresh, valid handle this type now owns solely.
        let server = unsafe { OwnedHandle::from_raw_handle(raw.cast()) };

        // Opening the client end is what connects it, so no ConnectNamedPipe
        // is needed.
        let client = std::fs::OpenOptions::new()
            .write(true)
            .open(&name)
            .expect("connect to the probe pipe");

        Self { server, client }
    }

    /// The read end, for handing to a Win32 call.
    pub(crate) fn server_handle(&self) -> *mut c_void {
        self.server.as_raw_handle().cast::<c_void>()
    }

    /// Fills the pipe, letting a pending read complete.
    pub(crate) fn fill(&mut self) {
        self.client
            .write_all(&vec![FILL; READ_LEN as usize])
            .expect("write to the probe pipe");
        self.client.flush().expect("flush the probe pipe");
    }
}

/// What the registration probe observed.
///
/// `IORING_INFO` does not report the file-table size, so the semantics are
/// inferred the only way available: register two handles, then one, then read
/// through an index that exists only if the table still holds two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegistrationObservation {
    /// A read through registered index 1 succeeded **after** a second
    /// registration of a single handle.
    ///
    /// If registration appended, index 1 still names the first batch's second
    /// handle and the read succeeds. If it replaced, index 1 is out of range.
    pub index_one_usable_after_second: bool,
    /// A read through index 0 succeeded after the same sequence.
    ///
    /// The control: index 0 is valid under either semantics, so a failure here
    /// means the probe broke rather than that the table shrank.
    pub index_zero_usable_after_second: bool,
}

impl RegistrationObservation {
    /// The second registration **replaced** the table -- the crate's
    /// assumption.
    #[must_use]
    pub fn replaces(self) -> bool {
        self.index_zero_usable_after_second && !self.index_one_usable_after_second
    }

    /// The second registration **appended** to the table.
    #[must_use]
    pub fn appends(self) -> bool {
        self.index_zero_usable_after_second && self.index_one_usable_after_second
    }
}

/// Registers `handles` and waits for the registration to complete.
fn register(ring: &Ring, handles: &[HANDLE]) -> i32 {
    // SAFETY: the ring is live; `handles` is valid for its length and its
    // handles outlive the registration by the caller's construction.
    let built = unsafe {
        BuildIoRingRegisterFileHandles(
            ring.0,
            u32::try_from(handles.len()).expect("a small count fits a u32"),
            handles.as_ptr(),
            0,
        )
    };
    if built < 0 {
        return built;
    }

    let submitted = ring.submit_and_wait(1);
    if submitted < 0 {
        return submitted;
    }

    ring.pop().map_or(i32::MIN, |cqe| cqe.ResultCode)
}

/// Reads one byte through registered file `index`, returning the result code.
fn read_through_index(ring: &Ring, index: u32) -> i32 {
    let mut byte = [0_u8; 1];

    let file_ref = IORING_HANDLE_REF {
        Kind: IORING_REF_REGISTERED,
        Handle: IORING_HANDLE_REF_0 { Index: index },
    };
    let buffer_ref = IORING_BUFFER_REF {
        Kind: IORING_REF_RAW,
        Buffer: IORING_BUFFER_REF_0 {
            Address: byte.as_mut_ptr().cast::<c_void>(),
        },
    };

    // SAFETY: the ring is live; the buffer is writable for one byte and stays
    // alive until the wait below drains the operation.
    let built = unsafe { BuildIoRingReadFile(ring.0, file_ref, buffer_ref, 1, 0, 0, 0) };
    if built < 0 {
        return built;
    }

    let submitted = ring.submit_and_wait(1);
    if submitted < 0 {
        return submitted;
    }

    ring.pop().map_or(i32::MIN, |cqe| cqe.ResultCode)
}

/// Reads the whole fixture through a raw handle on a fresh ring.
///
/// Returns the completion's result code, the bytes it reported, and the first
/// byte actually landed in the buffer. **All three matter**: a probe that
/// checked only where the completion arrived would call a failure a success,
/// which is exactly what the first completion-port probe did.
pub(crate) fn read_raw_handle(handle: HANDLE) -> (i32, usize, u8) {
    let Some(ring) = Ring::new() else {
        return (i32::MIN, 0, 0);
    };
    let mut buffer = vec![0_u8; FIXTURE_LEN];

    let file_ref = IORING_HANDLE_REF {
        Kind: IORING_REF_RAW,
        Handle: IORING_HANDLE_REF_0 { Handle: handle },
    };
    let buffer_ref = IORING_BUFFER_REF {
        Kind: IORING_REF_RAW,
        Buffer: IORING_BUFFER_REF_0 {
            Address: buffer.as_mut_ptr().cast::<c_void>(),
        },
    };

    // SAFETY: the ring is live; the buffer is writable for FIXTURE_LEN bytes
    // and outlives the wait below, which drains the operation.
    let built = unsafe {
        BuildIoRingReadFile(
            ring.0,
            file_ref,
            buffer_ref,
            u32::try_from(FIXTURE_LEN).expect("a small length fits a u32"),
            0,
            0,
            0,
        )
    };
    if built < 0 {
        return (built, 0, 0);
    }

    let submitted = ring.submit_and_wait(1);
    if submitted < 0 {
        return (submitted, 0, 0);
    }

    match ring.pop() {
        Some(cqe) => (cqe.ResultCode, cqe.Information, buffer[0]),
        None => (i32::MIN, 0, 0),
    }
}

/// Registers two handles, then one, and reports which indices still work.
///
/// # Panics
///
/// Panics if the premise fails -- a first registration that does not take, or
/// an index 1 that does not work before the second registration -- because a
/// probe whose fixture cannot exhibit the behaviour must stop rather than
/// report a misleading answer.
#[must_use]
pub fn measure_registration() -> IoRingSupport<RegistrationObservation> {
    let Some(ring) = Ring::new() else {
        return IoRingSupport::Unavailable;
    };

    let fixture = Fixture::new("registration");
    let first = fixture.open();
    let second = fixture.open();
    let handles = [
        first.as_raw_handle().cast::<c_void>(),
        second.as_raw_handle().cast::<c_void>(),
    ];

    assert!(
        register(&ring, &handles) >= 0,
        "the first registration must succeed"
    );
    assert!(
        read_through_index(&ring, 1) >= 0,
        "index 1 must work after registering two handles, or the probe measures nothing"
    );

    let third = fixture.open();
    let single = [third.as_raw_handle().cast::<c_void>()];
    assert!(
        register(&ring, &single) >= 0,
        "the second registration must succeed"
    );

    IoRingSupport::Measured(RegistrationObservation {
        index_zero_usable_after_second: read_through_index(&ring, 0) >= 0,
        index_one_usable_after_second: read_through_index(&ring, 1) >= 0,
    })
}

/// What the thread-agnosticism probe observed.
///
/// The interesting field is [`pending_at_submitter_exit`]. That the submitter
/// exited is not worth recording -- the probe joins it, so it is true by
/// construction. What decides whether the run measured anything is whether the
/// operation was still *outstanding* at that moment: an operation that had
/// already completed inside `SubmitIoRing` would be collected afterwards no
/// matter how thread-affine the platform were, and such a run is evidence of
/// nothing.
///
/// [`pending_at_submitter_exit`]: Self::pending_at_submitter_exit
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadAgnosticism {
    /// The operation was still outstanding when its submitting thread ended.
    ///
    /// Observed from the submitting thread itself, immediately before it
    /// returns, by checking that no completion is available yet.
    pub pending_at_submitter_exit: bool,
    /// The operation's result code.
    pub result_code: i32,
    /// The read actually transferred the fixture's fill byte.
    ///
    /// A success code alone would also be reported by a read that transferred
    /// nothing, leaving the caller inspecting the zero-filled buffer it
    /// allocated.
    pub filled: bool,
}

impl ThreadAgnosticism {
    /// An operation outlived the thread that submitted it.
    ///
    /// False when the operation completed before its submitter ended, because
    /// then nothing outlived anything.
    #[must_use]
    pub fn survives_submitter_exit(self) -> bool {
        self.pending_at_submitter_exit && self.result_code >= 0 && self.filled
    }
}

/// Submits a read from a thread, lets that thread exit, and collects the
/// completion from another.
///
/// # Panics
///
/// Panics if the read cannot be built or submitted on a host that has a ring.
#[must_use]
pub fn measure_thread_agnosticism() -> IoRingSupport<ThreadAgnosticism> {
    let Some(ring) = Ring::new() else {
        return IoRingSupport::Unavailable;
    };

    // A pipe rather than a file, so the read is still outstanding when the
    // submitter ends -- see `PipePair`.
    let mut pipe = PipePair::new("thread-agnostic");
    let mut buffer = vec![0_u8; READ_LEN as usize];

    /// The handle and buffer address the submitting thread needs.
    ///
    /// Raw pointers are not `Send`, and rightly so in general -- but these two
    /// point at a pipe and a buffer this function owns for the whole span
    /// including the join, so they outlive every use on the other thread.
    struct Borrowed {
        handle: *mut c_void,
        address: *mut c_void,
    }

    // SAFETY: as described above -- both targets are owned by this frame and
    // outlive the spawned thread, which is joined before either is dropped.
    unsafe impl Send for Borrowed {}

    let borrowed = Borrowed {
        handle: pipe.server_handle(),
        address: buffer.as_mut_ptr().cast::<c_void>(),
    };

    // Build and submit on a thread that then exits. `pipe` and `buffer` stay
    // owned here, so they outlive the operation regardless.
    let (ring, early) = std::thread::spawn(move || {
        let borrowed = borrowed;
        let file_ref = IORING_HANDLE_REF {
            Kind: IORING_REF_RAW,
            Handle: IORING_HANDLE_REF_0 {
                Handle: borrowed.handle,
            },
        };
        let buffer_ref = IORING_BUFFER_REF {
            Kind: IORING_REF_RAW,
            Buffer: IORING_BUFFER_REF_0 {
                Address: borrowed.address,
            },
        };

        // SAFETY: the ring is live; the pipe and buffer are owned by the
        // caller across the join below, so both outlive the operation.
        let built = unsafe { BuildIoRingReadFile(ring.0, file_ref, buffer_ref, READ_LEN, 0, 0, 0) };
        assert!(built >= 0, "build the read");
        assert!(ring.submit_and_wait(0) >= 0, "submit the read");

        // Whether the operation is still outstanding, observed here rather
        // than inferred: this is the submitting thread, and it has not
        // returned yet. `pop` consumes a completion when one is ready, so the
        // result is carried out rather than discarded -- dropping it would
        // leave the collector below waiting for a completion already taken.
        let early = ring.pop();

        (ring, early)
    })
    .join()
    .expect("the submitter did not panic");

    // The submitting thread is gone. A completion popped above would mean the
    // operation did not outlive it; with a pipe that had nothing written to it
    // yet, that must not happen, and the field records it either way.
    let pending_at_submitter_exit = early.is_none();

    // Only now does the read become completable -- after its submitter is
    // gone. This is the measurement: if the platform cancelled the IRP at
    // thread exit, no completion arrives, or it arrives failed.
    pipe.fill();

    let completion = early.unwrap_or_else(|| {
        let mut completion = None;
        while completion.is_none() {
            completion = ring.pop();
        }
        completion.expect("a completion was popped")
    });

    // A success code alone is not enough: a read that reported success without
    // transferring the fill byte would pass on the zero-filled buffer it never
    // read into, which is the mistake the first completion-port probe made.
    let transferred = completion.Information;
    let filled = transferred > 0 && buffer[..transferred].iter().all(|byte| *byte == FILL);

    drop(buffer);
    drop(pipe);

    IoRingSupport::Measured(ThreadAgnosticism {
        pending_at_submitter_exit,
        result_code: completion.ResultCode,
        filled,
    })
}
