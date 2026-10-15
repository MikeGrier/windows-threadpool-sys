// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
// DI-D-4: no unsafe code, from the first commit. A Win32 call no lower crate wraps safely is added
// to that crate, never written here.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod delivery;
mod dioring;
mod error_code;
#[cfg(test)]
mod fake;
#[cfg(test)]
mod held_pool;
pub mod readiness;
#[cfg(test)]
mod scratch_volume;
pub mod windows_backend;

pub use delivery::{DeliveryEvent, DeliveryHandle, DeliverySetupError, EntryDelivery};
pub use dioring::{
    DeliveryMode, Dioring, FromClock, InterruptStamps, Setup, SetupError, SetupRefusal,
    TargetSetup, TimeBase, WinInstant, WindowsIds,
};
pub use error_code::error_code_of;
pub use readiness::{ReadinessFailure, check_readiness};

// The portable crate's API, so a consumer needs no direct dependency on it to use this driver.
pub use durable_ioring::*;
// The types the driver's signatures name from the crates it is built on, so a consumer needs no
// direct dependency on them to call it.
pub use win_shared_os_owned_handle::SharedHandle;
pub use win_sync_sys::Event;
pub use win_time_sys::{InterruptTime, TimePoint};
