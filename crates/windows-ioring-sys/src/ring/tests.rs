// Copyright (c) 2026 Mike Grier
use super::{Completion, InjectedFailure, IoRing, Op, OpSupport};
use crate::IoRingErrorExt;
use crate::capability::{RingVersion, capabilities};
use std::sync::atomic::Ordering;

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
    // Read as "increased by at least one" rather than "increased by exactly
    // one": other tests' rings drop concurrently on this same counter, but
    // that only ever adds further increments, and can never mask this one --
    // so the assertion is race-free despite the shared static.
    let before = super::DROP_RUNS.load(Ordering::Relaxed);
    let ring = IoRing::new(8, 8).expect("create ring");
    drop(ring);
    let after = super::DROP_RUNS.load(Ordering::Relaxed);
    assert!(
        after > before,
        "dropping a ring must run its Drop impl at least once (before={before}, after={after})"
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
    let completion = loop {
        if let Some(completion) = ring.try_pop().expect("pop") {
            break completion;
        }
    };
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
