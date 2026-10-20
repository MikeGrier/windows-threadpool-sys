// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod anonymous_pipe;
mod connect;
mod name;
mod named_pipe;
mod peek;
mod peer;
mod pipe_state;
mod synchronous;
mod transact;
mod wait;

pub use anonymous_pipe::{AnonymousPipe, create_pipe};
pub use connect::{
    ConnectOutcome, ConnectStart, connect_named_pipe, connect_named_pipe_overlapped,
    connect_named_pipe_unchecked, disconnect_named_pipe,
};
pub use named_pipe::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};
pub use peek::{PeekedPipe, peek_named_pipe};
pub use peer::{
    get_named_pipe_client_computer_name, get_named_pipe_client_process_id,
    get_named_pipe_client_session_id, get_named_pipe_server_process_id,
    get_named_pipe_server_session_id,
};
pub use pipe_state::{
    PipeHandleState, PipeInfo, PipeInfoFlags, PipeStateQuery, PipeStateUpdate,
    get_named_pipe_handle_state, get_named_pipe_info, set_named_pipe_handle_state,
};
pub use synchronous::{PipeClientOptions, SynchronousPipe};
pub use transact::{
    TransactStart, Transaction, call_named_pipe, transact_named_pipe,
    transact_named_pipe_overlapped, transact_named_pipe_unchecked,
};
pub use wait::{PipeTimeout, wait_named_pipe};
