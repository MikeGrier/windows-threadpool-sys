// Copyright (c) Mike Grier.

//! What becomes of items still in the queue when it is torn down.
//!
//! # The hazard, which is not hypothetical
//!
//! A queue's items can own resources, and a descriptor for a completed async
//! open owns a **handle**. Closing a handle is not always cheap: closing one to
//! a dead network path can block for a long time, and keeping exactly that
//! operation off a caller's thread is the sort of thing the queue exists to
//! serve in the first place.
//!
//! So the question "who destroys the items nobody drained?" has a bad default
//! answer. Without this module, they are destroyed **in place, on whichever
//! thread happened to release the last handle** -- which may be a thread-pool
//! callback that must not block, or a producer that has no idea it is holding
//! the last reference. Nobody chose that thread, and nothing tells the owner it
//! happened.
//!
//! # Why `Drop` cannot simply hand them back
//!
//! The obvious fix -- return the remainder from teardown -- is not available.
//! [`Drop::drop`] takes `&mut self`, returns nothing, and cannot fail. By the
//! time it runs, every handle is already gone, so there is nobody left to
//! return anything *to*. Anything the queue is going to do with those items, it
//! must have been told in advance.
//!
//! Draining first does not close the hole either. A consumer can take
//! everything available, but a producer may push again afterwards, so an
//! orderly drain covers the orderly path and nothing else. **The last handle to
//! drop is the only place that sees every remaining item**, and it is the one
//! place with no way to report.
//!
//! # So the decision is made at construction
//!
//! A queue built with [`Disposal`] hands each surviving item to that sink
//! instead of destroying it. The owner therefore decides where disposal
//! happens: a sink that moves items to a reaper thread keeps the blocking off
//! the dropping thread entirely, and one that disposes inline is fine when the
//! dropping thread is allowed to block. Either way it is a decision somebody
//! made rather than one that fell out of which `Arc` clone happened to die
//! last.
//!
//! **The default is unchanged and still destroys in place**, because for the
//! overwhelmingly common case -- items that own nothing -- that is exactly
//! right, and a queue of `u32` should not have to think about any of this. What
//! changes is that the behaviour is now written down as a choice with a name.

use core::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Where a queue's surviving items go when it is torn down.
///
/// See the [module documentation](self) for why this has to be supplied up
/// front rather than asked for at teardown.
///
/// # Examples
///
/// Handing the remainder to a channel a reaper thread drains, so a destructor
/// that blocks does so somewhere that is allowed to:
///
/// ```
/// use std::sync::mpsc;
/// use windows_waitable_queues::{Disposal, Options, spsc};
///
/// let (undelivered, reaper) = mpsc::channel();
/// let (tx, rx) = spsc::bounded_with::<u32>(
///     4,
///     Options::new().disposal(Disposal::new(move |item| {
///         // Cheap and non-blocking: the reaper thread does the real work.
///         let _ = undelivered.send(item);
///     })),
/// )?;
///
/// tx.push(1).expect("a fresh queue has room");
/// tx.push(2).expect("a fresh queue has room");
/// drop((tx, rx));
///
/// // Nothing was destroyed behind the owner's back.
/// assert_eq!(reaper.into_iter().collect::<Vec<_>>(), vec![1, 2]);
/// # Ok::<(), windows_waitable_queues::CapacityError>(())
/// ```
pub struct Disposal<T> {
    sink: Box<dyn FnMut(T) + Send>,
}

impl<T> Disposal<T> {
    /// Builds a sink from a closure.
    ///
    /// The closure is called once per surviving item, on the thread that
    /// released the queue's last handle. **It should be cheap**: if disposal
    /// can block, the useful shape is to move the item somewhere a thread that
    /// may block will find it, rather than to do the blocking work here.
    ///
    /// `Send` because the thread that tears the queue down is whichever one
    /// happened to drop last, and is not knowable in advance. Not `Sync`,
    /// because it is only ever called from a teardown that has exclusive
    /// access.
    pub fn new(sink: impl FnMut(T) + Send + 'static) -> Self {
        Self {
            sink: Box::new(sink),
        }
    }
}

impl<T> fmt::Debug for Disposal<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Disposal(..)")
    }
}

/// A shape's teardown policy: the sink if it was given one, otherwise the
/// default.
///
/// Held by every shape's shared state and touched only from `Drop`, so it costs
/// the hot paths nothing but the space.
pub(crate) struct Teardown<T> {
    disposal: Option<Disposal<T>>,
}

impl<T> Teardown<T> {
    /// Hand each surviving item to `disposal`, or destroy it where it lies if
    /// there is none.
    pub(crate) const fn new(disposal: Option<Disposal<T>>) -> Self {
        Self { disposal }
    }

    /// Dispose of one surviving item.
    ///
    /// # A panicking sink does not strand the items behind it
    ///
    /// The sink is caller-supplied code running inside a destructor, which is
    /// the worst place for it to panic: a panic escaping here during an unwind
    /// aborts the process, and one escaping otherwise abandons every item not
    /// yet disposed -- precisely the handles this whole mechanism exists to
    /// account for.
    ///
    /// So a panic is caught and the walk continues. That is deliberately *not*
    /// "swallowing an error": the item has already been handed over, so there
    /// is nothing left to report about it, and the alternative is to lose the
    /// rest of the queue as well. A sink that panics is a bug in the caller;
    /// this only declines to make it a much larger one.
    pub(crate) fn dispose(&mut self, item: T) {
        let Some(disposal) = self.disposal.as_mut() else {
            // The default. Written as an explicit drop rather than left to fall
            // out of the binding going out of scope, because "destroy it here"
            // is a decision this type exists to name.
            drop(item);
            return;
        };

        // `AssertUnwindSafe` is the honest annotation rather than a way past
        // the bound: the only state that could be observed after a panic is the
        // caller's own closure, and the queue's own invariants do not depend on
        // the sink at all -- teardown is already past the point where anything
        // could observe them.
        let _ = catch_unwind(AssertUnwindSafe(|| (disposal.sink)(item)));
    }
}

impl<T> fmt::Debug for Teardown<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Teardown")
            .field("hands_off", &self.disposal.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests;
