// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod connect;
mod named_pipe;

pub use connect::{
    ConnectOutcome, ConnectStart, connect_named_pipe, connect_named_pipe_overlapped,
    disconnect_named_pipe,
};
pub use named_pipe::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};
