# Resolved test failures: windows-ioring-sys

Append-only. Entries arrive here from [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md)
in the same commit that removes them from it.

## Resolved 2026-08-28 21:03:52 -04:00 -- buffer registration completes with ERROR_NOACCESS

**Tests:** four in [tests/registration.rs](tests/registration.rs), all failing on the same call:

- `a_read_addressing_a_registered_file_and_a_registered_buffer_round_trips`
- `a_registered_buffers_from_a_different_ring_is_rejected`
- `a_second_file_or_buffer_registration_on_the_same_ring_is_refused`
- `dropping_a_registration_with_an_operation_in_flight_leaks_rather_than_frees`

**Root cause:** a live use-after-free in shipped 0.1.2, not a test defect.
`BuildIoRingRegisterBuffers` reads its `IORING_BUFFER_INFO` array when the registration op *runs*
-- during a later `SubmitIoRing` -- not when the `Build*` call returns. `Batch::register_buffers`
built that array in a local `Vec` and dropped it before submitting, so the kernel read freed heap
and the registration completed with `HRESULT_FROM_WIN32(ERROR_NOACCESS)` (`0x800703E6`).

Because `register_buffers` is a **safe** `pub fn`, safe code could cause the kernel to dereference
a dangling pointer. The observed symptom was benign (a reported error), but a reallocation landing
on those bytes first would have registered whatever addresses happened to be there.

**How it was established:** a spike (`.scratch/ioring-bufreg-spike`) crossed the two hypotheses that
produce this error code -- array lifetime and buffer alignment -- rather than testing either alone:

| | align 1 | page-aligned |
|---|---|---|
| infos dropped before submit | `ERROR_NOACCESS` | `ERROR_NOACCESS` |
| infos alive through submit | `S_OK` | `S_OK` |

Alignment was the initial suspicion and is **disproved** in both directions: an align-1 `Vec<u8>`
succeeds when the array is alive, and a page-aligned buffer still fails when it is dropped. Two
further measurements shaped the fix: the array may be released once `SubmitIoRing` returns (so the
requirement is "alive until submit", not "until the completion is observed"), and
`BuildIoRingRegisterFileHandles` genuinely *does* read its `handles` array synchronously -- which is
why the file-registration tests always passed, and why the crate's rustdoc had generalized the
synchronous claim from one registration to the other.

**Fix:** the `IORING_BUFFER_INFO` array is now owned by the `IoRing` for its remaining life, and the
SQE is built from that pointer rather than from a local about to go out of scope. Held by the ring
rather than the `Batch` because a failed submit leaves the SQE queued as ring state (D-5), so a
later unrelated submit can be what finally runs it. See
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> [D-32](DESIGN-NOTES.md#d-32).

**Regression cover:** `a_buffer_registration_survives_heap_churn_between_the_push_and_the_submit`
allocates hard between the push and the submit, so a future regression finds its freed bytes reused
rather than conveniently intact. Verified by sabotage: reverting the fix makes that test fail with
the original `0x800703E6`, and restoring it makes it pass.

## Resolved 2026-09-06 16:19:04 -04:00 -- the "flaky" flush-barrier test was asserting a false claim

**Test:** `flush_barrier::a_covering_flush_waits_for_preceding_writes_and_an_unordered_one_does_not`
in [tests/flush_barrier.rs](tests/flush_barrier.rs).

**What it was recorded as.** Flaky under a full-workspace run, green in isolation. Seen on
2026-09-03 (2899 of 2900 passed) and again on 2026-09-06 (978 of 979). Both times the test passed
alone and the observing change had not touched it, so it was filed as load sensitivity in a real-I/O
test, with the open question being whether to make the assertion load-independent or mark the test
serial.

**What it actually was.** Neither. The test asserted two guarantees and the second one is false.
`IOSQE_FLAGS_DRAIN_PRECEDING_OPS` drains operations queued *before* it -- solidly, in every one of
about 4,500 trials -- but does **not** hold back operations queued after it, which
[DESIGN-NOTES.md](DESIGN-NOTES.md) `D-24` claimed and `D-47` now corrects. Post-flush writes overtake
the flush at 0.03%-0.8%, and in the worst trial all 32 did.

**How it was settled.** Purpose-built stress instruments that repeat the sequence under quiet,
contended, and concurrent-ring conditions, keeping a per-trial event log -- submissions,
completion-queue drain rounds, and every pop with its phase and timing -- so a violation could be
read after the fact instead of leaving only a counter. Those instruments are deliberately **not** in
the change that carries this entry: correcting shipped documentation should not wait on reviewing a
new test harness, so they follow separately. Three findings came out of them:

- **Contention is not the cause.** The idle run failed. That was the leading hypothesis and it was
  wrong.
- **Ring depth is not the cause.** 128, 256 and 512 were indistinguishable.
- **The violation is real, not an observation artifact.** Every one was confined to a single drain of
  the completion queue, so it is the order the kernel posted, not the order we happened to sample.

**Resolution.** The false assertion is removed; the counter behind it is kept and reported, so the
rate stays visible and a platform that later held the line would show up as a run of zeros. The
documentation that stated the guarantee -- `D-24`, the prose under "Durability on the ring", and the
rustdoc on `FlushCoverage::CoversPrecedingOperations` -- is corrected rather than quietly dropped,
since a caller may have relied on it.

**Worth keeping from this.** A rare failure in a test of a *platform guarantee* should be
characterised before it is stabilised. The two natural repairs -- loosen the assertion, or mark the
test serial -- would both have suppressed the only evidence that shipped documentation was wrong,
and "flaky test" and "the platform does not do what we wrote down" produce the same symptom.
