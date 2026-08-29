// Copyright (c) 2026 Mike Grier
//! The `epoch-log` sample's **own** durability contract (M13.1).
//!
//! This module is written before any of the code that implements it, and it is
//! deliberately phrased as *this program's* specification rather than as a
//! description of what `windows-ioring-sys` or Windows happen to do. That
//! ordering is the repository's Design Autonomy rule: we define our behavior,
//! and then choose mechanisms that can satisfy the definition. If a dependency
//! ever stops satisfying what is written here, the dependency is wrong -- this
//! file does not change to match it.
//!
//! The mechanisms chosen, recorded so the dependency is traceable but never
//! authoritative: a covering flush (`Batch::flush` with
//! `FlushCoverage::CoversPrecedingOperations` and a syncing `FlushMode`) is
//! what makes an epoch commit real, and the ring's completion event is how
//! this program learns the commit finished. They were picked because they meet
//! the specification below, not the other way round.
//!
//! # The guarantee, in one sentence
//!
//! **A record is durable when the commit of the epoch containing it has
//! completed.**
//!
//! Unpacking every word of that, because each one is load-bearing:
//!
//! - *record* -- one append handed to this log. Its bytes, its sequence
//!   number, and its checksum are one unit.
//! - *the epoch containing it* -- the epoch that was open at the moment the
//!   append was accepted. Which epoch that is, is decided then and never
//!   changes.
//! - *the commit* -- the single covering flush this program pushes to close
//!   that epoch. One flush per epoch, not one per record.
//! - *has completed* -- this program has observed that flush's completion and
//!   it reported success. Not "was submitted", not "was accepted by the
//!   kernel", and not "the record's own write completed".
//!
//! Two consequences follow that a caller can rely on:
//!
//! - **Durability is monotonic in the epoch number.** If epoch *N* is durable
//!   then every epoch before it is durable too, so a caller only ever needs to
//!   remember the highest committed epoch.
//! - **What is reported durable survives.** A record this program has reported
//!   durable is present and intact when the log is replayed after a crash or
//!   power loss -- which is what the M13.5 replay pass exists to check rather
//!   than assert.
//!
//! # What this contract does *not* guarantee
//!
//! Stated as plainly as the guarantee, because a specification that lists only
//! its promises is the kind that gets read as promising everything.
//!
//! - **No per-record durability.** An append returning tells you the record
//!   was accepted into the open epoch, nothing more. Even that record's *own
//!   write completing* does not make it durable: a write completion means the
//!   kernel took the bytes, not that they have reached non-volatile media.
//!   There is no way to ask for one record to be made durable by itself,
//!   because the underlying ring offers no per-write durability primitive to
//!   build one from -- only the flush.
//! - **No ordering between records within an epoch.** Sequence numbers order
//!   records *logically*, and replay uses them to reconstruct the order. They
//!   say nothing about the order the bytes reach the device, which is
//!   unspecified within an epoch. Ordering across an epoch boundary is
//!   guaranteed; ordering inside one is not.
//! - **No atomicity for a record larger than the device's power-fail atomic
//!   write unit.** A record bigger than that unit can tear across power loss.
//!   The per-record checksum makes a torn record *detectable* at replay; it
//!   does not make it survivable. This program does not query the device for
//!   that unit and does not size records against it -- a real consumer should.
//! - **No guarantee about the tail.** Records appended after the last
//!   committed epoch may be wholly present, wholly absent, or torn, and all
//!   three are legal outcomes of the same crash. A reader must tolerate all
//!   three, which is exactly what the replay pass is written to do.
//!
//! # What this contract assumes
//!
//! - **The device honors the flush.** Everything above rests on the device
//!   actually committing its volatile write cache when the OS asks it to. A
//!   device that lies about that defeats this contract, the operating system's
//!   contract, and every other durability scheme built on the same primitive.
//!   Nothing here can detect it.
//! - **A record is at most one write.** This sample does not split a record
//!   across writes, so it never has to reason about a partially-written record
//!   whose pieces landed in different epochs.
//!
//! # Why an epoch at all
//!
//! Because the alternative is not available. Durability on this ring costs one
//! flush, the flush must carry a barrier to cover anything, and that barrier
//! stalls the whole ring while it runs. Paying that per record would serialize
//! the log completely. Paying it per epoch amortizes one expensive operation
//! over many records -- the group-commit shape every write-ahead log converges
//! on -- and the price is precisely the non-guarantees above.

/// Which part of the contract a statement belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clause {
    /// Something a caller may rely on.
    Guarantees,
    /// Something a caller must not assume, stated so the omission is explicit
    /// rather than left to be inferred from silence.
    DoesNotGuarantee,
    /// Something outside this program's control that the guarantees rest on.
    Assumes,
}

impl Clause {
    /// The heading this clause is printed under.
    pub fn heading(self) -> &'static str {
        match self {
            Self::Guarantees => "guarantees",
            Self::DoesNotGuarantee => "does NOT guarantee",
            Self::Assumes => "assumes",
        }
    }
}

/// One clause of the contract, in the form the program can print and a later
/// verification pass can refer to.
pub struct Statement {
    pub clause: Clause,
    pub text: &'static str,
}

/// The contract above, reduced to the statements that have to hold. The prose
/// in this module's documentation is the full form; this is what the program
/// itself can state at run time, so that a reader who only ever runs the
/// sample still learns what it does and does not promise.
pub const CONTRACT: &[Statement] = &[
    Statement {
        clause: Clause::Guarantees,
        text: "a record is durable when the commit of the epoch containing it has completed -- \
               that is, when this program has observed the epoch's covering flush complete \
               successfully",
    },
    Statement {
        clause: Clause::Guarantees,
        text: "durability is monotonic: if epoch N is durable, every earlier epoch is durable too",
    },
    Statement {
        clause: Clause::Guarantees,
        text: "a record reported durable is present and intact when the log is replayed",
    },
    Statement {
        clause: Clause::DoesNotGuarantee,
        text: "per-record durability -- an append being accepted, or even its own write \
               completing, does not make that record durable",
    },
    Statement {
        clause: Clause::DoesNotGuarantee,
        text: "ordering between records within one epoch -- sequence numbers order them \
               logically, not on the device",
    },
    Statement {
        clause: Clause::DoesNotGuarantee,
        text: "atomicity for a record larger than the device's power-fail atomic write unit -- \
               the checksum makes a torn record detectable, not survivable",
    },
    Statement {
        clause: Clause::DoesNotGuarantee,
        text: "anything about records after the last committed epoch -- they may be present, \
               absent, or torn, and a reader must tolerate all three",
    },
    Statement {
        clause: Clause::Assumes,
        text: "the device honors the flush and commits its volatile write cache; a device that \
               lies defeats this contract and every other built on the same primitive",
    },
    Statement {
        clause: Clause::Assumes,
        text: "a record is written by at most one write, so no record straddles an epoch boundary",
    },
];
