// Copyright (c) 2026 Mike Grier
//! The Windows backend over a real ring: pushes, completions and commits on registered and
//! attached targets, registered buffers, refusals and their causes, and construction.

use std::io;
use std::time::{Duration, Instant};

use windows_ioring_sys::{Batch, InjectedFailure, IoBuf, IoRing, PushOptions, RingCondition};

use super::{
    Win32FileTarget, WindowsBackend, WindowsBackendError, WindowsBackendRefusal,
    WindowsBackendSetup, refusal_cause, transfer,
};
use crate::dioring::tests::TempFile;
use durable_ioring::backend::{
    Completion, IoBackend, RefusalCause, RegisteredRegion, Submission, SubmissionData,
    SubmissionOp, SubmissionToken,
};
use durable_ioring::{FlushDomain, IoTargetKey, WriteCaching};
use durable_ioring::{Geometry, IoTarget, WarrantyClass};

const GIVEN: IoTargetKey = IoTargetKey(1);
const ADDED: IoTargetKey = IoTargetKey(2);
/// Generous: each wait is for a real completion on a local file.
const BOUND: Duration = Duration::from_secs(5);

type Backend = WindowsBackend<Vec<u8>>;

mod construction;

fn setup(
    targets: Vec<(IoTargetKey, Win32FileTarget)>,
    buffers: Vec<Vec<u8>>,
) -> WindowsBackendSetup<Vec<u8>> {
    WindowsBackendSetup {
        submission_queue_size: 16,
        completion_queue_size: 16,
        targets,
        buffers,
    }
}

/// A backend with one target registered under `GIVEN`, and one attached under `ADDED`.
fn backend(given: &TempFile, added: &TempFile) -> Backend {
    let mut backend = Backend::new(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        Vec::new(),
    ))
    .expect("build");
    backend
        .attach(ADDED, Win32FileTarget::new(added.open()))
        .expect("a new key");
    backend
}

fn write(token: u64, target: IoTargetKey, offset: u64, bytes: &[u8]) -> Submission<Vec<u8>> {
    Submission {
        token: SubmissionToken(token),
        target,
        op: SubmissionOp::Write {
            offset,
            data: SubmissionData::Owned(bytes.to_vec()),
            caching: WriteCaching::Cached,
        },
    }
}

fn read(token: u64, target: IoTargetKey, offset: u64, length: usize) -> Submission<Vec<u8>> {
    Submission {
        token: SubmissionToken(token),
        target,
        op: SubmissionOp::Read {
            offset,
            data: SubmissionData::Owned(vec![0; length]),
        },
    }
}

fn commit(token: u64, target: IoTargetKey) -> Submission<Vec<u8>> {
    Submission {
        token: SubmissionToken(token),
        target,
        op: SubmissionOp::Commit,
    }
}

/// The next completion, within the bound.
fn next<B: IoBuf>(backend: &mut WindowsBackend<B>) -> Completion<B> {
    let started = Instant::now();
    loop {
        if let Some(completion) = backend.pop().expect("pop") {
            return completion;
        }
        assert!(started.elapsed() < BOUND, "no completion within {BOUND:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Push `submission`, submit, and wait for its completion.
fn run<B: IoBuf + std::fmt::Debug>(
    backend: &mut WindowsBackend<B>,
    submission: Submission<B>,
) -> Completion<B> {
    let token = submission.token;
    backend.push(submission).expect("accepted");
    backend.submit().expect("submitted");
    let completion = next(backend);
    assert_eq!(completion.token, token);
    completion
}

#[test]
fn a_write_to_a_registered_target_is_read_back() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    let written = run(&mut backend, write(1, GIVEN, 0, b"registered"));
    assert_eq!(written.result, Ok(10));
    assert_eq!(written.buffer.as_deref(), Some(b"registered".as_slice()));
    let back = run(&mut backend, read(2, GIVEN, 0, 10));
    assert_eq!(back.result, Ok(10));
    assert_eq!(back.buffer.as_deref(), Some(b"registered".as_slice()));
}

#[test]
fn a_write_to_an_attached_target_is_read_back() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    assert_eq!(
        run(&mut backend, write(1, ADDED, 4, b"attached")).result,
        Ok(8)
    );
    let back = run(&mut backend, read(2, ADDED, 4, 8));
    assert_eq!(back.buffer.as_deref(), Some(b"attached".as_slice()));
    assert_eq!(
        std::fs::read(&added.0).expect("read the file")[4..],
        *b"attached"
    );
}

#[test]
fn a_read_past_the_end_is_short() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    // The temporary file holds one byte.
    assert_eq!(run(&mut backend, read(1, ADDED, 0, 16)).result, Ok(1));
}

#[test]
fn a_commit_completes_with_no_count_and_no_buffer() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    for (token, target) in [(1, GIVEN), (2, ADDED)] {
        run(&mut backend, write(token * 10, target, 0, b"x"));
        let committed = run(&mut backend, commit(token, target));
        assert_eq!(committed.result, Ok(0));
        assert!(committed.buffer.is_none());
    }
}

#[test]
fn an_armed_commit_failure_reports_the_code() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    backend.fail_next_commit(ADDED, 1117);
    let read_meanwhile = run(&mut backend, read(1, ADDED, 0, 1));
    assert!(read_meanwhile.result.is_ok(), "only a commit is failed");
    let failed = run(&mut backend, commit(2, ADDED));
    let fault = failed.result.expect_err("armed");
    assert_eq!(fault.source, crate::error_code::WINDOWS_IORING);
    assert_eq!(fault.code.and_then(crate::ErrorCode::win32), Some(1117));
    assert_eq!(
        run(&mut backend, commit(3, ADDED)).result,
        Ok(0),
        "used once"
    );
}

#[test]
fn completions_carry_their_tokens() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    for token in [7, 3, 9] {
        backend
            .push(write(token, GIVEN, token, b"t"))
            .expect("accepted");
    }
    let mut tokens: Vec<u64> = (0..3).map(|_| next(&mut backend).token.0).collect();
    tokens.sort_unstable();
    assert_eq!(tokens, [3, 7, 9]);
}

#[test]
fn an_unknown_target_is_refused_with_the_submission_handed_back() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    let refused = backend
        .push(write(4, IoTargetKey(99), 8, b"kept"))
        .expect_err("unknown");
    assert_eq!(refused.cause, RefusalCause::UnknownTarget);
    assert_eq!(refused.submission.token, SubmissionToken(4));
    let SubmissionOp::Write {
        offset,
        data: SubmissionData::Owned(buffer),
        ..
    } = refused.submission.op
    else {
        panic!("the write");
    };
    assert_eq!((offset, buffer.as_slice()), (8, b"kept".as_slice()));
}

#[test]
fn a_registered_region_without_registration_is_unsupported() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    assert_eq!(backend.registered_buffers(), 0);
    let region = RegisteredRegion {
        index: 0,
        offset: 0,
        length: 1,
    };
    let refused = backend
        .push(Submission {
            token: SubmissionToken(1),
            target: GIVEN,
            op: SubmissionOp::Read {
                offset: 0,
                data: SubmissionData::Registered(region),
            },
        })
        .expect_err("no registration");
    assert_eq!(refused.cause, RefusalCause::Unsupported);
    assert!(backend.registered_buffer(0).is_err());
}

#[test]
fn registered_regions_are_written_from_and_read_into() {
    let given = TempFile::new("wb-given");
    let mut backend = Backend::new(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        vec![b"..source..".to_vec(), vec![0; 6]],
    ))
    .expect("build");
    assert_eq!(backend.registered_buffers(), 2);
    let source = RegisteredRegion {
        index: 0,
        offset: 2,
        length: 6,
    };
    let sink = RegisteredRegion {
        index: 1,
        offset: 0,
        length: 6,
    };
    let written = run(
        &mut backend,
        Submission {
            token: SubmissionToken(1),
            target: GIVEN,
            op: SubmissionOp::Write {
                offset: 0,
                data: SubmissionData::Registered(source),
                caching: WriteCaching::Cached,
            },
        },
    );
    assert_eq!(written.result, Ok(6));
    assert!(written.buffer.is_none());
    let back = run(
        &mut backend,
        Submission {
            token: SubmissionToken(2),
            target: GIVEN,
            op: SubmissionOp::Read {
                offset: 0,
                data: SubmissionData::Registered(sink),
            },
        },
    );
    assert_eq!(back.result, Ok(6));
    assert_eq!(backend.registered_buffer(1).expect("registered"), b"source");
    backend.registered_buffer_mut(1).expect("registered")[0] = b'S';
    assert_eq!(backend.registered_buffer(1).expect("registered"), b"Source");
}

#[test]
fn a_region_the_ring_refuses_comes_back_with_its_cause() {
    let given = TempFile::new("wb-given");
    let mut backend = Backend::new(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        vec![vec![0; 8]],
    ))
    .expect("build");
    for (token, region) in [
        (
            1,
            RegisteredRegion {
                index: 5,
                offset: 0,
                length: 1,
            },
        ),
        (
            2,
            RegisteredRegion {
                index: 0,
                offset: 4,
                length: 8,
            },
        ),
    ] {
        let refused = backend
            .push(Submission {
                token: SubmissionToken(token),
                target: GIVEN,
                op: SubmissionOp::Write {
                    offset: 16,
                    data: SubmissionData::Registered(region),
                    caching: WriteCaching::WriteThrough,
                },
            })
            .expect_err("outside the registration");
        assert_eq!(refused.cause, RefusalCause::Invalid, "{region:?}");
        assert_eq!(refused.submission.token, SubmissionToken(token));
        assert_eq!(refused.submission.target, GIVEN);
        assert!(
            matches!(
                refused.submission.op,
                SubmissionOp::Write {
                    offset: 16,
                    data: SubmissionData::Registered(back),
                    caching: WriteCaching::WriteThrough,
                } if back == region
            ),
            "rebuilt as it was pushed"
        );
    }
}

#[test]
fn the_targets_geometry_is_enforced_before_the_ring_is_asked() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    let geometry = Geometry::new(512, 512, 1).expect("powers of two");
    backend
        .attach(
            IoTargetKey(3),
            Win32FileTarget::new(added.open()).with_geometry(geometry),
        )
        .expect("a new key");
    for (offset, length) in [(1, 512), (512, 1)] {
        let refused = backend
            .push(write(1, IoTargetKey(3), offset, &vec![0; length]))
            .expect_err("unaligned");
        assert_eq!(refused.cause, RefusalCause::Invalid, "{offset}, {length}");
    }
    run(&mut backend, write(2, IoTargetKey(3), 512, &[0; 512]));
}

#[test]
fn check_answers_what_push_does() {
    // Both directions over a real ring, case by case: every submission `check` admits a push
    // accepts, and every one it refuses a push refuses for the same cause -- the registration's
    // bounds included, which a push leaves to the ring and `check` asks of it (DI-D-54).
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let aligned = IoTargetKey(3);
    let mut backend = Backend::new(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        vec![vec![0; 8]],
    ))
    .expect("build");
    let geometry = Geometry::new(512, 512, 1).expect("powers of two");
    backend
        .attach(ADDED, Win32FileTarget::new(added.open()))
        .expect("a new key");
    backend
        .attach(
            aligned,
            Win32FileTarget::new(added.open()).with_geometry(geometry),
        )
        .expect("a new key");
    let region = |token, target, index, offset, length| Submission {
        token: SubmissionToken(token),
        target,
        op: SubmissionOp::Write {
            offset: 0,
            data: SubmissionData::Registered(RegisteredRegion {
                index,
                offset,
                length,
            }),
            caching: WriteCaching::Cached,
        },
    };
    let cases: Vec<(Submission<Vec<u8>>, Option<RefusalCause>)> = vec![
        (write(1, GIVEN, 0, b"abc"), None),
        (read(2, ADDED, 0, 4), None),
        (commit(3, GIVEN), None),
        (region(4, GIVEN, 0, 0, 8), None),
        (region(5, ADDED, 0, 4, 4), None),
        (write(6, aligned, 512, &[0; 512]), None),
        (
            write(7, IoTargetKey(99), 0, b"x"),
            Some(RefusalCause::UnknownTarget),
        ),
        (
            commit(8, IoTargetKey(99)),
            Some(RefusalCause::UnknownTarget),
        ),
        (region(9, GIVEN, 1, 0, 1), Some(RefusalCause::Invalid)),
        (region(10, GIVEN, 0, 4, 5), Some(RefusalCause::Invalid)),
        (
            region(11, ADDED, 0, u32::MAX, 2),
            Some(RefusalCause::Invalid),
        ),
        (
            write(12, aligned, 3, &[0; 512]),
            Some(RefusalCause::Invalid),
        ),
    ];
    for (submission, cause) in cases {
        let checked = backend.check(submission.target, &submission.op).err();
        assert_eq!(checked, cause, "check: {submission:?}");
        match backend.push(submission) {
            Ok(()) => {
                assert!(cause.is_none());
                backend.submit().expect("submitted");
                next(&mut backend);
            }
            Err(refused) => assert_eq!(Some(refused.cause), cause, "push"),
        }
    }

    // Without a registration, a region is one the backend cannot perform, checked or pushed.
    let mut bare = backend_without_buffers(&given);
    let submission = region(13, GIVEN, 0, 0, 1);
    assert_eq!(
        bare.check(submission.target, &submission.op),
        Err(RefusalCause::Unsupported)
    );
    assert_eq!(
        bare.push(submission).expect_err("no registration").cause,
        RefusalCause::Unsupported
    );
}

fn backend_without_buffers(given: &TempFile) -> Backend {
    Backend::new(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        Vec::new(),
    ))
    .expect("build")
}

#[test]
fn a_duplicate_key_at_construction_hands_everything_back() {
    let (one, two) = (TempFile::new("wb-one"), TempFile::new("wb-two"));
    let refused: WindowsBackendError<Vec<u8>> = Backend::new(setup(
        vec![
            (GIVEN, Win32FileTarget::new(one.open())),
            (GIVEN, Win32FileTarget::new(two.open())),
        ],
        vec![vec![0; 4]],
    ))
    .err()
    .expect("a repeated key");
    assert!(matches!(
        refused.reason,
        WindowsBackendRefusal::DuplicateKey(GIVEN)
    ));
    assert_eq!(refused.targets.len(), 2);
    assert_eq!(refused.buffers.map(|b| b.len()), Some(1));
}

#[test]
fn a_ring_that_cannot_be_created_hands_everything_back() {
    let given = TempFile::new("wb-given");
    let mut bad = setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        vec![vec![0; 4]],
    );
    // Zero is not refused -- the kernel rounds it up -- but a queue no system can allocate is.
    bad.submission_queue_size = u32::MAX;
    bad.completion_queue_size = u32::MAX;
    let refused = Backend::new(bad).err().expect("no queue");
    assert!(matches!(refused.reason, WindowsBackendRefusal::Ring(_)));
    assert_eq!(refused.targets.len(), 1);
    assert_eq!(refused.buffers.map(|b| b.len()), Some(1));
}

#[test]
fn attach_refuses_a_key_in_use_and_detach_hands_the_target_back() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    let target = Win32FileTarget::new(added.open()).with_warranty(WarrantyClass::Complete);
    let duplicate = backend.attach(ADDED, target).expect_err("taken");
    assert_eq!(duplicate.key, ADDED);
    assert_eq!(duplicate.target.warranty(), WarrantyClass::Complete);
    let detached = backend.detach(ADDED).expect("attached");
    assert_eq!(detached.warranty(), WarrantyClass::BestEffort);
    assert!(backend.detach(ADDED).is_none());
    assert!(backend.target(ADDED).is_none());
    assert!(
        backend.target(GIVEN).is_some(),
        "a registered target is detachable too"
    );
}

#[test]
fn a_file_target_states_what_it_is_built_with() {
    let file = TempFile::new("wb-target");
    let plain = Win32FileTarget::new(file.open());
    assert_eq!(plain.warranty(), WarrantyClass::BestEffort);
    assert_eq!(plain.geometry(), Geometry::UNALIGNED);
    assert!(plain.flush_domains().is_empty());
    assert!(!plain.capabilities().write_through());
    let stated = plain
        .with_domains([FlushDomain::new(b"disk".as_slice())])
        .with_warranty(WarrantyClass::SharedDevice)
        .with_capabilities(durable_ioring::Capabilities::NONE.with_write_through());
    let description = stated.describe();
    assert_eq!(
        description.flush_domains,
        [FlushDomain::new(b"disk".as_slice())]
    );
    assert_eq!(description.warranty, WarrantyClass::SharedDevice);
    assert!(description.capabilities.write_through());
}

#[test]
fn declaring_a_file_targets_domains_again_replaces_them() {
    // Carried over from `FileOptions`, which the target replaced: a declaration is the whole set.
    let file = TempFile::new("wb-domains");
    let domain = |name: &[u8]| FlushDomain::new(name.to_vec());
    let target = Win32FileTarget::new(file.open())
        .with_domains([domain(b"A")])
        .with_domains([domain(b"B"), domain(b"C")]);
    assert_eq!(target.flush_domains(), [domain(b"B"), domain(b"C")]);
    let cleared = target.with_domains([]);
    assert!(cleared.flush_domains().is_empty(), "none declared again");
}

#[test]
fn nothing_to_submit_is_success_and_notifications_are_handed_out() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    backend.submit().expect("nothing queued");
    backend.notification().expect("a handle");
    backend.notification().expect("another");
    assert!(backend.pop().expect("pop").is_none());
}

/// The failure the tests below arm on a ring's submits: `ERROR_IO_DEVICE`, which no ring condition
/// names, so it is of unknown cause.
fn device() -> InjectedFailure {
    InjectedFailure::Win32(1117)
}

#[test]
fn a_submit_that_fails_after_an_accepted_push_is_reported_and_reissued() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    let faults = backend.submit_faults();
    faults.fail_until_cleared(device());

    backend
        .push(write(1, GIVEN, 0, b"queued"))
        .expect("a push is accepted though its own submit failed");
    assert!(
        backend.pop().expect("pop").is_none(),
        "the ring never submitted it, so nothing completed"
    );
    let RefusalCause::Unknown(fault) = backend
        .submit()
        .expect_err("the submit is owed, and fails again")
    else {
        panic!("an error no ring condition names is of unknown cause");
    };
    assert_eq!(fault.source, crate::error_code::WINDOWS_IORING);
    assert_eq!(fault.code.and_then(crate::ErrorCode::win32), Some(1117));
    assert!(
        backend.pop().expect("pop").is_none(),
        "a failed reissue leaves it queued"
    );

    faults.clear();
    backend.submit().expect("the reissue succeeds");
    let completion = next(&mut backend);
    assert_eq!(completion.token, SubmissionToken(1));
    assert_eq!(completion.result, Ok(6));
    assert_eq!(
        std::fs::read(&given.0).expect("read the file"),
        b"queued",
        "and it ran"
    );
    assert_eq!(
        faults.failures().len(),
        2,
        "the push's own submit, and the first reissue"
    );
}

#[test]
fn the_ring_is_asked_to_submit_only_while_a_submit_is_owed() {
    let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
    let mut backend = backend(&given, &added);
    let faults = backend.submit_faults();

    faults.fail_until_cleared(device());
    backend
        .submit()
        .expect("nothing is owed, so the ring is not asked");
    assert!(faults.failures().is_empty());

    backend
        .push(write(1, GIVEN, 0, b"owed"))
        .expect("accepted, its submit failing");
    backend.submit().expect_err("owed, so asked, and fails");
    assert_eq!(faults.failures().len(), 2);

    faults.clear();
    backend.submit().expect("the reissue succeeds");
    next(&mut backend);

    faults.fail_until_cleared(device());
    backend
        .submit()
        .expect("the reissue cleared what was owed, so the ring is not asked");
    assert_eq!(
        faults.failures().len(),
        2,
        "no further failure: nothing was asked"
    );
    faults.clear();
}

#[test]
fn a_failed_submit_reports_its_cause_as_the_action_that_clears_it() {
    // The real flow, not `refusal_cause` alone: what the ring reports from a failed submit is what
    // the caller is told.
    for (condition, expected) in [
        (
            RingCondition::SubmissionQueueFull,
            RefusalCause::SubmitNeeded,
        ),
        (
            RingCondition::CompletionQueueTooFull,
            RefusalCause::DrainNeeded,
        ),
        (RingCondition::SubmitInProgress, RefusalCause::Busy),
    ] {
        let (given, added) = (TempFile::new("wb-given"), TempFile::new("wb-added"));
        let mut backend = backend(&given, &added);
        let faults = backend.submit_faults();
        faults.fail_until_cleared(InjectedFailure::Ring(condition));
        backend
            .push(write(1, GIVEN, 0, b"x"))
            .expect("accepted, its submit failing");

        assert_eq!(
            backend.submit().expect_err("owed, and failing"),
            expected,
            "{condition:?}"
        );

        faults.clear();
        backend.submit().expect("the reissue succeeds");
        next(&mut backend);
    }
}

/// A real ring completion, to inject conditions into.
fn real_completion() -> windows_ioring_sys::Completion {
    let file = TempFile::new("wb-ring");
    let handle = file.open();
    let mut ring: IoRing<Vec<u8>, ()> = IoRing::with_inventory(4, 4).expect("a ring");
    Batch::new(&mut ring)
        .read_owned(&handle, vec![0; 1], (), 0, PushOptions::new())
        .expect("a read");
    let (completion, _) = ring.pop_within(BOUND).expect("pop").expect("a completion");
    completion
}

#[test]
fn each_ring_condition_maps_to_the_action_that_clears_it() {
    let completion = real_completion();
    let cause = |condition| {
        refusal_cause(
            completion
                .with_injected_failure(InjectedFailure::Ring(condition))
                .result()
                .expect_err("injected"),
        )
    };
    assert_eq!(
        cause(RingCondition::SubmissionQueueFull),
        RefusalCause::SubmitNeeded
    );
    assert_eq!(
        cause(RingCondition::CompletionQueueTooFull),
        RefusalCause::DrainNeeded
    );
    assert_eq!(cause(RingCondition::SubmitInProgress), RefusalCause::Busy);
    assert!(matches!(
        cause(RingCondition::Corrupt),
        RefusalCause::Unusable(_)
    ));
    for other in [
        RingCondition::RequiredFlagNotSupported,
        RingCondition::VersionNotSupported,
        RingCondition::SubmissionQueueTooBig,
        RingCondition::CompletionQueueTooBig,
    ] {
        let RefusalCause::Unknown(fault) = cause(other) else {
            panic!("{other:?} is not one an action clears");
        };
        assert_eq!(fault.source, crate::error_code::WINDOWS_IORING);
        assert!(fault.code.is_some(), "the ring's report keeps its code");
    }
}

#[test]
fn the_rings_own_rejections_map_by_kind() {
    assert_eq!(
        refusal_cause(io::Error::new(io::ErrorKind::Unsupported, "no")),
        RefusalCause::Unsupported
    );
    assert_eq!(
        refusal_cause(io::Error::new(io::ErrorKind::InvalidInput, "no")),
        RefusalCause::Invalid
    );
    assert!(matches!(
        refusal_cause(io::Error::other("no")),
        RefusalCause::Unknown(fault) if fault.code.is_none()
    ));
}

#[test]
fn a_short_write_is_a_fault_and_a_short_read_is_not() {
    assert_eq!(transfer(Ok(8), true, 8), Ok(8));
    let fault = transfer(Ok(3), true, 8).expect_err("short");
    assert!(fault.error.to_string().contains("3 of 8"));
    assert_eq!(transfer(Ok(3), false, 0), Ok(3));
    assert!(transfer(Err(io::Error::from_raw_os_error(1117)), false, 0).is_err());
}

// The cases below were the relay's, on its `op_outcome` over `transfer`; the relay went in
// `DI-3.10.3.5.6.2.6`, and they are pinned on `transfer` itself.

/// A write that transferred fewer bytes than it asked for is a fault under the `Fault` policy: a
/// target never completes a write short, and no layer gave a code for it.
#[test]
fn a_short_write_is_the_rings_fault_with_no_code() {
    let fault = transfer(Ok(3), true, 8).expect_err("short");
    assert_eq!(fault.source, crate::error_code::WINDOWS_IORING);
    assert_eq!(fault.code, None);
}

/// A write that transferred nothing is short too.
#[test]
fn a_write_that_transferred_nothing_is_a_fault() {
    assert!(transfer(Ok(0), true, 8).is_err());
}

/// A read may complete short: the obligation is a write's. The read is given a requested length
/// here, though the backend passes zero for one, so that the count alone would make it short.
#[test]
fn a_short_read_is_transferred_whatever_was_requested() {
    assert_eq!(transfer(Ok(3), false, 8), Ok(3));
}

/// A failed completion is the ring's fault, with the code it carries.
#[test]
fn a_failed_completion_is_the_rings_fault_with_its_code() {
    let fault = transfer(Err(io::Error::from_raw_os_error(1117)), true, 8).expect_err("failed");
    assert_eq!(fault.source, crate::error_code::WINDOWS_IORING);
    assert_eq!(
        fault.code,
        Some(durable_ioring::ErrorCode::from_win32(1117))
    );
}

/// A failed completion with no code relays the error with none: the code is optional (DI-D-50).
#[test]
fn a_failure_with_no_code_is_relayed_without_one() {
    let fault = transfer(Err(io::Error::other("no code")), true, 8).expect_err("failed");
    assert_eq!(fault.code, None);
}

/// A transfer longer than a `u32` is counted as `u32::MAX` rather than wrapping.
#[test]
fn a_transfer_beyond_u32_saturates() {
    let huge = usize::try_from(u64::from(u32::MAX) + 1).expect("a 64-bit target");
    assert_eq!(transfer(Ok(huge), false, 0), Ok(u32::MAX));
}

#[test]
fn a_write_only_buffer_type_is_written_and_an_owned_read_is_refused_without_a_read_push() {
    // DI-D-17: an owned write needs only `IoBuf`, so `Arc<[u8]>` -- which is not `IoBufMut` -- is a
    // backend's buffer type. Built without owned reads, an owned read is refused as Unsupported by
    // `check` and `push` alike; a registered read needs nothing of the owned type, and still runs.
    use std::sync::Arc;

    let given = TempFile::new("wb-write-only");
    let mut backend: WindowsBackend<Arc<[u8]>> = WindowsBackend::without_owned_reads(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        vec![vec![0; 6]],
    ))
    .expect("build");
    let shared: Arc<[u8]> = Arc::from(b"shared".as_slice());
    let write = Submission {
        token: SubmissionToken(1),
        target: GIVEN,
        op: SubmissionOp::Write {
            offset: 0,
            data: SubmissionData::Owned(Arc::clone(&shared)),
            caching: WriteCaching::Cached,
        },
    };
    let written = run(&mut backend, write);
    assert_eq!(written.result, Ok(6));
    assert!(
        written
            .buffer
            .is_some_and(|back| Arc::ptr_eq(&back, &shared)),
        "the consumer's own buffer comes back"
    );

    let owned_read = Submission {
        token: SubmissionToken(2),
        target: GIVEN,
        op: SubmissionOp::Read {
            offset: 0,
            data: SubmissionData::Owned(Arc::clone(&shared)),
        },
    };
    assert_eq!(
        backend.check(owned_read.target, &owned_read.op),
        Err(RefusalCause::Unsupported)
    );
    let refused = backend.push(owned_read).expect_err("no owned reads");
    assert_eq!(refused.cause, RefusalCause::Unsupported);
    assert!(
        matches!(
            refused.submission.op,
            SubmissionOp::Read { data: SubmissionData::Owned(back), .. } if Arc::ptr_eq(&back, &shared)
        ),
        "handed back"
    );

    let into = RegisteredRegion {
        index: 0,
        offset: 0,
        length: 6,
    };
    let back = run(
        &mut backend,
        Submission {
            token: SubmissionToken(3),
            target: GIVEN,
            op: SubmissionOp::Read {
                offset: 0,
                data: SubmissionData::Registered(into),
            },
        },
    );
    assert_eq!(back.result, Ok(6));
    assert_eq!(backend.registered_buffer(0).expect("registered"), b"shared");

    // The other direction: it is the constructor, not the type, that decides. A buffer type that is
    // `IoBufMut` reads through `new`, and is refused through `without_owned_reads`.
    let mut readable = Backend::new(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        Vec::new(),
    ))
    .expect("build");
    assert_eq!(run(&mut readable, read(4, GIVEN, 0, 6)).result, Ok(6));
    let mut unreadable = Backend::without_owned_reads(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        Vec::new(),
    ))
    .expect("build");
    assert_eq!(
        unreadable
            .push(read(5, GIVEN, 0, 6))
            .expect_err("no owned reads")
            .cause,
        RefusalCause::Unsupported
    );
}

#[test]
fn the_core_writes_a_write_only_buffer_type_over_the_windows_backend() {
    // DI-D-17 through the core and the engine: one `Arc<[u8]>` written to two targets, sealed, and
    // reported durable, with no `IoBufMut` anywhere on the path.
    use std::sync::Arc;

    use crate::InterruptStamps;

    use durable_ioring::DurableIoCore;
    use durable_ioring::NoDiagnostics;
    use durable_ioring::driver::Engine;
    use durable_ioring::{Entry, Epoch, WriteOptions};

    let (given, added) = (TempFile::new("wb-wo-given"), TempFile::new("wb-wo-added"));
    let mut backend: WindowsBackend<Arc<[u8]>> = WindowsBackend::without_owned_reads(setup(
        vec![(GIVEN, Win32FileTarget::new(given.open()))],
        Vec::new(),
    ))
    .expect("build");
    backend
        .attach(ADDED, Win32FileTarget::new(added.open()))
        .expect("a new key");
    let core: DurableIoCore<u64, Arc<[u8]>, u32, InterruptStamps> =
        DurableIoCore::new(InterruptStamps, Arc::new(NoDiagnostics));
    let mut engine = Engine::new(core, backend);
    for key in [GIVEN, ADDED] {
        let description = engine.backend.target(key).expect("attached").describe();
        engine.core.add_target(key, description).expect("a new key");
    }
    let lineage = engine.core.default_lineage();
    let block: Arc<[u8]> = Arc::from(b"replicated".as_slice());
    for (context, key) in [(1, GIVEN), (2, ADDED)] {
        engine
            .write(
                key,
                0,
                SubmissionData::Owned(Arc::clone(&block)),
                Epoch::new(lineage, 1),
                context,
                WriteOptions::new(),
            )
            .expect("a write-only buffer is accepted");
    }
    engine
        .make_durable_through(Epoch::new(lineage, 1))
        .expect("the default lineage");
    let started = Instant::now();
    let mut entries = Vec::new();
    while !entries
        .iter()
        .any(|entry| matches!(entry, Entry::Durable { .. }))
    {
        engine.drain();
        entries.extend(std::iter::from_fn(|| engine.core.pop()));
        assert!(started.elapsed() < BOUND, "stalled with {entries:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(engine.core.durable_through(lineage), Ok(Some(1)));
    for path in [&given.0, &added.0] {
        assert_eq!(
            std::fs::read(path).expect("read back"),
            b"replicated",
            "{path:?}"
        );
    }
}

#[test]
fn the_core_runs_over_the_windows_backend_under_the_oracle() {
    use std::sync::Arc;

    use crate::InterruptStamps;

    use durable_ioring::DurableIoCore;
    use durable_ioring::NoDiagnostics;
    use durable_ioring::oracle::ConformanceOracle;
    use durable_ioring::{DurabilityRequest, Entry, Epoch, OpKind, WriteOptions};

    type V = crate::WindowsIds<u64>;
    let (given, added) = (
        TempFile::new("wb-core-given"),
        TempFile::new("wb-core-added"),
    );
    let mut backend = backend(&given, &added);
    let mut core: DurableIoCore<u64, Vec<u8>, u32, InterruptStamps> =
        DurableIoCore::new(InterruptStamps, Arc::new(NoDiagnostics));
    let mut oracle: ConformanceOracle<V, u32> = ConformanceOracle::new();
    for key in [GIVEN, ADDED] {
        let description = backend.target(key).expect("attached").describe();
        core.add_target(key, description).expect("a new key");
    }
    let lineage = core.default_lineage();
    for (i, key) in [GIVEN, ADDED, GIVEN].into_iter().enumerate() {
        let epoch = Epoch::new(lineage, 1);
        let context = u32::try_from(i).expect("small");
        let id = core
            .write(
                key,
                u64::from(context) * 4,
                SubmissionData::Owned(b"core".to_vec()),
                epoch,
                context,
                WriteOptions::new(),
                &mut backend,
            )
            .expect("accepted");
        oracle
            .pushed(id, OpKind::Write { epoch }, 4, context)
            .expect("the oracle accepts the push");
    }
    let through = Epoch::new(lineage, 1);
    let (answer, actions) = core
        .make_durable_through(through)
        .expect("the default lineage");
    assert_eq!(answer, DurabilityRequest::Submitted);
    oracle.sealed(through);
    let mut pending = actions.submit;
    let mut entries = Vec::new();
    let started = Instant::now();
    while entries.len() < 4 {
        for submission in pending.drain(..) {
            backend.push(submission).expect("room");
        }
        backend.submit().expect("submitted");
        if let Some(completion) = backend.pop().expect("pop") {
            pending.extend(core.complete(completion).submit);
        } else {
            assert!(started.elapsed() < BOUND, "stalled with {entries:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
        while let Some(entry) = core.pop() {
            oracle
                .observe(&entry)
                .expect("the oracle accepts the entry");
            entries.push(entry);
        }
    }
    assert!(matches!(entries[3], Entry::Durable { through: t } if t == through));
    assert_eq!(core.durable_through(lineage), Ok(Some(1)));
    oracle.finish().expect("every operation completed");
}
