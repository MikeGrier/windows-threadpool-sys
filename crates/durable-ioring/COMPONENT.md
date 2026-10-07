# durable-ioring

"dioring" is the engineer's short form of the name, and is used throughout these documents.

## What it is

A ring for file I/O with **durability scheduled into its work**, layered on
[windows-ioring-sys](../windows-ioring-sys/README.md)'s `IoRing`. A consumer submits reads and
writes as it would to the ring, tags each write with an epoch -- an epoch id of its own choosing
within a durability lineage -- asks for durability with `make_durable_through(n)`, and learns -- in the same completion stream as its
operations -- when everything through `n` has become durable, or that it has failed and which
writes are suspect as a result ([DI-D-9](DESIGN-NOTES.md#di-d-9)). Internally the grouping is the epoch construction worked out
in [examples/epoch_log/](../windows-ioring-sys/examples/epoch_log/); the epoch is the mechanism,
durability is the feature, and the name says the feature
([DI-D-2](DESIGN-NOTES.md#di-d-2)).

It is aimed first at storage engines built on a ring -- user-mode filesystems and databases -- and
is designed for two classes of consumer ([DI-D-5](DESIGN-NOTES.md#di-d-5)): those who want
durability without becoming experts in it, and those who already run their own logs and want only
truthful reporting and an exact account of what is at risk.

## What it owns, and what it does not

**Owns** ([DI-D-5](DESIGN-NOTES.md#di-d-5)): durability points and their bookkeeping; a high-water mark
that never reports a point durable while an earlier failure is unresolved; release of an operation
only after a named point is durable; an inventory, by identity, of the writes whose durability a
failure has put in doubt; and a protocol by which the consumer resolves a failure.

**Does not own:** retaining written data until it is durable, rewriting or replaying it after a
failure, record formats, checkpointing, or space reclamation. Retention and automatic rewrite are a
separate layer above this one, queued as its own future crate. Overlapping writes are the
consumer's to order ([DI-D-6](DESIGN-NOTES.md#di-d-6)).

## Where it sits

- **It is the durability layer `windows-ioring-sys` refuses to contain.**
  [D-54](../windows-ioring-sys/DESIGN-NOTES.md#d-54) keeps durability groups out of that crate and
  [D-26](../windows-ioring-sys/DESIGN-NOTES.md#d-26) keeps durability policy out of it; this crate
  is where both live.
- **It is C-3's durability crate, reshaped** ([DI-D-1](DESIGN-NOTES.md#di-d-1)). The 2026-08-30
  session planned it as a layer that contains an execution domain; it is built on `IoRing`
  directly instead, and the item that queued it, `M33+.5` in
  [CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md), has been transferred here.
- **It does not replace the `epoch_log` sample** ([DI-D-7](DESIGN-NOTES.md#di-d-7)). The two
  coexist until this crate is proven, and the merge-or-delete decision is queued.

## Design record

The specification is [CONTRACT.md](CONTRACT.md): what dioring guarantees, does not guarantee,
requires and assumes. Its spelling as Rust types is [API.md](API.md). Decisions are in
[DESIGN-NOTES.md](DESIGN-NOTES.md). The session that produced them is
[DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md).
