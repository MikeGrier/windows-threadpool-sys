// Copyright (c) 2026 Mike Grier
//! What the sink does for a session that is listening (DE-D-4): this test starts a real trace session logging to a file, enables
//! the provider in it, reports events through `EtwDiagnostics`, stops the session and reads the file back, with `win-etw-sys`'
//! `test-util` session. The events are decoded from the table (DE-D-2), as a reader that had the manifest would.
//!
//! # Why this is an integration test
//!
//! It starts a system-wide trace session and reads a file the operating system wrote, which a unit test in memory cannot.
//!
//! # Privilege, and why these are ignored
//!
//! Starting a session needs an elevated process or a user in the Performance Log Users group, which an ordinary developer
//! account is not. So these tests are ignored by default, and CI runs them with `--include-ignored` on a runner that is an
//! administrator. Run them by hand from an elevated shell:
//! `cargo test -p durable-ioring-events --test session -- --include-ignored`.
//!
//! # Serial
//!
//! A session records every event of a provider enabled in it, from any thread of the process, and these tests all use the one
//! provider, so they run one at a time.

#![cfg(windows)]

use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use durable_ioring::{
    Diagnostic, DiagnosticKind, DiagnosticSink, FlushDomain, FlushPart, IoTargetKey,
};
use durable_ioring_events::{
    ColumnType, EVENTS, EtwDiagnostics, EventSchema, INCONSISTENCIES, LOCKS, PROVIDER_ID, Value,
    encode, schema_of, schema_of_id,
};
use win_etw_sys::{Recorded, Session, level};

/// Held by each test: they share one provider, and a session records every event of it.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn session() -> Session {
    Session::start().expect("a trace session (see the privilege note in this file's documentation)")
}

/// A session with the provider enabled for everything.
fn listening() -> Session {
    let mut session = session();
    session
        .enable(PROVIDER_ID, level::VERBOSE, u64::MAX)
        .expect("enabled");
    session
}

/// Whether `condition` becomes true within a bound: a provider is told of a session after the call that started it returns.
fn becomes(condition: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Reads an event's data back into values, by the table, as a decoder with the manifest does.
fn decode(schema: &EventSchema, data: &[u8]) -> Vec<Value> {
    let mut at = 0;
    let mut values: Vec<Value> = Vec::new();
    for column in schema.columns {
        let take = |at: &mut usize, n: usize| {
            let bytes = &data[*at..*at + n];
            *at += n;
            bytes
        };
        let value = match column.ty {
            ColumnType::UInt32 => Value::UInt32(u32::from_ne_bytes(
                take(&mut at, 4).try_into().expect("four"),
            )),
            ColumnType::UInt64 => Value::UInt64(u64::from_ne_bytes(
                take(&mut at, 8).try_into().expect("eight"),
            )),
            ColumnType::Boolean => {
                Value::Boolean(u32::from_ne_bytes(take(&mut at, 4).try_into().expect("four")) != 0)
            }
            ColumnType::UnicodeString => {
                let mut units = Vec::new();
                loop {
                    let unit = u16::from_ne_bytes(take(&mut at, 2).try_into().expect("two"));
                    if unit == 0 {
                        break;
                    }
                    units.push(unit);
                }
                Value::Text(units)
            }
            ColumnType::Binary { length } => {
                let index = schema
                    .columns
                    .iter()
                    .position(|c| c.name == length)
                    .expect("a length column");
                let Value::UInt32(n) = values[index] else {
                    panic!("the length column is a UInt32")
                };
                Value::Bytes(take(&mut at, n as usize).to_vec())
            }
            _ => panic!("a column type the test does not know"),
        };
        values.push(value);
    }
    assert_eq!(
        at,
        data.len(),
        "{}: the fields did not use all of the data",
        schema.name
    );
    values
}

fn only(events: Vec<Recorded>) -> Recorded {
    assert_eq!(events.len(), 1, "{events:?}");
    events.into_iter().next().expect("one")
}

/// That a recorded event is the event reported: the schema by its id, the descriptor the table gives, and the values.
fn is_recorded_as(recorded: &Recorded, event: &Diagnostic<'_>) {
    let schema = schema_of(event).expect("an event the crate knows");
    assert_eq!(recorded.provider, PROVIDER_ID);
    assert_eq!(recorded.descriptor, schema.descriptor());
    assert_eq!(recorded.process_id, std::process::id());
    assert_eq!(
        decode(schema, &recorded.data),
        encode(event).expect("values")
    );
}

/// Counts how many times it is formatted.
struct Counted(Arc<AtomicU32>);

impl fmt::Display for Counted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fetch_add(1, Ordering::Relaxed);
        f.write_str("counted")
    }
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn every_event_reaches_a_session_as_the_values_it_was_reported_with() {
    let _serial = serial();
    let domain = FlushDomain::new(b"ssd-4".to_vec());
    let odd = FlushDomain::new(vec![0xff, 0, 0x61, 0x80]);
    let session = listening();
    let sink = EtwDiagnostics::new();
    assert!(becomes(|| sink.wants(DiagnosticKind::LockWait)));
    let events: [Diagnostic<'_>; 7] = [
        Diagnostic::Inconsistency {
            instance: 7,
            what: "a write completed twice",
            detail: format_args!("lineage {}", 3),
        },
        Diagnostic::LockWait {
            instance: 4,
            lock: "engine",
            waited: Duration::from_nanos(12),
        },
        Diagnostic::FlushIssued {
            instance: 2,
            lineage: 1,
            through: format_args!("{:?}", 5_u64),
            files: 3,
            domains: 1,
        },
        Diagnostic::FlushAnswered {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part: FlushPart::File(IoTargetKey(77)),
            elapsed: Duration::from_nanos(15),
            succeeded: false,
        },
        Diagnostic::FlushAnswered {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part: FlushPart::Domain(&domain),
            elapsed: Duration::from_nanos(40),
            succeeded: true,
        },
        Diagnostic::FlushAnswered {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part: FlushPart::Domain(&odd),
            elapsed: Duration::from_nanos(1),
            succeeded: true,
        },
        Diagnostic::SealWaited {
            instance: 2,
            lineage: 0,
            through: format_args!("1"),
            waited: Duration::from_nanos(30),
        },
    ];
    for event in &events {
        sink.report(event);
    }
    let recorded = session.stop().expect("stopped and read");
    assert_eq!(recorded.len(), events.len(), "{recorded:?}");
    for (recorded, event) in recorded.iter().zip(&events) {
        is_recorded_as(recorded, event);
    }
    // Each of the table's events was among them.
    for schema in &EVENTS {
        assert!(
            recorded.iter().any(|r| r.descriptor.id == schema.id),
            "{}",
            schema.name
        );
    }
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_session_with_one_keyword_is_asked_for_that_kind_and_no_other() {
    let _serial = serial();
    let mut session = session();
    session
        .enable(PROVIDER_ID, level::VERBOSE, LOCKS.mask)
        .expect("enabled");
    let sink = EtwDiagnostics::new();
    assert!(becomes(|| sink.wants(DiagnosticKind::LockWait)));
    for kind in [
        DiagnosticKind::FlushIssued,
        DiagnosticKind::FlushAnswered,
        DiagnosticKind::SealWaited,
        DiagnosticKind::Inconsistency,
    ] {
        assert!(!sink.wants(kind), "{kind:?}");
    }
    let formatted = Arc::new(AtomicU32::new(0));
    let through = Counted(formatted.clone());
    sink.report(&Diagnostic::FlushIssued {
        instance: 1,
        lineage: 0,
        through: format_args!("{through}"),
        files: 1,
        domains: 0,
    });
    assert_eq!(
        formatted.load(Ordering::Relaxed),
        0,
        "built for a keyword no session listens to"
    );
    sink.report(&Diagnostic::LockWait {
        instance: 1,
        lock: "engine",
        waited: Duration::from_nanos(5),
    });
    let event = only(session.stop().expect("stopped"));
    assert_eq!(event.descriptor.id, schema_of_id(2).expect("lock wait").id);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_session_at_the_error_level_is_asked_for_an_inconsistency_and_for_no_delay() {
    let _serial = serial();
    let mut session = session();
    session
        .enable(PROVIDER_ID, level::ERROR, u64::MAX)
        .expect("enabled");
    let sink = EtwDiagnostics::new();
    assert!(becomes(|| sink.wants(DiagnosticKind::Inconsistency)));
    for kind in [
        DiagnosticKind::LockWait,
        DiagnosticKind::FlushIssued,
        DiagnosticKind::FlushAnswered,
        DiagnosticKind::SealWaited,
    ] {
        assert!(!sink.wants(kind), "{kind:?}");
    }
    sink.report(&Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("y"),
    });
    sink.report(&Diagnostic::LockWait {
        instance: 1,
        lock: "engine",
        waited: Duration::ZERO,
    });
    let event = only(session.stop().expect("stopped"));
    assert_eq!(event.descriptor.keyword, INCONSISTENCIES.mask);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn text_is_built_once_for_an_event_a_session_is_listening_to() {
    let _serial = serial();
    let session = listening();
    let sink = EtwDiagnostics::new();
    assert!(becomes(|| sink.wants(DiagnosticKind::Inconsistency)));
    let formatted = Arc::new(AtomicU32::new(0));
    let detail = Counted(formatted.clone());
    sink.report(&Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("{detail}"),
    });
    assert_eq!(formatted.load(Ordering::Relaxed), 1);
    let _ = session.stop().expect("stopped");
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_sink_made_before_a_session_wants_events_once_one_listens_and_not_after_it_stops() {
    let _serial = serial();
    let sink = EtwDiagnostics::new();
    assert!(!sink.wants(DiagnosticKind::LockWait));
    let session = listening();
    assert!(
        becomes(|| sink.wants(DiagnosticKind::LockWait)),
        "never wanted while"
    );
    drop(session.stop().expect("stopped"));
    assert!(
        becomes(|| !sink.wants(DiagnosticKind::LockWait)),
        "still wanted after"
    );
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn two_sinks_each_write_through_the_one_registration_and_each_event_is_recorded_once() {
    let _serial = serial();
    let session = listening();
    let (a, b) = (EtwDiagnostics::new(), EtwDiagnostics::new());
    assert!(becomes(|| a.wants(DiagnosticKind::LockWait)));
    for (sink, instance) in [(&a, 1_u64), (&b, 2)] {
        sink.report(&Diagnostic::LockWait {
            instance,
            lock: "engine",
            waited: Duration::ZERO,
        });
    }
    let recorded = session.stop().expect("stopped");
    let schema = schema_of_id(2).expect("lock wait");
    let instances: Vec<Value> = recorded
        .iter()
        .map(|r| decode(schema, &r.data)[0].clone())
        .collect();
    assert_eq!(instances, [Value::UInt64(1), Value::UInt64(2)]);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn threads_report_through_one_sink_and_every_event_is_recorded() {
    let _serial = serial();
    let session = listening();
    let sink = EtwDiagnostics::new();
    assert!(becomes(|| sink.wants(DiagnosticKind::LockWait)));
    std::thread::scope(|scope| {
        for t in 0..4_u64 {
            let sink = &sink;
            scope.spawn(move || {
                for i in 0..25_u64 {
                    sink.report(&Diagnostic::LockWait {
                        instance: t,
                        lock: "engine",
                        waited: Duration::from_nanos(i),
                    });
                }
            });
        }
    });
    let schema = schema_of_id(2).expect("lock wait");
    let mut seen: Vec<(u64, u64)> = session
        .stop()
        .expect("stopped")
        .iter()
        .map(|r| {
            let values = decode(schema, &r.data);
            match (&values[0], &values[2]) {
                (Value::UInt64(t), Value::UInt64(i)) => (*t, *i),
                other => panic!("{other:?}"),
            }
        })
        .collect();
    seen.sort_unstable();
    let expected: Vec<(u64, u64)> = (0..4).flat_map(|t| (0..25).map(move |i| (t, i))).collect();
    assert_eq!(seen, expected);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_zero_in_text_is_written_whole_and_ends_the_string_for_a_reader() {
    let _serial = serial();
    let session = listening();
    let sink = EtwDiagnostics::new();
    assert!(becomes(|| sink.wants(DiagnosticKind::Inconsistency)));
    sink.report(&Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("ab\0cd"),
    });
    let event = only(session.stop().expect("stopped"));
    // Written whole: the data holds every unit, the zero and what follows it, then the terminator.
    let tail: Vec<u8> = "ab\0cd"
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_ne_bytes)
        .collect();
    assert!(event.data.ends_with(&tail), "{:?}", event.data);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn an_event_too_large_for_the_session_is_dropped_without_a_failure_and_the_next_is_recorded() {
    let _serial = serial();
    let session = listening();
    let sink = EtwDiagnostics::new();
    assert!(becomes(|| sink.wants(DiagnosticKind::Inconsistency)));
    let huge = "x".repeat(70_000);
    sink.report(&Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("{huge}"),
    });
    sink.report(&Diagnostic::LockWait {
        instance: 9,
        lock: "engine",
        waited: Duration::ZERO,
    });
    let event = only(session.stop().expect("stopped"));
    assert_eq!(event.descriptor.id, 2);
}
