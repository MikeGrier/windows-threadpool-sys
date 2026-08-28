// Copyright (c) Mike Grier.

//! Executable probes for the undocumented Windows behaviour this workspace's
//! designs rest on.
//!
//! Several decisions in this repository are justified by measurements of
//! behaviour Microsoft does not document, or documents differently from how it
//! behaves. A measurement recorded only in prose decays silently: the claim
//! stays in the design note while the platform, or our reading of it, moves.
//! These probes exist so a claim can be re-run rather than re-argued.
//!
//! # Each probe is a function, not a program
//!
//! A probe's logic lives in a library function that *returns* its observation.
//! The binaries print it and the tests assert it, so the fact has exactly one
//! implementation. Writing the check twice -- once to print, once to assert --
//! would make the test a check of the copy rather than of the platform.
//!
//! # Three tiers
//!
//! Not every probe can run in an ordinary test pass, so each belongs to exactly
//! one tier:
//!
//! - **Asserted.** Fast, deterministic, and free of side effects that outlive
//!   the call. These are real `#[test]`s, so a platform change that invalidates
//!   the claim fails the build instead of quietly falsifying a design note.
//! - **Ignored.** Correct to assert but too slow, too resource-hungry, or
//!   dependent on a specific environment. Marked `#[ignore]` with the reason and
//!   the cost stated, and run deliberately.
//! - **Binary only.** Cannot be a test at all -- it hangs by design, mutates
//!   process-wide state irreversibly, or needs privileges or fixtures a test run
//!   must not assume.
//!
//! Every tier is **compiled** by an ordinary workspace build. That is the floor:
//! a probe that no longer compiles is a probe that has already rotted.
//!
//! # "Cannot measure" is not "the answer is no"
//!
//! Several probes need something the host may not have -- an `IoRing`, a free
//! drive letter. Each reports that it could not run rather than returning a
//! negative, because conflating the two is how a design note ends up citing a
//! measurement that never happened. An ignored test that cannot set up its
//! fixture returns early; one whose fixture is set up but cannot exhibit the
//! behaviour **fails**.
//!
//! # Running them
//!
//! ```text
//! cargo test -p windows-platform-probes                      # asserted tier
//! cargo test -p windows-platform-probes -- --ignored          # + ignored tier
//! cargo run  -p windows-platform-probes --bin probe-cancel-io # binary only
//! ```
//!
//! The binaries print numbers; the tests assert shape. That split is
//! deliberate: host-specific magnitudes belong where a human reads them, and
//! only the invariant belongs where a build can fail on it.
//!
//! # What each probe establishes
//!
//! | Probe | Tier | Claim it supports |
//! |---|---|---|
//! | [`error_mode::probe_bit`] | asserted | which `SEM_` bits `SetThreadErrorMode` accepts |
//! | [`error_mode::combined_invalid_installs_nothing`] | asserted | an invalid bit fails the whole call, installing none of the valid ones |
//! | [`error_mode::thread_mode_independent_of_process`] | asserted | the thread error mode is independent storage, not a view of the process mode |
//! | [`error_mode::alignment_bit_is_sticky_at_process_scope`] | binary only | the alignment bit cannot be cleared once set -- irreversible, so never asserted in-process |
//! | [`handle_state::duplicate_shares_cursor`] | asserted | `DuplicateHandle` shares directory-enumeration state |
//! | [`handle_state::separate_opens_are_independent`] | asserted | the control that makes the above meaningful |
//! | [`handle_state::closing_duplicate_preserves_source`] | asserted | a request may own a duplicate and drop it without damaging its caller's handle |
//! | [`handle_state::query_disturbs_cursor`] | asserted | single-shot metadata queries do not disturb an enumeration in progress |
//! | [`worker_context::observe_on_worker`] | asserted | a pool worker starts with no impersonation token and the critical-error handler enabled |
//! | [`worker_context::observe_on_worker_while_impersonating`] | asserted | a worker does not inherit an impersonating submitter's token |
//! | [`pool_growth::measure_growth`] | ignored | a blocked pool grows to its maximum, and growth throttles after an initial burst |
//! | [`pool_growth::measure_raise_while_saturated`] | ignored | raising the maximum while saturated promptly releases more work |
//! | [`device_map::measure_with_subst`] | ignored | impersonation changes which DOS device map a drive letter resolves in |
//! | [`ioring::measure_registration`] | ignored | `BuildIoRingRegisterFileHandles` replaces the file table rather than appending |
//! | [`ioring::measure_thread_agnosticism`] | ignored | an `IoRing` operation outlives the thread that submitted it |
//! | [`cancel_io::cancel_against_idle_thread`] | binary only | `CancelSynchronousIo` is point-in-time against an idle thread |
//! | [`cancel_io::cancel_against_busy_thread`] | binary only | it can block indefinitely against a thread re-entering synchronous I/O |

#![cfg(windows)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod cancel_io;
pub mod device_map;
pub mod error_mode;
pub mod handle_state;
pub mod ioring;
pub mod pool_growth;
pub mod worker_context;

#[cfg(test)]
mod tests;
