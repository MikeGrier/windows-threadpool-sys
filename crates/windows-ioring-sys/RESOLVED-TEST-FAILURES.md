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
