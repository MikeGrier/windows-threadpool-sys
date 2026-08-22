// Copyright (c) 2026 Mike Grier
//! Owned buffers an operation can borrow from the kernel's point of view.
//!
//! # Why owned, and not a slice
//!
//! Completion-based I/O touches the caller's memory *after* the submitting call
//! returns: `WriteFile` reads the buffer while the submitting thread has already
//! moved on, and the completion arrives later. A `&[u8]` cannot describe that.
//! Its borrow would have to span the whole operation, and nothing in the API can
//! make it: the submission token has no `Drop` that cancels, and even one would
//! be defeated by `mem::forget`, so a caller could always end the borrow with the
//! kernel still reading. A cancel-on-drop that *blocked* would be sound and would
//! also defeat the point of submitting asynchronously.
//!
//! So the buffer is handed over instead -- a protracted borrow, made out of
//! ownership rather than a lifetime. The operation holds it for exactly as long
//! as the kernel might touch it, and returns it on completion, through
//! `claim` or through [`crate::Started::Completed`]. The blocking adapters take
//! plain slices precisely because they do not have this problem: they block for
//! the whole operation, so an ordinary borrow provably covers it.
//!
//! # Why a trait, and not `Vec<u8>`
//!
//! Hardcoding `Vec<u8>` would mean a caller holding anything else -- a
//! `Box<[u8]>`, an `Arc<[u8]>`, an alignment-constrained buffer, one from a pool
//! -- has to convert, and every one of those conversions is a copy of exactly the
//! data this crate exists to move without copying. These traits let a caller hand
//! over whatever it already has.
//!
//! # Why `unsafe`
//!
//! The whole contract is a promise the compiler cannot check: the address must
//! stay put. A type whose accessor returns a fresh address on each call, or that
//! reallocates while the operation is in flight, is what makes the kernel write
//! into freed memory long after the call that started it returned. Implementing
//! these traits is asserting that cannot happen.

use std::sync::Arc;

/// An owned buffer an operation reads bytes **from** (a write, a send).
///
/// # Safety
///
/// Implementors guarantee that, for as long as the value is owned by an
/// operation:
///
/// - [`IoBuf::stable_ptr`] returns the same address every time it is called, and
///   that address does not change when the value is moved. This is what a
///   pointer-to-heap buffer gives for free and an inline array does not: moving a
///   `[u8; N]` moves its bytes.
/// - The [`IoBuf::bytes_len`] bytes starting at that address stay allocated,
///   initialized, and unmodified.
/// - [`IoBuf::bytes_len`] returns the same value every time it is called.
///
/// `Send` because the operation's storage is reclaimed by whichever thread
/// dequeues the completion, which is not the submitting one; `'static` because
/// that storage is leaked to the kernel and carries no lifetime to check.
pub unsafe trait IoBuf: Send + 'static {
    /// The address of the first byte. Must not change for the value's life.
    fn stable_ptr(&self) -> *const u8;

    /// How many bytes from [`IoBuf::stable_ptr`] the operation may read.
    fn bytes_len(&self) -> usize;
}

/// An owned buffer an operation writes bytes **into** (a read, a receive).
///
/// Separate from [`IoBuf`] because not every owned buffer can be written to: an
/// `Arc<[u8]>` is perfectly good to send *from* and can never be a destination,
/// since handing out `&mut` to shared bytes would be unsound. Requiring one trait
/// for both would either exclude shared buffers from writes or let them be used
/// as read destinations.
///
/// # Safety
///
/// As [`IoBuf`], and additionally: the value must have exclusive access to the
/// [`IoBuf::bytes_len`] bytes at [`IoBufMut::stable_mut_ptr`], which must be the
/// same address [`IoBuf::stable_ptr`] reports, so the kernel writing into them
/// cannot race or alias anything else.
///
/// Those bytes must already be **initialized**. This crate does not track an
/// initialized prefix: a caller-supplied buffer is initialized once and reused
/// for the life of a pool, so the cost is per-pool rather than per-operation, and
/// the API carries no `set_init`-style obligation to forget.
pub unsafe trait IoBufMut: IoBuf {
    /// The mutable address of the first byte. Must equal
    /// [`IoBuf::stable_ptr`] and must not change for the value's life.
    fn stable_mut_ptr(&mut self) -> *mut u8;
}

// SAFETY: a `Vec`'s bytes live in a heap allocation whose address is independent
// of where the `Vec` itself sits, so moving the `Vec` does not move them. Nothing
// here reallocates: the operation only reads and writes within `len`, never
// pushes. All `len` bytes are initialized by construction.
unsafe impl IoBuf for Vec<u8> {
    fn stable_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.len()
    }
}

// SAFETY: as the `IoBuf` impl; `&mut self` proves exclusive access, and
// `as_mut_ptr` returns the same allocation `as_ptr` does.
unsafe impl IoBufMut for Vec<u8> {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }
}

// SAFETY: a boxed slice is a heap allocation of fixed length; moving the `Box`
// moves the pointer, not the bytes. Its length cannot change at all.
unsafe impl IoBuf for Box<[u8]> {
    fn stable_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.len()
    }
}

// SAFETY: as above; `Box` is a unique owner, so `&mut self` is exclusive access.
unsafe impl IoBufMut for Box<[u8]> {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }
}

// SAFETY: the bytes live in the `Arc`'s allocation, which outlives every clone
// and never moves. Read-only by nature, which is why there is no `IoBufMut`
// counterpart: other clones may be reading the same bytes concurrently, so
// handing the kernel a writable pointer to them would alias.
unsafe impl IoBuf for Arc<[u8]> {
    fn stable_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.len()
    }
}

// SAFETY: a `'static` slice's referent is valid forever and never moves. Not
// writable for the same reason as `Arc<[u8]>`: the reference is shared.
unsafe impl IoBuf for &'static [u8] {
    fn stable_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.len()
    }
}

#[cfg(test)]
mod tests;
