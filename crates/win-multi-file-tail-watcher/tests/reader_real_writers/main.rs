// Copyright (c) 2026 Mike Grier
//! Integration tests of the public `reader` layer over files that child processes write (TW-9.5).
//!
//! Only public items are used. The children are `cmd.exe` writing to files the parent opened and
//! handed over as their stdout and stderr, which is how the parent of a worker gives it its output.

#![cfg(windows)]

mod azure_shape;
mod harness;
mod held_chunks;
mod option_sets;
mod refusals;
mod runner_log;
