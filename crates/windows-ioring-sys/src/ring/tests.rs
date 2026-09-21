// Copyright (c) 2026 Mike Grier
use super::{Completion, InjectedFailure, IoRing, Op, OpSupport};
use crate::IoRingErrorExt;
use crate::capability::{RingVersion, capabilities};

#[test]
fn op_support_starts_empty() {
    assert!(!OpSupport::default().contains(Op::Read));
    assert!(!OpSupport::default().contains(Op::Nop));
}

#[test]
fn a_ring_negotiates_a_version_no_higher_than_the_hosts_maximum() {
    let ring = IoRing::new(64, 128).expect("create ring");
    let caps = capabilities().expect("capabilities");
    assert!(ring.version() <= caps.max_version);
    assert!(ring.version() <= RingVersion::HIGHEST_KNOWN);
}

#[test]
fn a_negotiated_ring_reports_its_version_back_through_get_ring_info() {
    let ring = IoRing::new(64, 128).expect("create ring");
    let info = ring.info().expect("GetIoRingInfo");
    assert_eq!(info.version, ring.version());
}

#[test]
fn every_named_version_the_host_supports_creates_and_closes() {
    let caps = capabilities().expect("capabilities");
    let mut created_at_least_one = false;
    for version in [RingVersion::V1, RingVersion::V2, RingVersion::V3] {
        if version > caps.max_version {
            continue;
        }
        let ring = IoRing::with_version(version, 64, 128).expect("create at a supported version");
        assert_eq!(ring.version(), version);
        drop(ring);
        created_at_least_one = true;
    }
    assert!(
        created_at_least_one,
        "the host should support at least IORING_VERSION_1"
    );
}

#[test]
fn creating_a_ring_above_the_hosts_maximum_version_fails() {
    let caps = capabilities().expect("capabilities");
    let too_high = RingVersion::from_raw(caps.max_version.raw() + 1);
    let error =
        IoRing::with_version(too_high, 64, 128).expect_err("an unsupported version must fail");
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
}

#[test]
fn capability_reporting_never_claims_more_than_is_io_ring_op_supported_reports() {
    let ring = IoRing::new(64, 128).expect("create ring");
    for op in [
        Op::Nop,
        Op::Read,
        Op::Write,
        Op::Flush,
        Op::RegisterFiles,
        Op::RegisterBuffers,
        Op::Cancel,
    ] {
        assert_eq!(
            ring.supports(op),
            ring.supports_raw(op.code()),
            "cached support for {op:?} disagrees with a direct IsIoRingOpSupported call"
        );
    }
}

#[test]
fn supports_reports_exactly_the_capability_set_it_was_given() {
    // `IoRing::supports -> true` survived: every op named in this crate is
    // genuinely supported on any real host these tests run on, so the honest
    // answer and the constant agree everywhere a test could ask a real ring.
    // `set_supported_ops_for_test` constructs the disagreement instead of
    // hoping to find a host that lacks something.
    let mut ring = IoRing::new(8, 8).expect("create ring");
    ring.set_supported_ops_for_test(&[Op::Nop, Op::Read]);

    assert!(ring.supports(Op::Nop));
    assert!(ring.supports(Op::Read));
    assert!(
        !ring.supports(Op::Write),
        "an op left out of the constructed set must read back as unsupported"
    );
    assert!(!ring.supports(Op::Cancel));
}

#[test]
fn nop_read_and_write_are_supported_on_any_real_ring() {
    // A sanity floor: every documented IoRing version supports at least
    // these three. If this ever fails, either the host is exotic enough to
    // need investigating, or the probe itself is broken.
    let ring = IoRing::new(64, 128).expect("create ring");
    assert!(ring.supports(Op::Nop));
    assert!(ring.supports(Op::Read));
    assert!(ring.supports(Op::Write));
}

// --- outstanding-operation accounting and rundown (M2.4) ---

#[test]
fn reserve_user_data_increments_outstanding_and_never_repeats_an_id() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    let a = ring.reserve_user_data().expect("reserve a");
    let b = ring.reserve_user_data().expect("reserve b");
    assert_ne!(a, b);
    assert_eq!(ring.outstanding(), 2);
    ring.record_completion();
    ring.record_completion();
}

#[test]
fn run_down_is_a_no_op_when_nothing_is_outstanding() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    ring.run_down().expect("run_down with nothing outstanding");
    assert_eq!(ring.outstanding(), 0);
}

#[test]
fn run_down_returns_once_a_recorded_completion_zeroes_the_count() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    ring.reserve_user_data().expect("reserve");
    assert_eq!(ring.outstanding(), 1);
    // Recording the completion up front proves run_down rechecks the count
    // rather than always performing at least one wait: it must return
    // without ever calling SubmitIoRing, or this test would hang for
    // RUN_DOWN_POLL_MS waiting on a completion that was never real.
    ring.record_completion();
    ring.run_down()
        .expect("run_down with the count already settled");
    assert_eq!(ring.outstanding(), 0);
}

#[test]
fn record_completion_saturates_rather_than_underflowing() {
    let mut ring = IoRing::new(64, 128).expect("create ring");
    assert_eq!(ring.outstanding(), 0);
    ring.record_completion();
    assert_eq!(
        ring.outstanding(),
        0,
        "recording more completions than were ever reserved must not wrap"
    );
}

#[test]
fn dropping_a_ring_with_nothing_outstanding_does_not_hang() {
    // The ordinary path: no tokens were ever minted, so Drop's run_down must
    // return immediately rather than waiting on SubmitIoRing at all.
    let ring = IoRing::new(64, 128).expect("create ring");
    drop(ring);
}

#[test]
fn dropping_a_ring_actually_runs_its_drop_body() {
    // `<impl Drop for IoRing>::drop -> ()` survived: nothing distinguished a
    // ring that ran rundown-and-close from one that silently leaked its
    // kernel handle, because closing it is invisible to every test that only
    // asks the ring itself. `DROP_RUNS` is incremented as the first line of
    // the real body, so a mutation that replaces the whole body removes the
    // increment along with everything else.
    //
    // The counter is thread-local, and an EXACT count is asserted. An earlier
    // version used a process-wide static and asserted `after > before`, which
    // is not race-free: `cargo test` runs tests as threads in one process, so
    // another test dropping any ring between the two reads satisfies the
    // assertion on its own -- masking precisely the mutant this exists to
    // catch. Only rings dropped on this thread can move a thread-local, and
    // this test drops exactly one.
    let before = super::DROP_RUNS.with(std::cell::Cell::get);
    let ring = IoRing::new(8, 8).expect("create ring");
    drop(ring);
    let after = super::DROP_RUNS.with(std::cell::Cell::get);
    assert_eq!(
        after,
        before + 1,
        "dropping one ring must run its Drop impl exactly once (before={before}, after={after})"
    );
}

// --- The fault-injection seam (M16.3) ---

/// A real completion for a real, finished operation.
///
/// Every test below starts from one of these rather than from
/// `Completion::synthetic`, because the point of the seam is that it transforms
/// something the ring genuinely popped. Starting from a fabrication would test
/// a different thing entirely.
fn real_completion(ring: &mut IoRing) -> (usize, crate::Completion) {
    use crate::{Batch, FlushCoverage, FlushMode};
    use std::os::windows::io::AsRawHandle;

    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-injection-{}-{:?}.tmp",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, b"x").expect("create fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open fixture");

    let mut batch = Batch::new(ring);
    // SAFETY: `file` outlives this call and the drain below.
    let user_data = unsafe {
        batch.flush_raw(
            file.as_raw_handle(),
            FlushCoverage::Unordered,
            FlushMode::Default,
        )
    }
    .expect("queue a flush");
    batch.submit_and_wait(1, 30_000).expect("submit and wait");

    let completion = super::pop_within(ring, "the flush's completion");
    let _ = std::fs::remove_file(&path);
    (user_data, completion)
}

#[test]
fn an_injected_failure_replaces_a_real_success() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (user_data, completion) = real_completion(&mut ring);
    completion
        .result()
        .expect("the flush really did succeed, or this test proves nothing");

    let injected = completion.with_injected_failure(crate::InjectedFailure::Win32(
        windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED,
    ));
    let error = injected.result().expect_err("the injected failure applies");
    assert_eq!(
        (crate::IoRingErrorExt::as_ioring_error(&error)
            .expect("an IoRingError")
            .code() as u32)
            & 0xFFFF,
        windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED,
        "the Win32 code must survive the HRESULT wrapping the kernel would apply"
    );
    assert_eq!(
        injected.user_data(),
        user_data,
        "injection must not disturb the operation's identity"
    );
}

#[test]
fn an_injected_failure_preserves_the_identity_a_token_claims_against() {
    // The safety-critical property, and the reason the seam is worth having:
    // injection transforms a completion for an operation that *genuinely
    // finished*, so claiming against the result stays exactly as sound as
    // claiming against the original -- and must still work, or the seam could
    // not test the claim paths that failure handling lives on.
    //
    // A real, token-carrying read throughout: nothing here is fabricated.
    use crate::{Batch, PushOptions};
    use std::os::windows::io::AsRawHandle;

    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-injection-claim-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"hello").expect("create fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open fixture");

    let mut ring = IoRing::new(16, 16).expect("create ring");
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `file` outlives the operation; the token is claimed below.
    let token =
        unsafe { batch.read_raw(file.as_raw_handle(), vec![0_u8; 5], 0, PushOptions::new()) }
            .expect("queue a read");
    batch.submit_and_wait(1, 30_000).expect("submit and wait");
    let completion = super::pop_within(&mut ring, "the read's completion");
    completion
        .result()
        .expect("the read really did succeed, or this test proves nothing");

    let injected = completion
        .with_injected_failure(crate::InjectedFailure::Ring(crate::RingCondition::Corrupt));
    assert!(injected.result().is_err(), "the injected failure applies");

    let buffer = token
        .claim_if(&injected)
        .expect("a failed completion still claims its own token");
    assert_eq!(
        buffer, b"hello",
        "claiming a failed operation must still hand the buffer back -- that is \
         exactly what stops a failure path from leaking it, which is the defect \
         this seam exists to let a test reach"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_injected_failure_zeroes_the_transferred_byte_count() {
    // The deletion of `information: 0,` from the struct-update survived: with
    // it gone, `..self` supplies the *original* transfer count, so an
    // injected "failure" completion silently keeps reporting real bytes
    // transferred. `Completion::result` cannot show this -- it only returns
    // `information` on success, and this seam only injects failure -- so the
    // field is read directly. This module is `ring.rs`'s own child and can
    // see it, which is exactly what an earlier version of this file's comment
    // (just above) said was impossible.
    use crate::{Batch, PushOptions};
    use std::os::windows::io::AsRawHandle;

    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-injection-information-{}-{:?}.tmp",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, b"hello").expect("create fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open fixture");

    let mut ring = IoRing::new(16, 16).expect("create ring");
    let mut batch = Batch::new(&mut ring);
    // SAFETY: `file` outlives the operation, and the completion is popped
    // below before it is dropped.
    let _token =
        unsafe { batch.read_raw(file.as_raw_handle(), vec![0_u8; 5], 0, PushOptions::new()) }
            .expect("queue a read");
    batch.submit_and_wait(1, 30_000).expect("submit and wait");
    let completion = super::pop_within(&mut ring, "the fixture read's completion");
    assert_eq!(
        completion.information, 5,
        "the fixture must transfer five real bytes, or this test proves nothing"
    );

    let injected = completion
        .with_injected_failure(crate::InjectedFailure::Ring(crate::RingCondition::Corrupt));
    assert_eq!(
        injected.information, 0,
        "an injected failure must report zero transferred, not the real completion's count"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn each_spelling_of_a_failure_produces_the_condition_it_names() {
    // The three variants exist so a call site reads as what it is testing.
    // If they did not agree with the codes they name, that legibility would be
    // a lie.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (_, completion) = real_completion(&mut ring);

    let ring_failure = completion.with_injected_failure(crate::InjectedFailure::Ring(
        crate::RingCondition::SubmissionQueueFull,
    ));
    let error = ring_failure.result().expect_err("fails");
    assert!(
        crate::IoRingErrorExt::is_submission_queue_full(&error),
        "a Ring(..) injection must be recognised by the named predicate"
    );

    let raw = crate::RingCondition::Corrupt.code();
    let hresult_failure = completion.with_injected_failure(crate::InjectedFailure::Hresult(raw));
    assert_eq!(
        crate::IoRingErrorExt::ring_condition(&hresult_failure.result().expect_err("fails")),
        Some(crate::RingCondition::Corrupt),
        "a raw HRESULT injection must resolve to the same condition"
    );
}

#[test]
#[should_panic(expected = "injects failure only, never success")]
fn injecting_a_success_code_is_refused() {
    // Found while writing these tests: `Hresult(0)` would have injected
    // *success*, quietly falsifying this seam's central guarantee and letting
    // a test conceal the very defect it was written to find. The guarantee is
    // now enforced by a panic rather than asserted in prose.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (_, completion) = real_completion(&mut ring);
    let _ = completion.with_injected_failure(crate::InjectedFailure::Hresult(0));
}

#[test]
fn every_named_condition_injects_a_genuine_failure() {
    // The other side of the guarantee: nothing a caller can spell through the
    // two *named* variants is capable of tripping the panic above, so the
    // enforcement constrains only the raw escape hatch.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (_, completion) = real_completion(&mut ring);

    for condition in [
        crate::RingCondition::SubmissionQueueFull,
        crate::RingCondition::Corrupt,
        crate::RingCondition::VersionNotSupported,
    ] {
        assert!(
            completion
                .with_injected_failure(crate::InjectedFailure::Ring(condition))
                .result()
                .is_err(),
            "{condition:?} must inject a genuine failure"
        );
    }
    for code in [1_u32, 5, 87, 0xFFFF] {
        assert!(
            completion
                .with_injected_failure(crate::InjectedFailure::Win32(code))
                .result()
                .is_err(),
            "Win32({code}) must inject a genuine failure"
        );
    }
}

// --- opcode mapping, support probing, and the injection seam (M18.4) ---------

#[test]
fn every_op_maps_to_its_own_win32_opcode() {
    // `Op::code -> Default::default()` survived M18.3: nothing asserted the
    // mapping, only that operations built successfully.
    use windows_sys::Win32::Storage::FileSystem::{
        IORING_OP_CANCEL, IORING_OP_FLUSH, IORING_OP_NOP, IORING_OP_READ,
        IORING_OP_REGISTER_BUFFERS, IORING_OP_REGISTER_FILES, IORING_OP_WRITE,
    };

    assert_eq!(Op::Nop.code(), IORING_OP_NOP);
    assert_eq!(Op::Read.code(), IORING_OP_READ);
    assert_eq!(Op::Write.code(), IORING_OP_WRITE);
    assert_eq!(Op::Flush.code(), IORING_OP_FLUSH);
    assert_eq!(Op::Cancel.code(), IORING_OP_CANCEL);
    assert_eq!(Op::RegisterFiles.code(), IORING_OP_REGISTER_FILES);
    assert_eq!(Op::RegisterBuffers.code(), IORING_OP_REGISTER_BUFFERS);

    // Distinctness matters as much as the values: a mapping that collapsed two
    // operations onto one opcode would still satisfy each equality above if the
    // constants happened to agree.
    let mut codes: Vec<_> = Op::ALL.iter().map(|op| op.code()).collect();
    codes.sort_unstable();
    let before = codes.len();
    codes.dedup();
    assert_eq!(codes.len(), before, "every Op must have a distinct opcode");
}

#[test]
fn op_support_reads_the_bit_belonging_to_the_op_it_was_asked_about() {
    // `OpSupport::contains` finds the op's position in `Op::ALL` and tests that
    // bit. Replacing `==` with `!=` in the search survived M18.3, because every
    // op is supported on a healthy host and every answer was `true` regardless.
    // Constructed masks make the question observable.
    for (index, &op) in Op::ALL.iter().enumerate() {
        let only_this_one = OpSupport(1 << index);
        assert!(
            only_this_one.contains(op),
            "{op:?} must be reported supported when its own bit is set"
        );
        for other in Op::ALL {
            if other != op {
                assert!(
                    !only_this_one.contains(other),
                    "{other:?} must not be reported supported by {op:?}'s bit"
                );
            }
        }
    }

    let none = OpSupport(0);
    for op in Op::ALL {
        assert!(!none.contains(op));
    }
}

#[test]
fn a_reserved_opcode_is_not_supported() {
    // `IoRing::supports_raw -> true` survived because every opcode this crate
    // names is supported here. An opcode Win32 does not define is not.
    let ring = IoRing::new(8, 8).expect("create ring");
    assert!(!ring.supports_raw(0xFFFF));
    assert!(
        ring.supports_raw(Op::Read.code()),
        "the same call must still report a real opcode as supported"
    );
}

#[test]
fn an_injected_failure_carries_the_condition_it_names() {
    // The seam's own arithmetic: `Win32` wraps into an `HRESULT_FROM_WIN32`,
    // and a `Ring` condition keeps its documented code.
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

    let ring = IoRing::new(8, 8).expect("create ring");
    let base = Completion::synthetic(9, 0, ring.ring_id());

    let win32 = base.with_injected_failure(InjectedFailure::Win32(ERROR_ACCESS_DENIED));
    let error = win32.result().expect_err("an injected failure must fail");
    // `check` wraps the HRESULT in a custom error rather than an OS one, so the
    // code comes back through the crate's own accessor.
    assert_eq!(
        error
            .as_ioring_error()
            .expect("an injected failure is an IoRing error")
            .code(),
        0x8007_0005_u32.cast_signed(),
        "Win32(ERROR_ACCESS_DENIED) must become HRESULT_FROM_WIN32 of that code"
    );

    let hresult = base.with_injected_failure(InjectedFailure::Hresult(-2_147_024_882));
    assert_eq!(
        hresult
            .result()
            .expect_err("an injected failure must fail")
            .as_ioring_error()
            .expect("an injected failure is an IoRing error")
            .code(),
        -2_147_024_882,
        "an HRESULT injection must be passed through unchanged"
    );

    // The transformation keeps the operation's identity, which is what makes
    // the seam sound: it rewrites a real completion rather than fabricating one.
    assert_eq!(win32.user_data(), base.user_data());
    assert_eq!(win32.ring_id(), base.ring_id());
}

#[test]
fn the_debug_rendering_names_the_ring_and_its_key_fields() {
    // `<impl Debug for IoRing>::fmt -> Ok(Default::default())` survived: that
    // mutation writes nothing to the formatter at all, so `format!("{ring:?}")`
    // comes back empty. Asserting the type name and a real field value is
    // enough to tell "wrote nothing" from "wrote the real struct".
    let ring = IoRing::new(8, 8).expect("create ring");
    let rendering = format!("{ring:?}");
    assert!(rendering.contains("IoRing"), "got {rendering}");
    assert!(
        rendering.contains("version"),
        "the version field name must appear: {rendering}"
    );
}

// ---------------------------------------------------------------------------
// M21.2: the bounded pop, and the wait it is generic over.
// ---------------------------------------------------------------------------

use super::{CompletionWait, RingWait, SubmitWait};

/// A scratch file to flush against, named per test so tests running as threads
/// in one process cannot collide on it.
fn pop_scratch(tag: &str) -> (std::path::PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-pop-within-{}-{tag}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"x").expect("create fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open fixture");
    (path, file)
}

/// Push `count` flushes and submit them, popping nothing.
fn push_flushes(ring: &mut IoRing, file: &std::fs::File, count: usize) -> Vec<usize> {
    use crate::{Batch, FlushCoverage, FlushMode};
    use std::os::windows::io::AsRawHandle;

    let mut batch = Batch::new(ring);
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        // SAFETY: `file` outlives every operation pushed here -- the caller
        // drains before dropping it.
        ids.push(
            unsafe {
                batch.flush_raw(
                    file.as_raw_handle(),
                    FlushCoverage::Unordered,
                    FlushMode::Default,
                )
            }
            .expect("queue a flush"),
        );
    }
    batch.submit().expect("submit");
    ids
}

/// Records what the pop loop asked of it, and sleeps instead of blocking in
/// the ring.
///
/// Deliberately does **not** call [`RingWait::block`]. These tests drive the
/// loop with a reservation that has no real SQE behind it, and `SubmitIoRing`
/// answers `E_INVALIDARG` when asked to wait for a completion the kernel has
/// no pending operation for. Keeping the wait out of the kernel is what makes
/// the loop's own deadline behaviour testable without a slow real device.
#[derive(Default)]
struct RecordingWait {
    calls: usize,
    last_timeout_ms: u32,
    outstanding_seen: usize,
}

impl CompletionWait for RecordingWait {
    fn wait(&mut self, ring: &mut RingWait<'_>, timeout_ms: u32) -> std::io::Result<()> {
        self.calls += 1;
        self.last_timeout_ms = timeout_ms;
        self.outstanding_seen = ring.outstanding();
        std::thread::sleep(std::time::Duration::from_millis(2));
        Ok(())
    }
}

/// Refuses to wait at all, so a test can observe the error path.
struct FailingWait;

impl CompletionWait for FailingWait {
    fn wait(&mut self, _: &mut RingWait<'_>, _: u32) -> std::io::Result<()> {
        Err(std::io::Error::other("the wait refused"))
    }
}

/// Returns without blocking, which the trait explicitly permits.
struct ImmediateWait;

impl CompletionWait for ImmediateWait {
    fn wait(&mut self, _: &mut RingWait<'_>, _: u32) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn pop_within_returns_the_completion_of_a_real_operation() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (path, file) = pop_scratch("real");
    let ids = push_flushes(&mut ring, &file, 1);

    let completion = ring
        .pop_within(std::time::Duration::from_secs(30))
        .expect("pop_within")
        .expect("the flush completes well inside the bound");
    assert_eq!(
        completion.user_data(),
        ids[0],
        "the completion popped must be the one that was pushed"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn submit_wait_is_what_the_convenience_uses() {
    // The same operation through the explicit spelling. This is what proves
    // `SubmitWait` really does block in the ring: no other wait is involved,
    // and the completion still arrives.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let (path, file) = pop_scratch("submit-wait");
    let ids = push_flushes(&mut ring, &file, 1);

    let completion = ring
        .pop_within_with(&mut SubmitWait, std::time::Duration::from_secs(30))
        .expect("pop_within_with")
        .expect("the flush completes");
    assert_eq!(completion.user_data(), ids[0]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn pop_within_returns_successive_completions_one_at_a_time() {
    let mut ring = IoRing::new(16, 32).expect("create ring");
    let (path, file) = pop_scratch("successive");
    let ids = push_flushes(&mut ring, &file, 3);

    let mut seen = Vec::new();
    for _ in 0..ids.len() {
        let completion = ring
            .pop_within(std::time::Duration::from_secs(30))
            .expect("pop_within")
            .expect("each flush completes");
        seen.push(completion.user_data());
    }
    seen.sort_unstable();
    let mut expected = ids.clone();
    expected.sort_unstable();
    assert_eq!(
        seen, expected,
        "every pushed flush must be popped exactly once"
    );

    assert!(
        ring.pop_within(std::time::Duration::ZERO)
            .expect("pop_within")
            .is_none(),
        "the queue is empty once every completion has been taken"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn pop_within_returns_none_at_once_when_nothing_can_ever_arrive() {
    // With no operation outstanding no completion is possible, so a
    // thirty-second bound must be answered immediately rather than slept
    // through. Asserting the elapsed time is what makes this a test of the
    // early return rather than of a short timeout.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let started = std::time::Instant::now();
    let popped = ring
        .pop_within(std::time::Duration::from_secs(30))
        .expect("pop_within");
    let elapsed = started.elapsed();
    assert!(popped.is_none(), "nothing was ever submitted");
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "an empty, quiesced ring must not wait out the bound; took {elapsed:?}"
    );
}

#[test]
fn a_supplied_wait_is_not_consulted_when_nothing_can_arrive() {
    // The partner to `a_supplied_wait_is_consulted_...` below. A loop that
    // always waited once before checking would pass that test and fail this
    // one.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let mut wait = RecordingWait::default();
    let popped = ring
        .pop_within_with(&mut wait, std::time::Duration::from_secs(30))
        .expect("pop_within_with");
    assert!(popped.is_none());
    assert_eq!(
        wait.calls, 0,
        "nothing can arrive, so there is nothing to wait for"
    );
}

#[test]
fn a_supplied_wait_is_consulted_when_the_queue_is_not_ready() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
    // A reservation with no real SQE behind it: outstanding, and no
    // completion will ever arrive for it.
    ring.reserve_user_data().expect("reserve");
    let mut wait = RecordingWait::default();
    let popped = ring.pop_within_with(&mut wait, std::time::Duration::from_millis(40));
    // Settled before any assertion: a panic here would otherwise unwind into
    // `Drop`, whose rundown cannot settle a reservation the kernel never saw,
    // and the second panic would abort the whole harness.
    ring.record_completion();

    assert!(popped.expect("pop_within_with").is_none());
    assert!(
        wait.calls >= 1,
        "the supplied wait must be the thing that blocks"
    );
}

#[test]
fn the_deadline_is_honoured_when_an_operation_never_completes() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
    ring.reserve_user_data().expect("reserve");

    let started = std::time::Instant::now();
    let popped = ring.pop_within_with(
        &mut RecordingWait::default(),
        std::time::Duration::from_millis(40),
    );
    let elapsed = started.elapsed();
    ring.record_completion();

    assert!(
        popped.expect("pop_within_with").is_none(),
        "no completion was ever going to arrive"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(20),
        "the bound must be waited out, not short-circuited; took {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "and it must be a bound rather than a hang; took {elapsed:?}"
    );
}

#[test]
fn a_zero_bound_does_not_block() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
    ring.reserve_user_data().expect("reserve");
    let mut wait = RecordingWait::default();
    let started = std::time::Instant::now();
    let popped = ring.pop_within_with(&mut wait, std::time::Duration::ZERO);
    let elapsed = started.elapsed();
    ring.record_completion();

    assert!(popped.expect("pop_within_with").is_none());
    assert_eq!(
        wait.calls, 0,
        "a zero bound is one try_pop, so the wait is never reached"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "a zero bound must not block; took {elapsed:?}"
    );
}

#[test]
fn the_wait_is_never_handed_a_zero_timeout() {
    // A sub-millisecond remainder rounds to zero milliseconds, which
    // `SubmitIoRing` reads as "poll and return" -- turning the tail of every
    // bound into a spin. The loop clamps it up to one.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    ring.reserve_user_data().expect("reserve");
    let mut wait = RecordingWait::default();
    let popped = ring.pop_within_with(&mut wait, std::time::Duration::from_micros(600));
    ring.record_completion();

    assert!(popped.expect("pop_within_with").is_none());
    assert!(wait.calls >= 1, "the wait must have been reached at all");
    assert!(
        wait.last_timeout_ms >= 1,
        "a sub-millisecond remainder must be clamped up, never passed as zero"
    );
}

#[test]
fn ring_wait_reports_the_rings_outstanding_count() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
    ring.reserve_user_data().expect("reserve a");
    ring.reserve_user_data().expect("reserve b");
    let mut wait = RecordingWait::default();
    let popped = ring.pop_within_with(&mut wait, std::time::Duration::from_millis(20));
    ring.record_completion();
    ring.record_completion();

    assert!(popped.expect("pop_within_with").is_none());
    assert_eq!(
        wait.outstanding_seen, 2,
        "RingWait::outstanding must agree with IoRing::outstanding"
    );
}

#[test]
fn a_wait_that_fails_ends_the_pop_with_its_error() {
    let mut ring = IoRing::new(16, 16).expect("create ring");
    ring.reserve_user_data().expect("reserve");
    let outcome = ring.pop_within_with(&mut FailingWait, std::time::Duration::from_secs(30));
    ring.record_completion();

    let error = outcome.expect_err("the wait's failure must reach the caller");
    assert_eq!(error.to_string(), "the wait refused");
}

#[test]
fn a_wait_that_never_blocks_is_permitted_and_still_terminates() {
    // The trait says returning early is always allowed. A caller that does so
    // spins, which is their choice -- but the bound must still hold.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    ring.reserve_user_data().expect("reserve");
    let started = std::time::Instant::now();
    let popped = ring.pop_within_with(&mut ImmediateWait, std::time::Duration::from_millis(30));
    let elapsed = started.elapsed();
    ring.record_completion();

    assert!(popped.expect("pop_within_with").is_none());
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "a non-blocking wait must still be bounded by the deadline; took {elapsed:?}"
    );
}

#[test]
fn a_bound_the_clock_cannot_represent_does_not_panic() {
    // `Instant + Duration` panics on overflow, and `Duration::MAX` is a
    // reasonable spelling of "no deadline". Nothing is outstanding here, so
    // the early return answers before any deadline arithmetic matters.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    let popped = ring
        .pop_within(std::time::Duration::MAX)
        .expect("pop_within");
    assert!(popped.is_none());
}

#[test]
fn a_bound_the_clock_cannot_represent_reaches_the_wait_rather_than_panicking() {
    // The half that exercises the overflow branch: something *is* outstanding,
    // so the loop computes a remaining duration from a deadline that could not
    // be represented. A failing wait is how the test escapes a bound that by
    // construction never arrives.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    ring.reserve_user_data().expect("reserve");
    let outcome = ring.pop_within_with(&mut FailingWait, std::time::Duration::MAX);
    ring.record_completion();

    let error = outcome.expect_err("the wait refuses, which is how this returns at all");
    assert_eq!(error.to_string(), "the wait refused");
}

#[test]
fn the_wait_can_be_supplied_as_a_trait_object() {
    // `?Sized` on the bound is what makes this compile, and a consumer
    // choosing a wait at run time is the reason to keep it.
    let mut ring = IoRing::new(16, 16).expect("create ring");
    ring.reserve_user_data().expect("reserve");
    let wait: &mut dyn CompletionWait = &mut FailingWait;
    let outcome = ring.pop_within_with(wait, std::time::Duration::from_secs(30));
    ring.record_completion();

    let error = outcome.expect_err("a trait-object wait still refuses");
    assert_eq!(error.to_string(), "the wait refused");
}
