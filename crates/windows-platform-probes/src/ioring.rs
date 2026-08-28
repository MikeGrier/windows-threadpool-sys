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
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingReadFile, BuildIoRingRegisterFileHandles, CloseIoRing, CreateIoRing, HIORING,
    IORING_BUFFER_REF, IORING_BUFFER_REF_0, IORING_CQE, IORING_CREATE_FLAGS, IORING_HANDLE_REF,
    IORING_HANDLE_REF_0, IORING_REF_RAW, IORING_REF_REGISTERED, IORING_VERSION_3,
    PopIoRingCompletion, SubmitIoRing,
};

/// How long a submit may wait for its completions.
const SUBMIT_TIMEOUT_MS: u32 = 5_000;

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
struct Ring(HIORING);

// SAFETY: moving a ring between threads is exactly what the thread-agnosticism
// probe measures. This impl makes the measurement expressible; the probe is the
// check, so nothing here rides on an assumption the probe has not tested.
unsafe impl Send for Ring {}

impl Ring {
    /// Creates a ring, or reports that the platform has none.
    fn new() -> Option<Self> {
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
    fn submit_and_wait(&self, operations: u32) -> i32 {
        let mut submitted = 0_u32;
        // SAFETY: the ring is live and `submitted` is writable.
        unsafe { SubmitIoRing(self.0, operations, SUBMIT_TIMEOUT_MS, &raw mut submitted) }
    }

    /// Pops one completion, or `None` when the queue is empty.
    fn pop(&self) -> Option<IORING_CQE> {
        let mut cqe = unsafe { std::mem::zeroed::<IORING_CQE>() };
        // SAFETY: the ring is live and `cqe` is writable.
        let popped = unsafe { PopIoRingCompletion(self.0, &raw mut cqe) };

        (popped >= 0).then_some(cqe)
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
struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "windows-platform-probes-ioring-{}-{label}.tmp",
            std::process::id()
        ));
        std::fs::write(&path, vec![b'x'; 4096]).expect("write the probe fixture");
        Self { path }
    }

    fn open(&self) -> std::fs::File {
        std::fs::File::open(&self.path).expect("open the probe fixture")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreadAgnosticism {
    /// The submitting thread had exited before the completion was collected.
    pub submitter_exited: bool,
    /// The operation's result code.
    pub result_code: i32,
}

impl ThreadAgnosticism {
    /// An operation outlived the thread that submitted it.
    #[must_use]
    pub fn survives_submitter_exit(self) -> bool {
        self.submitter_exited && self.result_code >= 0
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

    let fixture = Fixture::new("thread-agnostic");
    let file = fixture.open();
    let mut buffer = vec![0_u8; 512];

    /// The handle and buffer address the submitting thread needs.
    ///
    /// Raw pointers are not `Send`, and rightly so in general -- but these two
    /// point at a file and a buffer this function owns for the whole span
    /// including the join, so they outlive every use on the other thread.
    struct Borrowed {
        handle: *mut c_void,
        address: *mut c_void,
    }

    // SAFETY: as described above -- both targets are owned by this frame and
    // outlive the spawned thread, which is joined before either is dropped.
    unsafe impl Send for Borrowed {}

    let borrowed = Borrowed {
        handle: file.as_raw_handle().cast::<c_void>(),
        address: buffer.as_mut_ptr().cast::<c_void>(),
    };

    // Build and submit on a thread that then exits. `file` and `buffer` stay
    // owned here, so they outlive the operation regardless.
    let ring = std::thread::spawn(move || {
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

        // SAFETY: the ring is live; the handle and buffer are owned by the
        // caller across the join below, so both outlive the operation.
        let built = unsafe { BuildIoRingReadFile(ring.0, file_ref, buffer_ref, 512, 0, 0, 0) };
        assert!(built >= 0, "build the read");
        assert!(ring.submit_and_wait(0) >= 0, "submit the read");

        ring
    })
    .join()
    .expect("the submitter did not panic");

    // The submitting thread is gone; the completion is collected here.
    let mut completion = None;
    while completion.is_none() {
        completion = ring.pop();
    }
    let result_code = completion.expect("a completion was popped").ResultCode;

    drop(buffer);
    drop(file);

    IoRingSupport::Measured(ThreadAgnosticism {
        submitter_exited: true,
        result_code,
    })
}
