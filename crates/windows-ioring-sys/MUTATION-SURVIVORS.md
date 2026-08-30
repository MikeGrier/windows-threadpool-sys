# Mutation survivors: windows-ioring-sys (M18.3)

`cargo-mutants`, run over the crate with `--all-features`. This is the triage;
resolving the survivors is M18.4.

```
306 mutants tested in 32m: 182 caught, 48 missed, 7 timeouts, 69 unviable
```

Counting timeouts as detections (see T1), that is **189 of 237 viable mutants
caught, 79.7%**.

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

**Fault-injection seam.** Off by default, so lightly covered:

```
ring.rs:187    | -> ^ in InjectedFailure::as_hresult
ring.rs:334    delete field `information` in Completion::with_injected_failure
```

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

## T6 -- Equivalent under the current tests (1)

```
contract.rs:317  delete match arm Violation::BufferStillInUse in check_quiescent
```

This arm is the **sort key**, not the detection -- `check_quiescent` sorts busy
buffers so a failure reads the same way twice. `a_busy_registered_buffer_is_
reported_at_quiescence` has exactly one busy buffer, so ordering is
unobservable and deleting the arm changes nothing. Not a hole in the oracle's
detection; a hole in the test's ability to see the ordering the sort exists to
provide. Two busy buffers would close it.

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
