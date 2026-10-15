// Copyright (c) 2026 Mike Grier
//! Timelines, the points on them, and the distances between those points ([WT-D-2]).
//!
//! [WT-D-2]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/win-time-sys/DESIGN-NOTES.md#wt-d-2

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::num::NonZeroU64;
use core::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use core::time::Duration;

#[cfg(test)]
mod tests;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// Windows' 100-nanosecond unit, the tick of interrupt time and system time.
pub(crate) const HUNDRED_NANOSECOND_TICKS: NonZeroU64 = NonZeroU64::new(10_000_000).unwrap();

/// What a tick count means: how long one tick is, and what zero is.
///
/// A timeline is a type that is never made; it exists to keep values on different timelines
/// apart. Its tick is [`ticks_per_second`](Self::ticks_per_second); its zero is stated in its
/// documentation, because a value never needs it -- only a reader does.
pub trait Timeline: 'static {
    /// How many ticks make one second. It must not change for the life of the process.
    fn ticks_per_second() -> NonZeroU64;
}

/// A point on timeline `T`: a tick count since the timeline's zero.
///
/// Points on one timeline compare, subtract to [`Ticks`], and can be written down as a tick count
/// and read back ([`ticks`](Self::ticks), [`from_ticks`](Self::from_ticks)). Points on different
/// timelines do not compare at all, because their types differ:
///
/// ```compile_fail,E0308
/// use core::num::NonZeroU64;
/// use win_time_sys::{TimePoint, Timeline};
///
/// enum Boot {}
/// impl Timeline for Boot {
///     fn ticks_per_second() -> NonZeroU64 { NonZeroU64::new(10_000_000).unwrap() }
/// }
/// enum Calendar {}
/// impl Timeline for Calendar {
///     fn ticks_per_second() -> NonZeroU64 { NonZeroU64::new(10_000_000).unwrap() }
/// }
///
/// // The same unit, but not the same zero: refused.
/// let _ = TimePoint::<Boot>::from_ticks(1) < TimePoint::<Calendar>::from_ticks(2);
/// ```
pub struct TimePoint<T: Timeline> {
    ticks: u64,
    timeline: PhantomData<fn() -> T>,
}

/// A distance between two points on timeline `T`, in its ticks. Signed, because the later point
/// may be on either side.
pub struct Ticks<T: Timeline> {
    count: i64,
    timeline: PhantomData<fn() -> T>,
}

impl<T: Timeline> TimePoint<T> {
    /// The point `ticks` after the timeline's zero: how a recorded point is read back.
    #[must_use]
    pub const fn from_ticks(ticks: u64) -> Self {
        Self {
            ticks,
            timeline: PhantomData,
        }
    }

    /// The tick count since the timeline's zero: how a point is recorded.
    #[must_use]
    pub const fn ticks(self) -> u64 {
        self.ticks
    }

    /// How far this point is after `earlier` -- negative if it is before -- or `None` if the
    /// distance does not fit in [`Ticks`].
    #[must_use]
    pub fn checked_since(self, earlier: Self) -> Option<Ticks<T>> {
        let distance = i128::from(self.ticks) - i128::from(earlier.ticks);
        i64::try_from(distance).ok().map(Ticks::new)
    }

    /// The point `ticks` later, or `None` if it falls outside the timeline.
    #[must_use]
    pub fn checked_add(self, ticks: Ticks<T>) -> Option<Self> {
        let moved = i128::from(self.ticks) + i128::from(ticks.count);
        u64::try_from(moved).ok().map(Self::from_ticks)
    }

    /// The point `ticks` earlier, or `None` if it falls outside the timeline.
    #[must_use]
    pub fn checked_sub(self, ticks: Ticks<T>) -> Option<Self> {
        let moved = i128::from(self.ticks) - i128::from(ticks.count);
        u64::try_from(moved).ok().map(Self::from_ticks)
    }
}

impl<T: Timeline> Ticks<T> {
    /// No distance.
    pub const ZERO: Self = Self::new(0);

    /// `count` ticks of timeline `T`.
    #[must_use]
    pub const fn new(count: i64) -> Self {
        Self {
            count,
            timeline: PhantomData,
        }
    }

    /// The count of ticks, negative for a distance backwards.
    #[must_use]
    pub const fn count(self) -> i64 {
        self.count
    }

    /// Whether the distance is backwards.
    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.count < 0
    }

    /// The distance as a [`Duration`], or `None` if it is negative, since a `Duration` cannot be.
    /// A tick shorter than a nanosecond is truncated towards zero.
    #[must_use]
    pub fn to_duration(self) -> Option<Duration> {
        (!self.is_negative()).then(|| self.abs_duration())
    }

    /// The distance's length as a [`Duration`], whichever way it points. Truncated towards zero
    /// below a nanosecond.
    #[must_use]
    pub fn abs_duration(self) -> Duration {
        let per_second = T::ticks_per_second().get();
        let magnitude = self.count.unsigned_abs();
        let seconds = magnitude / per_second;
        let remainder = u128::from(magnitude % per_second);
        // The remainder is below one second's ticks, so this is below a second's nanoseconds.
        let nanos = remainder * NANOS_PER_SECOND / u128::from(per_second);
        Duration::new(
            seconds,
            u32::try_from(nanos).expect("below one second of nanoseconds"),
        )
    }

    /// `duration` in ticks of `T`, truncated towards zero, or `None` if it does not fit.
    #[must_use]
    pub fn from_duration(duration: Duration) -> Option<Self> {
        let per_second = u128::from(T::ticks_per_second().get());
        let whole = u128::from(duration.as_secs()).checked_mul(per_second)?;
        let part = u128::from(duration.subsec_nanos()) * per_second / NANOS_PER_SECOND;
        i64::try_from(whole.checked_add(part)?).ok().map(Self::new)
    }

    /// The sum, or `None` on overflow.
    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.count.checked_add(other.count).map(Self::new)
    }

    /// The difference, or `None` on overflow.
    #[must_use]
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.count.checked_sub(other.count).map(Self::new)
    }

    /// The distance the other way, or `None` for the one count that has no opposite.
    #[must_use]
    pub fn checked_neg(self) -> Option<Self> {
        self.count.checked_neg().map(Self::new)
    }
}

// By hand rather than derived: a derive would require the timeline marker itself to implement
// each trait, and a timeline is never made.

impl<T: Timeline> Clone for TimePoint<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Timeline> Copy for TimePoint<T> {}

impl<T: Timeline> PartialEq for TimePoint<T> {
    fn eq(&self, other: &Self) -> bool {
        self.ticks == other.ticks
    }
}

impl<T: Timeline> Eq for TimePoint<T> {}

impl<T: Timeline> PartialOrd for TimePoint<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Timeline> Ord for TimePoint<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ticks.cmp(&other.ticks)
    }
}

impl<T: Timeline> Hash for TimePoint<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ticks.hash(state);
    }
}

impl<T: Timeline> fmt::Debug for TimePoint<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TimePoint")
            .field("timeline", &core::any::type_name::<T>())
            .field("ticks", &self.ticks)
            .finish()
    }
}

impl<T: Timeline> Clone for Ticks<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Timeline> Copy for Ticks<T> {}

impl<T: Timeline> PartialEq for Ticks<T> {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count
    }
}

impl<T: Timeline> Eq for Ticks<T> {}

impl<T: Timeline> PartialOrd for Ticks<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Timeline> Ord for Ticks<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.count.cmp(&other.count)
    }
}

impl<T: Timeline> Hash for Ticks<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.count.hash(state);
    }
}

impl<T: Timeline> Default for Ticks<T> {
    fn default() -> Self {
        Self::ZERO
    }
}

impl<T: Timeline> fmt::Debug for Ticks<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ticks")
            .field("timeline", &core::any::type_name::<T>())
            .field("count", &self.count)
            .finish()
    }
}

// The operators panic on overflow, as `Duration`'s do; the `checked_` methods are the way to
// avoid that.

impl<T: Timeline> Sub for TimePoint<T> {
    type Output = Ticks<T>;

    fn sub(self, earlier: Self) -> Ticks<T> {
        self.checked_since(earlier)
            .expect("the distance between two time points overflowed")
    }
}

impl<T: Timeline> Add<Ticks<T>> for TimePoint<T> {
    type Output = Self;

    fn add(self, ticks: Ticks<T>) -> Self {
        self.checked_add(ticks)
            .expect("a time point moved outside its timeline")
    }
}

impl<T: Timeline> Sub<Ticks<T>> for TimePoint<T> {
    type Output = Self;

    fn sub(self, ticks: Ticks<T>) -> Self {
        self.checked_sub(ticks)
            .expect("a time point moved outside its timeline")
    }
}

impl<T: Timeline> AddAssign<Ticks<T>> for TimePoint<T> {
    fn add_assign(&mut self, ticks: Ticks<T>) {
        *self = *self + ticks;
    }
}

impl<T: Timeline> SubAssign<Ticks<T>> for TimePoint<T> {
    fn sub_assign(&mut self, ticks: Ticks<T>) {
        *self = *self - ticks;
    }
}

impl<T: Timeline> Add for Ticks<T> {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        self.checked_add(other).expect("a tick count overflowed")
    }
}

impl<T: Timeline> Sub for Ticks<T> {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        self.checked_sub(other).expect("a tick count overflowed")
    }
}

impl<T: Timeline> Neg for Ticks<T> {
    type Output = Self;

    fn neg(self) -> Self {
        self.checked_neg().expect("a tick count overflowed")
    }
}

impl<T: Timeline> AddAssign for Ticks<T> {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl<T: Timeline> SubAssign for Ticks<T> {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
