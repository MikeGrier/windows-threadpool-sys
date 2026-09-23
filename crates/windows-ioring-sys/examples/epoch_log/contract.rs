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
//! - **No bound on what a commit waits for.** The covering flush's barrier
//!   reaches *every operation outstanding on the ring when the flush is
//!   reached* -- not only the records of the epoch being closed. Appends
//!   already accepted into the *next* epoch are therefore often covered
//!   incidentally. That incidental coverage is not a promise and must not be
//!   read as one: a record is durable when **its own** epoch's commit
//!   completes, which is the guarantee above and the only one.
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
//! - **The log's ring carries only the log's operations.** The barrier waits
//!   for everything outstanding on the ring, so a shared ring couples this
//!   log's commit latency to work it knows nothing about. The durability
//!   *guarantee* survives such sharing -- the flush names a file -- but the
//!   cost model does not, and the cost model is why the guarantees above are
//!   worth having. See "The ring bounds the wait; the device bounds the
//!   durability" below.
//!
//! # Why an epoch at all
//!
//! Because the alternative is not available. Durability on this ring costs one
//! flush, the flush must carry a barrier to cover anything, and that flush
//! waits for every operation outstanding on the ring -- so it is a long
//! operation whose completion is the epoch's ordering point. (It does not hold
//! back later work; D-47 corrected that. What makes it expensive is its own
//! latency, not a stall it imposes on everything else.) Paying it per record
//! would put one such wait between every pair of records. Paying it per epoch
//! amortizes one expensive operation over many records -- the group-commit
//! shape every write-ahead log converges on -- and the price is precisely the
//! non-guarantees above.
//!
//! # The ring bounds the wait; the device bounds the durability
//!
//! Two scopes are in play here and they are **not** the same one, which is easy
//! to miss because a single call sets both. `Batch::flush` takes a *file*, and
//! `FlushCoverage::CoversPrecedingOperations` is a flag on the *ring*:
//!
//! - **The barrier is ring-wide.** A drained flush does not execute until every
//!   operation outstanding on the ring when it was reached has **completed**.
//!   D-47 measured that half over roughly 4,500 trials; nothing narrows it to
//!   the operations of one epoch, one file, or one component.
//! - **The flush names one file.** What a syncing flush pushes to stable media
//!   is that file's data, and the device cache behind it.
//!
//! Completion is not durability -- this contract says so above, about a
//! record's own write -- and the distinction is exactly what separates the two
//! scopes. The barrier bounds what a commit **waits for**. The flush bounds
//! what a commit **makes durable**.
//!
//! Two consequences, and only the first is fully known here:
//!
//! - **Cost.** This log's commit latency is a function of whatever else shares
//!   the ring, because the barrier waits for all of it. Somebody else's slow
//!   operation is this log's slow commit. That much follows directly from the
//!   barrier's measured scope.
//! - **Reach, which this program cannot currently determine.** Whether some
//!   *other* file's completed writes are also made durable by this log's commit
//!   depends on whether that file's data sits behind the same device cache this
//!   flush syncs. This log does not know which device backs any handle, so it
//!   cannot answer that -- and it must not assume either answer. `M23.2` is
//!   where that question is asked; until it is, treat a shared ring as giving
//!   you the cost coupling without any durability promise for the other party.
//!
//! So "one ring per log" is a precondition, and the reason is the first bullet
//! rather than the second: a shared ring couples this log's commit latency to
//! unrelated traffic, unconditionally and whatever the storage turns out to be.
//! It is a precondition of the *cost model* the guarantees above are worth
//! having -- not of their correctness, which the flush's own target secures.
//! A consumer whose log spans *two* rings gets no single durability point
//! across both, and needs two commits with an explicit join between them.
//!
//! **This sample honors the precondition, and does so for a second, independent
//! reason.** The checkpoint has its own ring ([`crate::checkpoint`]) because a
//! ring handed to `EventDelivery` is owned by it and cannot also be drained by
//! the log thread -- a *delivery* argument, and the only one stated at that
//! point of use. The structure is therefore right twice over, which is
//! comfortable and is also the hazard: a future change to the delivery model
//! would retire the reason written down over there, and nothing over there
//! mentions this one. The separation is load-bearing for the cost model
//! whatever the delivery model becomes.

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
    /// Every clause, in the order a report should present them.
    ///
    /// Exists so that a caller printing the contract **asks** for the list
    /// rather than restating it. [`crate::main`]'s report previously carried
    /// its own array of the three variants, which would have silently omitted
    /// a fourth: the report would simply have been one section short, with
    /// nothing failing.
    ///
    /// This list is still hand-maintained -- Rust offers no exhaustive
    /// iteration of a plain enum. What brings a new variant to the author's
    /// attention is [`Self::heading`] below, whose `match` is exhaustive and
    /// will not compile until the new variant is handled; this array sits
    /// beside it so the two are edited together.
    pub const ALL: [Self; 3] = [Self::Guarantees, Self::DoesNotGuarantee, Self::Assumes];

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
        clause: Clause::DoesNotGuarantee,
        text: "that a commit waits only for its own epoch -- the covering flush's barrier reaches \
               every operation outstanding on the ring when it is reached, so records already \
               accepted into the next epoch are often covered incidentally, which promises \
               nothing about them",
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
    Statement {
        clause: Clause::Assumes,
        text: "this log's ring carries only this log's operations -- the barrier waits for \
               everything outstanding on the ring, so a shared ring couples this log's commit \
               latency to unrelated work; the durability guarantee survives sharing because the \
               flush names a file, but the cost model does not",
    },
];

#[cfg(test)]
mod tests;
