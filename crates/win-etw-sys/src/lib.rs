// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod activity;
mod descriptor;
mod field;
mod guid;
mod registration;
#[cfg(feature = "test-util")]
mod session;

pub use activity::create_activity_id;
pub use descriptor::{EventDescriptor, level};
pub use field::Field;
pub use guid::Guid;
pub use registration::{Registration, WriteError};
#[cfg(feature = "test-util")]
pub use session::{Recorded, Session};
