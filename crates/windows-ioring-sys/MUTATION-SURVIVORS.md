# Mutation survivors: windows-ioring-sys (M18.3, resolved in M18.4)

`cargo-mutants`, run over the crate with `--all-features`. M18.3 produced the
triage below; M18.4 resolved it, and the outcome per category is recorded
against each heading.

```
M18.3   306 mutants: 182 caught, 48 missed,  7 timeouts, 69 unviable   79.7%
M18.4   306 mutants: 219 caught, 12 missed,  6 timeouts, 69 unviable   94.9%
M18.7   307 mutants: 221 caught, 10 missed,  6 timeouts, 70 unviable   95.8%
```

Counting timeouts as detections (see T1). **38 survivors killed**; the ten that
remain are itemised in "What is left, and why" at the end -- two of them
provably unkillable, the rest with a named reason rather than a shrug. M18.7
adds a mutant because it split a function out; the extra unviable one is on it.

**The result is not perfectly reproducible, and the number should be read with
that in mind.** Two mutants -- `Batch::require -> Ok(())` and `IoRing`'s `Debug`
-- were reported caught in one run and missed in another with identical sources.
This suite contains timing-dependent tests, and `cargo-mutants` runs four jobs
in parallel, so a mutant can be "caught" by a test that happened to flake.
Treat a single run's score as approximate, and a mutant that moves between runs
as unresolved rather than as fixed.

**Why this was worth running.** M18.3 was justified by a measured rate rather
than a hunch: this branch had already produced two vacuously-passing tests and
one `debug_assert` that could never fire, all found by eye. Mutation testing is
the general form of that search. It found a third vacuous pair (T2) that four
rounds of review had read past.

## T1 -- Detected as a hang (7)

Not survivors in any meaningful sense. Each of these breaks the ring so
thoroughly that the suite stops making progress and hits the 180 s cap, which is
a failure signal in CI exactly as an assertion is.

```
batch.rs:1931  do_submit -> Ok(0) / Ok(1)
batch.rs:1956  <impl Drop for Batch>::drop -> ()  /  delete !
ring.rs:902    drain_for_rundown -> Ok(())
ring.rs:935    try_pop -> Ok(None)
ring.rs:942    == -> != in try_pop
```

Listed rather than dropped: a future run that reports one of these as *missed*
instead of *timeout* would mean a test stopped waiting for work it used to wait
for, which is worth noticing.

## T2 -- A vacuous test, found (2)

**The finding.** Both `Arc<[u8]>` tests in `src/buf/tests.rs` compare
`stable_ptr()` against *another call to the same function*:

```rust
assert_eq!(buffer.stable_ptr(), clone.stable_ptr());   // null == null
assert_eq!(moved.stable_ptr(), before);                // null == null
```

So `stable_ptr -> Default::default()` -- a **null pointer**, which is what this
crate hands the kernel as a buffer address -- passes both. The tests assert
self-consistency and never that the address is the real one.

The neighbouring `a_static_slice_is_readable` shows the fix already: it compares
against `DATA.as_ptr()`, an independently obtained address, and would catch
this. The `Arc` tests need the same.

```
buf.rs:136     <impl IoBuf for Arc<[u8]>>::stable_ptr -> Default::default()
```

## T3 -- Untested predicate, bound, or guard (23)

Real gaps: each is a decision the code makes that no test observes being made
either way.

**Bounds and index arithmetic.** Nothing exercises a registered index out of
range, or a non-zero base:

```
batch.rs:512   < -> <= in RegisteredFiles::get       (hands out one past the end)
batch.rs:513   + -> - and + -> * in RegisteredFiles::get
batch.rs:875   < -> <= in RegisteredBuffers::checked_index
```

**Claim identity.** `Token::claim_if`'s mismatch path *is* tested; these two
separate `claim_if` impls are not, so the guard that refuses a foreign
completion is unobserved:

```
batch.rs:546   || -> && in PendingFileRegistration::claim_if
batch.rs:1006  || -> && in PendingBufferRegistration::claim_if
```

**Capability gating.** The whole probe-and-refuse path is unexercised. Note
`completion_event_reports_unsupported_exactly_when_the_capability_is_absent`
cannot force absence on a capable host, so it does not cover this:

```
batch.rs:1080  Batch::require -> Ok(())     (the gate never refuses)
capability.rs:93,94  & -> |, & -> ^, != -> ==   (flag decoding)
ring.rs:131    == -> != in OpSupport::contains
ring.rs:547    IoRing::supports -> true
ring.rs:682    IoRing::supports_raw -> true
ring.rs:92     Op::code -> Default::default()
```

**Submit accounting.** The count `submit` returns is never asserted:

```
batch.rs:1910  Batch::submit -> Ok(0) / Ok(1)
```

**Error classification.** Two condition predicates have no test:

```
error.rs:263   is_completion_queue_too_full -> false
error.rs:268   is_submit_in_progress -> false
```

**Drop guards.** Both of these exist to refuse or to run down, and neither is
observed doing it. `RegisteredBuffers`'s guard is M5.3's, and M17.3 tripped it
by accident -- but no committed test asserts it:

```
batch.rs:923   <impl Drop for RegisteredBuffers>::drop -> ()
ring.rs:963    <impl Drop for IoRing>::drop -> ()
```

**Fault-injection seam.** Off by default, so lightly covered. Both of these
turned out to be **provably equivalent** -- see the T6 section, which is where
they were moved after M18.4 analysed them rather than assumed they were gaps.

## T4 -- Accessors no test ever calls (19)

These survive because nothing reads them, not because an assertion is weak.

**Including, one commit later, every read-only accessor M18.6 just added.**
`RingScope` was given the whole read-only surface of `IoRing` on the stated
principle that a platform layer should not be narrowed to the current caller --
and mutation testing immediately shows that none of it is exercised. That is the
technique catching the author of the previous commit, which is the most useful
thing it could have done here.

```
event_delivery.rs:241  RingScope::outstanding -> 0
event_delivery.rs:262  RingScope::supports -> false / true
event_delivery.rs:268  RingScope::supports_raw -> false / true
event_delivery.rs:274  RingScope::registered_file_count -> 0 / 1
event_delivery.rs:280  RingScope::registered_buffer_count -> 0 / 1
batch.rs:411   RegisteredFile::index -> 0
batch.rs:499   RegisteredFiles::len -> 1
batch.rs:505   RegisteredFiles::is_empty -> true / false, == -> !=
batch.rs:533   PendingFileRegistration::user_data -> 0 / 1
batch.rs:656   RegisteredBuffers::is_empty -> true / false
batch.rs:989   PendingBufferRegistration::user_data -> 1
```

## T5 -- Debug formatting (4)

Nothing asserts any `Debug` output. Lowest value of the five categories, and the
one where "record why the mutant is harmless" is the likely M18.4 answer -- with
one exception worth a look: `Token`'s `Debug` is deliberately hand-written
rather than derived, printing only the id, and that choice is currently
unverified.

```
batch.rs:1050  PendingBufferRegistration::fmt
error.rs:275   IoRingError::fmt
ring.rs:427    IoRing::fmt
token.rs:110   Token::fmt
```

## T6 -- Equivalent mutants (3)

Two of these are equivalent **as a matter of arithmetic and visibility**, not
merely under the current tests. No test can kill them, and writing one that
appeared to would mean asserting something the code does not promise.

**`ring.rs:187  | -> ^ in InjectedFailure::as_hresult`.** The expression is
`0x8007_0000 | (code & 0xFFFF)`. The right operand is masked to the low 16 bits
and the left has no bits below bit 16, so the two bit sets are **disjoint** --
and on disjoint operands `|` and `^` are the same function. The mutant is the
identity.

**`ring.rs:334  delete field 'information' in with_injected_failure`.** The
struct literal ends in `..self`, so deleting the explicit `information: 0`
makes the field inherit the original completion's byte count instead of being
zeroed. That value is unreachable: `information` is private, and the only path
that returns it is `Completion::result`, which returns `Err` first because
`with_injected_failure` set a failing `result_code`. The zeroing is a
correctness-of-modelling choice ("a failed operation transferred nothing"), not
an observable one, and its comment already says so.

**`contract.rs:317  delete match arm Violation::BufferStillInUse in
check_quiescent`** -- equivalent only under the *old* tests, and now killed.
This arm is the **sort key**, not the detection: `check_quiescent` sorts busy
buffers so a failure reads the same way twice. `a_busy_registered_buffer_is_
reported_at_quiescence` had exactly one busy buffer, so ordering was
unobservable and deleting the arm changed nothing. Not a hole in the oracle's
detection; a hole in the test's ability to see the ordering the sort exists to
provide. M18.4 added `busy_registered_buffers_are_reported_in_index_order`,
which registers four buffers out of order with one quiet, so the arm is now
load-bearing.

## Reproducing

`cargo-mutants` is not in the pinned toolchain and does not install cleanly
here. Two notes, both measured:

- `cargo install cargo-mutants --locked` **fails on this host**: the locked
  `winapi` version does not compile for `aarch64-pc-windows-msvc` (285 errors).
  Install without `--locked` so resolution picks a version that does.
- Install from outside the repository, or with `+stable`. Run from inside it,
  where `rust-toolchain.toml` pins 1.98.0 for the crate under test.

```
cargo install cargo-mutants
cargo mutants --package windows-ioring-sys --all-features -j 4 --timeout 180
```

The 180 s cap matters: several mutants hang rather than fail, and the default
derived from a ~5 s baseline is too tight for a suite whose own waits are 5 s.

## What is left, and why (M18.4)

Ten survivors remain after M18.4 and M18.7. The rule is that each is either
killed, or given a reason -- and "hard to test" is a reason to record, not a
synonym for "equivalent". These are separated accordingly.

### Provably unkillable (2)

Both are argued in T6 above: `as_hresult`'s `|` and `^` are the same function on
disjoint bit sets, and `with_injected_failure`'s zeroed `information` is
unreachable through any public path. No test can distinguish either, and one
that appeared to would be asserting something the code does not promise.

### Blocked on the host, not on the tests (3)

```
event_delivery.rs:262 RingScope::supports -> true
ring.rs:547           IoRing::supports -> true
batch.rs:1080         Batch::require -> Ok(())
```

Killing the two `supports -> true` mutants needs an `Op` this host does *not*
support, and every operation this crate names is supported on Windows 11. The
negative direction is covered for `supports_raw`, which accepts an arbitrary
opcode and so can be handed a reserved one -- but `Op` is a closed enum by
design, and widening it to carry an unsupported variant purely to satisfy a
mutant would be inventing API for the benefit of a test. Revisit if the crate
ever runs against an emulated ring with a narrower operation set.

`Batch::require -> Ok(())` is the same fact one layer up: `require` returns
`Ok` exactly when `supports` is true, so on a host where every operation is
supported the mutant is **equivalent in practice**, and only a host with a
narrower operation set could distinguish it. It is filed here rather than under
"provably unkillable" because the equivalence is a property of the *host*, not
of the code -- and it is one of the two mutants that moved between runs, which
is consistent with its one "caught" result having been a flake.

### Genuinely open (1)

```
batch.rs:656          RegisteredBuffers::is_empty -> false
```

`is_empty -> false` needs a registration covering zero buffers. Nothing rejects
`register_buffers(vec![])` in this crate, but whether Win32 accepts a
zero-length registration was not established, so this is left open rather than
guessed at.

### Closed by M18.7: the capability decoding (4)

```
capability.rs:93,94   & -> |, & -> ^, != -> ==  (4 mutants)   [all killed]
```

The flag decoding was inline in `capabilities()`, which reads the real OS
through `QueryIoRingCapabilities`. Nothing can vary `FeatureFlags`, so no test
could reach the decision either way. M18.7 extracted a pure
`decode(&IORING_CAPABILITIES) -> Capabilities`, and the flag combinations that
kill each mutant fall straight out of it: an empty mask (kills `& -> |`, since
`flags | FLAG` is non-zero whatever `flags` holds), a mask carrying exactly one
named flag (kills `& -> ^`, which clears the bit it is testing), and any
positive case at all (kills `!= -> ==`). An unknown-feature-bit case covers the
future Windows this crate has already been surprised by once.

`capability.rs` now reports 12 mutants, 9 caught and 3 unviable -- none missed.

### Debug formatting (3)

```
batch.rs:1050  PendingBufferRegistration::fmt
error.rs:275   IoRingError::fmt
ring.rs:427    IoRing::fmt
```

Nothing asserts any of these renderings. `Token`'s `Debug` mutant *was* killed,
deliberately: its hand-written impl exists to avoid demanding `T: Debug` from a
caller's buffer type, and that design choice was worth pinning. These three
carry no comparable decision, and an assertion on their exact wording would be a
change-detector rather than a test. `IoRing`'s is the second mutant that moved
between runs.
