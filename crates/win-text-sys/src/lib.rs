// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod compare;

pub use compare::{Case, CompareStringOrdinalError, Operand, compare_string_ordinal};
