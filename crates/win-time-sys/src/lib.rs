// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Every `unsafe` here is a Win32 call, and each states why it is sound (WT-1.1).
#![deny(clippy::undocumented_unsafe_blocks)]

mod clock;
mod interrupt;
mod performance;
mod system;
mod timeline;

pub use clock::Clock;
pub use interrupt::{
    InterruptClock, InterruptTime, PreciseInterruptClock, PreciseUnbiasedInterruptClock,
    UnbiasedInterruptClock, UnbiasedInterruptTime,
};
pub use performance::{PerformanceClock, PerformanceCounter};
pub use system::{CoarseSystemClock, FileTime, PreciseSystemClock};
pub use timeline::{Ticks, TimePoint, Timeline};
