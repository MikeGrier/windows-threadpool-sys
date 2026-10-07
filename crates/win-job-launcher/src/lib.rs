// Copyright (c) 2026 Mike Grier
//! Runs one command inside a fresh Windows job object under a wall-clock bound,
//! and kills the whole process tree at the bound.
//!
//! Written for this repository's sabotage harness,
//! [run-sabotage.ps1](../../tools/run-sabotage.ps1), which must kill a hung test
//! run reliably and promptly. A parent-PID walk cannot do that: it is a query
//! with no bound of its own, and it cannot find a descendant whose parent has
//! already exited. A job object answers both, because membership is decided by
//! the kernel when a process is created rather than discovered afterwards.
//!
//! # What the launcher guarantees
//!
//! - **Every descendant is in the job.** The command is created suspended,
//!   assigned to the job, and only then resumed, so nothing it starts can be
//!   created outside the job.
//! - **The bound kills the tree.** At the bound the whole job is terminated in
//!   one call, and the launcher waits a further [`launch::CONFIRM_BOUND`] for
//!   the job to empty before reporting.
//! - **Nothing outlives the launcher.** The job is kill-on-close, so if the
//!   launcher itself is killed, closing its job handle kills the tree.
//! - **Nothing outlives the command either.** Processes still in the job when
//!   the command exits are terminated, and counted in the result as strays.
//! - **One result, written whole.** The outcome is written to the result file as
//!   one line of JSON, through a temporary file and a rename, so a reader sees
//!   the complete result or none. See [`outcome`] for the format.
//!
//! The command's stdin is null; its stdout and stderr go to the files named on
//! the command line. Its arguments are passed through `std::process::Command`,
//! which owns their quoting and the running of `.cmd`/`.bat` files.
#![cfg(windows)]
#![warn(missing_docs)]

pub mod cli;
pub mod job;
pub mod launch;
pub mod narrate;
pub mod outcome;
pub mod suspended;
