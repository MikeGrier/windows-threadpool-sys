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
//! - **Every descendant is in the job.** The command is created as a member of
//!   the job, named in `PROC_THREAD_ATTRIBUTE_JOB_LIST`, so there is no moment
//!   at which it, or anything it starts, exists outside the job -- including a
//!   moment in which the launcher could be killed.
//! - **The bound kills the tree.** At the bound the whole job is terminated in
//!   one call, and the launcher waits a further [`launch::CONFIRM_BOUND`] for
//!   the job to empty before reporting.
//! - **Nothing outlives the launcher.** The job is kill-on-close, so if the
//!   launcher itself is killed, closing its job handle kills the tree.
//! - **Nothing outlives the command either.** Processes still in the job when
//!   the command exits are terminated, and counted in the result as strays.
//! - **Cleanup is never assumed.** Both the `exited` and the `timed-out` outcome
//!   say whether the job was seen empty afterwards, and an unconfirmed cleanup is
//!   a result a caller must not score as the command's own.
//! - **One result, written whole.** The outcome is written to the result file as
//!   one line of JSON, through a temporary file and a rename, so a reader sees
//!   the complete result or none. See [`outcome`] for the format.
//!
//! The command's stdin is null; its stdout and stderr go to the files named on
//! the command line, and they are the only handles it inherits. The launcher
//! creates the process itself, because the standard library cannot name a job at
//! creation on the pinned toolchain; [`process`] says how it finds the program,
//! quotes the arguments, and runs `.cmd` and `.bat` files -- and which arguments
//! it refuses for those.
#![cfg(windows)]
#![warn(missing_docs)]

pub mod cli;
pub mod job;
pub mod launch;
pub mod narrate;
pub mod outcome;
pub mod process;
