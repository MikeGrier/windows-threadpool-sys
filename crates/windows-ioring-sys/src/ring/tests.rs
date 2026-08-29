// Copyright (c) 2026 Mike Grier
use super::{IoRing, Op, OpSupport};
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

    let completion = loop {
        if let Some(completion) = ring.try_pop().expect("pop") {
            break completion;
        }
    };
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
    let completion = loop {
        if let Some(completion) = ring.try_pop().expect("pop") {
            break completion;
        }
    };
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

// Deliberately not tested: that injection zeroes the transferred byte count.
// `information` is private and `result()` yields `Err` for an injected
// failure, so the zeroing is unobservable through the public API -- there is
// no assertion to write. It is still done, because modelling a state the
// kernel never produces would be wrong even where nothing can see it, but a
// test asserting only `is_err()` under that name would be coverage in
// appearance and nothing in substance.

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
