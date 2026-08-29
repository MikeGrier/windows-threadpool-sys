# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
`RESOLVED-TEST-FAILURES.md` (append-only) rather than deleting it.

## Recorded 2026-08-28 20:11:32 -04:00 -- buffer registration completes with ERROR_NOACCESS

**Tests:** four in [tests/registration.rs](tests/registration.rs), all failing on the same call:

- `a_read_addressing_a_registered_file_and_a_registered_buffer_round_trips`
- `a_registered_buffers_from_a_different_ring_is_rejected`
- `a_second_file_or_buffer_registration_on_the_same_ring_is_refused`
- `dropping_a_registration_with_an_operation_in_flight_leaks_rather_than_frees`

**Symptom:** `Batch::register_buffers` queues successfully, but the registration's *completion*
reports `IoRingError { code: 0x800703E6 }` -- `HRESULT_FROM_WIN32(ERROR_NOACCESS)`, "invalid access
to memory location". Every one of the four tests panics at its
`.expect("buffer registration succeeded")`, so all four are the same failure observed from four
call sites rather than four independent defects.

**Scope:** buffer registration only. File-handle registration
(`a_registered_file_from_a_different_ring_is_rejected`,
`a_zero_length_registration_does_not_spend_the_ring_s_one_registration`) passes, as does every
unit test and every other integration target.

**Established:** pre-existing and unrelated to M10.1. Confirmed by stashing the M10.1 working tree
and re-running `cargo test -p windows-ioring-sys --test registration` against the clean tree at
`b449f5c`: the same four tests fail with the identical error code.

**Not established:** the root cause. The failure is at the kernel's completion, not at
`BuildIoRingRegisterBuffers`, so the address the kernel rejected was accepted at build time and
refused when the registration op actually ran. Candidate explanations not yet distinguished --
an alignment or page-residency requirement on registered buffers that a plain `Vec<u8>` does not
meet, a ring-version-dependent behavior, or a host-specific regression -- would need a spike to
separate. Host observed on: Windows 10.0.28000.2804.

**Bearing on shipped code:** unknown until the cause is separated. If registered buffers require
placement this crate does not document, that is a contract gap on `Batch::register_buffers` rather
than a test defect, and the tests are correctly reporting it.
