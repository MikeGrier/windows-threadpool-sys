# Design session 2026-09-02: what other bounded queues do about claim-protocol ABA

Resulted in [D-34](../DESIGN-NOTES.md#d-34), an amendment to
[D-18](../DESIGN-NOTES.md#d-18), and milestone M15 of
[CHECKLIST-ship-topology-and-queues.md](../../../CHECKLIST-ship-topology-and-queues.md).

Prompted by SH-14.1: `reserving_mpsc` can overwrite a live slot after 2^32 pushes, on every
target, because a producer's room decision is made against a separately-read `head` and the
compare-exchange that acts on it covers only the claim word. The question put to this session
was deliberately narrow -- *what does everyone else do* -- rather than *how should we fix it*,
because SH-14.3 had already enumerated four ways out and all four were unsatisfying.

Sources were restricted to permissively licensed implementations (MIT, Apache-2.0, BSD,
public domain) and open-access papers, so that anything found could be cited and reasoned
from. **No code was copied.** What follows is a description of algorithms and a record of
citations; the mechanisms described are ideas, and any implementation this workspace adopts
is written here.

## 1. The exact hazard, restated so the survey has something to match against

1. Load the shared claim word `W`; extract `position`.
2. Decide "there is room" by comparing `position` against a separately-loaded `head`.
3. `compare_exchange(W, W with position + 1)` to claim the slot.
4. On success, write `slot[position % capacity]` and publish.

A producer stalled between (2) and (3) resumes after other producers have driven the position
field through a complete wrap. The word recurs bit-for-bit, the exchange succeeds, and the
write proceeds on a room decision that is now generations stale.

The essential point, and the thing to match implementations against: **the exchange protects
the counter; nothing protects the earlier observation.**

## 2. Rust implementations read directly

All three were read at their push path. All three are the same algorithm.

### crossbeam-queue `ArrayQueue` (MIT OR Apache-2.0)

`crossbeam-rs/crossbeam:crossbeam-queue/src/array_queue.rs`. The file's own header credits
Vyukov's page. `tail` is one `usize` packing `{lap, index}`; each slot carries a `stamp:
AtomicUsize`. The push path:

- load `tail`; split into `index` and `lap`
- load `slot.stamp`
- **require `tail == stamp`**
- `compare_exchange_weak(tail, new_tail)`
- on success, write the value, then `slot.stamp.store(tail + 1, Release)`

There is no re-check of `stamp` after the exchange. The predicate depends on `stamp`, which is
read separately and is not covered by the exchange -- structurally the same window as ours.
What makes it safe in practice is width: `tail` is a whole `usize`, so recurrence needs on the
order of 2^64 pushes on a 64-bit target.

**On a 32-bit target `tail` is 32 bits, and the exposure is the same as ours.** No comment in
the file discusses this bound.

### concurrent-queue (Apache-2.0 OR MIT)

`smol-rs/concurrent-queue:src/bounded.rs`. The same algorithm, derived from crossbeam. Worth
noting for a different reason: it steals a `mark_bit` from the top of the tail word for the
"closed" flag, which narrows the recurrence space by a bit. A precedent for taking bits out of
a position word, and evidence that doing so is not treated as dangerous.

### thingbuf (MIT)

`hawkw/thingbuf:src/lib.rs`. `tail` is one `usize` packing `{gen, closed, idx}`; each slot has
a `state`. The push path requires `state == tail`, then compare-exchanges `tail`. Again no
post-exchange re-validation.

### What none of them do

Searched all three for `overflow`, `wrap around`, `wraparound`, `ABA`, `2^64`, `64-bit`:
**zero matches.** The assumption that the counter cannot recur is load-bearing in all three
and documented in none.

## 3. The literature

### Vyukov's bounded MPMC queue

`1024cores.net/home/lock-free-algorithms/queues/bounded-mpmc-queue` (read via Internet Archive
snapshot 2024-01-12; the live domain is intermittently unreachable). Per-cell `sequence_`
initialised to the cell index; enqueue computes `dif = seq - pos` and compare-exchanges
`enqueue_pos_` when `dif == 0`.

One genuine structural difference from ours worth recording: **Vyukov's enqueue never reads the
consumer's position at all.** The decision is made against the destination cell's sequence,
keyed to the same `pos` the exchange validates. That removes one stale input relative to our
design -- though the cell sequence is still read separately and still not covered by the
exchange.

Vyukov does not discuss ABA, wraparound, or counter width anywhere on the page (verified
negative). He also explicitly disclaims lock-freedom for this queue.

### Morrison and Afek, CRQ / LCRQ

*Fast Concurrent Queues for x86 Processors*, PPoPP '13, pp. 103-112, DOI
`10.1145/2442516.2442527`.

**The paper's own text could not be extracted** (the PDF returned raw compressed streams).
Everything below is from the MIT-licensed reference implementation
`chaoran/fast-wait-free-queue:lcrq.c` and from peer-reviewed secondary description in
Nikolaev's papers. Anyone quoting CRQ's prose must read the PDF first.

From the source: a cell is `{uint64_t val; uint64_t idx;}` updated by a genuine double-width
`CAS2`. `idx` is a full 64-bit monotonically increasing position with bit 63 stolen as an
`unsafe` flag. Enqueue does `t = FAA(&rq->tail, 1)` -- **no compare-exchange on the tail at
all** -- and then validates the *cell*.

Cycle recurrence is not a concern for CRQ because the epoch stored in the cell is the
full-width absolute position, compared with `<=`; there is no narrow modular subfield to recur.

**"Closing" is a livelock escape, not an ABA device.** `close_crq()` sets bit 63 of the tail
when an enqueuer cannot find a usable cell; the producer then allocates a fresh ring.
Corroborated by Nikolaev DISC'19 section 5: CRQ "is not standalone due to its inherent
susceptibility to livelocks ... a slow path is taken, where the current CRQ instance is
'closed'."

### Nikolaev, SCQ -- the citation that matters most

*A Scalable, Portable, and Memory-Efficient Lock-Free FIFO Queue*, DISC 2019, LIPIcs vol. 146,
pp. 28:1-28:16, DOI `10.4230/LIPIcs.DISC.2019.28`. **Open access, CC-BY.** Extended version
arXiv:1908.04511. Reference implementation `rusnikola/lfqueue` (dual BSD-2-Clause / MIT).

Section 3, under the heading "ABA safety", verbatim:

> "The ABA problem is prevented by comparing cycles. As both `Head` and `Tail` are incremented
> sequentially, regardless of queue size, they will not wrap around until after the number of
> operations exceeds the CPU word's largest value, a reasonable assumption made by other
> ABA-safe designs as well."

This appears to be the only explicit, citable statement in this literature of the assumption
everyone relies on. Note exactly what it licenses: **CPU-word width**. A 32-bit subfield of a
64-bit word does not satisfy it. That single sentence is the strongest available statement
that SH-14.1 is a real defect rather than a theoretical curiosity -- we are doing the standard
thing below the width at which the standard argument holds.

The structural mechanism, Fig. 6 line 15:

```
if ( Cycle(Ent) < Cycle(T) and Index(Ent) = () and (IsSafe(Ent) or Load(&Head) <= T) )
      New = { Cycle(T), 1, index };
      if ( !CAS(&Entries[j], Ent, New) ) goto retry
```

Two things to take from it: the **compare-exchange is on the cell, not the counter** (the
counter is advanced by an unconditional fetch-and-add that authorizes nothing), and the reuse
decision plus the write are validated **together**, because both live in the word the exchange
covers. No observation survives across the exchange unvalidated.

Cycle width is `word width - log2(slots) - 1` (the `-1` is the `IsSafe` bit), derived from
`lfring_cas1.h`. With their benchmark's 2^16 slots on 64-bit that is a 47-bit cycle.

**The `threshold` is not an ABA device.** It is `2n - 1` (infinite array) or `3n - 1` (SCQ) and
its stated purpose is livelock-freedom and empty detection: "Livelocks occur when dequeuers
incessantly invalidate slots that enqueuers are about to use." A web summary claimed it was a
2^32 anti-aliasing constant; that is false. Recorded because the misreading is plausible.

### Nikolaev and Ravindran, wCQ

*wCQ: A Fast Wait-Free Queue with Bounded Memory Usage*, SPAA '22, DOI
`10.1145/3490148.3538572`; preprint arXiv:2201.02179.

The passage that matters to this crate is not about ABA at all. On the family our shapes belong
to, section 1:

> "such queues require a thread to reserve a ring buffer slot prior to writing new data. These
> approaches ... are technically blocking since one stalled (e.g., preempted) thread in the
> middle of an operation can adversely affect other threads."

and it names DPDK's ring as a "straight-forward implementation ... erroneously dubbed as
'lock-free'". `reserving_mpsc` is squarely in that family. This is the citation behind
SH-inf.1's note that the crate should not repeat the error by implication.

Also: "wCQ requires double-width CAS, which is nowadays widespread (i.e., x86 and
ARM/AArch64)", with a separate LL/SC construction for architectures lacking it.

## 4. DPDK `rte_ring` -- the closest published twin of our exact protocol

*DPDK Programmer's Guide*, section 6.5.4 "Modulo 32-bit Indexes"; code
`DPDK/dpdk:lib/ring/rte_ring_c11_pvt.h` (BSD-3-Clause). 32-bit indexes; room computed against a
separately loaded counterpart index; compare-exchange to claim. That is our protocol, in
production, at very large scale.

Its published justification, verbatim:

> "we can do subtractions between 2 index values in a modulo-32bit base: that's why the
> overflow of the indexes is not a problem."

**That argument covers modular arithmetic of the difference and nothing else.** It does not
address a producer stalled across a full 2^32 recurrence. A search of `DPDK/dpdk path:lib/ring`
for "ABA" returns zero hits.

So the closest thing to a published defence of our design defends a different property than
the one SH-14.1 attacks. This is the single most useful citation from the session: it shows the
shape is mainstream, shows the standard justification for it is insufficient for our hazard,
and shows nobody has written the gap down.

Incidentally: DPDK's RTS and HTS modes pair head and tail into a single 64-bit compare-exchange.
That is a double-width fix in effect, but it is motivated by lock-waiter preemption, not ABA.

## 5. Double-width compare-and-swap on this workspace's targets

Checked against the pinned toolchain rather than against documentation, because the
documentation and D-18 disagreed. `rustc 1.98.0 --print cfg --target ...`:

| target | `cmpxchg16b` feature | `target_has_atomic="128"` |
|---|---|---|
| `x86_64-pc-windows-msvc` | **set by default** | yes |
| `aarch64-pc-windows-msvc` | n/a (`ldxp`/`stxp` is ARMv8-A baseline) | yes |
| `i686-pc-windows-msvc` | n/a | **no** |

And `core::sync::atomic::AtomicU128` was test-compiled: **still unstable**
(rust-lang/rust#99069), so reaching a 128-bit exchange from stable means a dependency such as
`taiki-e/portable-atomic` (Apache-2.0 OR MIT), whose own table records `cmpxchg16b` as "enabled
by default on Apple, Windows (except Windows 7, since Rust 1.78)".

Consequences for [D-18](../DESIGN-NOTES.md#d-18), which refuses the 128-bit exchange:

- "It is not in the x86-64 baseline ... does not enable the target feature by default" is
  **false** on 1.98 for our target. No floor to raise, no runtime detection to pay.
- "There is no usable `AtomicU128`" is **true and verified**. The dependency cost stands.
- The fact D-18 never had, and the decisive one: **`i686` has no 128-bit atomic at all**, so a
  128-bit claim word is not "widen the word" but "widen the word *and* drop 32-bit support" --
  which collapses SH-14.3's option 1 into its option 4, an engineer's decision under the
  platform-integrity rule.
- And the premise: D-18 says the exchange "would lift the 2^31 cap and nothing else", written
  before SH-14.1 existed. It would also collapse the recurrence.

## 6. What the survey concluded

**Every design surveyed is safe for exactly one of two reasons**, and it is worth being blunt
that neither is "the protocol is careful":

- **By width** -- Vyukov, crossbeam, concurrent-queue, thingbuf, SCQ's `Head`/`Tail`. The
  counter is a whole machine word, so recurrence is unreachable. This is what Nikolaev states
  explicitly and what the others rely on silently.
- **By structure** -- CRQ and SCQ. The counter is a fetch-and-add authorizing nothing, and the
  authorizing compare-exchange is on the cell, where the decision and the write are validated
  together.

Ours is safe for neither reason. The position is a 32-bit subfield, not a machine word, and the
authorizing exchange does not cover `head`.

The principle, stated once so it need not be re-derived -- **this is our inference, and no
source phrases it this way**, though SCQ's Fig. 6 and CRQ's `CAS2` are both instances:

> The atomic operation that authorizes the write must cover everything the decision depended
> on. Where it does not, correctness rests entirely on the counter being too wide to recur.

That is what makes the central-permit shape (M15 arm A) worth prototyping: admission becomes a
single atomic on one counter, so the predicate is a function solely of the word being modified,
and the position degrades to a ticket with no predicate at all.

## 7. Gaps -- do not cite these without checking

- **The LCRQ paper's own text.** Not extracted; all CRQ claims here are from MIT-licensed
  reference source plus Nikolaev's secondary description.
- **Michael, *ABA Prevention Using Single-Word Instructions*, IBM RC23089 (2004).** Existence
  well attested, full text not retrieved. It is the usual citation for tag-width reasoning.
- **Herlihy and Shavit, *The Art of Multiprocessor Programming*.** Not consulted. Whether it
  treats bounded-counter ABA is unknown; section 10.6 and the `AtomicStampedReference` material
  are the places to look.
- **wCQ section 5 (Correctness).** Only sections 1-3 were read; if wCQ restates a counter-width
  assumption formally it would be there.
- **No published counter-argument was found** demonstrating a 64-bit monotonic counter being
  wrapped in practice, and no paper states a stall-duration-versus-wrap-rate inequality.
