// Copyright (c) 2026 Mike Grier
//! Measures what thread placement costs on this machine.
//!
//! # What this answers
//!
//! Where two communicating threads run changes how fast they can hand work to
//! each other, and by more than most optimisations are worth: on one machine
//! measured here, moving a producer/consumer pair from one locality domain to
//! another cost **5.6x** on the same code. This tool measures that on *your*
//! machine and writes one record you can send back.
//!
//! # Why your machine is interesting
//!
//! The designs this measurement informs are shared, and the hosts available to
//! the author are not. Every machine this has run on so far presents a **single
//! NUMA node**, so the cost of crossing between nodes -- the thing a
//! multi-socket server does constantly -- is entirely unmeasured, and no amount
//! of work on a one-node machine will change that.
//!
//! The two hosts measured to date also turn out to express **disjoint** sets of
//! placements: neither can produce a single row the other can. So this is not a
//! matter of collecting more of the same, and a result from a machine unlike
//! either is worth more than a hundred repetitions here.
//!
//! # What it collects, and what it does not
//!
//! **Collected:** the shape of the machine (logical processors, cores, cache
//! domains, efficiency classes, NUMA nodes), the CPU model, the OS build, a
//! hint about whether virtualisation was detected, and the timings this tool
//! measures.
//!
//! **Not collected:** host name, user name, file paths, environment variables,
//! serial numbers, or anything about installed software. That list is a
//! commitment rather than a description of the current implementation.
//!
//! **The tool makes no network connections.** It writes a file; sending it is
//! your decision and your action. The record is text, so you can read it before
//! deciding -- and if you would rather not share the CPU model, there is a
//! switch for that.
//!
//! **If the hardware is confidential, do not send the record.** The model name
//! can be suppressed, but the topology *is* the measurement, and an unreleased
//! part is identified by its shape at least as well as by its name. No switch
//! fixes that, and pretending otherwise would be worse than saying so.
//!
//! # An instrument, not a library
//!
//! This crate exists to produce measurements, not to be built on. It is not a
//! placement policy, it does not choose where your threads should run, and
//! nothing here is tuned for use in a running system.

#![cfg(windows)]

/// Which build produced a measurement.
pub mod build_identity;
/// What a handoff costs, by where the two threads run.
pub mod core_affinity;
/// What shape the machine is, and which slice a measurement ran on.
pub mod fingerprint;
/// What machine this was, beyond its measurable shape.
pub mod machine;
/// The handoff itself, and the strategies the placement experiment compares.
pub mod peer_index_cache;
/// The record a run produces and a runner sends back.
pub mod record;
/// The human-readable report, rendered from the record.
pub mod report;
/// Turning a run into something a person can paste into a discussion thread.
pub mod submission;
