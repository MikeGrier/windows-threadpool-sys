// Copyright (c) 2026 Mike Grier
//! Thread-pool I/O (`TP_IO`): a completion backend over the shared overlapped
//! submission seam.
//!
//! [`ThreadpoolIo`] is the third completion backend for the overlapped model
//! defined by `windows-overlapped-io-sys`. It reuses that crate's endpoint
//! ownership and pinned [`Operation`] storage unchanged, and adds the two
//! concerns that only the thread pool has:
//!
//! - **Balanced accounting.** `StartThreadpoolIo` must precede every overlapped
//!   operation, and every start must be balanced exactly once -- by the I/O
//!   callback when a completion will be delivered, or by `CancelThreadpoolIo`
//!   when the submission failed immediately or completed synchronously on a
//!   handle in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode.
//! - **Callback-driven reclamation.** The pool, not the caller, dequeues
//!   completions. Each callback reclaims its operation's storage: typed through
//!   [`IoCompletion::claim`] when the payload type is known, or generically
//!   through the seam's `reclaim_overlapped` when the callback lets the
//!   completion drop.
//!
//! The pool's internal completion port is system-managed and is never exposed:
//! this backend neither posts to it nor dequeues from it.

use std::cell::Cell;
use std::fmt;
use std::io;
use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, OwnedHandle};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use windows_overlapped_io_sys::{
    Issued, Operation, OperationId, OperationRegistry, OperationState, Submitted,
    UnassociatedEndpoint, reclaim_overlapped,
};
use windows_sys::Win32::Foundation::{FALSE, HANDLE, NO_ERROR};
use windows_sys::Win32::System::IO::{CancelIoEx, OVERLAPPED};
use windows_sys::Win32::System::Threading::{
    CancelThreadpoolIo, CloseThreadpoolIo, CreateThreadpoolIo, PTP_CALLBACK_INSTANCE, PTP_IO,
    StartThreadpoolIo, WaitForThreadpoolIoCallbacks,
};

use crate::callback_env::CallbackEnviron;

/// Heap-allocated callback state kept alive for the lifetime of the `TP_IO`
/// object, and freed only after [`ThreadpoolIo::drop`] has drained every
/// operation and waited for every executing callback.
struct IoContext {
    live: Arc<OperationRegistry>,
    callback: Box<dyn Fn(&IoCompletion) + Send + Sync + 'static>,
}

/// Balances one `StartThreadpoolIo` when the callback frame ends, including
/// while unwinding, so a panicking callback cannot corrupt the accounting.
///
/// Removing the operation from the registry is what balances the start: the
/// registry's length is the number of unbalanced starts.
struct StartBalance<'state> {
    live: &'state OperationRegistry,
    overlapped: *mut OVERLAPPED,
}

impl Drop for StartBalance<'_> {
    fn drop(&mut self) {
        self.live.remove(self.overlapped);
    }
}

/// Trampoline from the raw `PTP_WIN32_IO_CALLBACK` ABI into the boxed closure.
///
/// SAFETY: `context` must point to a live [`IoContext`] for the entire duration
/// of every callback invocation, and `overlapped` must be the identity of an
/// operation submitted through [`ThreadpoolIo::submit`] whose storage has not
/// been reclaimed. [`ThreadpoolIo`]'s `Drop` ordering guarantees both.
unsafe extern "system" fn io_trampoline(
    _instance: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    overlapped: *mut core::ffi::c_void,
    io_result: u32,
    bytes_transferred: usize,
    _io: PTP_IO,
) {
    // SAFETY: context is a valid *mut IoContext for the full callback duration (see Drop).
    let ctx = unsafe { &*(context as *const IoContext) };

    let overlapped = overlapped.cast::<OVERLAPPED>();

    // A panic must never unwind across the FFI boundary into the pool's frame.
    // Inside the guarded frame, locals drop in reverse declaration order, so the
    // operation's storage is reclaimed first and the start is balanced second --
    // on the normal path and while unwinding alike. Balancing last is what makes
    // "outstanding reached zero" mean "no storage is still pool-owned".
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _balance = StartBalance {
            live: &ctx.live,
            overlapped,
        };
        let completion = IoCompletion {
            overlapped,
            // Read the generation while the operation is still registered, so
            // the callback can name it exactly; the balance above deregisters it
            // only after the callback has returned.
            generation: ctx.live.generation_of(overlapped),
            io_result,
            bytes_transferred,
            claimed: Cell::new(false),
        };
        (ctx.callback)(&completion);
    }));
}

/// An owned thread-pool I/O object bound to one overlapped endpoint.
///
/// Creating a `ThreadpoolIo` consumes an [`UnassociatedEndpoint`], which is the
/// same one-time, consuming association the other backends use: `CreateThreadpoolIo`
/// binds the handle to the pool's internal completion port, and no second
/// association is possible afterward.
///
/// Every [`ThreadpoolIo::submit`] is paired with exactly one balancing action, so
/// [`ThreadpoolIo::outstanding`] is always the number of operations whose storage
/// the kernel or pool still owns. `Drop` never frees that storage early: it
/// cancels what is outstanding, waits for the resulting callbacks, waits for any
/// callback still executing, and only then releases the object, the handle, and
/// the callback context.
pub struct ThreadpoolIo {
    tp_io: PTP_IO,
    handle: OwnedHandle,
    // Kept alive as a raw pointer until Drop has drained and waited.
    context: *mut IoContext,
    live: Arc<OperationRegistry>,
}

// SAFETY: PTP_IO is a cross-thread pool object and OwnedHandle is Send + Sync.
// `context` points to an IoContext whose callback is Fn + Send + Sync and whose
// registry is an Arc<OperationRegistry>; it is only read (never reassigned)
// until Drop frees it after all callbacks have finished.
unsafe impl Send for ThreadpoolIo {}
unsafe impl Sync for ThreadpoolIo {}

impl ThreadpoolIo {
    /// Bind an overlapped endpoint to the thread pool, invoking `callback` for
    /// every operation completion the pool delivers.
    ///
    /// Pass `Some(env)` to select a private pool, cleanup group, or callback
    /// priority; `None` uses the process-default pool with default priority.
    ///
    /// The callback runs on a shared, process-managed pool thread. It must
    /// restore any thread state it changes, must not terminate its thread, and
    /// must not block waiting on this object's rundown. A panic inside it is
    /// caught at the FFI boundary rather than unwinding into the pool, and the
    /// operation's storage is still reclaimed.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateThreadpoolIo`, most commonly when the
    /// handle was not opened for overlapped I/O.
    pub fn new<F>(
        endpoint: UnassociatedEndpoint,
        callback: F,
        env: Option<&mut CallbackEnviron>,
    ) -> io::Result<Self>
    where
        F: Fn(&IoCompletion) + Send + Sync + 'static,
    {
        let handle = endpoint.into_handle();
        let live = Arc::new(OperationRegistry::new());
        let context = Box::into_raw(Box::new(IoContext {
            live: Arc::clone(&live),
            callback: Box::new(callback),
        }));

        let env_ptr = env.map_or(ptr::null_mut(), |e| e.as_mut_ptr());

        // SAFETY: the handle is a live overlapped endpoint that no other backend
        // has associated, context is a valid heap pointer that outlives every
        // callback, and env_ptr is valid (or null) for the duration of this call.
        let tp_io = unsafe {
            CreateThreadpoolIo(
                handle.as_raw_handle(),
                Some(io_trampoline),
                context.cast(),
                env_ptr.cast_const(),
            )
        };

        if tp_io == 0 {
            let error = io::Error::last_os_error();
            // SAFETY: the pool never saw context; reclaim it immediately.
            unsafe { drop(Box::from_raw(context)) };
            return Err(error);
        }

        Ok(Self {
            tp_io,
            handle,
            context,
            live,
        })
    }

    /// Borrow the underlying handle for issuing native operations.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// The number of submitted operations whose storage has not yet been
    /// reclaimed, which is also the number of unbalanced `StartThreadpoolIo`
    /// calls.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.live.len()
    }

    /// Submit an owned operation on this endpoint.
    ///
    /// `StartThreadpoolIo` is issued before `issue` runs, as the SDK requires,
    /// and is balanced exactly once: by the I/O callback on the
    /// [`Issued::Pending`] path, or by `CancelThreadpoolIo` here on the
    /// synchronous-completion and immediate-failure paths.
    ///
    /// `issue` performs the single native overlapped call using the endpoint's
    /// handle and the operation's stable `OVERLAPPED` pointer, and classifies the
    /// outcome as an [`Issued`]: [`Issued::Pending`] when the pool will deliver a
    /// completion callback, or [`Issued::Completed`] when the call finished
    /// synchronously and no callback will arrive -- the outcome a handle in
    /// `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode reports on synchronous
    /// success. It returns `Err` for an immediate failure that yields no
    /// callback.
    ///
    /// On the pending path the operation's storage is transferred out and is
    /// recovered later inside the callback with [`IoCompletion::claim`]. On the
    /// synchronous and failure paths the operation is returned intact through
    /// [`Submitted`] so its storage can be reused or inspected.
    ///
    /// # Safety
    ///
    /// `issue` must start exactly one overlapped operation using the provided
    /// `OVERLAPPED` pointer and no other storage, and must classify the outcome
    /// correctly: [`Issued::Pending`] only when a completion callback will be
    /// delivered for this object, [`Issued::Completed`] only when the operation
    /// is already complete and no callback will arrive, and `Err` only when the
    /// submission failed and no callback will arrive. Misclassifying either
    /// unbalances the pool's accounting or frees storage the kernel still owns.
    pub unsafe fn submit<P, F>(&self, operation: Operation<P>, issue: F) -> Submitted<P>
    where
        P: Send,
        F: FnOnce(BorrowedHandle<'_>, *mut OVERLAPPED) -> io::Result<Issued>,
    {
        // Transfer the operation's storage out; the kernel owns it until it is
        // reclaimed. `into_overlapped` arms the type-erased reclaim thunk.
        let overlapped = operation.into_overlapped();
        // Stamp this submission with a fresh generation, so the identity names
        // this operation and not whatever later operation may reuse the address.
        let id = OperationId::mint(overlapped);

        // Register before starting so a callback can never race ahead of the
        // accounting; the registry's length is the unbalanced-start count.
        self.live.insert(id);
        // SAFETY: tp_io is valid for the lifetime of self. This start is balanced
        // exactly once on every path below.
        unsafe { StartThreadpoolIo(self.tp_io) };

        match issue(self.handle(), overlapped) {
            Ok(Issued::Pending) => Submitted::Pending(id),
            Ok(Issued::Completed { bytes_transferred }) => {
                // SAFETY: no callback will arrive, so the start must be balanced
                // here or the pool would wait for a completion that never comes.
                unsafe { CancelThreadpoolIo(self.tp_io) };
                self.live.remove(overlapped);
                // SAFETY: the operation completed synchronously and no callback
                // will arrive, so the kernel is done with the storage; reclaim
                // the box we just leaked, exactly once.
                let mut operation = unsafe { Operation::<P>::from_overlapped(overlapped) };
                operation.set_state(OperationState::Completed);
                Submitted::Completed {
                    operation,
                    bytes_transferred,
                }
            }
            Err(error) => {
                // SAFETY: the submission failed and no callback will arrive, so
                // the start must be balanced here.
                unsafe { CancelThreadpoolIo(self.tp_io) };
                self.live.remove(overlapped);
                // SAFETY: no callback will arrive, so reclaim the operation we
                // just leaked, exactly once.
                let mut operation = unsafe { Operation::<P>::from_overlapped(overlapped) };
                operation.set_state(OperationState::Idle);
                Submitted::Failed { operation, error }
            }
        }
    }

    /// Request cancellation of a single outstanding operation.
    ///
    /// Cancellation is only a request: the operation still completes, typically
    /// with `ERROR_OPERATION_ABORTED`, and that completion callback remains the
    /// point at which its storage is reclaimed.
    ///
    /// The identity is checked against this object's live operations first. An
    /// identity whose operation has already completed is rejected with
    /// [`io::ErrorKind::NotFound`] and no native call is made, even if another
    /// operation has since been given the same storage address. Cancelling
    /// therefore races safely against completion: the worst outcome of a late
    /// cancel is this error, never the cancellation of an unrelated operation.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::NotFound`] if `id` no longer names a live
    /// operation, or the error from `CancelIoEx` if the native request fails.
    pub fn cancel(&self, id: OperationId) -> io::Result<()> {
        if !self.live.is_live(id) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the operation named by this identity is no longer outstanding",
            ));
        }
        // SAFETY: cancelling by a valid handle and an OVERLAPPED identity that
        // the registry just confirmed still names a live operation.
        let ok = unsafe { CancelIoEx(self.raw_handle(), id.as_ptr()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Request cancellation of every outstanding operation on this endpoint.
    ///
    /// # Errors
    ///
    /// Returns the error from `CancelIoEx`, which reports `ERROR_NOT_FOUND` when
    /// nothing was outstanding.
    pub fn cancel_all(&self) -> io::Result<()> {
        // SAFETY: a null OVERLAPPED cancels all operations on the handle.
        let ok = unsafe { CancelIoEx(self.raw_handle(), ptr::null()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Block until every outstanding operation has completed and been reclaimed.
    ///
    /// Every outstanding operation must already be cancelled or otherwise
    /// destined to complete -- which [`ThreadpoolIo::cancel_all`] guarantees --
    /// or this waits indefinitely. Each completion callback reclaims its own
    /// operation, so rundown is just waiting for the count to reach zero.
    pub fn run_down(&self) {
        self.live.wait_until_empty();
    }

    /// Block until no I/O callback for this object is executing.
    ///
    /// This waits for callbacks that have already started; it does not cancel
    /// pending ones. There is deliberately no cancelling variant: cancelling a
    /// pending I/O callback would neither cancel the underlying operation nor
    /// make its `OVERLAPPED` storage safe to free, so the only sound way to stop
    /// outstanding I/O is [`ThreadpoolIo::cancel_all`] followed by
    /// [`ThreadpoolIo::run_down`].
    pub fn wait(&self) {
        // SAFETY: tp_io is valid for the lifetime of self; FALSE never cancels
        // pending callbacks, so no operation's storage is orphaned.
        unsafe { WaitForThreadpoolIoCallbacks(self.tp_io, FALSE) };
    }

    fn raw_handle(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

impl fmt::Debug for ThreadpoolIo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadpoolIo")
            .field("outstanding", &self.outstanding())
            .finish_non_exhaustive()
    }
}

impl Drop for ThreadpoolIo {
    fn drop(&mut self) {
        let count = self.outstanding();
        if count > 0 {
            // A blocking Drop signals that rundown was skipped. Report it from
            // this single site, then make the block terminate: because this
            // object owns the handle, cancelling guarantees every outstanding
            // operation delivers its callback.
            eprintln!(
                "windows-threadpool-sys: ThreadpoolIo dropped with {count} operation(s) still \
                 outstanding; call cancel_all() and run_down() before dropping to control when \
                 this blocks."
            );
            let _ = self.cancel_all();
            self.live.wait_until_empty();
        }

        // The count reaches zero inside the last callback, just before it
        // returns, so a callback frame may still be live here. Both calls below
        // must happen before the context is freed and before the handle closes.
        //
        // SAFETY: tp_io is valid and no operation is outstanding, so waiting
        // without cancelling cannot orphan any storage, and closing the object
        // is legal once its callbacks have finished.
        unsafe {
            WaitForThreadpoolIoCallbacks(self.tp_io, FALSE);
            CloseThreadpoolIo(self.tp_io);
        }

        // SAFETY: the TP_IO object is closed and every callback has finished, so
        // nothing can reach the context again; free it exactly once. `handle`
        // closes after this, when its field is dropped.
        unsafe { drop(Box::from_raw(self.context)) };
    }
}

/// One operation completion delivered to a [`ThreadpoolIo`] callback.
///
/// The completion borrows the operation's storage for the duration of the
/// callback. Recover the owned operation with [`IoCompletion::claim`] when the
/// payload type is known; otherwise the storage is reclaimed generically when
/// the callback returns, which is what lets one object carry operations of mixed
/// payload types.
pub struct IoCompletion {
    overlapped: *mut OVERLAPPED,
    /// The generation of the completing operation, read from the registry while
    /// it was still registered. `None` only if the entry was already gone, which
    /// a correctly-operating backend does not produce.
    generation: Option<u64>,
    io_result: u32,
    bytes_transferred: usize,
    claimed: Cell<bool>,
}

impl fmt::Debug for IoCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoCompletion")
            .field("overlapped", &self.overlapped)
            .field("generation", &self.generation)
            .field("io_result", &self.io_result)
            .field("bytes_transferred", &self.bytes_transferred)
            .finish_non_exhaustive()
    }
}

impl IoCompletion {
    /// The number of bytes transferred by the operation.
    #[must_use]
    pub fn bytes_transferred(&self) -> usize {
        self.bytes_transferred
    }

    /// The raw Win32 result the pool reported, `NO_ERROR` on success.
    #[must_use]
    pub fn io_result(&self) -> u32 {
        self.io_result
    }

    /// The failure of the completed operation, if it did not succeed.
    ///
    /// A cancelled operation completes here as `ERROR_OPERATION_ABORTED`.
    #[must_use]
    pub fn error(&self) -> Option<io::Error> {
        if self.io_result == NO_ERROR {
            return None;
        }
        Some(io::Error::from_raw_os_error(self.io_result as i32))
    }

    /// The identity of the completed operation, as returned by
    /// [`ThreadpoolIo::submit`].
    ///
    /// It compares equal to the [`OperationId`] that submission returned, so a
    /// caller holding submission-time identities can match a completion against
    /// them directly.
    #[must_use]
    pub fn id(&self) -> Option<OperationId> {
        // The registry recorded this generation for this address at submission,
        // so pairing them reproduces the identity that submit returned.
        Some(OperationId::from_parts(self.overlapped, self.generation?))
    }

    /// The `OVERLAPPED` pointer identifying the completed operation.
    #[must_use]
    pub fn overlapped_ptr(&self) -> *mut OVERLAPPED {
        self.overlapped
    }

    /// Recover the owned operation whose completion this is.
    ///
    /// The operation is returned in [`OperationState::Completed`] whether or not
    /// it succeeded, matching the raw IOCP backend; read [`IoCompletion::error`]
    /// for the outcome.
    ///
    /// # Safety
    ///
    /// This completion must be for an `Operation<P>` of this exact type
    /// submitted through [`ThreadpoolIo::submit`], and it must be claimed at
    /// most once.
    pub unsafe fn claim<P>(&self) -> Operation<P> {
        // Mark claimed so this completion's own drop will not also reclaim it.
        self.claimed.set(true);
        // SAFETY: by this function's contract, the identity is a matching leaked
        // Operation<P>, reclaimed exactly once here.
        let mut operation = unsafe { Operation::<P>::from_overlapped(self.overlapped) };
        operation.set_state(OperationState::Completed);
        operation
    }
}

impl Drop for IoCompletion {
    fn drop(&mut self) {
        // A claimed completion handed ownership to the callback.
        if self.claimed.get() || self.overlapped.is_null() {
            return;
        }
        // SAFETY: the callback arrived, so the kernel and pool are done with the
        // storage; the operation's armed reclaim thunk frees the box exactly once.
        unsafe { reclaim_overlapped(self.overlapped) };
    }
}

#[cfg(test)]
mod tests;
