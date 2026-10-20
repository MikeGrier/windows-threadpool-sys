// Copyright (c) 2026 Mike Grier
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{Diagnostic, DiagnosticKind, DiagnosticSink, NoDiagnostics};
use crate::test_support::{RecordingSink, TempDir, wtf};
use crate::{CreateError, Watcher};

fn probe_failed(registration: u64, code: Option<u32>) -> Diagnostic {
    Diagnostic::ProbeFailed {
        registration,
        win32_code: code,
    }
}

#[test]
fn an_event_is_of_its_kind() {
    assert_eq!(
        probe_failed(1, Some(21)).kind(),
        DiagnosticKind::ProbeFailed
    );
    assert_eq!(probe_failed(1, None).kind(), DiagnosticKind::ProbeFailed);
}

#[test]
fn an_event_names_its_registration_and_code() {
    let text = probe_failed(7, Some(21)).to_string();
    assert!(text.contains("registration 7"), "{text}");
    assert!(text.contains("Win32 code 21"), "{text}");
    assert!(text.contains("probed again"), "{text}");
}

#[test]
fn an_event_with_no_code_says_so() {
    let text = probe_failed(7, None).to_string();
    assert!(text.contains("registration 7"), "{text}");
    assert!(text.contains("no Win32 code"), "{text}");
}

#[test]
fn events_compare_by_content() {
    assert_eq!(probe_failed(1, Some(2)), probe_failed(1, Some(2)));
    assert_ne!(probe_failed(1, Some(2)), probe_failed(2, Some(2)));
    assert_ne!(probe_failed(1, Some(2)), probe_failed(1, Some(3)));
    assert_ne!(probe_failed(1, Some(2)), probe_failed(1, None));
}

#[test]
fn no_diagnostics_wants_nothing_and_ignores_what_it_is_given() {
    assert!(!NoDiagnostics.wants(DiagnosticKind::ProbeFailed));
    NoDiagnostics.report(&probe_failed(1, Some(2)));
}

#[test]
fn a_watcher_made_with_new_builds_no_event() {
    let watcher = Watcher::<u32>::new(4).expect("watcher");
    watcher.diagnose(DiagnosticKind::ProbeFailed, || {
        panic!("the default sink wants nothing, so no event is built")
    });
}

#[test]
fn a_sink_that_wants_nothing_costs_a_question_and_no_event() {
    let sink = RecordingSink::unwilling();
    let watcher = Watcher::<u32>::with_diagnostics(4, sink.clone()).expect("watcher");
    let built = AtomicUsize::new(0);
    for n in 0..5 {
        watcher.diagnose(DiagnosticKind::ProbeFailed, || {
            built.fetch_add(1, Ordering::SeqCst);
            probe_failed(n, None)
        });
    }
    assert_eq!(built.load(Ordering::SeqCst), 0);
    assert!(sink.seen().is_empty());
    assert_eq!(sink.asked(), [DiagnosticKind::ProbeFailed; 5]);
}

#[test]
fn a_sink_that_wants_the_kind_is_given_each_event_in_order() {
    let sink = RecordingSink::wanting();
    let watcher = Watcher::<u32>::with_diagnostics(4, sink.clone()).expect("watcher");
    let built = AtomicUsize::new(0);
    for n in 0..4 {
        watcher.diagnose(DiagnosticKind::ProbeFailed, || {
            built.fetch_add(1, Ordering::SeqCst);
            probe_failed(n, Some(21 + u32::try_from(n).expect("small")))
        });
    }
    assert_eq!(built.load(Ordering::SeqCst), 4);
    assert_eq!(
        sink.seen(),
        [
            probe_failed(0, Some(21)),
            probe_failed(1, Some(22)),
            probe_failed(2, Some(23)),
            probe_failed(3, Some(24)),
        ]
    );
}

#[test]
fn one_sink_can_serve_two_watchers() {
    let sink = RecordingSink::wanting();
    let first = Watcher::<u32>::with_diagnostics(4, sink.clone()).expect("first");
    let second = Watcher::<u32>::with_diagnostics(4, sink.clone()).expect("second");
    first.diagnose(DiagnosticKind::ProbeFailed, || probe_failed(1, None));
    second.diagnose(DiagnosticKind::ProbeFailed, || probe_failed(2, None));
    assert_eq!(sink.seen(), [probe_failed(1, None), probe_failed(2, None)]);
}

#[test]
fn a_sink_serves_events_from_many_threads() {
    let sink = RecordingSink::wanting();
    let watcher = Watcher::<u32>::with_diagnostics(4, sink.clone()).expect("watcher");
    std::thread::scope(|scope| {
        for thread in 0..8u64 {
            let watcher = &watcher;
            scope.spawn(move || {
                for n in 0..100 {
                    watcher.diagnose(DiagnosticKind::ProbeFailed, || {
                        probe_failed(thread * 100 + n, None)
                    });
                }
            });
        }
    });
    let mut registrations: Vec<_> = sink
        .seen()
        .iter()
        .map(|event| match event {
            Diagnostic::ProbeFailed { registration, .. } => *registration,
            other => panic!("only probe failures were reported, not {other}"),
        })
        .collect();
    registrations.sort_unstable();
    assert_eq!(registrations, (0..800).collect::<Vec<_>>());
}

#[test]
fn a_bad_capacity_is_refused_whichever_constructor_is_used() {
    let sink = RecordingSink::wanting();
    for requested in [0usize, 1, 3, 100] {
        match Watcher::<u32>::with_diagnostics(requested, sink.clone()).expect_err("invalid") {
            CreateError::Capacity(error) => assert_eq!(error.requested, requested),
            CreateError::Io(error) => panic!("a capacity error, not {error}"),
            other => panic!("a rule refusal, not {other}"),
        }
    }
    assert!(Watcher::<u32>::with_diagnostics(2, sink).is_ok());
}

#[test]
fn a_registrations_id_is_unique_ordered_and_never_reused() {
    let dir = TempDir::new("d-id");
    let path = dir.file("a.log", b"abc");
    let watcher = Watcher::<u32>::new(4).expect("watcher");
    let first = watcher.register(&wtf(&path), 0, 1).expect("first");
    let second = watcher.register(&wtf(&path), 0, 2).expect("second");
    assert!(first.id() < second.id());
    let dropped = second.id();
    drop(second);
    let third = watcher.register(&wtf(&path), 0, 3).expect("third");
    assert!(third.id() > dropped);
    assert_ne!(third.id(), first.id());
}

#[test]
fn a_registrations_id_is_the_one_the_snapshot_copies() {
    let dir = TempDir::new("d-snapshot");
    let path = dir.file("a.log", b"abc");
    let watcher = Watcher::<u32>::new(4).expect("watcher");
    let registrations: Vec<_> = (0..5)
        .map(|n| watcher.register(&wtf(&path), 0, n).expect("register"))
        .collect();
    let mut out = crate::chunked::Chunked::new();
    watcher.snapshot(&mut out);
    let copied: Vec<_> = out.iter().map(|item| item.id).collect();
    let ids: Vec<_> = registrations.iter().map(crate::Registration::id).collect();
    assert_eq!(copied, ids);
}

#[test]
fn a_sink_and_its_events_cross_threads() {
    fn send_sync<X: Send + Sync>() {}
    send_sync::<Diagnostic>();
    send_sync::<NoDiagnostics>();
    send_sync::<Arc<dyn DiagnosticSink>>();
    send_sync::<Watcher<Arc<u32>>>();
}

fn read_failed(registration: u64, offset: u64, len: u32, code: Option<u32>) -> Diagnostic {
    Diagnostic::ReadFailed {
        registration,
        offset,
        len,
        win32_code: code,
    }
}

#[test]
fn a_read_failure_is_of_its_own_kind() {
    assert_eq!(
        read_failed(1, 0, 8, Some(5)).kind(),
        DiagnosticKind::ReadFailed
    );
    assert_ne!(DiagnosticKind::ReadFailed, DiagnosticKind::ProbeFailed);
    assert_ne!(
        read_failed(1, 0, 8, Some(5)).kind(),
        probe_failed(1, Some(5)).kind()
    );
}

#[test]
fn a_read_failure_names_its_registration_range_and_code() {
    let text = read_failed(7, 4096, 512, Some(23)).to_string();
    assert!(text.contains("512 bytes"), "{text}");
    assert!(text.contains("offset 4096"), "{text}");
    assert!(text.contains("registration 7"), "{text}");
    assert!(text.contains("Win32 code 23"), "{text}");
    assert!(text.contains("not read again"), "{text}");
}

#[test]
fn a_read_failure_with_no_code_says_so() {
    let text = read_failed(7, 0, 1, None).to_string();
    assert!(text.contains("no Win32 code"), "{text}");
    assert!(text.contains("not read again"), "{text}");
}

#[test]
fn read_failures_compare_by_content() {
    let base = read_failed(1, 2, 3, Some(4));
    assert_eq!(base, read_failed(1, 2, 3, Some(4)));
    assert_ne!(base, read_failed(9, 2, 3, Some(4)));
    assert_ne!(base, read_failed(1, 9, 3, Some(4)));
    assert_ne!(base, read_failed(1, 2, 9, Some(4)));
    assert_ne!(base, read_failed(1, 2, 3, Some(9)));
    assert_ne!(base, read_failed(1, 2, 3, None));
    assert_ne!(read_failed(1, 2, 3, None), probe_failed(1, None));
}

#[test]
fn a_sink_is_asked_about_each_kind_separately() {
    let sink = RecordingSink::wanting();
    let watcher = Watcher::<u32>::with_diagnostics(4, sink.clone()).expect("watcher");
    watcher.diagnose(DiagnosticKind::ReadFailed, || read_failed(3, 0, 8, None));
    assert_eq!(sink.asked(), [DiagnosticKind::ReadFailed]);
    assert_eq!(sink.seen(), [read_failed(3, 0, 8, None)]);
}
