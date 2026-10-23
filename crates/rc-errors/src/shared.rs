// Copyright (c) 2026 Mike Grier
//! The shared error (RCE-D-4, RCE-D-5): one block holding a `u64` count, the allocator that made
//! it, and the error. No weak references, so the error is dropped and the block freed together,
//! by whichever reference is dropped last, through the allocator recorded in the block.

use crate::allocator::{AllocError, Allocator, Global};
use core::alloc::Layout;
use core::error::Error;
use core::fmt;
use core::ops::Deref;
use core::ptr::{self, NonNull};
use core::sync::atomic::{self, AtomicU64, Ordering};

#[cfg(test)]
mod tests;

/// The error a [`SharedError`] holds, once its type is erased.
type DynError = dyn Error + Send + Sync;

/// The one allocation behind a [`SharedError`]. `repr(C)` makes the layout computed from the
/// erased form with `Layout::for_value` the documented layout of the block that was allocated
/// for the concrete error (RCE-D-4).
#[repr(C)]
struct Block<E: ?Sized> {
    count: AtomicU64,
    allocator: &'static (dyn Allocator + Sync),
    error: E,
}

/// An error shared by reference counting, made without the global allocator if the caller
/// chooses.
///
/// Cloning it shares the same error; the last clone dropped, on any thread, drops the error and
/// frees its block through the allocator that made it. It dereferences to
/// `dyn Error + Send + Sync`, so a caller reads the error, its source, and its concrete type
/// through `downcast_ref`, and it compares by identity: two `SharedError`s are equal when they
/// share one block.
///
/// ```
/// use rc_errors::SharedError;
///
/// #[derive(Debug)]
/// struct DiskFull;
/// impl core::fmt::Display for DiskFull {
///     fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
///         f.write_str("the disk is full")
///     }
/// }
/// impl core::error::Error for DiskFull {}
///
/// let error = SharedError::try_new(DiskFull).map_err(|(_, cause)| cause)?;
/// let shared = error.clone();
/// assert_eq!(shared, error);
/// assert_eq!(shared.to_string(), "the disk is full");
/// assert!(shared.downcast_ref::<DiskFull>().is_some());
/// # Ok::<(), rc_errors::AllocError>(())
/// ```
pub struct SharedError {
    block: NonNull<Block<DynError>>,
}

// SAFETY: the error is `Send + Sync`, the count is atomic, and the allocator is `Sync` and
// `'static`, so the block may be reached from, and freed on, any thread.
unsafe impl Send for SharedError {}
// SAFETY: as above; `&SharedError` hands out only `&dyn Error + Send + Sync` and atomic counting.
unsafe impl Sync for SharedError {}

impl SharedError {
    /// Shares `error`, allocating its block from the global allocator.
    ///
    /// # Errors
    ///
    /// When the block cannot be allocated, `error` is handed back, undropped, with the
    /// [`AllocError`]. Never panics on running out of memory.
    pub fn try_new<E>(error: E) -> Result<Self, (E, AllocError)>
    where
        E: Error + Send + Sync + 'static,
    {
        Self::try_new_in(error, &Global)
    }

    /// Shares `error`, allocating its block from `allocator`, which also frees it.
    ///
    /// # Errors
    ///
    /// When `allocator` cannot provide the block, `error` is handed back, undropped, with the
    /// [`AllocError`]. Never panics on running out of memory.
    pub fn try_new_in<E>(
        error: E,
        allocator: &'static (dyn Allocator + Sync),
    ) -> Result<Self, (E, AllocError)>
    where
        E: Error + Send + Sync + 'static,
    {
        let layout = Layout::new::<Block<E>>();
        let block = match allocator.allocate(layout) {
            Ok(block) => block.cast::<Block<E>>(),
            Err(cause) => return Err((error, cause)),
        };
        // SAFETY: the allocator promises a block valid for writes of `layout`'s size and aligned
        // to its alignment, which is `Block<E>`'s.
        unsafe {
            block.write(Block {
                count: AtomicU64::new(1),
                allocator,
                error,
            });
        }
        // The one unsizing: the concrete error's block, seen as the erased one (RCE-D-5).
        let block: NonNull<Block<DynError>> = block;
        Ok(Self { block })
    }

    /// Whether `a` and `b` share one block, which is what `==` asks.
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        ptr::addr_eq(a.block.as_ptr(), b.block.as_ptr())
    }

    fn block(&self) -> &Block<DynError> {
        // SAFETY: the block stays allocated and initialised while any `SharedError` holds it,
        // and this one does.
        unsafe { self.block.as_ref() }
    }
}

impl Clone for SharedError {
    fn clone(&self) -> Self {
        // `Relaxed`, as `Arc`'s: a new reference is made from one already held, so nothing
        // needs ordering against it. The count is a `u64` and moves by one, so it cannot
        // overflow (RCE-D-4).
        self.block().count.fetch_add(1, Ordering::Relaxed);
        Self { block: self.block }
    }
}

impl Drop for SharedError {
    fn drop(&mut self) {
        // `Release`, so this reference's use of the error happens before the free; the last
        // decrement's `Acquire` fence below pairs with every other reference's.
        if self.block().count.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        atomic::fence(Ordering::Acquire);
        let allocator = self.block().allocator;
        // Read while the error is still alive: its layout comes from its vtable.
        let layout = Layout::for_value(self.block());
        // SAFETY: the count reached zero, so this is the last reference: nothing else reads
        // the block. It was initialised by `try_new_in`, is dropped once here, and freed through
        // the allocator that made it with the layout it was made with (`repr(C)`, RCE-D-4).
        unsafe {
            ptr::drop_in_place(self.block.as_ptr());
            allocator.deallocate(self.block.cast(), layout);
        }
    }
}

impl Deref for SharedError {
    type Target = DynError;

    fn deref(&self) -> &DynError {
        &self.block().error
    }
}

impl fmt::Display for SharedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl fmt::Debug for SharedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl Error for SharedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        (**self).source()
    }
}

impl PartialEq for SharedError {
    fn eq(&self, other: &Self) -> bool {
        Self::ptr_eq(self, other)
    }
}

impl Eq for SharedError {}
