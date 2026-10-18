// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod named_pipe;

pub use named_pipe::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};
