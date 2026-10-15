// Copyright (c) 2026 Mike Grier
//! What a ring may be asked to carry: [`Open`] or [`Owned`] (`M41.1`,
//! [D-91](../../DESIGN-NOTES.md#d-91)).
//!
//! Every operation a safe push queues is an *owned* operation: the ring holds what it needs, and
//! the pop that observes its completion hands that back. A handful of calls queue an operation
//! the ring holds **nothing** for -- the registrations, the `_raw` flush and cancel, and
//! [`IoRing::push_raw`](super::IoRing::push_raw) -- and their completions arrive from a pop with
//! no payload and no sidecar. That is a legitimate outcome for a caller who chose it, and a
//! branch every other caller has to write and cannot reach.
//!
//! A ring is therefore typed by whether it can have such an operation in flight. An [`Open`] ring
//! can, and is what every constructor makes. An [`Owned`] ring cannot: the calls that queue one do
//! not exist for it, and a pop never hands back an absent entry. The one way from the first to the
//! second is [`IoRing::into_owned`](super::IoRing::into_owned), which consumes the ring and refuses
//! while any such operation is in flight; there is no way back, so the property is not one a
//! later caller can undo.

use std::io;

use crate::ring::Completion;

/// A ring that may have an operation in flight that the ring holds nothing for: the kind every
/// constructor makes. All of the ring's calls exist for it.
///
/// Uninhabited: it only ever appears as a type argument.
#[derive(Debug, Clone, Copy)]
pub enum Open {}

/// A ring none of whose in-flight operations can be one the ring holds nothing for, reached from
/// an [`Open`] ring by [`IoRing::into_owned`](super::IoRing::into_owned). The calls that queue such
/// an operation do not exist for it, and a pop returns what the ring held, without the case of
/// there being nothing.
///
/// Uninhabited: it only ever appears as a type argument.
#[derive(Debug, Clone, Copy)]
pub enum Owned {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Open {}
    impl Sealed for super::Owned {}
}

/// What a ring's kind decides: the shape a pop returns it in. Implemented by [`Open`] and
/// [`Owned`] only.
pub trait RingKind: sealed::Sealed {
    /// What a pop returns: the completion, and what the ring held for it.
    type Popped<T, X>;

    /// Shape a popped completion and what the ring held for it.
    ///
    /// # Errors
    ///
    /// Only when the kind promises the ring holds something for every completion and it did not,
    /// during an unwind, where a panic would abort.
    #[doc(hidden)]
    fn shape<T, X>(
        completion: Completion,
        held: Option<(Option<T>, X)>,
    ) -> io::Result<Self::Popped<T, X>>;
}

/// The kinds that may queue an operation the ring holds nothing for: [`Open`]. A method with this
/// bound is one an [`Owned`] ring does not have.
///
/// The registrations and the `_raw` flush and cancel do not build for an [`Owned`] ring:
///
/// ```compile_fail,E0277
/// use windows_ioring_sys::{Batch, IoRing, Owned};
///
/// fn register(ring: &mut IoRing<Vec<u8>, (), Owned>) {
///     let _ = Batch::new(ring).register_buffers(vec![vec![0_u8; 8]]);
/// }
/// ```
///
/// ```compile_fail,E0277
/// use windows_ioring_sys::{Batch, FlushCoverage, FlushMode, IoRing, Owned};
///
/// fn flush(ring: &mut IoRing<(), (), Owned>, file: std::os::windows::io::RawHandle) {
///     // SAFETY: not reached; this is a compile failure.
///     let _ = unsafe {
///         Batch::new(ring).flush_raw(file, FlushCoverage::Unordered, FlushMode::Default)
///     };
/// }
/// ```
///
/// ```compile_fail,E0277
/// use windows_ioring_sys::{IoRing, Owned};
///
/// fn push(ring: &mut IoRing<(), (), Owned>) {
///     // SAFETY: not reached; this is a compile failure.
///     let _ = unsafe { ring.push_raw(|_, _| 0) };
/// }
/// ```
///
/// And no constructor makes one, nor is there a way back to [`Open`]:
///
/// ```compile_fail,E0308
/// use windows_ioring_sys::{IoRing, Owned};
///
/// let _ring: IoRing<(), (), Owned> = IoRing::new(8, 8).unwrap();
/// ```
///
/// ```compile_fail,E0599
/// use windows_ioring_sys::{IoRing, Owned};
///
/// fn reopen(ring: IoRing<(), (), Owned>) {
///     let _ = ring.into_open();
/// }
/// ```
///
/// Over an [`Open`] ring the same calls build:
///
/// ```no_run
/// use windows_ioring_sys::{Batch, IoRing};
///
/// # fn main() -> std::io::Result<()> {
/// let mut ring = IoRing::<Vec<u8>>::with_inventory(8, 8)?;
/// let _pending = Batch::new(&mut ring).register_buffers(vec![vec![0_u8; 8]]);
/// # Ok(())
/// # }
/// ```
pub trait AllowsRaw: RingKind {}

impl RingKind for Open {
    /// The outer `Option` is whether the ring held anything for the completion; the inner is
    /// whether what it held included a buffer.
    type Popped<T, X> = (Completion, Option<(Option<T>, X)>);

    fn shape<T, X>(
        completion: Completion,
        held: Option<(Option<T>, X)>,
    ) -> io::Result<Self::Popped<T, X>> {
        Ok((completion, held))
    }
}

impl AllowsRaw for Open {}

impl RingKind for Owned {
    /// The buffer, if the operation had one, and the sidecar: the ring held an entry for every
    /// completion, so there is no case of it holding none.
    type Popped<T, X> = (Completion, Option<T>, X);

    fn shape<T, X>(
        completion: Completion,
        held: Option<(Option<T>, X)>,
    ) -> io::Result<Self::Popped<T, X>> {
        match held {
            Some((payload, extra)) => Ok((completion, payload, extra)),
            // Nothing can queue an operation this ring holds nothing for, and `into_owned`
            // refused while one was in flight, so this is a defect in the ring or in a caller
            // that reached its internals -- as a completion for an identity not in flight is
            // (D-79). An unwind is the exception: a second panic there aborts, so it is an error.
            None if std::thread::panicking() => Err(io::Error::other(
                "an owned ring held nothing for a completion, while unwinding",
            )),
            None => panic!(
                "an owned ring held nothing for the completion of operation {}: nothing can \
                 queue an operation it does not hold, so the ring's bookkeeping is inconsistent \
                 (D-91)",
                completion.user_data()
            ),
        }
    }
}
