// Copyright (c) 2026 Mike Grier
//! Integration tests for the `TP_IO` backend's behavioral matrix.
//!
//! The five states a thread-pool I/O submission can reach are each covered
//! directly -- immediate failure, immediate success (skip-on-success),
//! pending completion, cancellation, and object rundown with operations
//! outstanding -- together with the accounting and reclamation invariants that
//! must hold across them at scale.
//!
//! Every test asserts on `outstanding()`, because that count is simultaneously
//! the number of unbalanced `StartThreadpoolIo` calls and the number of
//! operations whose storage the kernel or pool still owns. A drift in either
//! direction is the failure mode this matrix exists to catch.

#![cfg(windows)]

use std::collections::{HashMap, HashSet};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use windows_overlapped_io_sys::{
    Issued, Operation, OperationState, Submitted, UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_OPERATION_ABORTED};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, ReadFile, SetFileCompletionNotificationModes, WriteFile,
};
use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

use windows_threadpool_sys::callback_env::CallbackEnviron;
use windows_threadpool_sys::io::{IoCompletion, ThreadpoolIo};

/// The Win32 `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` flag for
/// `SetFileCompletionNotificationModes`; windows-sys does not export it.
/// Changing this value is a breaking change.
const FILE_SKIP_COMPLETION_PORT_ON_SUCCESS: u8 = 0x1;

/// Named-pipe creation flags; windows-sys exports these only as loose constants
/// in feature-gated modules, so the two this file needs are named here.
/// Changing either value is a breaking change.
mod pipe_mode {
    /// `PIPE_ACCESS_DUPLEX`.
    pub const ACCESS_DUPLEX: u32 = 0x0000_0003;
    /// `PIPE_TYPE_BYTE`.
    pub const TYPE_BYTE: u32 = 0x0000_0000;
}

/// How long a test will wait for the pool to deliver expected callbacks before
/// declaring the backend broken. Generous enough to absorb a loaded CI machine,
/// short enough that a genuine hang fails the run rather than stalling it.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(30);

// --- harness ---

/// One completion as observed by a test's callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Record {
    /// The `OVERLAPPED` address, which is the operation's identity.
    identity: usize,
    io_result: u32,
    bytes: usize,
    /// The payload the operation carried, when the test claims a typed payload.
    payload: usize,
}

/// Collects callback observations and lets the test thread wait for a specific
/// number of them rather than sleeping.
struct Recorder {
    records: Mutex<Vec<Record>>,
    arrived: Condvar,
}

impl Recorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            records: Mutex::new(Vec::new()),
            arrived: Condvar::new(),
        })
    }

    fn push(&self, record: Record) {
        let mut records = self.records.lock().expect("record a completion");
        records.push(record);
        self.arrived.notify_all();
    }

    fn records(&self) -> Vec<Record> {
        self.records.lock().expect("read completions").clone()
    }

    fn len(&self) -> usize {
        self.records.lock().expect("read completions").len()
    }

    /// Block until at least `count` completions have been recorded, failing the
    /// test rather than hanging if they never arrive.
    fn wait_for(&self, count: usize) -> Vec<Record> {
        let records = self.records.lock().expect("await completions");
        let (records, timeout) = self
            .arrived
            .wait_timeout_while(records, CALLBACK_TIMEOUT, |records| records.len() < count)
            .expect("await completions");
        assert!(
            !timeout.timed_out(),
            "timed out waiting for {count} completion(s); saw {}",
            records.len()
        );
        records.clone()
    }
}

/// A payload whose `Drop` is observable, proving that reclaiming an operation
/// really frees its payload rather than leaking the storage.
#[derive(Debug)]
struct DropTracked {
    index: usize,
    dropped: Arc<AtomicUsize>,
}

impl Drop for DropTracked {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

/// A second, differently-sized tracked payload, so a single object can be shown
/// to reclaim operations of mixed payload types through the type-erased thunk.
#[derive(Debug)]
struct DropTrackedWide {
    _filler: [u64; 16],
    dropped: Arc<AtomicUsize>,
}

impl Drop for DropTrackedWide {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

/// A raw buffer base pointer shared across submitting threads.
///
/// SAFETY: the pointee is a `Vec<u8>` that outlives every operation submitted
/// against it, and each operation is given a disjoint one-byte slot.
#[derive(Clone, Copy)]
struct SharedBuffer(*mut u8);

// SAFETY: the pointer is only offset into disjoint per-operation slots of a
// buffer that outlives all of them; no two operations touch the same byte.
unsafe impl Send for SharedBuffer {}
unsafe impl Sync for SharedBuffer {}

impl SharedBuffer {
    /// The one-byte slot reserved for `index`.
    ///
    /// This is a method rather than a field access on purpose: a closure that
    /// touched `self.0` directly would capture the bare `*mut u8` under
    /// disjoint closure capture and stop being `Send`.
    ///
    /// # Safety
    ///
    /// `index` must be within the buffer this was built from.
    unsafe fn slot(self, index: usize) -> *mut u8 {
        // SAFETY: forwarded from this function's own contract.
        unsafe { self.0.add(index) }
    }
}

fn temp_file_with(content: &[u8], tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-threadpool-sys-tp-io-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

fn read_endpoint(path: &Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open overlapped endpoint")
}

/// A connected server pipe end carrying no data, so a read on it stays pending
/// until it is cancelled. The client end is returned and must be kept alive.
fn pending_pipe(tag: &str) -> (UnassociatedEndpoint, std::fs::File) {
    let name = format!(
        "{}{}-{}",
        r"\\.\pipe\windows-threadpool-sys-tp-io-",
        tag,
        std::process::id()
    );
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: creates a fresh overlapped named pipe from a valid wide name.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            pipe_mode::ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            pipe_mode::TYPE_BYTE,
            1,
            4096,
            4096,
            0,
            ptr::null(),
        )
    };
    assert!(
        !handle.is_null() && handle as isize != -1,
        "CreateNamedPipeW failed: {}",
        io::Error::last_os_error()
    );

    // SAFETY: fresh, exclusively owned, and opened with FILE_FLAG_OVERLAPPED.
    let endpoint =
        unsafe { UnassociatedEndpoint::assume_overlapped(OwnedHandle::from_raw_handle(handle)) };

    // Connecting a client makes a server read pend rather than fail with
    // ERROR_PIPE_LISTENING; no data is ever written, so it never completes.
    let client = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect("connect the client pipe end");

    (endpoint, client)
}

/// Classify an overlapped `ReadFile` for a handle that is not in
/// skip-on-success mode: both synchronous success and `ERROR_IO_PENDING`
/// deliver a completion callback.
///
/// SAFETY: `buffer` must point to at least `len` writable bytes that stay valid
/// until the operation completes, and `overlapped` must be the operation's own
/// identity pointer.
unsafe fn issue_read(
    handle: std::os::windows::io::BorrowedHandle<'_>,
    overlapped: *mut windows_sys::Win32::System::IO::OVERLAPPED,
    buffer: *mut u8,
    len: u32,
) -> io::Result<Issued> {
    // SAFETY: forwarded from this function's own contract.
    let ok = unsafe {
        ReadFile(
            handle.as_raw_handle(),
            buffer,
            len,
            ptr::null_mut(),
            overlapped,
        )
    };
    if ok != 0 {
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        return Ok(Issued::Pending);
    }
    Err(error)
}

// --- matrix state 1: immediate failure ---

/// A native call that fails immediately delivers no callback, so `submit` must
/// balance its own start with `CancelThreadpoolIo` and return the operation.
#[test]
fn immediate_failure_returns_the_operation_and_balances_accounting() {
    let path = temp_file_with(b"immediate failure", "immediate-failure");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: 0,
            });
        },
        None,
    )
    .expect("create TP_IO");

    let source = *b"denied";
    let src_ptr = source.as_ptr();
    let src_len = source.len() as u32;

    let operation = Operation::new(());
    // SAFETY: issues exactly one overlapped WriteFile on a read-only handle,
    // which fails immediately with ERROR_ACCESS_DENIED and queues no callback.
    let submitted = unsafe {
        tp.submit(operation, |handle, overlapped| {
            let ok = WriteFile(
                handle.as_raw_handle(),
                src_ptr,
                src_len,
                ptr::null_mut(),
                overlapped,
            );
            if ok != 0 {
                return Ok(Issued::Pending);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                return Ok(Issued::Pending);
            }
            Err(error)
        })
    };

    match submitted {
        Submitted::Failed { operation, error } => {
            assert_eq!(operation.state(), OperationState::Idle);
            assert!(error.raw_os_error().is_some(), "expected an OS error");
        }
        other => panic!("expected an immediate failure, got {other:?}"),
    }

    assert_eq!(tp.outstanding(), 0, "the start must have been balanced");
    assert_eq!(recorder.len(), 0, "a failed submission must not call back");

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// Repeating the failure path at scale must not drift the accounting.
#[test]
fn repeated_immediate_failures_do_not_drift_accounting() {
    const ATTEMPTS: usize = 500;

    let path = temp_file_with(b"repeat", "repeat-failure");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: 0,
            });
        },
        None,
    )
    .expect("create TP_IO");

    let dropped = Arc::new(AtomicUsize::new(0));
    for index in 0..ATTEMPTS {
        let operation = Operation::new(DropTracked {
            index,
            dropped: Arc::clone(&dropped),
        });
        // SAFETY: `issue` starts no operation and reports the failure, so no
        // callback will arrive -- exactly what the `Err` contract requires.
        let submitted = unsafe {
            tp.submit(operation, |_handle, _overlapped| {
                Err(io::Error::from_raw_os_error(5))
            })
        };
        match submitted {
            Submitted::Failed { operation, .. } => {
                assert_eq!(operation.payload().index, index, "payload must survive");
            }
            other => panic!("expected an immediate failure, got {other:?}"),
        }
        assert_eq!(tp.outstanding(), 0, "accounting drifted at attempt {index}");
    }

    assert_eq!(
        dropped.load(Ordering::SeqCst),
        ATTEMPTS,
        "every returned operation's payload must be dropped"
    );
    assert_eq!(recorder.len(), 0, "failed submissions must not call back");

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

// --- matrix state 2: immediate success (skip-on-success) ---

/// With `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` a synchronous success queues no
/// callback, so `submit` must balance the start and hand back the operation.
///
/// The mode is a request, not a guarantee: an uncached read can still pend, so
/// this test accepts either outcome and asserts the invariants of whichever one
/// occurred.
#[test]
fn immediate_success_balances_accounting_without_a_callback() {
    let content = b"skip on success payload";
    let path = temp_file_with(content, "skip-success");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<()> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<()>() };
            assert_eq!(operation.state(), OperationState::Completed);
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: 0,
            });
        },
        None,
    )
    .expect("create TP_IO");

    // SAFETY: the object owns a valid overlapped handle bound to the pool.
    let modes_ok = unsafe {
        SetFileCompletionNotificationModes(
            tp.handle().as_raw_handle(),
            FILE_SKIP_COMPLETION_PORT_ON_SUCCESS,
        )
    };
    assert_ne!(
        modes_ok,
        0,
        "SetFileCompletionNotificationModes failed: {}",
        io::Error::last_os_error()
    );

    let mut buffer = [0_u8; 64];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;
    let mut bytes: u32 = 0;
    let bytes_ptr: *mut u32 = &mut bytes;

    let mut operation = Operation::new(());
    operation.set_offset(0);

    // SAFETY: issues exactly one overlapped ReadFile into `buffer`, which
    // outlives the operation because this test blocks below. Under
    // skip-on-success a synchronous success writes `bytes` and queues no
    // callback, so it is reported as Completed.
    let submitted = unsafe {
        tp.submit(operation, |handle, overlapped| {
            let ok = ReadFile(
                handle.as_raw_handle(),
                buf_ptr,
                buf_len,
                bytes_ptr,
                overlapped,
            );
            if ok != 0 {
                return Ok(Issued::Completed {
                    bytes_transferred: *bytes_ptr,
                });
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                return Ok(Issued::Pending);
            }
            Err(error)
        })
    };

    match submitted {
        Submitted::Completed {
            operation,
            bytes_transferred,
        } => {
            assert_eq!(operation.state(), OperationState::Completed);
            assert_eq!(bytes_transferred as usize, content.len());
            assert_eq!(&buffer[..content.len()], content);
            assert_eq!(tp.outstanding(), 0, "the start must have been balanced");
            assert_eq!(recorder.len(), 0, "no callback may run on this path");
        }
        Submitted::Pending(id) => {
            let records = recorder.wait_for(1);
            assert_eq!(records[0].identity, id.as_ptr() as usize);
            assert_eq!(records[0].bytes, content.len());
            tp.run_down();
            assert_eq!(tp.outstanding(), 0);
        }
        Submitted::Failed { error, .. } => panic!("submit failed: {error}"),
    }

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

// --- matrix state 3: pending completion ---

/// A pending read is completed by the pool's callback, which claims the typed
/// operation and thereby reclaims its storage.
#[test]
fn pending_completion_delivers_the_callback_and_claims_the_operation() {
    let content = b"thread pool overlapped read";
    let path = temp_file_with(content, "pending-read");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            assert_eq!(operation.state(), OperationState::Completed);
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        None,
    )
    .expect("create TP_IO");

    let mut buffer = [0_u8; 64];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;

    let mut operation = Operation::new(0xABCD_usize);
    operation.set_offset(0);

    // SAFETY: issues exactly one overlapped ReadFile into `buffer`, which stays
    // alive until this test blocks in `wait_for` / `run_down` below.
    let submitted = unsafe {
        tp.submit(operation, |handle, ov| {
            issue_read(handle, ov, buf_ptr, buf_len)
        })
    };

    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected a pending submission, got {other:?}"),
    };

    let records = recorder.wait_for(1);
    assert_eq!(records.len(), 1, "expected exactly one completion");
    assert_eq!(
        records[0].identity,
        id.as_ptr() as usize,
        "identity mismatch"
    );
    assert_eq!(records[0].io_result, 0, "expected a successful read");
    assert_eq!(records[0].bytes, content.len());
    assert_eq!(
        records[0].payload, 0xABCD,
        "payload must survive the round trip"
    );
    assert_eq!(&buffer[..content.len()], content);

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// Identities must stay distinct across operations that are outstanding *at the
/// same time*, and each must call back exactly once carrying its own payload.
///
/// The reads are issued on a pipe carrying no data, so none of them can complete
/// while the rest are being submitted. That matters: an identity is the address
/// of the operation's storage, so once an operation completes and is reclaimed
/// the allocator may hand the same address to a later operation. Uniqueness is a
/// property of the live set, not of all operations for all time -- which is
/// exactly why this test keeps every operation live until it asserts.
#[test]
fn simultaneously_outstanding_operations_keep_distinct_identities() {
    const OPERATIONS: usize = 256;

    let (endpoint, _client) = pending_pipe("identity");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        endpoint,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        None,
    )
    .expect("create TP_IO");

    // One byte of stable storage per operation; never reallocated after `base`
    // is taken, and it outlives every operation.
    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    let mut identities: HashMap<usize, usize> = HashMap::new();
    for slot in 0..OPERATIONS {
        let operation = Operation::new(slot);
        // SAFETY: issues exactly one 1-byte overlapped ReadFile into this slot's
        // own byte of `landed`; slot < OPERATIONS and `landed` outlives it
        // because the rundown below blocks until every callback has run.
        let submitted = unsafe {
            tp.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(slot), 1)
            })
        };
        match submitted {
            Submitted::Pending(id) => {
                assert!(
                    identities.insert(id.as_ptr() as usize, slot).is_none(),
                    "two simultaneously outstanding operations shared identity {:p}",
                    id.as_ptr()
                );
            }
            other => panic!("expected pending at slot {slot}, got {other:?}"),
        }
    }

    // Every operation is still live, so every identity must be distinct.
    assert_eq!(tp.outstanding(), OPERATIONS);
    assert_eq!(identities.len(), OPERATIONS);

    tp.cancel_all().expect("cancel every outstanding read");
    let records = recorder.wait_for(OPERATIONS);
    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
    assert_eq!(records.len(), OPERATIONS, "every operation must call back");

    let mut payloads_seen: HashSet<usize> = HashSet::new();
    for record in &records {
        let slot = *identities
            .get(&record.identity)
            .unwrap_or_else(|| panic!("unknown identity {:#x}", record.identity));
        assert_eq!(
            record.payload, slot,
            "identity and payload disagree about the slot"
        );
        assert!(
            payloads_seen.insert(slot),
            "slot {slot} completed more than once"
        );
        assert_eq!(
            record.io_result, ERROR_OPERATION_ABORTED,
            "slot {slot} should have been aborted"
        );
    }
    assert_eq!(payloads_seen.len(), OPERATIONS);
}

/// At scale, every read must complete exactly once and land its own data.
///
/// Unlike the identity test above these reads complete as fast as they are
/// issued, so operations are *not* all simultaneously live and their storage
/// addresses may be recycled. Correlation is therefore by payload, which is
/// carried inside the operation and is unique for all time.
#[test]
fn many_file_reads_each_complete_once_with_their_own_data() {
    const OPERATIONS: usize = 512;

    // A distinct byte per offset, so each read's landed byte identifies its slot.
    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "scale-reads");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        None,
    )
    .expect("create TP_IO");

    // One byte of stable storage per operation; never reallocated after `base`
    // is taken, and it outlives every operation.
    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    for slot in 0..OPERATIONS {
        let mut operation = Operation::new(slot);
        operation.set_offset(slot as u64);
        // SAFETY: issues exactly one 1-byte overlapped ReadFile into this slot's
        // own byte of `landed`; slot < OPERATIONS and `landed` outlives it.
        let submitted = unsafe {
            tp.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(slot), 1)
            })
        };
        assert!(
            matches!(submitted, Submitted::Pending(_)),
            "slot {slot}: expected pending, got {submitted:?}"
        );
    }

    let records = recorder.wait_for(OPERATIONS);
    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
    assert_eq!(records.len(), OPERATIONS, "every operation must call back");

    let mut payloads_seen: HashSet<usize> = HashSet::new();
    for record in &records {
        assert!(
            record.payload < OPERATIONS,
            "payload {} is out of range",
            record.payload
        );
        assert!(
            payloads_seen.insert(record.payload),
            "slot {} completed more than once",
            record.payload
        );
        assert_eq!(record.io_result, 0, "slot {} failed", record.payload);
        assert_eq!(
            record.bytes, 1,
            "slot {} read the wrong length",
            record.payload
        );
    }
    assert_eq!(payloads_seen.len(), OPERATIONS);

    // Each slot must have received the byte stored at its own offset.
    for (slot, byte) in landed.iter().enumerate() {
        assert_eq!(*byte, slot as u8, "slot {slot} landed the wrong byte");
    }

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// Operations submitted concurrently from several threads must be accounted for
/// exactly once each: `ThreadpoolIo` is `Send + Sync` and advertises this.
#[test]
fn concurrent_submissions_from_many_threads_are_accounted_exactly_once() {
    const THREADS: usize = 8;
    const PER_THREAD: usize = 64;
    const OPERATIONS: usize = THREADS * PER_THREAD;

    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "threads");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let mut landed = vec![0_u8; OPERATIONS];
    let base = SharedBuffer(landed.as_mut_ptr());

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        None,
    )
    .expect("create TP_IO");

    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let tp = &tp;
            scope.spawn(move || {
                for step in 0..PER_THREAD {
                    let slot = thread * PER_THREAD + step;
                    let mut operation = Operation::new(slot);
                    operation.set_offset(slot as u64);
                    // SAFETY: one 1-byte overlapped ReadFile into this slot's own
                    // byte; slots are disjoint across threads and `landed`
                    // outlives every operation.
                    let submitted = unsafe {
                        tp.submit(operation, |handle, ov| {
                            issue_read(handle, ov, base.slot(slot), 1)
                        })
                    };
                    assert!(
                        matches!(submitted, Submitted::Pending(_)),
                        "slot {slot}: expected pending, got {submitted:?}"
                    );
                }
            });
        }
    });

    let records = recorder.wait_for(OPERATIONS);
    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
    assert_eq!(records.len(), OPERATIONS);

    let payloads: HashSet<usize> = records.iter().map(|record| record.payload).collect();
    assert_eq!(payloads.len(), OPERATIONS, "every slot must complete once");
    for record in &records {
        assert_eq!(record.io_result, 0, "slot {} failed", record.payload);
    }
    for (slot, byte) in landed.iter().enumerate() {
        assert_eq!(*byte, slot as u8, "slot {slot} landed the wrong byte");
    }

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// Sequential submissions reuse one object without accumulating state.
#[test]
fn sequential_submissions_reuse_the_object() {
    const ROUNDS: usize = 25;

    let content: Vec<u8> = (0..ROUNDS).map(|i| (i * 3) as u8).collect();
    let path = temp_file_with(&content, "sequential");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = vec![0_u8; ROUNDS];
    let base = landed.as_mut_ptr();

    for round in 0..ROUNDS {
        let mut operation = Operation::new(round);
        operation.set_offset(round as u64);
        // SAFETY: one 1-byte overlapped ReadFile into this round's own byte;
        // `landed` outlives every operation and this loop drains each round.
        let submitted = unsafe {
            tp.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(round), 1)
            })
        };
        assert!(
            matches!(submitted, Submitted::Pending(_)),
            "round {round}: expected pending, got {submitted:?}"
        );

        // Drain this round before starting the next one.
        recorder.wait_for(round + 1);
        tp.run_down();
        assert_eq!(tp.outstanding(), 0, "round {round} left work outstanding");
    }

    let records = recorder.records();
    assert_eq!(records.len(), ROUNDS);
    for (round, record) in records.iter().enumerate() {
        assert_eq!(record.payload, round, "rounds completed out of order");
        assert_eq!(record.io_result, 0);
    }
    for (round, byte) in landed.iter().enumerate() {
        assert_eq!(*byte, (round * 3) as u8);
    }

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

// --- matrix state 4: cancellation ---

/// Cancelling one operation still completes it -- as `ERROR_OPERATION_ABORTED`
/// -- and that callback remains the point at which its storage is reclaimed.
#[test]
fn cancelling_one_operation_completes_it_as_aborted() {
    let (endpoint, _client) = pending_pipe("cancel-one");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        endpoint,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        None,
    )
    .expect("create TP_IO");

    let mut buffer = [0_u8; 32];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;

    let operation = Operation::new(7_usize);
    // SAFETY: one overlapped ReadFile on a connected pipe carrying no data, so
    // it stays pending until cancelled; `buffer` outlives the wait below.
    let submitted = unsafe {
        tp.submit(operation, |handle, ov| {
            issue_read(handle, ov, buf_ptr, buf_len)
        })
    };

    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected a pending submission, got {other:?}"),
    };

    assert_eq!(tp.outstanding(), 1, "the read must still be outstanding");
    assert_eq!(
        recorder.len(),
        0,
        "a pending read must not have called back"
    );

    tp.cancel(id).expect("cancel the pending read");

    let records = recorder.wait_for(1);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].identity, id.as_ptr() as usize);
    assert_eq!(
        records[0].io_result, ERROR_OPERATION_ABORTED,
        "a cancelled operation must complete as ERROR_OPERATION_ABORTED"
    );
    assert_eq!(records[0].payload, 7);

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
}

/// `cancel_all` aborts every outstanding operation on the endpoint, and each one
/// still reports its own completion.
#[test]
fn cancel_all_aborts_every_outstanding_operation() {
    const OPERATIONS: usize = 32;

    let (endpoint, _client) = pending_pipe("cancel-all");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        endpoint,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    let mut identities = HashSet::new();
    for slot in 0..OPERATIONS {
        let operation = Operation::new(slot);
        // SAFETY: one 1-byte overlapped ReadFile per slot on a pipe carrying no
        // data; each slot is disjoint and `landed` outlives every operation.
        let submitted = unsafe {
            tp.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(slot), 1)
            })
        };
        match submitted {
            Submitted::Pending(id) => {
                identities.insert(id.as_ptr() as usize);
            }
            other => panic!("expected pending at slot {slot}, got {other:?}"),
        }
    }

    assert_eq!(tp.outstanding(), OPERATIONS);

    tp.cancel_all().expect("cancel every outstanding read");

    let records = recorder.wait_for(OPERATIONS);
    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
    assert_eq!(records.len(), OPERATIONS);

    let mut payloads = HashSet::new();
    for record in &records {
        assert!(
            identities.contains(&record.identity),
            "unknown identity {:#x}",
            record.identity
        );
        assert_eq!(
            record.io_result, ERROR_OPERATION_ABORTED,
            "slot {} was not aborted",
            record.payload
        );
        assert!(payloads.insert(record.payload), "a slot completed twice");
    }
    assert_eq!(payloads.len(), OPERATIONS);
}

/// Cancelling when nothing is outstanding is a benign no-op that reports
/// `ERROR_NOT_FOUND` rather than corrupting the accounting.
#[test]
fn cancel_all_with_nothing_outstanding_is_benign() {
    let (endpoint, _client) = pending_pipe("cancel-empty");
    let tp = ThreadpoolIo::new(endpoint, |_| {}, None).expect("create TP_IO");

    // Either outcome is acceptable; what matters is that nothing is disturbed.
    let _ = tp.cancel_all();

    assert_eq!(tp.outstanding(), 0);
    tp.run_down();
    tp.wait();
    assert_eq!(tp.outstanding(), 0);
}

// --- matrix state 5: rundown with operations outstanding ---

/// Dropping the object with operations outstanding must cancel them, wait for
/// the resulting callbacks, and terminate -- never free storage the kernel still
/// owns, and never block forever.
#[test]
fn drop_with_operations_outstanding_cancels_drains_and_terminates() {
    const OPERATIONS: usize = 16;

    let (endpoint, _client) = pending_pipe("drop-outstanding");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);
    let dropped = Arc::new(AtomicUsize::new(0));
    let payload_dropped = Arc::clone(&dropped);

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    {
        let tp = ThreadpoolIo::new(
            endpoint,
            move |completion: &IoCompletion| {
                // SAFETY: only Operation<DropTracked> is submitted below, claimed once.
                let operation = unsafe { completion.claim::<DropTracked>() };
                seen.push(Record {
                    identity: completion.overlapped_ptr() as usize,
                    io_result: completion.io_result(),
                    bytes: completion.bytes_transferred(),
                    payload: operation.payload().index,
                });
            },
            None,
        )
        .expect("create TP_IO");

        for slot in 0..OPERATIONS {
            let operation = Operation::new(DropTracked {
                index: slot,
                dropped: Arc::clone(&payload_dropped),
            });
            // SAFETY: one 1-byte overlapped ReadFile per slot on a pipe carrying
            // no data, so each stays pending; `landed` outlives the whole block
            // because Drop below blocks until every callback has run.
            let submitted = unsafe {
                tp.submit(operation, |handle, ov| {
                    issue_read(handle, ov, base.add(slot), 1)
                })
            };
            assert!(
                matches!(submitted, Submitted::Pending(_)),
                "slot {slot}: expected pending, got {submitted:?}"
            );
        }

        assert_eq!(tp.outstanding(), OPERATIONS, "reads must be outstanding");

        // Drop here, with every operation still outstanding. This must cancel,
        // drain, and return rather than deadlock or leak.
    }

    let records = recorder.records();
    assert_eq!(
        records.len(),
        OPERATIONS,
        "rundown must let every outstanding operation call back"
    );
    for record in &records {
        assert_eq!(
            record.io_result, ERROR_OPERATION_ABORTED,
            "slot {} should have been aborted by rundown",
            record.payload
        );
    }
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        OPERATIONS,
        "every claimed payload must have been dropped"
    );
}

/// The voluntary path -- `cancel_all` then `run_down` -- reaches the same state
/// as `Drop`, but under the caller's control.
#[test]
fn explicit_cancel_all_then_run_down_drains_every_operation() {
    const OPERATIONS: usize = 24;

    let (endpoint, _client) = pending_pipe("explicit-rundown");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        endpoint,
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    for slot in 0..OPERATIONS {
        let operation = Operation::new(slot);
        // SAFETY: one 1-byte overlapped ReadFile per slot on a pipe carrying no
        // data; `landed` outlives every operation because run_down blocks below.
        let submitted = unsafe {
            tp.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(slot), 1)
            })
        };
        assert!(matches!(submitted, Submitted::Pending(_)));
    }

    assert_eq!(tp.outstanding(), OPERATIONS);

    tp.cancel_all().expect("cancel every outstanding read");
    tp.run_down();

    assert_eq!(tp.outstanding(), 0, "run_down must drain completely");
    assert_eq!(recorder.len(), OPERATIONS, "every operation must call back");

    // Dropping after a completed rundown must not block or double-free.
    drop(tp);
}

// --- reclamation invariants ---

/// A callback that claims nothing still reclaims the operation, freeing its
/// payload through the type-erased thunk armed at submission.
#[test]
fn unclaimed_completions_reclaim_their_payload() {
    const OPERATIONS: usize = 256;

    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "unclaimed");

    let dropped = Arc::new(AtomicUsize::new(0));
    let payload_dropped = Arc::clone(&dropped);
    let observed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&observed);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |_completion: &IoCompletion| {
            // Deliberately claim nothing: the completion's own drop must reclaim
            // the operation generically.
            counter.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    for slot in 0..OPERATIONS {
        let mut operation = Operation::new(DropTracked {
            index: slot,
            dropped: Arc::clone(&payload_dropped),
        });
        operation.set_offset(slot as u64);
        // SAFETY: one 1-byte overlapped ReadFile into this slot's own byte;
        // `landed` outlives every operation because run_down blocks below.
        let submitted = unsafe {
            tp.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(slot), 1)
            })
        };
        assert!(
            matches!(submitted, Submitted::Pending(_)),
            "slot {slot}: expected pending, got {submitted:?}"
        );
    }

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
    assert_eq!(observed.load(Ordering::SeqCst), OPERATIONS);
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        OPERATIONS,
        "an unclaimed completion must still free its payload"
    );

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// One object may carry operations of different payload types simultaneously,
/// because unclaimed reclamation reads the thunk armed for each operation's own
/// payload type rather than assuming a single `P`.
#[test]
fn mixed_payload_types_reclaim_generically() {
    const PAIRS: usize = 100;
    const OPERATIONS: usize = PAIRS * 2;

    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "mixed-payloads");

    let narrow_dropped = Arc::new(AtomicUsize::new(0));
    let wide_dropped = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&observed);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |_completion: &IoCompletion| {
            // The callback cannot know which payload type this completion
            // carries, so it claims nothing and lets the armed thunk decide.
            counter.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    for pair in 0..PAIRS {
        let narrow_slot = pair * 2;
        let mut narrow = Operation::new(DropTracked {
            index: narrow_slot,
            dropped: Arc::clone(&narrow_dropped),
        });
        narrow.set_offset(narrow_slot as u64);
        // SAFETY: one 1-byte overlapped ReadFile into this slot's own byte.
        let submitted = unsafe {
            tp.submit(narrow, |handle, ov| {
                issue_read(handle, ov, base.add(narrow_slot), 1)
            })
        };
        assert!(matches!(submitted, Submitted::Pending(_)));

        let wide_slot = pair * 2 + 1;
        let mut wide = Operation::new(DropTrackedWide {
            _filler: [0; 16],
            dropped: Arc::clone(&wide_dropped),
        });
        wide.set_offset(wide_slot as u64);
        // SAFETY: one 1-byte overlapped ReadFile into this slot's own byte.
        let submitted = unsafe {
            tp.submit(wide, |handle, ov| {
                issue_read(handle, ov, base.add(wide_slot), 1)
            })
        };
        assert!(matches!(submitted, Submitted::Pending(_)));
    }

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);
    assert_eq!(observed.load(Ordering::SeqCst), OPERATIONS);
    assert_eq!(
        narrow_dropped.load(Ordering::SeqCst),
        PAIRS,
        "narrow payloads must be reclaimed with their own thunk"
    );
    assert_eq!(
        wide_dropped.load(Ordering::SeqCst),
        PAIRS,
        "wide payloads must be reclaimed with their own thunk"
    );

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

// --- edge cases ---

/// A panicking callback must not unwind into the pool, and must still balance
/// its start so rundown can complete.
///
/// The caught panic prints to stderr; that output is expected.
#[test]
fn panicking_callback_is_contained_and_still_balances_accounting() {
    const OPERATIONS: usize = 8;

    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "panicking");

    let observed = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&observed);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |_completion: &IoCompletion| {
            counter.fetch_add(1, Ordering::SeqCst);
            panic!("callback panics on purpose");
        },
        None,
    )
    .expect("create TP_IO");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    for slot in 0..OPERATIONS {
        let mut operation = Operation::new(slot);
        operation.set_offset(slot as u64);
        // SAFETY: one 1-byte overlapped ReadFile into this slot's own byte;
        // `landed` outlives every operation because run_down blocks below.
        let submitted = unsafe {
            tp.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(slot), 1)
            })
        };
        assert!(matches!(submitted, Submitted::Pending(_)));
    }

    // If the panic escaped or skipped the balance, this would hang or abort.
    tp.run_down();
    assert_eq!(
        tp.outstanding(),
        0,
        "a panicking callback unbalanced a start"
    );
    assert_eq!(observed.load(Ordering::SeqCst), OPERATIONS);

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// A read starting past end-of-file transfers nothing; whichever way Windows
/// reports it, the accounting must balance.
#[test]
fn read_past_end_of_file_still_balances_accounting() {
    let content = b"short";
    let path = temp_file_with(content, "past-eof");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<()> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<()>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: 0,
            });
        },
        None,
    )
    .expect("create TP_IO");

    let mut buffer = [0_u8; 16];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;

    let mut operation = Operation::new(());
    operation.set_offset(4096);

    // SAFETY: one overlapped ReadFile well past EOF; `buffer` outlives the wait.
    let submitted = unsafe {
        tp.submit(operation, |handle, ov| {
            issue_read(handle, ov, buf_ptr, buf_len)
        })
    };

    match submitted {
        Submitted::Pending(_) => {
            let records = recorder.wait_for(1);
            assert_eq!(records[0].bytes, 0, "a read past EOF transfers nothing");
        }
        Submitted::Failed { .. } => {
            assert_eq!(recorder.len(), 0, "a failed submission must not call back");
        }
        Submitted::Completed {
            bytes_transferred, ..
        } => {
            assert_eq!(bytes_transferred, 0, "a read past EOF transfers nothing");
        }
    }

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// A zero-length read completes without transferring anything and is accounted
/// for like any other operation.
#[test]
fn zero_length_read_completes_and_balances_accounting() {
    let path = temp_file_with(b"zero length read", "zero-length");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<()> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<()>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: 0,
            });
        },
        None,
    )
    .expect("create TP_IO");

    let mut buffer = [0_u8; 8];
    let buf_ptr = buffer.as_mut_ptr();

    let mut operation = Operation::new(());
    operation.set_offset(0);

    // SAFETY: one zero-length overlapped ReadFile; `buffer` outlives the wait.
    let submitted =
        unsafe { tp.submit(operation, |handle, ov| issue_read(handle, ov, buf_ptr, 0)) };

    match submitted {
        Submitted::Pending(_) => {
            let records = recorder.wait_for(1);
            assert_eq!(records[0].bytes, 0);
        }
        Submitted::Completed {
            bytes_transferred, ..
        } => assert_eq!(bytes_transferred, 0),
        Submitted::Failed { error, .. } => panic!("zero-length read failed: {error}"),
    }

    tp.run_down();
    assert_eq!(tp.outstanding(), 0);

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// An object created with an explicit callback environment behaves identically.
#[test]
fn operations_run_under_an_explicit_callback_environment() {
    const OPERATIONS: usize = 16;

    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "callback-env");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let mut env = CallbackEnviron::new();
    env.set_runs_long();

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<usize> is submitted below, claimed once.
            let operation = unsafe { completion.claim::<usize>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: *operation.payload(),
            });
        },
        Some(&mut env),
    )
    .expect("create TP_IO with a callback environment");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    for slot in 0..OPERATIONS {
        let mut operation = Operation::new(slot);
        operation.set_offset(slot as u64);
        // SAFETY: one 1-byte overlapped ReadFile into this slot's own byte.
        let submitted = unsafe {
            tp.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(slot), 1)
            })
        };
        assert!(matches!(submitted, Submitted::Pending(_)));
    }

    let records = recorder.wait_for(OPERATIONS);
    tp.run_down();
    assert_eq!(tp.outstanding(), 0);

    let payloads: HashSet<usize> = records.iter().map(|record| record.payload).collect();
    assert_eq!(payloads.len(), OPERATIONS);
    for (slot, byte) in landed.iter().enumerate() {
        assert_eq!(*byte, slot as u8);
    }

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// An object that never submits anything runs down and drops cleanly.
#[test]
fn object_with_no_submissions_runs_down_and_drops_cleanly() {
    let path = temp_file_with(b"unused", "no-submissions");

    let tp = ThreadpoolIo::new(read_endpoint(&path), |_| {}, None).expect("create TP_IO");
    assert_eq!(tp.outstanding(), 0);
    tp.run_down();
    tp.wait();
    assert_eq!(tp.outstanding(), 0);

    drop(tp);
    let _ = std::fs::remove_file(&path);
}

/// `run_down` and `wait` are idempotent once the object is quiescent.
#[test]
fn run_down_and_wait_are_idempotent_when_quiescent() {
    let content = b"idempotent rundown";
    let path = temp_file_with(content, "idempotent");
    let recorder = Recorder::new();
    let seen = Arc::clone(&recorder);

    let tp = ThreadpoolIo::new(
        read_endpoint(&path),
        move |completion: &IoCompletion| {
            // SAFETY: only Operation<()> is submitted below, claimed once.
            let _operation = unsafe { completion.claim::<()>() };
            seen.push(Record {
                identity: completion.overlapped_ptr() as usize,
                io_result: completion.io_result(),
                bytes: completion.bytes_transferred(),
                payload: 0,
            });
        },
        None,
    )
    .expect("create TP_IO");

    let mut buffer = [0_u8; 64];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;

    let mut operation = Operation::new(());
    operation.set_offset(0);
    // SAFETY: one overlapped ReadFile into `buffer`, which outlives the wait.
    let submitted = unsafe {
        tp.submit(operation, |handle, ov| {
            issue_read(handle, ov, buf_ptr, buf_len)
        })
    };
    assert!(matches!(submitted, Submitted::Pending(_)));

    recorder.wait_for(1);

    for _ in 0..5 {
        tp.run_down();
        tp.wait();
        assert_eq!(tp.outstanding(), 0);
    }
    assert_eq!(recorder.len(), 1, "rundown must not replay completions");

    drop(tp);
    let _ = std::fs::remove_file(&path);
}
