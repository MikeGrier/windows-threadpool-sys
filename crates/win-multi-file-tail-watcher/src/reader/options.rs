// Copyright (c) 2026 Mike Grier
//! The sizes the reading layer works to.

use windows_ioring_sys::capabilities;

use super::options_error::{Cause, OptionsError};
use crate::pool::{self, MAX_BUFFER_LEN, PoolError};
use crate::sq::queue_of;

#[cfg(test)]
mod tests;

/// The sizes the reading layer works to, which the caller chooses.
///
/// Every size is the caller's: there is no ceiling here that exists to protect the defaults, so a
/// size well above them is accepted wherever the pool, the queue and the ring can have it. A size
/// that cannot work is refused by [`validate`](Self::validate), naming the option and the rule it
/// broke ([`OptionsError`]), before anything is allocated.
///
/// | Option | Accepted |
/// |---|---|
/// | `buffer_len` | from 1 to the most a ring read takes, `u32::MAX` bytes |
/// | `buffer_count` | at least 1 |
/// | `client_sq_capacity` | a power of two, within the bounds the queue reports |
/// | `ring_sq_size` | a power of two, at least [`MIN_RING_SQ_SIZE`](Self::MIN_RING_SQ_SIZE), at most what the ring reports |
/// | `ring_cq_size` | a power of two, at least [`MIN_RING_CQ_PER_SQ`](Self::MIN_RING_CQ_PER_SQ) times `ring_sq_size`, at most what the ring reports |
///
/// # Ring sizes are refused, not rounded
///
/// The ring would accept a size that is zero, not a power of two, or smaller than it needs, and
/// give the caller a ring of a size they did not ask for. These options refuse each of those
/// instead, so the ring a caller gets is the ring they asked for. The bounds are those the ring
/// was measured to apply (the tests of this crate fail on a ring that behaves differently), and
/// the largest sizes are the ones [`windows_ioring_sys::capabilities`] reports on the machine.
///
/// # What the sizes cost
///
/// The buffers are allocated and zeroed when the layer is made: `buffer_len` times `buffer_count`
/// bytes, held for the layer's whole life. Validation does not check that the memory is there; an
/// allocation that fails is reported when the layer is made, not as a refused size. Reads in flight
/// at once are at most the least of the pieces announced, `buffer_count`, and the completion queue
/// the ring gives.
///
/// # Example
///
/// ```
/// use win_multi_file_tail_watcher::reader::{Cause, OptionsError, TailReaderOptions};
///
/// let options = TailReaderOptions::default();
/// assert!(options.validate().is_ok());
///
/// // 63 is not a power of two: the ring would round it up to 64, so it is refused.
/// let mut options = TailReaderOptions::default();
/// options.ring_sq_size = 63;
/// let error = options.validate().unwrap_err();
/// assert!(matches!(error, OptionsError::RingSqSize { given: 63, .. }));
/// assert_eq!(error.cause(), Some(Cause::NotAPowerOfTwo));
///
/// // A completion queue below twice the submission queue would be raised to it, so it is refused.
/// let mut options = TailReaderOptions::default();
/// options.ring_cq_size = options.ring_sq_size;
/// let error = options.validate().unwrap_err();
/// assert_eq!(
///     error.cause(),
///     Some(Cause::Below {
///         least: u64::from(options.ring_sq_size) * u64::from(TailReaderOptions::MIN_RING_CQ_PER_SQ)
///     })
/// );
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TailReaderOptions {
    /// The size in bytes of each buffer a read fills. A line often spans several buffers, and the
    /// bytes of one file arrive in chunks of at most this size, in any order.
    pub buffer_len: usize,
    /// How many buffers there are, which also bounds the reads in flight at once. The memory is
    /// `buffer_len` times this, allocated when the layer is made.
    pub buffer_count: usize,
    /// How many entries the client's queue holds before the layer stops and waits for the client to
    /// drain it. Entries that carry bytes hold a buffer, so those are at most `buffer_count`;
    /// shrink, terminal and poll-done entries hold none.
    pub client_sq_capacity: usize,
    /// The ring's submission queue size.
    pub ring_sq_size: u32,
    /// The ring's completion queue size, which is the most reads the ring holds at once.
    pub ring_cq_size: u32,
}

impl TailReaderOptions {
    /// The least `ring_sq_size`: the ring raises anything smaller to this.
    pub const MIN_RING_SQ_SIZE: u32 = 8;
    /// The least `ring_cq_size` for a `ring_sq_size`, as a multiple of it: the ring raises
    /// anything smaller to this.
    pub const MIN_RING_CQ_PER_SQ: u32 = 2;

    /// The default `buffer_len`.
    pub const DEFAULT_BUFFER_LEN: usize = 64;
    /// The default `buffer_count`.
    pub const DEFAULT_BUFFER_COUNT: usize = 64;
    /// The default `client_sq_capacity`.
    pub const DEFAULT_CLIENT_SQ_CAPACITY: usize = 64;
    /// The default `ring_sq_size`.
    pub const DEFAULT_RING_SQ_SIZE: u32 = 64;
    /// The default `ring_cq_size`: the least the ring would give for the default `ring_sq_size`.
    pub const DEFAULT_RING_CQ_SIZE: u32 = Self::DEFAULT_RING_SQ_SIZE * Self::MIN_RING_CQ_PER_SQ;

    /// Check every size, allocating nothing.
    ///
    /// The options are checked in the order `buffer_len`, `buffer_count`, `client_sq_capacity`,
    /// `ring_sq_size`, `ring_cq_size`, and the first to fail is the one reported. The ring's two
    /// sizes are checked against what [`windows_ioring_sys::capabilities`] reports.
    ///
    /// # Errors
    ///
    /// An [`OptionsError`] naming the option, the value and the [`Cause`], or
    /// [`OptionsError::RingCapabilities`] when the ring's limits cannot be read.
    pub fn validate(&self) -> Result<(), OptionsError> {
        self.check_pool_and_queue()?;
        // Untested: `capabilities()` cannot be made to fail on demand, so no test reaches this
        // arm. It carries the operating system's error unchanged.
        let capabilities = capabilities().map_err(OptionsError::RingCapabilities)?;
        self.check_ring(RingLimits {
            max_sq: capabilities.max_submission_queue_size,
            max_cq: capabilities.max_completion_queue_size,
        })
    }

    /// The pool's and the client queue's sizes, which need nothing from the operating system.
    fn check_pool_and_queue(&self) -> Result<(), OptionsError> {
        match pool::check(self.buffer_len, self.buffer_count) {
            Ok(()) => {}
            Err(PoolError::BufferLen(given)) => {
                return Err(OptionsError::BufferLen {
                    given,
                    cause: if given == 0 {
                        Cause::Zero
                    } else {
                        Cause::Above {
                            most: wide(MAX_BUFFER_LEN),
                        }
                    },
                });
            }
            Err(PoolError::NoBuffers) => {
                return Err(OptionsError::BufferCount {
                    given: self.buffer_count,
                    cause: Cause::Zero,
                });
            }
            Err(PoolError::Allocation(_) | PoolError::Io(_)) => {
                unreachable!("the pool's dimension check allocates nothing and makes no call")
            }
        }
        let (least, most) = client_queue_bounds();
        check_power_of_two(wide(self.client_sq_capacity), least, most).map_err(|cause| {
            OptionsError::ClientSqCapacity {
                given: self.client_sq_capacity,
                cause,
            }
        })
    }

    /// The ring's two sizes against `limits`.
    fn check_ring(&self, limits: RingLimits) -> Result<(), OptionsError> {
        check_power_of_two(
            u64::from(self.ring_sq_size),
            u64::from(Self::MIN_RING_SQ_SIZE),
            u64::from(limits.max_sq),
        )
        .map_err(|cause| OptionsError::RingSqSize {
            given: self.ring_sq_size,
            cause,
        })?;
        check_power_of_two(
            u64::from(self.ring_cq_size),
            u64::from(self.ring_sq_size) * u64::from(Self::MIN_RING_CQ_PER_SQ),
            u64::from(limits.max_cq),
        )
        .map_err(|cause| OptionsError::RingCqSize {
            given: self.ring_cq_size,
            ring_sq_size: self.ring_sq_size,
            cause,
        })
    }
}

impl Default for TailReaderOptions {
    /// The defaults: [`DEFAULT_BUFFER_LEN`](Self::DEFAULT_BUFFER_LEN) bytes in each of
    /// [`DEFAULT_BUFFER_COUNT`](Self::DEFAULT_BUFFER_COUNT) buffers, a client queue of
    /// [`DEFAULT_CLIENT_SQ_CAPACITY`](Self::DEFAULT_CLIENT_SQ_CAPACITY) entries, and a ring of
    /// [`DEFAULT_RING_SQ_SIZE`](Self::DEFAULT_RING_SQ_SIZE) and
    /// [`DEFAULT_RING_CQ_SIZE`](Self::DEFAULT_RING_CQ_SIZE).
    ///
    /// They are a starting point for a light load, not a ceiling: a heavier one chooses larger
    /// sizes.
    fn default() -> Self {
        Self {
            buffer_len: Self::DEFAULT_BUFFER_LEN,
            buffer_count: Self::DEFAULT_BUFFER_COUNT,
            client_sq_capacity: Self::DEFAULT_CLIENT_SQ_CAPACITY,
            ring_sq_size: Self::DEFAULT_RING_SQ_SIZE,
            ring_cq_size: Self::DEFAULT_RING_CQ_SIZE,
        }
    }
}

/// The largest ring queue sizes the ring reports.
#[derive(Clone, Copy, Debug)]
struct RingLimits {
    max_sq: u32,
    max_cq: u32,
}

/// `value` as a `u64`, which every size here fits.
fn wide(value: usize) -> u64 {
    u64::try_from(value).expect("a usize fits a u64")
}

/// A size that must be a power of two from `least` to `most`: nonzero, not rounded, not raised.
/// The checks run in the order the queue's own do, so the two report the same cause.
fn check_power_of_two(given: u64, least: u64, most: u64) -> Result<(), Cause> {
    if given == 0 {
        return Err(Cause::Zero);
    }
    if !given.is_power_of_two() {
        return Err(Cause::NotAPowerOfTwo);
    }
    if given < least {
        return Err(Cause::Below { least });
    }
    if given > most {
        return Err(Cause::Above { most });
    }
    Ok(())
}

/// The least and the most capacity the client queue accepts, read from the queue's own refusal of
/// a capacity of zero, which it makes before it allocates, so nothing is asked of the allocator.
///
/// The rule that the capacity is a power of two within them is [`check_power_of_two`]'s, written a
/// second time here; a test compares it with the queue's verdict over a range of capacities.
fn client_queue_bounds() -> (u64, u64) {
    match queue_of::<()>(0) {
        Err(refusal) => (wide(refusal.min_valid), wide(refusal.max_valid)),
        Ok(_) => unreachable!("the queue refuses a capacity of zero"),
    }
}
