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
//! **Always collected:** the shape of the machine (logical processors, cores,
//! cache domains, efficiency classes, NUMA nodes) and the timings this tool
//! measures. That is the measurement, so it is not redactable.
//!
//! **Collected only with `--include-metadata`:** the CPU model, the OS build, a
//! hint about whether virtualisation was detected, and the minute the run
//! finished. These are *context* rather than measurement, so they are
//! **withheld by default** -- see
//! [`redaction::MetadataPolicy`]. A field the
//! policy withholds is not read at all, rather than read and then dropped.
//!
//! **Never collected:** host name, user name, file paths, environment
//! variables, serial numbers, or anything about installed software. That list
//! is a commitment rather than a description of the current implementation.
//!
//! **The tool makes no network connections.** It writes a file; sending it is
//! your decision and your action. The record is text, so you can read it before
//! deciding.
//!
//! **If the hardware is confidential, do not send the record.** Redaction
//! reduces incidental leakage and nothing more: the topology *is* the
//! measurement, and an unreleased part is identified by its shape at least as
//! well as by its name. No switch fixes that, and pretending otherwise would be
//! worse than saying so.
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
/// JSON laid out to be read in a terminal rather than by a machine.
///
/// Gated, because it is serialization and nothing else: it parses and re-lays
/// out `serde_json` values and cannot be built without `serde` in scope. The
/// `serde` feature exists so the *measurement* code can be used without it, and
/// leaving this module ungated made `--no-default-features` -- a configuration
/// this crate's manifest advertises -- fail to compile outright.
#[cfg(feature = "serde")]
pub mod paste_json;
/// The handoff itself, and the strategies the placement experiment compares.
pub mod peer_index_cache;
/// The record a run produces and a runner sends back.
pub mod record;
/// Which secondary metadata a submission carries.
pub mod redaction;
/// The human-readable report, rendered from the record.
pub mod report;
/// Turning a run into something a person can paste into a discussion thread.
///
/// Gated for the same reason as [`paste_json`]: the paste *is* the serialized
/// record wrapped in fences and a checksum, so there is nothing here that can
/// exist without a serializer. Measurement, the fingerprint, and the
/// human-readable report all remain available without the feature.
#[cfg(feature = "serde")]
pub mod submission;
