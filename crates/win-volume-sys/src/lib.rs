// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod device_control;
mod extents;
mod geometry;
mod lock;
#[cfg(test)]
mod test_support;

pub use extents::{DiskExtent, volume_disk_extents};
pub use geometry::bytes_per_sector;
pub use lock::{lock_volume, unlock_volume};
