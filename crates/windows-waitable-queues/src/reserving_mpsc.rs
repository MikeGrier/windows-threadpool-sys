// Copyright (c) Mike Grier.

//! The multi-producer, single-consumer bounded array queue **that can reserve**.
//!
//! Everything [`slotwise_mpsc`](crate::slotwise_mpsc) is, plus [`Producer::reserve`]: a slot
//! claimed in advance, so that a later delivery cannot be refused for want of
//! room. *Reserved is guaranteed, unreserved is best-effort.*
//!
//! # The claim position recurs, and how soon is a layout choice
//!
//! **Under the default layout this shape can lose an item after 2^32 pushes, on
//! every target and not only 32-bit ones** -- [`Balanced`] gives the claim
//! position a 32-bit half of the packed word below, so this reaches x86-64 and
//! ARM64 exactly as it reaches i686.
//!
//! A producer that has checked for room, been descheduled, and resumed after
//! other producers drove the position through a full wrap will claim
//! successfully against a numerically identical but generations-later value,
//! and write into a slot whose emptiness was decided long ago. **The failure is
//! silent**: the consumer receives a different item than was sent, and no error,
//! panic, or counter reports it.
//!
//! Under `Balanced`, 2^32 pushes is 37 seconds to about four minutes of
//! *sustained* pushing at this crate's measured rates, roughly two minutes at
//! two producers. The wrap alone is not enough -- a producer must also stall
//! inside a window a few instructions wide -- but a preemption suffices.
//!
//! **[`ClaimLayout`] is how far away that is.** [`Perpetual`] moves it to 2^56
//! pushes, about twenty years at the same rate, for the cost of a reservation
//! ceiling of 255 and nothing measurable besides -- it is the same exchange on
//! the same word, differing only in shift constants. [`Enduring`] sits between
//! them, and the `dwcas` feature adds a 128-bit word that removes the
//! recurrence outright.
//!
//! ```
//! use windows_waitable_queues::reserving_mpsc::{self, Perpetual};
//!
//! let (tx, rx) = reserving_mpsc::bounded_as::<u32, Perpetual>(64)?;
//! # let _ = (tx, rx);
//! # Ok::<(), windows_waitable_queues::CapacityError>(())
//! ```
//!
//! The default stays `Balanced` so that introducing the choice changed no
//! existing caller's behaviour; it is not the recommended layout.
//! [`slotwise_mpsc`](crate::slotwise_mpsc) does not have this hazard under any
//! layout, its positions being 64 bits on every target; [`spsc`](crate::spsc)
//! never had it. The full statement is in the [crate documentation](crate).
//!
//! # Why this is a separate shape rather than a method on `slotwise_mpsc`
//!
//! Because the two ask different questions to claim a slot, and only this one's
//! question can answer a reservation. They are two claim protocols, not one
//! queue with a switch.
//!
//! Honouring a reservation costs the producer a read of the consumer's position
//! on **every** push, including the pushes that never reserve anything -- which
//! is what `slotwise_mpsc` avoids and why it cannot offer reservation at all.
//!
//! **That cost is not what makes either shape slower.** This one measured
//! *faster* than `slotwise_mpsc` under contention on both architectures tried, by up to
//! 6.4x, because the slot sequence `slotwise_mpsc` reads instead marches through memory
//! while other producers write it. See the crate documentation for the numbers
//! and for how to choose.
//!
//! `slotwise_mpsc`'s producer never reads the consumer's position. It asks a different
//! question -- "is the slot I am about to claim free?" -- and reads that from
//! the slot's own sequence number, which is spread across the slot array, so
//! producers working at different positions touch different cache lines.
//! Avoiding a single shared position is not incidental to that design; it is
//! most of the point of it.
//!
//! A reservation cannot be honoured from that question. "Is this slot free" does
//! not tell you **how many** slots remain, and holding one back for a reserver
//! requires exactly that count -- which requires the consumer's position, on one
//! line every thread in the system touches.
//!
//! So the two ship as peers ([D-16](../DESIGN-NOTES.md#d-16)): `slotwise_mpsc` for a
//! caller who wants the cheapest possible push and can treat a refusal as
//! backpressure, this shape for a caller with a message it must not lose. That
//! is the narrow-trait argument from [D-2](../DESIGN-NOTES.md#d-2) reaching
//! its sharpest case -- `slotwise_mpsc` does not implement
//! [`Reserving`](crate::Reserving) because it genuinely cannot, not because
//! nobody got round to it.
//!
//! # The claim word, which is why reservation is sound here
//!
//! The reservation count and the claim position live in **one** [`AtomicU64`]:
//! the low 32 bits are the position, the high 32 the number of outstanding
//! reservations. Every operation that changes either changes both together, with
//! one compare-and-swap.
//!
//! That is not tidiness, it is the correctness argument, and the obvious
//! alternative is broken in a way worth recording. With the count in its own
//! atomic:
//!
//! 1. A pushing producer reads the count, sees room, and claims the position.
//! 2. A reserving producer increments the count, reads the position, sees room,
//!    and hands out the reservation.
//!
//! Each read before the other's write, and the queue now owes a slot that does
//! not exist. **Sequentially consistent fences do not close this**, unlike the
//! superficially similar hazard in the internal `Doorbell`: the
//! Dekker argument needs store-then-load on both sides, and the pushing producer
//! is load-then-store -- it *reads* the count and then *writes* the position. In
//! a total order over the four operations, both sides missing each other is
//! consistent, so no fence forbids it. Two independent claimants on one resource
//! must synchronise on one location, so the count and the position become one
//! location.
//!
//! With that, redeeming a reservation is a single compare-and-swap that
//! decrements the count and advances the position at once -- so the quantity the
//! invariant is about, `occupied + reserved`, is never momentarily wrong.
//!
//! # What the packing costs, and what it does not
//!
//! Splitting a 64-bit word 32/32 caps this shape at
//! a maximum of 2^31 items, and that split is forced rather than chosen:
//! a position of `b` bits keeps a wrapping difference unambiguous only up to
//! `2^(b-1)`, and the count needs `b` bits because it can reach the capacity, so
//! `b + b = 64` gives `b = 32`. There is no cleverer division of the word.
//!
//! **A 128-bit compare-and-swap is deliberately not used *here***
//! ([D-37](../DESIGN-NOTES.md#d-37)). It would not remove the cost that
//! matters -- the consumer's position still has to be read -- and 2^31 slots is
//! a ring this shape allocates in full at construction.
//!
//! The operative reason is that widening *this* shape's word would change what
//! it offers depending on the target: `i686-pc-windows-msvc` has no lock-free
//! 128-bit exchange, so the same module would be lock-free on one target and
//! silently mutex-backed on another. A wider claim ships instead as its own
//! shape (`reserving_mpsc_wide`, not yet built -- see D-37), to exist only
//! where the exchange is genuinely lock-free. That keeps *this* module's
//! contract the same on every target, which is the property being protected
//! here: a caller who wants 2^62 slots and no wrap hazard will ask for it by
//! name rather than get it by accident of where they compiled.

use core::cell::{Cell, UnsafeCell};
use core::fmt;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::io;
use std::os::windows::io::{BorrowedHandle, OwnedHandle};
use std::sync::Arc;
use std::time::Duration;

use crate::CacheAligned;
use crate::blocking::{self, Parked};
use crate::capacity::{Bounds, MAX_ADMISSIBLE_CAPACITY, WRAPPING_MAX_CAPACITY, validate_capacity};
use crate::disposal::Teardown;
use crate::doorbell::Doorbell;
use crate::error::{
    CapacityError, Disconnected, PushError, RecvError, RecvTimeoutError, TryRecvError,
};
use crate::metrics::Metrics;
use crate::options::Options;

/// How the claim word's 64 bits are divided between the two things it packs.
///
/// The word carries an outstanding-reservation count and a claim position, and
/// it must carry both because a single compare-and-swap has to update them
/// together (see the [module documentation](self)). Dividing 64 bits between
/// them is therefore a trade, and this trait is where a caller chooses which
/// side to spend them on.
///
/// **The two things being traded are not equally valuable, and the shipping
/// default spends the bits on the less valuable one.** The reservation count
/// bounds how many messages may be held in flight at once -- in practice the
/// number of producers mid-send, so hundreds or thousands. The position decides
/// how many pushes occur before it recurs, and a recurrence is the `SH-14.1`
/// hazard: a producer descheduled across a full wrap can claim against a
/// numerically identical but generations-later value.
///
/// | Layout | reserved / position | Outstanding reservations | Pushes to recurrence |
/// |---|---|---|---|
/// | [`Balanced`] | 32 / 32 | 2^32 | 2^32 |
/// | [`Enduring`] | 16 / 48 | 65,535 | 2^48 |
/// | [`Perpetual`] | 8 / 56 | 255 | 2^56 |
///
/// At this crate's disclosed sustained rate of about 116 million pushes per
/// second, those recurrences are roughly **37 seconds**, **28 days**, and
/// **20 years** respectively. The rate is the one `reserving_mpsc`'s own hazard
/// note quotes; a queue that must drain cannot sustain the fastest rate
/// measured, so treat these as a floor on time rather than a forecast.
///
/// **Choosing a deeper position costs nothing measurable.** All three issue the
/// same `lock cmpxchg` on the same `u64` and differ only in shift and mask
/// constants; a probe comparing them found no difference outside noise. The
/// trade is entirely against the reservation ceiling.
///
/// This trait is sealed: the layouts are a fixed set because each one's
/// constants are checked against each other at compile time, and a caller
/// supplying its own could pick a division this shape cannot honour.
pub trait ClaimLayout: sealed::Sealed {
    /// The integer the two halves are packed into.
    ///
    /// `u64` for every layout the crate offers by default. The `dwcas` feature
    /// adds `Wide`, whose word is a `u128` -- and the arithmetic is done in
    /// this type rather than uniformly in the wider one, so a `u64` layout
    /// issues `u64` instructions exactly as it did before the type became a
    /// parameter.
    type Word: ClaimWord;

    /// How wide [`Self::Word`] is, in bits.
    const WORD_BITS: u32;

    /// How many of the claim word's bits carry the position.
    const POSITION_BITS: u32;

    /// Isolates the position half of the claim word.
    ///
    /// A position is carried in a `u64` whatever the word's width, since no
    /// layout gives it more than 64 bits. At exactly 64 the shift that would
    /// build this mask overflows, so the whole-width case is spelled out.
    const POSITION_MASK: u64 = if Self::POSITION_BITS >= 64 {
        u64::MAX
    } else {
        (1u64 << Self::POSITION_BITS) - 1
    };

    /// The largest outstanding-reservation count the word's other half holds.
    ///
    /// **A ceiling on reservations, not on capacity.** An earlier form of this
    /// shape required the count's half to be wide enough for the whole
    /// capacity, because every slot could be reserved at once. That is what made
    /// a large capacity consume the position's bits. Capping the reservations
    /// instead leaves the capacity bounded only by the ring.
    ///
    /// **Capped at [`u32::MAX`] however wide the field is**, because the count
    /// is reported to callers as a `u32`. A field wider than that would let the
    /// queue hold a count it could not describe.
    const MAX_RESERVED: u64 = {
        let field = Self::WORD_BITS - Self::POSITION_BITS;
        if field >= 32 {
            u32::MAX as u64
        } else {
            (1u64 << field) - 1
        }
    };

    /// The largest capacity this layout accepts.
    ///
    /// A wrapping position difference is unambiguous only up to half the
    /// position space, and the crate-wide ceiling applies as well -- on a
    /// 32-bit target it is the narrower of the two, and a shift by the position
    /// width would overflow `usize` outright.
    const BOUNDS_MAX: usize = {
        let ring_bits = Self::POSITION_BITS - 1;
        if ring_bits >= usize::BITS {
            MAX_ADMISSIBLE_CAPACITY
        } else {
            let packed = 1_usize << ring_bits;
            if packed <= MAX_ADMISSIBLE_CAPACITY {
                packed
            } else {
                MAX_ADMISSIBLE_CAPACITY
            }
        }
    };

    /// The relationships this layout's constants depend on.
    ///
    /// **Forced at construction rather than left to be evaluated.** An
    /// associated constant in a generic context is only evaluated where it is
    /// used, so assertions written here and never mentioned would compile for
    /// every layout including a broken one. The constructors name it, so
    /// creating a queue is what checks it.
    ///
    /// Note what is deliberately *not* asserted: that `BOUNDS_MAX` is at most
    /// `MAX_RESERVED`. That assertion is what previously tied the capacity to
    /// the reservation field, and removing it is the point of the decoupling
    /// above.
    const VALID: () = {
        assert!(
            Self::POSITION_BITS >= 32,
            "the reservation count is read out as a u32, so a count field wider than 32 bits -- \
             that is, a position narrower than 32 -- would be truncated on the way out"
        );
        assert!(
            Self::POSITION_BITS < Self::WORD_BITS,
            "the count needs at least one bit, so the position cannot take the whole word"
        );
        assert!(
            Self::POSITION_BITS <= 64,
            "a position is carried in a u64 between the packing and the ring, so a layout giving \
             it more bits than that would lose them on the way out"
        );
        assert!(
            Self::MAX_RESERVED <= u32::MAX as u64,
            "the count is read back out through `reserved_of`'s cast to `u32`, so a field the \
             word could hold but the cast could not would make this constant's name a lie"
        );
        assert!(
            Self::MAX_RESERVED >= 1,
            "a layout that permits no reservation at all would make `reserve` always fail, which \
             is the one capability this shape exists to provide"
        );
        assert!(
            Self::BOUNDS_MAX >= 2,
            "two is the smallest capacity this shape accepts, so a layout offering less accepts \
             nothing"
        );
        assert!(
            Self::BOUNDS_MAX.is_power_of_two(),
            "the maximum is offered to a caller as a capacity it could use, so it must itself be \
             one this shape would accept"
        );
        assert!(
            Self::BOUNDS_MAX <= WRAPPING_MAX_CAPACITY,
            "a shape may be narrower than the crate-wide bound but never wider"
        );
    };
}

mod sealed {
    /// Prevents a caller outside this crate from adding a layout.
    pub trait Sealed {}
    /// Prevents a caller outside this crate from adding a word width.
    pub trait SealedWord {}
}

/// The integer a claim word is packed into, and the atomic that holds it.
///
/// Exists so a layout can choose between a `u64` and a `u128` word without the
/// narrow layouts paying for the wide one: each is monomorphised to the
/// instructions its own width needs. Sealed for [`ClaimLayout`]'s reason, and
/// because an implementation that got the packing wrong would corrupt the two
/// halves into each other.
pub trait ClaimWord: sealed::SealedWord + Copy + PartialEq {
    /// The atomic this word lives in.
    type Atomic;

    /// A fresh atomic holding a zeroed word.
    fn zeroed() -> Self::Atomic;

    /// Read the word.
    fn load(cell: &Self::Atomic, order: Ordering) -> Self;

    /// Attempt to replace `current` with `new`.
    fn compare_exchange_weak(
        cell: &Self::Atomic,
        current: Self,
        new: Self,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self, Self>;

    /// Read the word through a unique borrow, without synchronization.
    fn read_mut(cell: &mut Self::Atomic) -> Self;

    /// Pack a reservation count and a position together.
    fn pack(reserved: u32, position: u64, position_bits: u32, position_mask: u64) -> Self;

    /// Read the position back out.
    fn position(self, position_mask: u64) -> u64;

    /// Read the reservation count back out.
    fn reserved(self, position_bits: u32) -> u32;
}

impl sealed::SealedWord for u64 {}
impl ClaimWord for u64 {
    type Atomic = AtomicU64;

    // `AtomicU64::new(0)` and `AtomicU64::default()` are the same value, so a
    // mutation run reports this as a survivor. It is an equivalent mutant: the
    // explicit zero is kept because the protocol depends on the initial claim
    // word being zero, which `default()` states only by coincidence.
    #[inline]
    fn zeroed() -> Self::Atomic {
        AtomicU64::new(0)
    }

    #[inline]
    fn load(cell: &Self::Atomic, order: Ordering) -> Self {
        cell.load(order)
    }

    #[inline]
    fn compare_exchange_weak(
        cell: &Self::Atomic,
        current: Self,
        new: Self,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self, Self> {
        cell.compare_exchange_weak(current, new, success, failure)
    }

    #[inline]
    fn read_mut(cell: &mut Self::Atomic) -> Self {
        *cell.get_mut()
    }

    #[inline]
    // **The `|` could equally be `^`, or `+`, and a mutation run reports as
    // much.** The halves are disjoint by construction -- the shift clears every
    // bit the position occupies -- so all three agree on every input and no test
    // can tell them apart. `|` says "these are separate fields" where the others
    // say "these are numbers". Recorded at both `pack` impls as well as on
    // `claim_word`, because that is where the operation actually lives: a run
    // reported it here after the word became a type parameter and the note
    // stayed behind on the caller.
    fn pack(reserved: u32, position: u64, position_bits: u32, position_mask: u64) -> Self {
        ((reserved as u64) << position_bits) | (position & position_mask)
    }

    #[inline]
    fn position(self, position_mask: u64) -> u64 {
        self & position_mask
    }

    #[inline]
    fn reserved(self, position_bits: u32) -> u32 {
        (self >> position_bits) as u32
    }
}

#[cfg(feature = "dwcas")]
impl sealed::SealedWord for u128 {}
#[cfg(feature = "dwcas")]
impl ClaimWord for u128 {
    type Atomic = portable_atomic::AtomicU128;

    // Equivalent-mutant note as on the `u64` impl above.
    #[inline]
    fn zeroed() -> Self::Atomic {
        portable_atomic::AtomicU128::new(0)
    }

    #[inline]
    fn load(cell: &Self::Atomic, order: Ordering) -> Self {
        cell.load(order)
    }

    #[inline]
    fn compare_exchange_weak(
        cell: &Self::Atomic,
        current: Self,
        new: Self,
        success: Ordering,
        failure: Ordering,
    ) -> Result<Self, Self> {
        cell.compare_exchange_weak(current, new, success, failure)
    }

    #[inline]
    fn read_mut(cell: &mut Self::Atomic) -> Self {
        *cell.get_mut()
    }

    #[inline]
    // Equivalent-mutant note as on the `u64` impl above.
    fn pack(reserved: u32, position: u64, position_bits: u32, position_mask: u64) -> Self {
        ((reserved as u128) << position_bits) | ((position & position_mask) as u128)
    }

    #[inline]
    fn position(self, position_mask: u64) -> u64 {
        (self as u64) & position_mask
    }

    #[inline]
    fn reserved(self, position_bits: u32) -> u32 {
        (self >> position_bits) as u32
    }
}

/// The shipping division: 32 bits each.
///
/// Holds 2^32 outstanding reservations and recurs after 2^32 pushes -- about
/// **37 seconds** of sustained maximum-rate pushing. This is the default
/// because it is what the shape shipped with, not because it is the best
/// choice: the reservation ceiling it buys is far beyond any real use, and it
/// is paid for with the whole of the `SH-14.1` exposure. Prefer [`Enduring`] or
/// [`Perpetual`] unless you genuinely hold more than 65,535 reservations at
/// once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Balanced;
impl sealed::Sealed for Balanced {}
impl ClaimLayout for Balanced {
    type Word = u64;
    const WORD_BITS: u32 = 64;
    const POSITION_BITS: u32 = 32;
}

/// A deeper position: 16 bits of reservations, 48 of position.
///
/// Holds 65,535 outstanding reservations and recurs after 2^48 pushes -- about
/// **28 days** of sustained maximum-rate pushing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enduring;
impl sealed::Sealed for Enduring {}
impl ClaimLayout for Enduring {
    type Word = u64;
    const WORD_BITS: u32 = 64;
    const POSITION_BITS: u32 = 48;
}

/// The deepest position: 8 bits of reservations, 56 of position.
///
/// Holds 255 outstanding reservations and recurs after 2^56 pushes -- about
/// **20 years** of sustained maximum-rate pushing, which puts the recurrence
/// beyond any real deployment rather than merely far away.
///
/// 255 reservations is the whole of the trade, and it is a real limit rather
/// than a nominal one: [`Producer::reserve`] returns `None` once that many are
/// outstanding, however empty the queue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Perpetual;
impl sealed::Sealed for Perpetual {}
impl ClaimLayout for Perpetual {
    type Word = u64;
    const WORD_BITS: u32 = 64;
    const POSITION_BITS: u32 = 56;
}

/// A 128-bit claim word: 64 bits of position, and the count in the other half.
///
/// Requires the `dwcas` feature, which is what brings in the `portable-atomic`
/// dependency this crate otherwise does not have. The position needs 2^64
/// pushes to recur, which no deployment reaches -- not "not for twenty years",
/// but not at all.
///
/// **Read the cost before choosing it.** The 128-bit exchange measured 2-3x
/// slower than a `u64` one on the claim itself, and the penalty grows with
/// producer count; against a draining consumer the difference is much smaller.
/// [`Perpetual`] reaches about twenty years on a plain `AtomicU64` at no
/// measured cost, so this is worth taking only when a guarantee is wanted in
/// place of an argument about deployment lifetimes.
///
/// The reservation ceiling is [`u32::MAX`] rather than the 64 bits the field
/// could hold, because the count is reported to callers as a `u32`.
#[cfg(feature = "dwcas")]
#[cfg_attr(docsrs, doc(cfg(feature = "dwcas")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wide;
#[cfg(feature = "dwcas")]
impl sealed::Sealed for Wide {}
#[cfg(feature = "dwcas")]
impl ClaimLayout for Wide {
    type Word = u128;
    const WORD_BITS: u32 = 128;
    const POSITION_BITS: u32 = 64;
}

/// The position after `position`, wrapping at the width the layout gives it.
///
/// **Centralised because the width is no longer the type's.** A position is
/// carried in a `u64` but is only `L::POSITION_BITS` wide, so it wraps where the
/// packing says rather than where `u64` would. Spelling that as
/// `wrapping_add(1) & L::POSITION_MASK` at each of the dozen sites that need it
/// would be a dozen chances to omit the mask, and an omitted mask is not a
/// compile error -- it is a position that escapes its half of the claim word
/// and silently corrupts the reservation count beside it.
#[inline]
const fn advance<L: ClaimLayout>(position: u64) -> u64 {
    position.wrapping_add(1) & L::POSITION_MASK
}

/// How far `position` leads `head`, in the modular arithmetic the position
/// width defines.
///
/// Masked for [`advance`]'s reason. When the position was a `u32` the type
/// supplied this wrap for free; it no longer does.
#[inline]
const fn distance<L: ClaimLayout>(position: u64, head: u64) -> u64 {
    position.wrapping_sub(head) & L::POSITION_MASK
}

/// The two handles a constructor hands back.
///
/// Named because the layout parameter makes the pair long enough to obscure the
/// error type beside it, not because a caller is expected to write it: the
/// constructors return it and a caller destructures it immediately.
pub type Pair<T, L = Balanced> = (Producer<T, L>, Consumer<T, L>);

// The layouts' relationship to one another, checked by the compiler rather than
// by a test. These are facts about constants, so a test could only report after
// the fact, on a build somebody chose to run -- and the trade they describe is
// the whole reason more than one layout exists.
const _: () = {
    assert!(
        <Perpetual as ClaimLayout>::MAX_RESERVED < <Enduring as ClaimLayout>::MAX_RESERVED,
        "a deeper position must cost reservations, or it would be free and there would be no \
         choice to offer"
    );
    assert!(
        <Enduring as ClaimLayout>::MAX_RESERVED < <Balanced as ClaimLayout>::MAX_RESERVED,
        "the layouts must order consistently, or the table documenting them is wrong"
    );
    assert!(
        <Perpetual as ClaimLayout>::POSITION_MASK > <Enduring as ClaimLayout>::POSITION_MASK,
        "and the reservations given up must buy positions with them"
    );
    assert!(
        <Enduring as ClaimLayout>::POSITION_MASK > <Balanced as ClaimLayout>::POSITION_MASK,
        "as above, across the whole ordering"
    );
};

/// The largest capacity the default layout accepts.
///
/// Retained as a plain constant because it is public API and a caller may name
/// it. It is [`Balanced`]'s ceiling; other layouts have their own, reachable as
/// `<L as ClaimLayout>::BOUNDS_MAX`.
pub const BOUNDS_MAX: usize = <Balanced as ClaimLayout>::BOUNDS_MAX;

/// The capacities a layout accepts.
///
/// The minimum is two for the same reason [`slotwise_mpsc`](crate::slotwise_mpsc)'s is: with a
/// single slot, "published at `p`" and "free again on the next lap" would be the
/// same sequence number. The maximum is the layout's own, since the position
/// width decides how large a wrapping difference stays unambiguous.
const fn bounds<L: ClaimLayout>() -> Bounds {
    Bounds {
        min: 2,
        max: L::BOUNDS_MAX,
    }
}

/// Reads the position out of a claim word.
#[inline]
fn position_of<L: ClaimLayout>(word: L::Word) -> u64 {
    word.position(L::POSITION_MASK)
}

/// Reads the outstanding-reservation count out of a claim word.
#[inline]
fn reserved_of<L: ClaimLayout>(word: L::Word) -> u32 {
    word.reserved(L::POSITION_BITS)
}

/// Builds a claim word from its two halves.
///
/// **Why the word is one `AtomicU64` and not two `AtomicU32`s**, given that every
/// operation on it is `Relaxed` (see D-38 in DESIGN-NOTES.md): relaxed is a
/// statement about *ordering*, and says nothing about atomicity. The two halves
/// are read and written as a unit, so the load must be indivisible -- a torn read
/// would return a `(reserved, position)` pair that was never a state this queue
/// was in, and the compare-and-swap protocol would be building on a value that
/// never existed. On `i686-pc-windows-msvc`, which D-18 keeps supported, that
/// costs a `cmpxchg8b` or an 8-byte SSE load rather than the two `mov`s a plain
/// `u64` would get. That cost is the point, not an overhead to optimize away.
///
/// The `|` could equally be `^`, or `+`, and a mutation run will report as much.
/// The halves are disjoint by construction -- the shift clears every bit the
/// position occupies -- so all three agree on every input, and no test can tell
/// them apart. `|` is kept because it says "these are separate fields" where the
/// others say "these are numbers"; the equivalence is recorded here so it is not
/// investigated again.
#[inline]
fn claim_word<L: ClaimLayout>(reserved: u32, position: u64) -> L::Word {
    L::Word::pack(reserved, position, L::POSITION_BITS, L::POSITION_MASK)
}

/// Creates a reserving multi-producer, single-consumer bounded array queue.
///
/// One producer handle is returned; further producers are made by cloning it,
/// and the queue is disconnected when the last of them -- and the last
/// outstanding [`Reservation`] -- is gone.
///
/// `capacity` must be a power of two between two and [`BOUNDS_MAX`], and is the
/// exact number of items the queue holds -- not a hint, and not rounded.
///
/// # Errors
///
/// Returns [`CapacityError`] if `capacity` is zero, is not a power of two, is
/// less than two, or exceeds [`BOUNDS_MAX`].
///
/// # Examples
///
/// A slot taken before the work that will fill it, so the delivery cannot fail
/// for want of room:
///
/// ```
/// use windows_waitable_queues::reserving_mpsc;
///
/// let (tx, rx) = reserving_mpsc::bounded::<u32>(2)?;
///
/// // Claimed up front, while failing is still cheap.
/// let slot = tx.reserve().expect("a fresh queue has room");
///
/// // The rest of the queue fills. Best-effort pushes cannot take the
/// // reserved slot, so one of these is refused.
/// tx.push(1).expect("one slot remains unreserved");
/// assert!(tx.push(2).is_err(), "the other belongs to the reservation");
///
/// // And the reservation is still honoured, on a queue that is otherwise full.
/// slot.send(99).expect("the room was already ours");
///
/// assert_eq!(rx.pop(), Ok(1));
/// assert_eq!(rx.pop(), Ok(99));
/// # Ok::<(), windows_waitable_queues::CapacityError>(())
/// ```
pub fn bounded<T>(capacity: usize) -> Result<Pair<T>, CapacityError> {
    build(capacity, Options::new())
}

/// Creates a queue whose claim word is divided as `L` says.
///
/// [`bounded`] is this with `L` left at [`Balanced`], and the two are otherwise
/// identical. See [`ClaimLayout`] for what the division trades: a lower ceiling
/// on outstanding reservations against a longer run before the claim position
/// recurs.
///
/// A separate entry point rather than a defaulted parameter on [`bounded`],
/// because Rust permits generic defaults on types but not on functions. The
/// types carry the default, so a caller who never names a layout never sees
/// one.
///
/// ```
/// use windows_waitable_queues::reserving_mpsc::{self, Perpetual};
///
/// // 255 outstanding reservations, and a claim position that recurs after
/// // 2^56 pushes rather than 2^32.
/// let (tx, rx) = reserving_mpsc::bounded_as::<u32, Perpetual>(4)?;
/// tx.push(1).expect("an empty queue has room");
/// assert_eq!(rx.pop(), Ok(1));
/// # Ok::<(), windows_waitable_queues::CapacityError>(())
/// ```
///
/// # Errors
///
/// As [`bounded`], against `L`'s own capacity ceiling.
pub fn bounded_as<T, L: ClaimLayout>(capacity: usize) -> Result<Pair<T, L>, CapacityError> {
    build(capacity, Options::new())
}

/// Creates a queue with both a layout and non-default behaviour.
///
/// [`bounded_as`] with [`Options`], as [`bounded_with`] is to [`bounded`].
///
/// # Errors
///
/// As [`bounded`], against `L`'s own capacity ceiling.
pub fn bounded_with_as<T, L: ClaimLayout>(
    capacity: usize,
    options: Options<T>,
) -> Result<Pair<T, L>, CapacityError> {
    build(capacity, options)
}

/// Creates a queue with something other than the default behaviour.
///
/// Identical to [`bounded`] except for what [`Options`] asks for.
///
/// **This is the shape where disposal matters most.** A reservation exists
/// because its message must not be lost; a message redeemed into a queue that
/// is then torn down undrained would be lost after all, just later and more
/// quietly. Pairing a reservation with a disposal sink is what closes that.
///
/// [`Options::tracking_high_water`] costs this shape almost nothing, unlike
/// [`slotwise_mpsc`](crate::slotwise_mpsc): the producer already reads the consumer's position
/// to decide whether there is room beyond the reservations, so the depth is a
/// subtraction of two numbers it is already holding.
///
/// # Errors
///
/// As [`bounded`].
pub fn bounded_with<T>(capacity: usize, options: Options<T>) -> Result<Pair<T>, CapacityError> {
    build(capacity, options)
}
fn build<T, L: ClaimLayout>(
    capacity: usize,
    options: Options<T>,
) -> Result<Pair<T, L>, CapacityError> {
    validate_capacity(capacity, bounds::<L>())?;

    let mut slots = Vec::with_capacity(capacity);
    for index in 0..capacity {
        slots.push(Slot {
            // Anything that is not `position + 1` for the position this slot
            // first serves, so the consumer sees it as unpublished. The
            // position's own value is the natural choice and matches the state
            // the slot returns to on every later lap.
            sequence: AtomicU64::new(index as u64),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        });
    }

    // Names `L::VALID` so the layout's own const assertions are evaluated.
    // An associated constant in a generic context is only checked where it is
    // used, so a layout whose constants contradict each other would otherwise
    // compile untouched until something happened to mention them.
    let () = L::VALID;

    let shared = Arc::new(Shared {
        layout: PhantomData,
        teardown: Teardown::new(options.disposal),
        metrics: Metrics::new(options.track_high_water),
        slots: slots.into_boxed_slice(),
        mask: capacity - 1,
        capacity,
        head: CacheAligned(AtomicU64::new(0)),
        claim: CacheAligned(<L::Word as ClaimWord>::zeroed()),
        producers: AtomicUsize::new(1),
        consumer_live: AtomicBool::new(true),
        doorbell: Doorbell::new(),
    });

    Ok((
        Producer {
            shared: Arc::clone(&shared),
            not_sync: PhantomData,
        },
        Consumer {
            shared,
            not_sync: PhantomData,
        },
    ))
}

/// One cell of the ring: an item, and a sequence number saying whether it has
/// been published.
struct Slot<T> {
    /// `position + 1` once the producer that claimed `position` has finished
    /// writing, and anything else before that.
    ///
    /// **This shape uses the sequence for one direction only.** In
    /// [`slotwise_mpsc`](crate::slotwise_mpsc) it answers both "has this been published?" for the
    /// consumer and "is this slot free?" for the producer. Here the producer
    /// answers the second from the consumer's position instead -- it has to read
    /// that position anyway, to count free slots for the reservations -- so
    /// nothing ever stores a "free again" value and the consumer's `pop` is one
    /// store shorter than `slotwise_mpsc`'s.
    sequence: AtomicU64,
    value: UnsafeCell<MaybeUninit<T>>,
}

struct Shared<T, L: ClaimLayout> {
    /// Ties the shared state to the layout its arithmetic is done in.
    ///
    /// Carries no data: the layout is entirely a set of compile-time constants,
    /// so this exists only because a type parameter must appear in the type.
    layout: PhantomData<L>,
    /// What becomes of undrained items at teardown.
    ///
    /// Read only by [`Shared::drop`], which holds `&mut self`, so it needs no
    /// synchronization and costs the hot paths nothing but its space.
    teardown: Teardown<T>,
    /// The counters this queue keeps about itself. See [`crate::metrics`].
    metrics: Metrics,
    slots: Box<[Slot<T>]>,
    mask: usize,
    capacity: usize,
    /// Where the consumer will next read. Written only by the consumer.
    ///
    /// Padded onto its own cache line, and here the padding earns its place
    /// twice over: unlike `slotwise_mpsc`, *every* producer reads this on *every* push,
    /// so letting the claim word share the line would put the consumer's writes
    /// directly in their path.
    head: CacheAligned<AtomicU64>,
    /// The outstanding-reservation count and the claim position, packed.
    ///
    /// One word because they must be claimed together; see the [module
    /// documentation](self) for why two atomics cannot be made correct with any
    /// amount of fencing.
    claim: CacheAligned<<L::Word as ClaimWord>::Atomic>,
    /// How many producer handles and outstanding reservations are alive.
    ///
    /// **A reservation counts as a producer**, which is not bookkeeping
    /// pedantry: a reservation is a promise of a message still to come, so a
    /// consumer that saw the stream end while one was outstanding would be told
    /// the queue was finished and then handed an item. That would lose exactly
    /// the message the reservation existed to protect.
    producers: AtomicUsize,
    consumer_live: AtomicBool,
    /// Readiness as a waitable `HANDLE`. Costs nothing until somebody asks for
    /// the handle, so a polling consumer never allocates a kernel object.
    doorbell: Doorbell,
}

// SAFETY: a slot is written by exactly one producer -- the one whose
// compare-and-swap claimed that position -- and read by exactly one consumer,
// which reads it only after observing the release store of `position + 1` that
// publishes it. The write of the item therefore happens-before the read, and no
// two threads ever touch the same slot's contents at the same time. `T: Send` is
// required and sufficient because an item is moved between threads and never
// referenced from both.
//
// The `teardown` field is deliberately NOT covered by that argument, because it
// cannot be: it holds a boxed FnMut, which is Send but not Sync, so this
// impl is forcing Sync onto a field that does not have it. That is sound for
// a narrower reason -- the field is unreachable through a shared reference. It
// is private, no method reads it, and the only access is from Drop, which
// holds &mut self and runs when the last handle is already gone. So no two
// threads can reach it at all, concurrently or otherwise.
unsafe impl<T: Send, L: ClaimLayout> Sync for Shared<T, L> {}
// SAFETY: as above; sending the shared state is sending the items it holds.
unsafe impl<T: Send, L: ClaimLayout> Send for Shared<T, L> {}

impl<T, L: ClaimLayout> Shared<T, L> {
    /// The capacity as the width the positions are counted in.
    ///
    /// Lossless by construction: [`BOUNDS`] caps the capacity at 2^31.
    fn capacity_u64(&self) -> u64 {
        debug_assert!(self.capacity <= BOUNDS_MAX);
        self.capacity as u64
    }

    /// Whether a *best-effort* claim may take the slot at `position`, given the
    /// reservations currently outstanding.
    ///
    /// Written as a subtraction from the capacity rather than as
    /// `occupied + reserved >= capacity`, because both terms can reach 2^31 and
    /// their sum would overflow the width the positions are counted in. The
    /// invariant guarantees `reserved <= capacity`, so this cannot underflow.
    ///
    /// **The answer is only meaningful for a claim word that is still current.**
    /// `position` comes from a claim word and `head` is read here, so the two
    /// need not describe the same instant: if other producers claim and publish
    /// past a stale `position` and the consumer drains them, `head` overtakes it
    /// and the subtraction wraps to near [`u32::MAX`] -- "full" computed from a
    /// pair of readings that never coexisted. Callers therefore treat a `false`
    /// as provisional and re-read the claim before reporting it (see
    /// [`Producer::push`]).
    fn has_room_beyond_reservations(&self, position: u64, reserved: u32) -> bool {
        let capacity = self.capacity_u64();
        debug_assert!(
            u64::from(reserved) <= capacity,
            "reservations may never exceed the capacity they are claimed from"
        );
        let occupied = distance::<L>(position, self.head.0.load(Ordering::Acquire));
        occupied < capacity - u64::from(reserved)
    }

    /// Items currently held, as a snapshot.
    ///
    /// Counts slots a producer has claimed but not yet finished writing, for the
    /// reason `slotwise_mpsc`'s does: counting only published items would need a walk of
    /// the ring, and this number is a metric rather than a control-flow input.
    ///
    /// **Clamped to the capacity**, for the reason given on `slotwise_mpsc`'s
    /// twin: the claim word and `head` are two loads at two instants, so a
    /// consumer draining past the sampled position makes the wrapping
    /// subtraction produce a number near `u32::MAX`. A bounded queue must never
    /// report holding more than it can.
    fn len(&self) -> usize {
        let position = position_of::<L>(L::Word::load(&self.claim.0, Ordering::Relaxed));
        let head = self.head.0.load(Ordering::Acquire);
        (distance::<L>(position, head) as usize).min(self.capacity)
    }

    /// How many further items a best-effort push could still place, as a
    /// snapshot.
    ///
    /// **Not `capacity - len()`, which is what the [`Bounded`](crate::Bounded)
    /// default computes and is wrong for this shape.** `len` deliberately
    /// excludes outstanding reservations, so on an empty queue of four with one
    /// slot reserved the default answers four while only three items fit -- and
    /// a caller sizing a batch from it would be told there is room the
    /// reservation is holding.
    ///
    /// The claim word is read **once**: the position and the reservation count
    /// share it precisely so the two cannot be sampled at different instants,
    /// and reading it twice would reintroduce the skew this shape packs them
    /// together to avoid. `head` is still a second load, so the result is
    /// clamped for the reason `len` is.
    fn remaining(&self) -> usize {
        let word = L::Word::load(&self.claim.0, Ordering::Relaxed);
        let head = self.head.0.load(Ordering::Acquire);
        let capacity = self.capacity_u64();
        let occupied = distance::<L>(position_of::<L>(word), head).min(capacity);
        let spoken_for = occupied.saturating_add(u64::from(reserved_of::<L>(word)));
        capacity.saturating_sub(spoken_for) as usize
    }

    /// Whether the consumer would find an item right now.
    ///
    /// Asks precisely what [`Consumer::pop`] asks -- is the slot at the head
    /// position published? A claimed-but-unpublished slot answers `false`, which
    /// is the right answer: the consumer may safely park on it, because the
    /// producer's publishing store is followed by a signal.
    fn has_ready_item(&self) -> bool {
        // Acquire, matching every other load of `head`. This thread is `head`'s
        // only writer, so coherence alone would make a relaxed load read its
        // own latest value -- but `head` carries a release store (in `pop`), and
        // a relaxed load on an atomic that also carries acquire/release
        // operations is a plain load: unanchored, free to be moved by the
        // optimizer or the processor, with no defined position relative to the
        // ordered operations on the same object. Uniform acquire is what makes
        // the load mean, at this point in the source, what it appears to mean.
        let position = self.head.0.load(Ordering::Acquire);
        let slot = &self.slots[position as usize & self.mask];
        slot.sequence.load(Ordering::Acquire) == advance::<L>(position)
    }

    /// Give up one unit of the producer count, signalling if it was the last.
    ///
    /// Shared by [`Producer`] and [`Reservation`] because they are the same
    /// obligation: both represent a message that may still arrive, and the last
    /// of either to leave is the one that ends the stream.
    fn release_producer(&self) {
        // `AcqRel` carries both halves. The release half publishes everything
        // this producer pushed to whichever thread observes the count reaching
        // zero, so a consumer that sees the disconnection can trust that
        // draining to empty really has drained everything. The acquire half
        // makes *this* thread -- when it is the one that drives the count to
        // zero -- see the other producers' pushes, which is what makes the
        // signal below meaningful.
        if self.producers.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }

        // Disconnection is a wakeup like any other, and the only one nobody else
        // can deliver. A consumer blocked on the doorbell would otherwise wait
        // forever for an item that can no longer be sent.
        //
        // Only the *last* departure rings: an earlier one changes nothing a
        // consumer could act on, and waking it to discover that would be a
        // spurious wakeup per departing thread.
        self.doorbell.signal();
    }

    /// Write an item into a claimed position and publish it.
    ///
    /// # Safety
    ///
    /// The caller must have claimed `position` by advancing the claim word, and
    /// must not have published it already. A position is claimed by exactly one
    /// producer, so this is the only writer of the slot.
    unsafe fn publish(&self, position: u64, item: T) {
        // Gated, matching `slotwise_mpsc`: untracked, this costs one predictable
        // branch on a field written once at construction, and the shared `head`
        // line is not touched at all.
        //
        // **Before the publication below, and that placement is load-bearing.**
        // The subtraction is only non-negative while the consumer cannot have
        // passed `position`, and what holds it back is precisely that `position`
        // is not published yet. Taken afterwards, the consumer is free to drain
        // past it, the subtraction wraps, and `fetch_max` keeps a vast number
        // forever -- the defect `slotwise_mpsc`'s twin comment records measuring.
        //
        // **Clamped, which its twin does not need to be.** There, the producer's
        // acquire load of the slot's sequence synchronizes-with the consumer
        // freeing that slot, so the `head` read here cannot be older than
        // `position - capacity + 1` and the depth is bounded by construction.
        // This shape has a second entry point with no such edge:
        // [`Reservation::send`] redeems without a room check, so the only `head`
        // its thread is ordered against is the one *`reserve`* read -- which may
        // be arbitrarily old by the time the reservation is redeemed. A stale
        // read can only over-report, never under-report, so clamping to the
        // capacity keeps the value an upper bound on a depth the queue really
        // reached rather than an unbounded one. See [`Observable::high_water`]
        // for what that bound is contracted to mean.
        //
        // [`Observable::high_water`]: crate::Observable::high_water
        // **This load is unconditional because the slot write below needs it,
        // not because the metric does.** It is the acquire half of the pair
        // [`Consumer::pop`] describes: freeing a slot is `head.store(Release)`,
        // and a producer may only write that slot after acquiring a `head` that
        // has passed it. [`Producer::push`] gets that edge from the room check
        // in [`Self::has_room_beyond_reservations`]; [`Reservation::send`]
        // deliberately has no room check, so without this load its non-atomic
        // write would race the consumer's non-atomic read of the previous
        // occupant -- a data race, and undefined behaviour, however reliably a
        // given target's codegen happens to order it today.
        //
        // Placing it here rather than in `send` covers every path with one
        // load.
        //
        // **The load must be fresh *enough*, and a single acquire load does not
        // guarantee that.** An earlier version of this comment argued that
        // because the claim invariant makes `head >= position - capacity + 1`
        // true at the exchange, and `head` never moves backwards, a later load
        // "can only be fresher". That conflates what `head` *is* in modification
        // order with what a load is *guaranteed to observe*: an acquire load may
        // legally return any earlier value in the modification order, and
        // synchronizes only with the release store whose value it actually
        // reads.
        //
        // Nothing else forces freshness here. The claim exchange is `Relaxed`,
        // so it carries no edge; and while `reserve` does read `head`, a
        // `Reservation` is `Send`, so the thread that redeems one **need never
        // have read `head` at all** -- leaving no coherence constraint to
        // inherit. A reservation held across a full lap and redeemed elsewhere
        // is exactly the case. Raised in PR #56 review.
        //
        // So the load is repeated until it observes a `head` that has actually
        // passed this position's previous occupant. That is the store which
        // frees the slot, so observing it (or any later one, by the same
        // consumer and therefore sequenced after its read) is precisely the
        // edge the write below needs. The loop terminates because the claim
        // invariant makes the condition already true in modification order --
        // this waits to *see* it, not for it to *become* true.
        let mut head = self.head.0.load(Ordering::Acquire);
        while distance::<L>(position, head) >= self.capacity_u64() {
            std::hint::spin_loop();
            head = self.head.0.load(Ordering::Acquire);
        }
        if self.metrics.tracks_high_water() {
            let depth = (distance::<L>(position, head) + 1) as usize;
            // **The clamp is unreachable from here, and is kept deliberately.**
            // The wait above exits only once `position - head < capacity`, so
            // `depth <= capacity` already holds and `min` never binds. It was
            // load-bearing when this was a single unvalidated load: a stale
            // `head` then made the depth an unbounded over-report, and
            // `the_high_water_mark_never_exceeds_the_capacity` drove exactly
            // that. Waiting for a fresh `head` removes the over-report at its
            // source, so that test was replaced by
            // `publish_waits_for_a_head_that_has_freed_the_slot`, which asserts
            // the fix instead of the mitigation.
            //
            // Kept because it costs one register-to-register `min` on a path
            // already doing an atomic load, and because it bounds the metric by
            // the shape's own contract rather than by an argument a future
            // change to the wait might invalidate silently. A mutation run will
            // report it as a survivor; that is expected, and it is unreachable
            // code rather than a missing test.
            self.metrics.record_depth(depth.min(self.capacity));
        }

        let slot = &self.slots[position as usize & self.mask];
        // SAFETY: the caller's claim makes this thread the only writer, and the
        // acquire load of `head` above -- repeated until it observed a value
        // past this position's previous occupant -- synchronizes-with the
        // `head.store` by which the consumer freed this slot a lap ago, so its
        // read of the previous occupant happens-before this write.
        //
        // The claim alone is not enough. It establishes that the slot is
        // *logically* free -- `occupied + reserved <= capacity` with
        // `reserved >= 1` -- but a non-atomic write racing a non-atomic read
        // needs a happens-before edge, not merely a logical guarantee that the
        // read is over. The load above is that edge.
        unsafe {
            (*slot.value.get()).write(item);
        }

        // Release, and this is the publication: it must come after the write,
        // and this is what forbids the compiler and the processor from moving it
        // earlier. Until it lands, the consumer sees the slot as
        // claimed-but-empty and skips it.
        slot.sequence
            .store(advance::<L>(position), Ordering::Release);

        // After the publication, never before: the doorbell says "there is
        // something to take", and that must not become true before the item is
        // actually takeable. A consumer woken early would find nothing, clear
        // the doorbell, and go back to sleep on an item that is about to exist.
        //
        // A producer may signal while an *earlier* position is still
        // unpublished, so the consumer wakes and finds nothing. That is a
        // spurious wakeup, which the protocol tolerates by construction: the
        // producer holding the earlier slot signals in its turn.
        self.doorbell.signal();
    }
}

impl<T, L: ClaimLayout> Drop for Shared<T, L> {
    fn drop(&mut self) {
        // Every handle is gone, so no synchronization is needed and the
        // positions can be read directly. A slot between the two positions still
        // holds an item nobody took, and dropping the queue must drop those
        // rather than leak them.
        //
        // The sequence is consulted per slot rather than assuming every position
        // in the range holds an item. A producer cannot be mid-push here -- it
        // would have to hold a handle, and there are none -- so in practice
        // every one does; the check states the invariant the read depends on
        // instead of leaving it to that argument.
        let mask = self.mask;
        let head = *self.head.0.get_mut();
        let tail = position_of::<L>(L::Word::read_mut(&mut self.claim.0));
        let mut position = head;
        while position != tail {
            let published = advance::<L>(position);
            let slot = &mut self.slots[position as usize & mask];
            if *slot.sequence.get_mut() == published {
                // SAFETY: the slot's sequence says the producer finished writing
                // it and the consumer never took it, so it holds an initialized
                // item. It is read exactly once, because `position` advances
                // every iteration and the slot is never read again.
                let item = unsafe { slot.value.get_mut().assume_init_read() };
                self.teardown.dispose(item);
            }
            position = advance::<L>(position);
        }
    }
}

/// A writing half of a [`reserving_mpsc`](self) queue.
///
/// [`Clone`], so producers multiply by cloning rather than by sharing: each
/// thread owns its own handle. Not [`Sync`], so a handle is used by one thread
/// at a time.
pub struct Producer<T, L: ClaimLayout = Balanced> {
    shared: Arc<Shared<T, L>>,
    /// Removes [`Sync`] without removing [`Send`]. A [`Cell`] is exactly that
    /// shape, and no value of it is ever created.
    not_sync: PhantomData<Cell<()>>,
}

impl<T, L: ClaimLayout> Producer<T, L> {
    /// Appends an item, best-effort.
    ///
    /// **Cannot take a reserved slot.** A queue with one free slot and one
    /// outstanding reservation refuses this, which is the reservation doing its
    /// job rather than a malfunction.
    ///
    /// # Errors
    ///
    /// [`PushError::Full`] when no unreserved room remains, which is the
    /// backpressure signal, and [`PushError::Disconnected`] when the consumer is
    /// gone. Either way the item comes back.
    pub fn push(&self, item: T) -> Result<(), PushError<T>> {
        // Relaxed: this load only proposes a claim. The compare-and-swap below
        // is what makes it, and fails if the proposal was stale, so a stale read
        // costs a retry rather than correctness.
        let mut word = L::Word::load(&self.shared.claim.0, Ordering::Relaxed);
        let position = loop {
            let position = position_of::<L>(word);
            let reserved = reserved_of::<L>(word);
            #[cfg(test)]
            crate::race_hooks::CLAIM.run();

            if !self.shared.has_room_beyond_reservations(position, reserved) {
                // Provisional, not authoritative. `position` came from `word`
                // and `head` was read inside the check, so a `word` that has
                // since moved makes the two readings describe different
                // instants -- and once `head` passes a stale `position` the
                // subtraction wraps, so an *empty* queue reports full. Re-read
                // the claim: if it moved, this answer was computed from a
                // snapshot that never existed, so retry rather than refuse.
                let current = L::Word::load(&self.shared.claim.0, Ordering::Relaxed);
                if current != word {
                    word = current;
                    continue;
                }
                // Report disconnection in preference to fullness: a full queue
                // whose consumer is gone will never drain, and telling the
                // caller to retry would be telling it to spin forever.
                if !self.shared.consumer_live.load(Ordering::Acquire) {
                    // Not counted as a refusal: this is the end of the stream,
                    // not backpressure.
                    return Err(PushError::Disconnected(item));
                }
                self.shared.metrics.record_refusal();
                return Err(PushError::Full(item));
            }
            if !self.shared.consumer_live.load(Ordering::Acquire) {
                return Err(PushError::Disconnected(item));
            }

            // Relaxed on both sides is sufficient: this exchange orders nothing
            // but the claim itself. The item's visibility comes from the release
            // store that publishes the slot, and the freedom to write the slot
            // comes from the acquire load of `head` inside the room check.
            //
            // The reservation count is carried through unchanged, which is what
            // makes a racing `reserve` fail its own exchange and re-read rather
            // than have its increment silently overwritten.
            match L::Word::compare_exchange_weak(
                &self.shared.claim.0,
                word,
                claim_word::<L>(reserved, advance::<L>(position)),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break position,
                Err(actual) => word = actual,
            }
        };

        // SAFETY: this thread's compare-and-swap claimed `position`, which no
        // other producer can also have claimed, and it has not been published.
        unsafe {
            self.shared.publish(position, item);
        }
        Ok(())
    }

    /// Claims one slot for a message that must not be lost.
    ///
    /// See [`Reserving::reserve`](crate::Reserving::reserve) for what a
    /// reservation is for. The short form: failing here is cheap, because no
    /// work has been started yet, whereas failing at delivery means blocking or
    /// losing the message.
    ///
    /// The queue stays connected while a reservation is outstanding, so a
    /// consumer will not be told the stream ended and then handed the item.
    #[must_use = "a reservation withholds capacity from every other producer until it is used or dropped"]
    pub fn reserve(&self) -> Option<Reservation<T, L>> {
        let mut word = L::Word::load(&self.shared.claim.0, Ordering::Relaxed);
        loop {
            let position = position_of::<L>(word);
            let reserved = reserved_of::<L>(word);
            #[cfg(test)]
            crate::race_hooks::CLAIM.run();

            if !self.shared.has_room_beyond_reservations(position, reserved) {
                // Provisional for the reason `push`'s matching check is: a
                // stale `word` and a freshly-read `head` need not describe the
                // same instant, and once `head` passes a stale `position` the
                // subtraction wraps and an empty queue refuses a reservation.
                let current = L::Word::load(&self.shared.claim.0, Ordering::Relaxed);
                if current != word {
                    word = current;
                    continue;
                }
                return None;
            }

            // The count's own half of the word can overflow into the position's
            // before the capacity is exhausted, once a layout gives it fewer
            // bits than the capacity has slots. Refusing here is what decouples
            // the two ceilings: the capacity is bounded by the ring, and the
            // reservations by whatever the layout left room for.
            //
            // **Checked against the word this iteration read, not against a
            // separate load.** The count and the position share the word
            // precisely so a decision about one cannot be made against a stale
            // reading of the other, and the exchange below re-validates the
            // whole word -- so a racing `reserve` that got there first makes
            // this one fail and re-read rather than exceed the ceiling.
            if u64::from(reserved) >= L::MAX_RESERVED {
                return None;
            }

            // The position is carried through unchanged: a reservation claims
            // capacity, not an order. Where the item lands is decided when the
            // reservation is redeemed, so a slot held for a long time does not
            // stall everything queued behind it.
            match L::Word::compare_exchange_weak(
                &self.shared.claim.0,
                word,
                claim_word::<L>(reserved + 1, position),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // Relaxed: this thread already holds a live producer handle,
                    // so the count cannot reach zero during this call and no
                    // other thread's decision depends on when the increment
                    // becomes visible. The pairing that matters is in
                    // `release_producer`.
                    self.shared.producers.fetch_add(1, Ordering::Relaxed);
                    return Some(Reservation {
                        shared: Arc::clone(&self.shared),
                        not_sync: PhantomData,
                    });
                }
                Err(actual) => word = actual,
            }
        }
    }

    /// The exact number of items this queue holds when full.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Items currently held, as a snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Whether the queue holds nothing, as a snapshot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Slots currently claimed by a reservation and not yet redeemed, as a
    /// snapshot.
    #[must_use]
    pub fn outstanding_reservations(&self) -> usize {
        reserved_of::<L>(L::Word::load(&self.shared.claim.0, Ordering::Relaxed)) as usize
    }

    /// Whether the next best-effort push would be refused, as a snapshot.
    ///
    /// True when the queue is full *or* every remaining slot is reserved, since
    /// those are indistinguishable to a best-effort caller. Advisory only:
    /// another producer may take the last slot between this call and the push.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.remaining() == 0
    }

    /// How many further items a best-effort push could still place, as a
    /// snapshot.
    ///
    /// **Reservations are subtracted**, unlike `capacity() - len()`: a reserved
    /// slot is spoken for, so counting it as room would promise a push that
    /// [`push`](Self::push) is guaranteed to refuse. Advisory only, like every
    /// other gauge here.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.shared.remaining()
    }

    /// Whether the consumer has been dropped.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        !self.shared.consumer_live.load(Ordering::Acquire)
    }
}

impl<T, L: ClaimLayout> Clone for Producer<T, L> {
    fn clone(&self) -> Self {
        // Relaxed, for the reason given in `reserve`.
        self.shared.producers.fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(&self.shared),
            not_sync: PhantomData,
        }
    }
}

// Hand-written rather than derived: deriving would demand `T: Debug`, which
// would make a handle to a queue of non-`Debug` items un-printable for no
// reason. The item type is not the handle's business, so the handle reports the
// queue's state instead.
impl<T, L: ClaimLayout> fmt::Debug for Producer<T, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("reserving_mpsc::Producer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("reserved", &self.outstanding_reservations())
            .field("producers", &self.shared.producers.load(Ordering::Relaxed))
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T, L: ClaimLayout> Drop for Producer<T, L> {
    fn drop(&mut self) {
        self.shared.release_producer();
    }
}

/// A slot claimed in advance, which [`Reservation::send`] redeems.
///
/// Owned rather than borrowed from the [`Producer`], and [`Send`], because that
/// is the shape the use case has: an operation reserves its completion slot when
/// it is submitted and redeems it from whichever thread the completion arrives
/// on. ([`spsc`](crate::spsc)'s reservation borrows instead, because there the
/// producer handle *is* the single-producer guarantee and letting a reservation
/// outlive it would create a second one.)
///
/// Dropping it returns the slot to the queue.
#[must_use = "a reservation withholds capacity from every other producer until it is used or dropped"]
pub struct Reservation<T, L: ClaimLayout = Balanced> {
    shared: Arc<Shared<T, L>>,
    /// See [`Producer::not_sync`]. A reservation may be *moved* between threads
    /// but is used by one at a time, exactly like the handle that made it.
    not_sync: PhantomData<Cell<()>>,
}

impl<T, L: ClaimLayout> Reservation<T, L> {
    /// Delivers into the reserved slot.
    ///
    /// **This cannot fail for want of room**, which is the entire purpose: the
    /// slot was withheld from every other producer from the moment the
    /// reservation was taken. See [`Disconnected`] for why that is the only
    /// error and why the type says so.
    ///
    /// # Errors
    ///
    /// [`Disconnected`] if the consumer is gone, carrying the item back so it
    /// can be accounted for rather than silently dropped.
    pub fn send(self, item: T) -> Result<(), Disconnected<T>> {
        if !self.shared.consumer_live.load(Ordering::Acquire) {
            // Dropping `self` on the way out releases the slot and the producer
            // count, which is what should happen: this message is never coming.
            return Err(Disconnected(item));
        }

        // Redeem and claim in ONE exchange: the count falls by one as the
        // position rises by one, so `occupied + reserved` -- the quantity the
        // whole invariant is about -- is never momentarily wrong, and no
        // concurrent producer can observe a state in which this slot looks
        // available.
        //
        // There is no room check here, and its absence is the guarantee. The
        // invariant `occupied + reserved <= capacity` with `reserved >= 1` means
        // `occupied < capacity`, so the slot at this position is one the
        // consumer has already finished with.
        let mut word = L::Word::load(&self.shared.claim.0, Ordering::Relaxed);
        let position = loop {
            let position = position_of::<L>(word);
            let reserved = reserved_of::<L>(word);
            debug_assert!(
                reserved >= 1,
                "this reservation is outstanding, so the count cannot be zero"
            );

            match L::Word::compare_exchange_weak(
                &self.shared.claim.0,
                word,
                claim_word::<L>(reserved - 1, advance::<L>(position)),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break position,
                Err(actual) => word = actual,
            }
        };

        // SAFETY: the exchange above claimed `position` for this thread alone,
        // and the invariant argued for in the comment means the slot is free.
        unsafe {
            self.shared.publish(position, item);
        }

        // The slot has been given up as part of the exchange above, so the
        // `Drop` that would give it up again must not run. The producer count,
        // however, still has to be released -- this reservation's promise is now
        // fulfilled, and if it was the last outstanding one the stream ends
        // here.
        //
        // **`mem::forget` would be wrong here, and was wrong here**: this type
        // owns an `Arc`, and forgetting it leaks that strong reference, so the
        // shared state is never dropped and every item still in the ring leaks
        // with it. `ManuallyDrop` plus a move-out suppresses only *this type's*
        // `Drop` while leaving the `Arc`'s own to run exactly once.
        let this = core::mem::ManuallyDrop::new(self);
        // SAFETY: `this` is a `ManuallyDrop`, so its own destructor never runs
        // and the field is not read again after this move.
        let shared = unsafe { core::ptr::read(&this.shared) };
        shared.release_producer();
        // `shared` falls out of scope here, releasing the reference this
        // reservation held.
        Ok(())
    }

    /// Whether the consumer has been dropped, so redeeming would fail.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        !self.shared.consumer_live.load(Ordering::Acquire)
    }
}

impl<T, L: ClaimLayout> fmt::Debug for Reservation<T, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("reserving_mpsc::Reservation")
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T, L: ClaimLayout> Drop for Reservation<T, L> {
    fn drop(&mut self) {
        // Give the slot back. Only the count moves: the position is untouched,
        // because an unredeemed reservation never occupied a position.
        let mut word = L::Word::load(&self.shared.claim.0, Ordering::Relaxed);
        loop {
            let reserved = reserved_of::<L>(word);
            debug_assert!(
                reserved >= 1,
                "this reservation is outstanding, so the count cannot be zero"
            );
            match L::Word::compare_exchange_weak(
                &self.shared.claim.0,
                word,
                claim_word::<L>(reserved - 1, position_of::<L>(word)),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => word = actual,
            }
        }
        self.shared.release_producer();
    }
}

/// The reading half of a [`reserving_mpsc`](self) queue.
///
/// Neither [`Clone`] nor [`Sync`], which is what makes "single consumer" a fact
/// the compiler checks rather than a rule to remember.
pub struct Consumer<T, L: ClaimLayout = Balanced> {
    shared: Arc<Shared<T, L>>,
    /// See [`Producer::not_sync`].
    not_sync: PhantomData<Cell<()>>,
}

impl<T, L: ClaimLayout> Consumer<T, L> {
    /// Takes the oldest item.
    ///
    /// # Errors
    ///
    /// [`TryRecvError::Empty`] when nothing is queued right now, and
    /// [`TryRecvError::Disconnected`] when every producer is gone *and* the
    /// queue has been drained -- in that order, so the tail of a stream whose
    /// producers have already departed is still delivered. See
    /// [`Consumer::pop`](crate::Consumer::pop) for why that ordering is a
    /// guarantee rather than an implementation detail.
    pub fn pop(&self) -> Result<T, TryRecvError> {
        match self.take() {
            Some(item) => Ok(item),
            // Only on the empty path, so a successful take never pays for this
            // load. The queue must be observed empty *before* disconnection is
            // reported, which is exactly what this ordering enforces.
            None if self.is_disconnected() => Err(TryRecvError::Disconnected),
            None => Err(TryRecvError::Empty),
        }
    }

    /// The take itself, without the disconnection question.
    fn take(&self) -> Option<T> {
        // Acquire, matching every other load of `head`. Sole-writer coherence
        // would suffice to read this thread's own latest value, but `head` also
        // carries the release store below, and a relaxed load mixed onto such an
        // atomic is a plain load the code generator may move. See
        // `has_ready_item` for the full argument.
        let position = self.shared.head.0.load(Ordering::Acquire);
        let slot = &self.shared.slots[position as usize & self.shared.mask];
        // Acquire: pairs with the producer's release store, so an item it
        // published is visible here.
        if slot.sequence.load(Ordering::Acquire) != advance::<L>(position) {
            return None;
        }

        // SAFETY: the sequence says the producer that claimed this position
        // finished writing it, and the release/acquire pair above makes that
        // write visible here. This is the only consumer, and the position is
        // given up below, so the item is read exactly once.
        let item = unsafe { (*slot.value.get()).assume_init_read() };

        // Release, and this is what frees the slot: a producer reads `head` with
        // an acquire load to count free slots, so this store must not become
        // visible before the read above completes, or that producer could claim
        // the position and overwrite an item this thread had not finished
        // taking.
        //
        // Note that nothing stores a "free again" sequence here, unlike `slotwise_mpsc`.
        // Advancing `head` *is* the release, because this shape's producers
        // decide freedom from `head` rather than from the sequence.
        self.shared
            .head
            .0
            .store(advance::<L>(position), Ordering::Release);
        Some(item)
    }

    /// The exact number of items this queue holds when full.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity
    }

    /// Items currently held, as a snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.len()
    }

    /// Whether the queue holds nothing, as a snapshot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Slots currently claimed by a reservation and not yet redeemed, as a
    /// snapshot.
    ///
    /// Offered on the consumer as well as the producer because it is the
    /// difference between "nothing is coming" and "something was promised":
    /// a drained queue with an outstanding reservation is not an idle one.
    #[must_use]
    pub fn outstanding_reservations(&self) -> usize {
        reserved_of::<L>(L::Word::load(&self.shared.claim.0, Ordering::Relaxed)) as usize
    }

    /// How many further items a best-effort push could still place, as a
    /// snapshot.
    ///
    /// The same number [`Producer::remaining`] reports, and offered here for
    /// the same reason `outstanding_reservations` is: a consumer deciding
    /// whether to keep draining wants the producers' view of the room left, and
    /// that view subtracts reservations rather than treating them as free.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.shared.remaining()
    }

    /// Whether a further best-effort push would be refused for want of room.
    ///
    /// The consumer's view of the question the producer answers, so a caller
    /// holding only this handle need not import [`Bounded`](crate::Bounded).
    #[must_use]
    pub fn is_full(&self) -> bool {
        crate::Bounded::is_full(self)
    }

    /// Takes items until the queue is momentarily empty.
    ///
    /// The inherent form of [`Consumer::drain`](crate::Consumer::drain), so it
    /// works without importing the trait.
    pub fn drain(&self) -> crate::Drain<'_, Self> {
        crate::Consumer::drain(self)
    }

    /// Takes items until the queue is momentarily empty.
    ///
    /// An alias for [`Self::drain`] under the name most of the ecosystem uses.
    pub fn try_iter(&self) -> crate::Drain<'_, Self> {
        crate::Consumer::drain(self)
    }

    /// Whether every producer and every outstanding reservation is gone.
    ///
    /// **Check this only after [`Self::pop`] has returned `None`.** A producer
    /// may push and then drop, so a queue can be disconnected and still hold
    /// items; testing this first would discard them.
    #[must_use]
    pub fn is_disconnected(&self) -> bool {
        self.shared.producers.load(Ordering::Acquire) == 0
    }

    /// Borrows the queue's readiness as a waitable `HANDLE`.
    ///
    /// The event is created on the first call, so a consumer that only ever
    /// polls with [`Self::pop`] is charged for no kernel object.
    ///
    /// # Waiting on it correctly
    ///
    /// **Do not simply wait and then drain.** Use [`Self::arm`] to decide
    /// whether waiting is safe, or the wait can miss an item and block forever;
    /// [`spsc::Consumer::doorbell`](crate::spsc::Consumer::doorbell) carries the
    /// worked example, and the protocol is identical here.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        self.shared.doorbell.handle()
    }

    /// A duplicate of [`Self::doorbell`] that the caller owns.
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` or `DuplicateHandle`.
    pub fn doorbell_owned(&self) -> io::Result<OwnedHandle> {
        self.shared.doorbell.owned()
    }

    /// Clears the doorbell and reports whether a later push could be missed.
    ///
    /// `true` means the queue had nothing takeable after the doorbell was
    /// cleared, so any later push is guaranteed to signal. `false` means
    /// something arrived in the meantime.
    ///
    /// **`true` is not by itself permission to wait indefinitely.** It answers
    /// only whether a later *push* can be missed, and says nothing about the
    /// end of the stream: with every producer gone it still returns `true`,
    /// having just cleared the single ring their drop left behind. See
    /// [`Waitable::arm`](crate::Waitable::arm) for the four-step protocol an
    /// indefinite wait needs, and the example on [`Self::doorbell`] for it
    /// written out.
    ///
    /// Clearing must come before the check, which is the reverse of the order
    /// that reads naturally; see [D-9](../DESIGN-NOTES.md#d-9).
    ///
    /// # Errors
    ///
    /// Returns the error from `CreateEventW` on the first call.
    pub fn arm(&self) -> io::Result<bool> {
        // Before the clear, and so before the check: a producer running while no
        // event exists skips signalling, so the check has to come after the
        // event exists to catch what that skip left behind.
        self.shared.doorbell.handle()?;
        self.shared.doorbell.clear();
        #[cfg(test)]
        crate::race_hooks::ARM.run();
        // Deliberately not `is_empty`: the question is whether `pop` would find
        // something, and a claimed-but-unpublished slot is not something `pop`
        // can find.
        Ok(!self.shared.has_ready_item())
    }

    /// The last take before reporting the end of the stream.
    ///
    /// Called only after [`Self::is_disconnected`] has returned `true`, which
    /// makes the answer final rather than a snapshot. It guards a race that is
    /// real and narrow: a producer may push *and then* drop in the window
    /// between a receive's first `pop` and its disconnection check.
    fn finish(&self) -> Option<T> {
        self.take()
    }

    /// Takes the oldest item, blocking until one arrives.
    ///
    /// # Errors
    ///
    /// [`RecvError::Disconnected`] once every producer *and every outstanding
    /// reservation* is gone and the queue is drained. [`RecvError::Io`] if the
    /// doorbell cannot be created or waited on.
    pub fn recv(&self) -> Result<T, RecvError> {
        blocking::recv(self)
    }

    /// Takes the oldest item, blocking until one arrives or the deadline passes.
    ///
    /// # Errors
    ///
    /// [`RecvTimeoutError::Timeout`] if the deadline passes with the queue still
    /// empty, which is not a malfunction. Otherwise as [`Self::recv`].
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        blocking::recv_timeout(self, timeout)
    }
}

impl<T, L: ClaimLayout> Parked for Consumer<T, L> {
    type Item = T;

    fn pop(&self) -> Option<T> {
        Self::take(self)
    }

    fn finish(&self) -> Option<T> {
        Self::finish(self)
    }

    fn arm(&self) -> io::Result<bool> {
        Self::arm(self)
    }

    fn is_disconnected(&self) -> bool {
        Self::is_disconnected(self)
    }

    fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        Self::doorbell(self)
    }
}

/// See [`Producer`]'s impl for why this is hand-written.
impl<T, L: ClaimLayout> fmt::Debug for Consumer<T, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("reserving_mpsc::Consumer")
            .field("capacity", &self.capacity())
            .field("len", &self.len())
            .field("reserved", &self.outstanding_reservations())
            .field("producers", &self.shared.producers.load(Ordering::Relaxed))
            .field("disconnected", &self.is_disconnected())
            .finish()
    }
}

impl<T, L: ClaimLayout> Drop for Consumer<T, L> {
    fn drop(&mut self) {
        self.shared.consumer_live.store(false, Ordering::Release);
    }
}

impl<T, L: ClaimLayout> crate::Producer for Producer<T, L> {
    type Item = T;

    fn push(&self, item: T) -> Result<(), PushError<T>> {
        Self::push(self, item)
    }

    fn is_disconnected(&self) -> bool {
        Self::is_disconnected(self)
    }
}

impl<T, L: ClaimLayout> crate::Claim for Reservation<T, L> {
    type Item = T;

    fn send(self, item: T) -> Result<(), Disconnected<T>> {
        Self::send(self, item)
    }

    fn is_disconnected(&self) -> bool {
        Self::is_disconnected(self)
    }
}

impl<T, L: ClaimLayout> crate::Reserving for Producer<T, L> {
    type Item = T;
    type Reservation<'a>
        = Reservation<T, L>
    where
        Self: 'a;

    fn reserve(&self) -> Option<Reservation<T, L>> {
        Self::reserve(self)
    }

    fn outstanding_reservations(&self) -> usize {
        Self::outstanding_reservations(self)
    }
}

impl<T, L: ClaimLayout> crate::Consumer for Consumer<T, L> {
    type Item = T;

    fn pop(&self) -> Result<T, TryRecvError> {
        Self::pop(self)
    }

    fn is_disconnected(&self) -> bool {
        Self::is_disconnected(self)
    }
}

impl<T, L: ClaimLayout> crate::Bounded for Producer<T, L> {
    fn capacity(&self) -> usize {
        Self::capacity(self)
    }

    fn len(&self) -> usize {
        Self::len(self)
    }

    fn is_empty(&self) -> bool {
        Self::is_empty(self)
    }

    // Overridden, because the default `capacity - len` counts a reserved slot
    // as room: `len` excludes reservations by design, so an empty queue of four
    // holding one reservation would answer four while only three items fit.
    fn remaining(&self) -> usize {
        self.shared.remaining()
    }
}

impl<T, L: ClaimLayout> crate::Bounded for Consumer<T, L> {
    fn capacity(&self) -> usize {
        Self::capacity(self)
    }

    fn len(&self) -> usize {
        Self::len(self)
    }

    fn is_empty(&self) -> bool {
        Self::is_empty(self)
    }

    // The consumer's view has to agree with the producer's: both describe the
    // same queue, and a caller generic over `Bounded` should not get a different
    // answer depending on which handle it holds.
    fn remaining(&self) -> usize {
        Self::remaining(self)
    }
}

impl<T, L: ClaimLayout> Shared<T, L> {
    /// The counters, as the [`Observable`](crate::Observable) trait reports
    /// them. Written once so the two handles cannot drift apart.
    fn refused(&self) -> u64 {
        self.metrics.refused()
    }

    fn doorbell_rings(&self) -> u64 {
        self.doorbell.rings()
    }

    fn high_water(&self) -> Option<usize> {
        self.metrics.high_water()
    }
}

impl<T, L: ClaimLayout> Producer<T, L> {
    /// How many pushes have been refused for want of room.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.shared.refused()
    }

    /// How many times the doorbell has actually rung.
    #[must_use]
    pub fn doorbell_rings(&self) -> u64 {
        self.shared.doorbell_rings()
    }

    /// The deepest this queue has been, if tracking was asked for.
    #[must_use]
    pub fn high_water(&self) -> Option<usize> {
        self.shared.high_water()
    }
}

impl<T, L: ClaimLayout> Consumer<T, L> {
    /// How many pushes have been refused for want of room.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.shared.refused()
    }

    /// How many times the doorbell has actually rung.
    #[must_use]
    pub fn doorbell_rings(&self) -> u64 {
        self.shared.doorbell_rings()
    }

    /// The deepest this queue has been, if tracking was asked for.
    #[must_use]
    pub fn high_water(&self) -> Option<usize> {
        self.shared.high_water()
    }
}

impl<T, L: ClaimLayout> crate::Observable for Producer<T, L> {
    fn refused(&self) -> u64 {
        Self::refused(self)
    }

    fn doorbell_rings(&self) -> u64 {
        Self::doorbell_rings(self)
    }

    fn high_water(&self) -> Option<usize> {
        Self::high_water(self)
    }
}

impl<T, L: ClaimLayout> crate::Observable for Consumer<T, L> {
    fn refused(&self) -> u64 {
        Self::refused(self)
    }

    fn doorbell_rings(&self) -> u64 {
        Self::doorbell_rings(self)
    }

    fn high_water(&self) -> Option<usize> {
        Self::high_water(self)
    }
}

impl<T, L: ClaimLayout> crate::Waitable for Consumer<T, L> {
    fn doorbell(&self) -> io::Result<BorrowedHandle<'_>> {
        Self::doorbell(self)
    }

    fn doorbell_owned(&self) -> io::Result<OwnedHandle> {
        Self::doorbell_owned(self)
    }

    fn arm(&self) -> io::Result<bool> {
        Self::arm(self)
    }
}

#[cfg(test)]
mod tests;
