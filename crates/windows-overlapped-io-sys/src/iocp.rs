//! Raw I/O completion port backend: port ownership, association, and dequeue.
//!
//! A [`CompletionPort`] owns a completion-port handle and can service many
//! endpoints, each associated with a caller-chosen completion key. Association
//! is the consuming transition that binds an [`UnassociatedEndpoint`] to this
//! backend. The port does not create worker threads; the owner decides where
//! [`CompletionPort::get`] runs. Submission of real overlapped operations, and
//! the reclamation that follows their completion, are built on top of this
//! module.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
use std::panic::Location;
use std::sync::{Arc, Mutex, MutexGuard};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT};
use windows_sys::Win32::System::IO::{
    CancelIoEx, CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
    PostQueuedCompletionStatus,
};

use crate::{Operation, OperationState, UnassociatedEndpoint};

/// Waits without timeout in `GetQueuedCompletionStatus`; used while draining.
const INFINITE: u32 = u32::MAX;

/// Per-operation bookkeeping the port keeps while an operation is in flight.
struct InFlight {
    /// Frees the leaked `Box<Operation<P>>` from its `OVERLAPPED` pointer.
    reclaim: unsafe fn(*mut OVERLAPPED),
    /// The call site that submitted the operation.
    location: &'static Location<'static>,
    #[cfg(feature = "operation-backtrace")]
    backtrace: std::backtrace::Backtrace,
}

/// State shared between a port, its in-flight operations, and their completions.
struct PortState {
    inflight: Mutex<HashMap<usize, InFlight>>,
}

impl PortState {
    fn new() -> Self {
        Self {
            inflight: Mutex::new(HashMap::new()),
        }
    }
}

/// Lock a mutex, recovering the guard even if a previous holder panicked.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Reclaim a leaked `Box<Operation<P>>` from its `OVERLAPPED` pointer.
///
/// # Safety
///
/// `overlapped` must be the base of a live `Box<Operation<P>>` reclaimed exactly
/// once.
unsafe fn reclaim_operation<P>(overlapped: *mut OVERLAPPED) {
    drop(unsafe { Box::from_raw(overlapped.cast::<Operation<P>>()) });
}

/// An owned I/O completion port.
pub struct CompletionPort {
    handle: OwnedHandle,
    state: Arc<PortState>,
}

impl CompletionPort {
    /// Create a new completion port.
    ///
    /// `concurrency` is the maximum number of threads the system lets run
    /// completions for this port concurrently; zero means one per processor.
    pub fn new(concurrency: u32) -> io::Result<Self> {
        // SAFETY: creating a fresh port with no associated file handle.
        let handle = unsafe {
            CreateIoCompletionPort(INVALID_HANDLE_VALUE, std::ptr::null_mut(), 0, concurrency)
        };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the call returned a fresh, exclusively owned port handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        Ok(Self {
            handle,
            state: Arc::new(PortState::new()),
        })
    }

    /// Associate an overlapped endpoint with this port under `key`.
    ///
    /// Completions for operations issued on the endpoint are delivered to this
    /// port and tagged with `key`. The association is permanent for the life of
    /// the handle, so the returned endpoint borrows the port.
    pub fn associate(
        &self,
        endpoint: UnassociatedEndpoint,
        key: usize,
    ) -> io::Result<AssociatedEndpoint<'_>> {
        let handle = endpoint.into_handle();
        // SAFETY: associating a valid handle with a valid port; the concurrency
        // argument is ignored when an existing port is supplied.
        let result = unsafe { CreateIoCompletionPort(handle.as_raw_handle(), self.raw(), key, 0) };
        if result.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(AssociatedEndpoint {
            port: self,
            handle,
            key,
        })
    }

    /// Post a user-defined completion packet to this port.
    ///
    /// The `overlapped` value is delivered verbatim and is not dereferenced, so
    /// it may be null or any sentinel the caller uses to distinguish wakeups.
    pub fn post(
        &self,
        key: usize,
        bytes_transferred: u32,
        overlapped: *mut OVERLAPPED,
    ) -> io::Result<()> {
        // SAFETY: the port handle is valid; the overlapped value is opaque here.
        let ok = unsafe {
            PostQueuedCompletionStatus(self.raw(), bytes_transferred, key, overlapped.cast_const())
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Dequeue one completion packet, waiting up to `timeout_ms` milliseconds.
    ///
    /// Returns `Ok(None)` when the wait times out with no packet. A packet is
    /// returned even when its operation failed; the failure is reported through
    /// [`Completion::error`].
    pub fn get(&self, timeout_ms: u32) -> io::Result<Option<Completion>> {
        let mut bytes_transferred: u32 = 0;
        let mut key: usize = 0;
        let mut overlapped: *mut OVERLAPPED = std::ptr::null_mut();
        // SAFETY: all out-parameters are valid for the duration of the call.
        let ok = unsafe {
            GetQueuedCompletionStatus(
                self.raw(),
                &mut bytes_transferred,
                &mut key,
                &mut overlapped,
                timeout_ms,
            )
        };
        if ok != 0 {
            return Ok(Some(Completion {
                key,
                bytes_transferred,
                overlapped,
                error: None,
                state: Arc::clone(&self.state),
            }));
        }

        let error = io::Error::last_os_error();
        if overlapped.is_null() {
            if error.raw_os_error() == Some(WAIT_TIMEOUT as i32) {
                return Ok(None);
            }
            return Err(error);
        }
        // A packet for a failed operation was dequeued.
        Ok(Some(Completion {
            key,
            bytes_transferred,
            overlapped,
            error: Some(error),
            state: Arc::clone(&self.state),
        }))
    }

    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }

    /// The number of operations submitted through this port that have not yet
    /// been claimed, reclaimed, or drained.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        lock(&self.state.inflight).len()
    }

    /// Block until every outstanding operation has completed and its storage has
    /// been reclaimed.
    ///
    /// Every outstanding operation must already be cancelled or otherwise
    /// destined to complete -- which closing or cancelling the endpoints
    /// guarantees -- or this waits indefinitely. Dequeued completions that are
    /// not claimed reclaim their own storage when dropped, so draining is just
    /// dequeuing until nothing remains.
    pub fn run_down(&self) -> io::Result<()> {
        while !lock(&self.state.inflight).is_empty() {
            // Dropping the dequeued completion reclaims one outstanding
            // operation, or is a no-op for a user packet.
            self.get(INFINITE)?;
        }
        Ok(())
    }

    fn report_outstanding_at_drop(&self, count: usize) {
        let guard = lock(&self.state.inflight);
        let mut message = format!(
            "windows-overlapped-io-sys: CompletionPort dropped with {count} operation(s) still \
             outstanding; call run_down() before dropping to control when this blocks. Sources:"
        );
        for inflight in guard.values() {
            message.push_str("\n  - ");
            message.push_str(&inflight.location.to_string());
            #[cfg(feature = "operation-backtrace")]
            {
                message.push_str("\n    backtrace:\n");
                message.push_str(&inflight.backtrace.to_string());
            }
        }
        eprintln!("{message}");
    }
}

impl fmt::Debug for CompletionPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompletionPort")
            .field("outstanding", &self.outstanding())
            .finish_non_exhaustive()
    }
}

impl Drop for CompletionPort {
    fn drop(&mut self) {
        let count = self.outstanding();
        if count == 0 {
            return;
        }
        // A blocking Drop signals that run_down() was skipped; name the sources.
        self.report_outstanding_at_drop(count);
        // Block until the kernel is done with every operation's storage.
        let _ = self.run_down();
    }
}

/// An overlapped endpoint bound to exactly one [`CompletionPort`].
///
/// The endpoint owns its handle and borrows the port it is associated with, so
/// the port cannot be dropped while any endpoint still routes completions to it.
/// It is intentionally not `Clone`.
#[derive(Debug)]
pub struct AssociatedEndpoint<'port> {
    port: &'port CompletionPort,
    handle: OwnedHandle,
    key: usize,
}

impl<'port> AssociatedEndpoint<'port> {
    /// Borrow the underlying handle for issuing native operations.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// The completion key packets from this endpoint are tagged with.
    #[must_use]
    pub fn key(&self) -> usize {
        self.key
    }

    /// The completion port this endpoint is associated with.
    #[must_use]
    pub fn port(&self) -> &'port CompletionPort {
        self.port
    }

    /// Submit an owned operation on this endpoint.
    ///
    /// `issue` performs the single native overlapped call using the endpoint's
    /// handle and the operation's stable `OVERLAPPED` pointer. It returns `Ok`
    /// when a completion will arrive (native success or `ERROR_IO_PENDING`) and
    /// `Err` for an immediate failure that yields no completion.
    ///
    /// On the completion path the operation's storage is transferred to the
    /// kernel and recovered later with [`Completion::claim`]. On the failure
    /// path the operation is returned intact so its storage can be reused.
    ///
    /// # Safety
    ///
    /// `issue` must start exactly one overlapped operation using the provided
    /// `OVERLAPPED` pointer and no other storage, and must classify the outcome
    /// correctly: `Ok` only when a completion packet will be delivered to this
    /// endpoint's port, `Err` only when none will.
    #[track_caller]
    pub unsafe fn submit<P, F>(&self, operation: Operation<P>, issue: F) -> Submitted<P>
    where
        P: Send,
        F: FnOnce(BorrowedHandle<'_>, *mut OVERLAPPED) -> io::Result<()>,
    {
        let location = Location::caller();
        let mut boxed = Box::new(operation);
        boxed.set_state(OperationState::Submitted);
        let overlapped = boxed.overlapped_ptr();
        let identity = overlapped as usize;

        // Register before issuing so a completion cannot arrive ahead of the
        // bookkeeping the port needs to reclaim or drain the operation.
        lock(&self.port.state.inflight).insert(
            identity,
            InFlight {
                reclaim: reclaim_operation::<P>,
                location,
                #[cfg(feature = "operation-backtrace")]
                backtrace: std::backtrace::Backtrace::capture(),
            },
        );

        match issue(self.handle(), overlapped) {
            Ok(()) => {
                boxed.set_state(OperationState::Pending);
                // Ownership moves to the kernel until the completion is
                // claimed, reclaimed, or drained.
                let _ = Box::into_raw(boxed);
                Submitted::Pending(OperationId(overlapped))
            }
            Err(error) => {
                lock(&self.port.state.inflight).remove(&identity);
                boxed.set_state(OperationState::Idle);
                Submitted::Failed {
                    operation: *boxed,
                    error,
                }
            }
        }
    }

    /// Request cancellation of a single outstanding operation.
    ///
    /// Cancellation is only a request: the operation still completes, typically
    /// with `ERROR_OPERATION_ABORTED`, and that completion remains the point at
    /// which its storage is reclaimed with [`Completion::claim`].
    pub fn cancel(&self, id: OperationId) -> io::Result<()> {
        // SAFETY: cancelling by a valid handle and an OVERLAPPED identity.
        let ok = unsafe { CancelIoEx(self.raw_handle(), id.as_ptr()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Request cancellation of every outstanding operation on this endpoint.
    pub fn cancel_all(&self) -> io::Result<()> {
        // SAFETY: a null OVERLAPPED cancels all operations on the handle.
        let ok = unsafe { CancelIoEx(self.raw_handle(), std::ptr::null()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn raw_handle(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

/// An identity for an in-flight operation: the address of its `OVERLAPPED`.
///
/// It is used to cancel a specific operation and to match its later completion.
/// The pointer must not be dereferenced or freed; the kernel owns the storage
/// until the completion is claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationId(*mut OVERLAPPED);

impl OperationId {
    /// The `OVERLAPPED` pointer this identity refers to.
    #[must_use]
    pub fn as_ptr(self) -> *mut OVERLAPPED {
        self.0
    }
}

/// The outcome of [`AssociatedEndpoint::submit`].
#[derive(Debug)]
pub enum Submitted<P> {
    /// A completion will arrive; the storage was transferred to the kernel and
    /// is recovered later with [`Completion::claim`]. The [`OperationId`]
    /// identifies the in-flight operation for cancellation and matching.
    Pending(OperationId),
    /// Submission failed immediately with no completion; the operation is
    /// returned so its storage can be reused or dropped.
    Failed {
        /// The operation whose submission failed.
        operation: Operation<P>,
        /// The immediate failure reported by the native call.
        error: io::Error,
    },
}

/// A completion packet dequeued from a [`CompletionPort`].
pub struct Completion {
    key: usize,
    bytes_transferred: u32,
    overlapped: *mut OVERLAPPED,
    error: Option<io::Error>,
    state: Arc<PortState>,
}

impl fmt::Debug for Completion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Completion")
            .field("key", &self.key)
            .field("bytes_transferred", &self.bytes_transferred)
            .field("overlapped", &self.overlapped)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        // A completion observed but never claimed still owns its operation
        // storage; reclaim it so rundown can complete.
        let entry = lock(&self.state.inflight).remove(&(self.overlapped as usize));
        if let Some(inflight) = entry {
            // SAFETY: the completion arrived, so the kernel is done with the
            // storage, and the entry was removed here exactly once.
            unsafe { (inflight.reclaim)(self.overlapped) };
        }
    }
}

impl Completion {
    /// The completion key the packet was tagged with.
    #[must_use]
    pub fn key(&self) -> usize {
        self.key
    }

    /// The number of bytes transferred by the operation.
    #[must_use]
    pub fn bytes_transferred(&self) -> u32 {
        self.bytes_transferred
    }

    /// The `OVERLAPPED` pointer identifying the completed operation.
    ///
    /// For a user packet this is whatever value was passed to
    /// [`CompletionPort::post`].
    #[must_use]
    pub fn overlapped_ptr(&self) -> *mut OVERLAPPED {
        self.overlapped
    }

    /// The failure of the completed operation, if it did not succeed.
    #[must_use]
    pub fn error(&self) -> Option<&io::Error> {
        self.error.as_ref()
    }

    /// Recover the owned operation whose completion this is.
    ///
    /// # Safety
    ///
    /// This completion must have been produced by submitting an `Operation<P>`
    /// of this exact type through [`AssociatedEndpoint::submit`], and it must be
    /// claimed exactly once.
    pub unsafe fn claim<P>(&self) -> Operation<P> {
        // Deregister first so rundown and this completion's own drop will not
        // also try to reclaim the operation.
        lock(&self.state.inflight).remove(&(self.overlapped as usize));
        let ptr = self.overlapped.cast::<Operation<P>>();
        // SAFETY: by this function's contract, `ptr` is the box leaked by a
        // matching submit and is reclaimed exactly once here.
        let mut operation = unsafe { *Box::from_raw(ptr) };
        operation.set_state(OperationState::Completed);
        operation
    }
}

#[cfg(test)]
mod tests {
    use super::{AssociatedEndpoint, CompletionPort, Submitted};
    use crate::{Operation, OperationState, UnassociatedEndpoint};
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::OwnedHandle;
    use std::path::PathBuf;

    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

    fn associate_temp_file<'port>(
        port: &'port CompletionPort,
        tag: &str,
    ) -> (AssociatedEndpoint<'port>, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "windows-overlapped-io-sys-{tag}-{}.tmp",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&path)
            .expect("create overlapped temp file");
        let owned = OwnedHandle::from(file);
        // SAFETY: the file was just created with FILE_FLAG_OVERLAPPED, is not
        // associated with any port, has no duplicates, and moves in exclusively.
        let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
        let associated = port.associate(endpoint, 0).expect("associate");
        (associated, path)
    }

    #[test]
    fn posts_and_dequeues_a_user_packet() {
        let port = CompletionPort::new(0).expect("create port");
        let sentinel = 0x1234_usize as *mut _;

        port.post(0xABCD, 42, sentinel).expect("post packet");
        let completion = port.get(1_000).expect("get packet").expect("a packet");

        assert_eq!(completion.key(), 0xABCD);
        assert_eq!(completion.bytes_transferred(), 42);
        assert_eq!(completion.overlapped_ptr(), sentinel);
        assert!(completion.error().is_none());
    }

    #[test]
    fn get_times_out_when_empty() {
        let port = CompletionPort::new(0).expect("create port");
        assert!(port.get(0).expect("get").is_none());
    }

    #[test]
    fn associates_an_overlapped_handle() {
        let port = CompletionPort::new(0).expect("create port");
        let path = std::env::temp_dir().join(format!(
            "windows-overlapped-io-sys-iocp-{}.tmp",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&path)
            .expect("create overlapped temp file");
        let owned = OwnedHandle::from(file);

        // SAFETY: the file was just created with FILE_FLAG_OVERLAPPED, is not
        // associated with any port, has no duplicates, and moves in exclusively.
        let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
        let associated = port.associate(endpoint, 0x55).expect("associate");
        assert_eq!(associated.key(), 0x55);

        drop(associated);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn submit_pending_then_claim_recovers_the_operation() {
        let port = CompletionPort::new(0).expect("create port");
        let (endpoint, path) = associate_temp_file(&port, "submit-pending");

        let operation = Operation::new(vec![1_u8, 2, 3]);
        // SAFETY: the closure issues exactly one operation using the given
        // OVERLAPPED pointer; here it simulates a device that queues a
        // completion for that pointer, so a packet will arrive.
        let submitted = unsafe {
            endpoint.submit(operation, |_handle, overlapped| {
                port.post(7, 3, overlapped)?;
                Ok(())
            })
        };
        assert!(matches!(submitted, Submitted::Pending(_)));
        let Submitted::Pending(id) = submitted else {
            unreachable!("just asserted pending");
        };
        assert_eq!(port.outstanding(), 1);

        let completion = port.get(1_000).expect("get").expect("a packet");
        assert_eq!(completion.overlapped_ptr(), id.as_ptr());
        // SAFETY: this completion is from the Operation<Vec<u8>> submitted above
        // and is claimed exactly once.
        let operation = unsafe { completion.claim::<Vec<u8>>() };
        assert_eq!(operation.state(), OperationState::Completed);
        assert_eq!(operation.payload(), &vec![1_u8, 2, 3]);
        assert_eq!(port.outstanding(), 0);

        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn submit_immediate_failure_returns_the_operation() {
        let port = CompletionPort::new(0).expect("create port");
        let (endpoint, path) = associate_temp_file(&port, "submit-fail");

        let operation = Operation::new(vec![9_u8]);
        // SAFETY: the closure issues no operation and reports an immediate
        // failure, so no completion will arrive.
        let submitted = unsafe {
            endpoint.submit(operation, |_handle, _overlapped| {
                Err(std::io::Error::from_raw_os_error(5))
            })
        };
        match submitted {
            Submitted::Failed { operation, error } => {
                assert_eq!(operation.payload(), &vec![9_u8]);
                assert_eq!(operation.state(), OperationState::Idle);
                assert_eq!(error.raw_os_error(), Some(5));
            }
            Submitted::Pending(_) => panic!("expected immediate failure"),
        }
        assert_eq!(port.outstanding(), 0);

        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unclaimed_completion_reclaims_on_drop() {
        let port = CompletionPort::new(0).expect("create port");
        let (endpoint, path) = associate_temp_file(&port, "unclaimed");

        let operation = Operation::new(vec![7_u8]);
        // SAFETY: the closure issues one operation using the given OVERLAPPED
        // pointer; here it simulates a device that queues its completion.
        let submitted = unsafe {
            endpoint.submit(operation, |_handle, overlapped| {
                port.post(0, 1, overlapped)?;
                Ok(())
            })
        };
        assert!(matches!(submitted, Submitted::Pending(_)));
        assert_eq!(port.outstanding(), 1);

        // Drop the completion without claiming it; its storage is reclaimed.
        let completion = port.get(1_000).expect("get").expect("a packet");
        drop(completion);
        assert_eq!(port.outstanding(), 0);

        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }
}
