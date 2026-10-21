// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
// This crate has no unsafe code: the calls are `win-etw-sys`'.
#![forbid(unsafe_code)]

mod encode;
mod manifest;
mod schema;
mod sink;

pub use encode::{Value, encode, schema_of};
pub use manifest::{MANIFEST_FILE, manifest};
pub use schema::{
    Column, ColumnType, EVENTS, EventSchema, FLUSH_TASK, FLUSHES, INCONSISTENCIES, KEYWORDS,
    Keyword, LOCK_WAIT_TASK, LOCKS, PROVIDER_ID, PROVIDER_NAME, SEAL_TASK, SEALS, TASKS, Task,
    VERSION, id, opcode, schema_of_id, schema_of_kind,
};
pub use sink::EtwDiagnostics;
pub use win_etw_sys::{EventDescriptor, Guid};
