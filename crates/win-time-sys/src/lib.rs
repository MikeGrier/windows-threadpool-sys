// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]
// Every `unsafe` here is a Win32 call, and each states why it is sound (WT-1.1).
#![deny(clippy::undocumented_unsafe_blocks)]
