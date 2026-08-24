// Copyright (c) 2026 Mike Grier
//! Owned buffers an operation (or a buffer registration) can hold.
//!
//! Duplicated from `windows-overlapped-io-sys`'s `IoBuf`/`IoBufMut` (D-1 in
//! `DESIGN-NOTES.md`): the reasoning below is identical, because it is about
//! memory and address stability, not about which completion mechanism reads
//! or writes through the pointer. The two crates share no code; see D-1 for
//! why, and M6+ for when that might change.
//!
//! # The contract is extended to cover registration (M2.1)
//!
//! `windows-overlapped-io-sys`'s version of this contract binds "for as long
//! as the value is owned by an operation." Here it binds for as long as this
//! crate holds the value at all, because that span is no longer always one
//! operation: `BuildIoRingRegisterBuffers` (M5) registers a buffer once and
//! reuses it, by index, across many later reads and writes. A buffer held by
//! a live registration must stay exactly as stable and valid as one held by
//! a single in-flight [`crate::Token`] -- the kernel can address it through
//! either path -- so the promise below is phrased in terms of "this crate
//! holds it," not "an operation holds it."
//!
//! # Why owned, and not a slice
//!
//! Completion-based I/O touches the caller's memory *after* the submitting
//! call returns. A `&[u8]` cannot describe that: its borrow would have to
//! span the whole operation, and nothing in the API can make it -- a
//! [`crate::Token`] has no `Drop` that cancels, and even one would be
//! defeated by `mem::forget`, so a caller could always end the borrow with
//! the kernel still reading. The buffer is handed over instead, and returned
//! on completion through `Token::claim_if`.
//!
//! # Why `unsafe`
//!
//! The whole contract is a promise the compiler cannot check: the address
//! must stay put. A type whose accessor returns a fresh address on each
//! call, or that reallocates while held, is what makes the kernel write into
//! freed memory long after the call that started it returned. Implementing
//! these traits is asserting that cannot happen.

use std::sync::Arc;

/// An owned buffer an operation reads bytes **from** (a write).
///
/// # Safety
///
/// Implementors guarantee that, for as long as this crate holds the value --
/// whether as a single operation's [`crate::Token`], or as a buffer
/// registration spanning many operations (M5) -- :
///
/// - [`IoBuf::stable_ptr`] returns the same address every time it is called,
///   and that address does not change when the value is moved. This is what
///   a pointer-to-heap buffer gives for free and an inline array does not:
///   moving a `[u8; N]` moves its bytes.
/// - The [`IoBuf::bytes_len`] bytes starting at that address stay allocated,
///   initialized, and unmodified.
/// - [`IoBuf::bytes_len`] returns the same value every time it is called.
///
/// `Send` because a completion may be observed on a different thread from
/// the one that submitted the operation; `'static` because a registered or
/// in-flight buffer carries no lifetime this crate can check.
pub unsafe trait IoBuf: Send + 'static {
    /// The address of the first byte. Must not change for the value's life.
    fn stable_ptr(&self) -> *const u8;

    /// How many bytes from [`IoBuf::stable_ptr`] may be read.
    fn bytes_len(&self) -> usize;
}

/// An owned buffer an operation writes bytes **into** (a read).
///
/// Separate from [`IoBuf`] because not every owned buffer can be written to:
/// an `Arc<[u8]>` is perfectly good to send *from* and can never be a
/// destination, since handing out `&mut` to shared bytes would be unsound.
///
/// # Safety
///
/// As [`IoBuf`], and additionally: the value must have exclusive access to
/// the [`IoBuf::bytes_len`] bytes at [`IoBufMut::stable_mut_ptr`], which must
/// be the same address [`IoBuf::stable_ptr`] reports, so the kernel writing
/// into them cannot race or alias anything else. Those bytes must already be
/// **initialized**: this crate does not track an initialized prefix.
pub unsafe trait IoBufMut: IoBuf {
    /// The mutable address of the first byte. Must equal
    /// [`IoBuf::stable_ptr`] and must not change for the value's life.
    fn stable_mut_ptr(&mut self) -> *mut u8;
}

// SAFETY: a `Vec`'s bytes live in a heap allocation whose address is
// independent of where the `Vec` itself sits, so moving the `Vec` does not
// move them. Nothing here reallocates: the operation only reads and writes
// within `len`, never pushes. All `len` bytes are initialized by construction.
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

// SAFETY: a boxed slice is a heap allocation of fixed length; moving the
// `Box` moves the pointer, not the bytes. Its length cannot change at all.
unsafe impl IoBuf for Box<[u8]> {
    fn stable_ptr(&self) -> *const u8 {
        self.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.len()
    }
}

// SAFETY: as above; `Box` is a unique owner, so `&mut self` is exclusive
// access.
unsafe impl IoBufMut for Box<[u8]> {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }
}

// SAFETY: the bytes live in the `Arc`'s allocation, which outlives every
// clone and never moves. Read-only by nature, which is why there is no
// `IoBufMut` counterpart: other clones may be reading the same bytes
// concurrently, so handing the kernel a writable pointer to them would alias.
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

// SAFETY: as `&'static [u8]` -- the referent is valid forever and never
// moves, and the reference itself moving does not move the bytes.
unsafe impl IoBuf for &'static mut [u8] {
    fn stable_ptr(&self) -> *const u8 {
        <[u8]>::as_ptr(self)
    }

    fn bytes_len(&self) -> usize {
        <[u8]>::len(self)
    }
}

// SAFETY: the one reference type that *is* a legal read destination. Unlike
// `Arc<[u8]>` and `&'static [u8]`, a `&'static mut` is exclusive by
// construction -- no other live reference to those bytes can exist -- so the
// kernel writing into them cannot race or alias anything. Excluding it would
// be the arbitrary half of the split, not a safety measure.
unsafe impl IoBufMut for &'static mut [u8] {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        <[u8]>::as_mut_ptr(self)
    }
}

#[cfg(test)]
mod tests;
