// Copyright (c) 2026 Mike Grier
//! What a provider's events are when a session records them (WE-D-3): this test starts a real trace session logging to a
//! file, enables a provider in it, writes events through a `Registration`, stops the session, and reads the file back,
//! with the crate's `test-util` session, which uses only documented functions.
//!
//! # Why this is an integration test
//!
//! It starts a system-wide trace session and reads a file the operating system wrote: it crosses the boundary into the
//! operating system's tracing, which a unit test in memory cannot.
//!
//! # Privilege, and why these are ignored
//!
//! Starting a session needs an elevated process, or a user in the Performance Log Users group, which an ordinary
//! developer account is not. So these tests are ignored by default and a plain `cargo test` reports the environment
//! limitation as ignored and not as a defect, and CI runs them with `--include-ignored` on a runner that is an
//! administrator, where a failure is a result. Run them by hand from an elevated shell:
//! `cargo test -p win-etw-sys --test session -- --include-ignored`.

#![cfg(windows)]

use win_error_codes::Win32Error;
use win_etw_sys::{
    EventDescriptor, Field, Guid, Recorded, Registration, Session, WriteError, level,
};

/// A provider no other test of this binary uses: tests run in parallel, and a session records every event of a provider it
/// has enabled, so providers are not shared.
const fn provider(n: u128) -> Guid {
    Guid::from_u128(0x3c0d_5f12_8a44_49b7_b2e1_0000_0000_0000 | n)
}

const INFO_EVENT: EventDescriptor = EventDescriptor {
    id: 10,
    version: 2,
    channel: 0,
    level: level::INFO,
    opcode: 0,
    task: 3,
    keyword: 0x4,
};

fn session() -> Session {
    Session::start().expect("a trace session (see the privilege note in this file's documentation)")
}

/// Whether `condition` becomes true within a bound: the provider's state follows a session's change, and is not set by the
/// time the call that made the change returns.
fn becomes(condition: impl Fn() -> bool) -> bool {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(10) {
        if condition() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

fn only(events: Vec<Recorded>) -> Recorded {
    assert_eq!(events.len(), 1, "{events:?}");
    events.into_iter().next().expect("one")
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn an_event_written_to_an_enabled_provider_is_recorded_with_its_descriptor_and_data() {
    let id = provider(1);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    registration
        .write(&INFO_EVENT, &[Field::U32(0xdead_beef), Field::U8(7)])
        .expect("written");
    drop(registration);
    let event = only(session.stop().expect("stopped and read"));
    assert_eq!(event.provider, id);
    assert_eq!(event.descriptor, INFO_EVENT);
    assert_eq!(
        event.data,
        [0xdead_beef_u32.to_ne_bytes().as_slice(), &[7]].concat()
    );
    assert_eq!(event.process_id, std::process::id());
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_provider_is_enabled_while_a_session_listens_and_not_before_or_after() {
    let id = provider(2);
    let registration = Registration::register(id).expect("registered");
    assert!(!registration.is_enabled(&INFO_EVENT), "before");
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    // Observed: a provider that was registered before a session enabled it is told so afterwards, not at the moment the
    // call returns, so the state is waited for.
    assert!(
        becomes(|| registration.is_enabled(&INFO_EVENT)),
        "never enabled while"
    );
    assert!(registration.is_level_enabled(level::INFO, 0x4));
    drop(session.stop().expect("stopped"));
    assert!(
        becomes(|| !registration.is_enabled(&INFO_EVENT)),
        "never disabled after"
    );
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_session_at_a_level_does_not_enable_events_above_it() {
    let id = provider(3);
    let mut session = session();
    session.enable(id, level::WARNING, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    assert!(registration.is_level_enabled(level::ERROR, 0x4));
    assert!(registration.is_level_enabled(level::WARNING, 0x4));
    assert!(!registration.is_level_enabled(level::INFO, 0x4));
    assert!(!registration.is_level_enabled(level::VERBOSE, 0x4));
    let warning = EventDescriptor {
        level: level::WARNING,
        ..INFO_EVENT
    };
    registration
        .write(&INFO_EVENT, &[Field::U8(1)])
        .expect("written");
    registration
        .write(&warning, &[Field::U8(2)])
        .expect("written");
    drop(registration);
    let event = only(session.stop().expect("stopped"));
    assert_eq!(event.data, [2]);
    assert_eq!(event.descriptor.level, level::WARNING);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_session_with_a_keyword_records_only_events_that_share_a_bit_with_it() {
    let id = provider(4);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0x2).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    let wanted = EventDescriptor {
        keyword: 0x3,
        ..INFO_EVENT
    };
    let other = EventDescriptor {
        keyword: 0x4,
        ..INFO_EVENT
    };
    assert!(registration.is_enabled(&wanted));
    assert!(!registration.is_enabled(&other));
    registration
        .write(&other, &[Field::U8(1)])
        .expect("written");
    registration
        .write(&wanted, &[Field::U8(2)])
        .expect("written");
    drop(registration);
    assert_eq!(only(session.stop().expect("stopped")).data, [2]);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_provider_that_was_not_enabled_is_not_recorded() {
    let (enabled, other) = (provider(5), provider(6));
    let mut session = session();
    session
        .enable(enabled, level::VERBOSE, 0xff)
        .expect("enabled");
    let a = Registration::register(enabled).expect("registered");
    let b = Registration::register(other).expect("registered");
    assert!(!b.is_enabled(&INFO_EVENT));
    b.write(&INFO_EVENT, &[Field::U8(9)]).expect("written");
    a.write(&INFO_EVENT, &[Field::U8(1)]).expect("written");
    drop((a, b));
    let event = only(session.stop().expect("stopped"));
    assert_eq!(event.provider, enabled);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn events_are_recorded_in_the_order_they_were_written() {
    let id = provider(7);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    for n in 0..50_u32 {
        registration
            .write(&INFO_EVENT, &[Field::U32(n)])
            .expect("written");
    }
    drop(registration);
    let events = session.stop().expect("stopped");
    let seen: Vec<u32> = events
        .iter()
        .map(|e| u32::from_ne_bytes(e.data[..4].try_into().expect("four bytes")))
        .collect();
    assert_eq!(seen, (0..50).collect::<Vec<_>>());
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn every_kind_of_field_is_recorded_as_the_bytes_it_was_written_as() {
    let id = provider(8);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    let guid = Guid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
    let text = [0x68_u16, 0x69];
    let fields = [
        Field::U8(1),
        Field::I8(-1),
        Field::U16(2),
        Field::I16(-2),
        Field::U32(3),
        Field::I32(-3),
        Field::U64(4),
        Field::I64(-4),
        Field::F32(1.5),
        Field::F64(2.5),
        Field::boolean(true),
        Field::Pointer(0x10),
        Field::Guid(guid),
        Field::Bytes(&[9, 8]),
        Field::Utf16(&text),
        Field::Utf16Terminated(&text),
        Field::BytesTerminated(b"ok"),
    ];
    registration.write(&INFO_EVENT, &fields).expect("written");
    drop(registration);
    let mut expected = vec![1, 0xff];
    expected.extend(2_u16.to_ne_bytes());
    expected.extend((-2_i16).to_ne_bytes());
    expected.extend(3_u32.to_ne_bytes());
    expected.extend((-3_i32).to_ne_bytes());
    expected.extend(4_u64.to_ne_bytes());
    expected.extend((-4_i64).to_ne_bytes());
    expected.extend(1.5_f32.to_ne_bytes());
    expected.extend(2.5_f64.to_ne_bytes());
    expected.extend(1_u32.to_ne_bytes());
    expected.extend(0x10_usize.to_ne_bytes());
    expected.extend(0x1234_5678_u32.to_ne_bytes());
    expected.extend(0x9abc_u16.to_ne_bytes());
    expected.extend(0xdef0_u16.to_ne_bytes());
    expected.extend([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    expected.extend([9, 8]);
    for unit in text {
        expected.extend(unit.to_ne_bytes());
    }
    for unit in text {
        expected.extend(unit.to_ne_bytes());
    }
    expected.extend([0, 0]);
    expected.extend([b'o', b'k', 0]);
    assert_eq!(only(session.stop().expect("stopped")).data, expected);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn an_event_with_no_fields_is_recorded_with_no_data() {
    let id = provider(9);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    registration.write(&INFO_EVENT, &[]).expect("written");
    drop(registration);
    assert!(only(session.stop().expect("stopped")).data.is_empty());
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn two_sessions_each_record_the_same_event() {
    let id = provider(10);
    let (mut a, mut b) = (session(), session());
    a.enable(id, level::VERBOSE, 0xff).expect("enabled");
    b.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    registration
        .write(&INFO_EVENT, &[Field::U8(5)])
        .expect("written");
    drop(registration);
    assert_eq!(only(a.stop().expect("stopped")).data, [5]);
    assert_eq!(only(b.stop().expect("stopped")).data, [5]);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn threads_write_through_one_registration_and_every_event_is_recorded() {
    let id = provider(11);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    std::thread::scope(|scope| {
        for t in 0..4_u8 {
            let registration = &registration;
            scope.spawn(move || {
                for i in 0..25_u8 {
                    registration
                        .write(&INFO_EVENT, &[Field::U8(t), Field::U8(i)])
                        .expect("written");
                }
            });
        }
    });
    drop(registration);
    let mut seen: Vec<(u8, u8)> = session
        .stop()
        .expect("stopped")
        .iter()
        .map(|e| (e.data[0], e.data[1]))
        .collect();
    seen.sort_unstable();
    let expected: Vec<(u8, u8)> = (0..4).flat_map(|t| (0..25).map(move |i| (t, i))).collect();
    assert_eq!(seen, expected);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn an_event_larger_than_the_page_allows_is_the_calls_failure_and_is_not_recorded() {
    // The page lists `ERROR_ARITHMETIC_OVERFLOW` for "The event size is larger than the allowed maximum (64KB - header)" and
    // `ERROR_MORE_DATA` for "The session buffer size is too small for the event". The session's buffers are 64 KB, so
    // either fits; which the call gave is what was observed.
    let id = provider(12);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    let big = vec![0_u8; 70_000];
    let result = registration.write(&INFO_EVENT, &[Field::Bytes(&big)]);
    assert!(matches!(result, Err(WriteError::Call(_))), "{result:?}");
    if let Err(WriteError::Call(code)) = result {
        const ERROR_MORE_DATA: u32 = 234;
        const ERROR_ARITHMETIC_OVERFLOW: u32 = 534;
        assert!(
            [ERROR_MORE_DATA, ERROR_ARITHMETIC_OVERFLOW].contains(&code.code()),
            "{code}"
        );
    }
    drop(registration);
    assert!(session.stop().expect("stopped").is_empty());
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn more_blocks_than_the_page_gives_as_the_maximum_are_the_calls_to_judge() {
    // The page gives `UserDataCount`'s maximum as 128 and this crate passes however many there are. What the call
    // made of 129 blocks on the machine this ran on is what is asserted: evidence, and not a contract.
    let id = provider(13);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    let fields = vec![Field::U8(1); 129];
    let result = registration.write(&INFO_EVENT, &fields);
    // `ERROR_INVALID_PARAMETER` is 87, which the page lists for "One or more of the parameters is not valid".
    assert_eq!(result, Err(WriteError::Call(Win32Error::from_code(87))));
    drop(registration);
    assert!(session.stop().expect("stopped").is_empty());
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_hundred_and_twenty_eight_blocks_are_one_event_of_that_many_bytes() {
    let id = provider(14);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    let fields: Vec<Field<'_>> = (0..128_u8).map(Field::U8).collect();
    registration.write(&INFO_EVENT, &fields).expect("written");
    drop(registration);
    assert_eq!(
        only(session.stop().expect("stopped")).data,
        (0..128).collect::<Vec<u8>>()
    );
}

/// A pair's two events, as a provider that pairs them writes them: a start and a stop, opcodes 1 and 2.
const START_EVENT: EventDescriptor = EventDescriptor {
    opcode: 1,
    ..INFO_EVENT
};
const STOP_EVENT: EventDescriptor = EventDescriptor {
    opcode: 2,
    id: 11,
    ..INFO_EVENT
};

const ACTIVITY: Guid = Guid::from_u128(0x0a11_0000_0000_4000_8000_0000_0000_0001);
const RELATED: Guid = Guid::from_u128(0x0a11_0000_0000_4000_8000_0000_0000_0002);

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_begin_and_its_end_written_on_two_threads_carry_the_activity_the_caller_gave() {
    let id = provider(30);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    let (began, ended) = std::thread::scope(|scope| {
        let began = scope
            .spawn(|| {
                registration
                    .write_transfer(
                        &START_EVENT,
                        Some(&ACTIVITY),
                        Some(&RELATED),
                        &[Field::U8(1)],
                    )
                    .expect("the begin");
                std::thread::current().id()
            })
            .join()
            .expect("the first thread");
        let ended = scope
            .spawn(|| {
                registration
                    .write_transfer(&STOP_EVENT, Some(&ACTIVITY), None, &[Field::U8(2)])
                    .expect("the end");
                std::thread::current().id()
            })
            .join()
            .expect("the second thread");
        (began, ended)
    });
    assert_ne!(began, ended, "two threads");
    drop(registration);
    let events = session.stop().expect("stopped and read");
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(
        events
            .iter()
            .map(|e| (
                e.descriptor.opcode,
                e.activity,
                e.related_activity,
                e.data[0]
            ))
            .collect::<Vec<_>>(),
        [(1, ACTIVITY, Some(RELATED), 1), (2, ACTIVITY, None, 2)]
    );
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn an_activity_given_is_recorded_and_the_writing_threads_own_is_left_as_it_was() {
    let id = provider(31);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    registration
        .write(&INFO_EVENT, &[Field::U8(1)])
        .expect("before");
    registration
        .write_transfer(&INFO_EVENT, Some(&ACTIVITY), None, &[Field::U8(2)])
        .expect("given");
    registration
        .write(&INFO_EVENT, &[Field::U8(3)])
        .expect("after");
    drop(registration);
    let events = session.stop().expect("stopped and read");
    assert_eq!(events.len(), 3, "{events:?}");
    let thread_own = events[0].activity;
    assert_ne!(thread_own, ACTIVITY);
    assert_eq!(
        events.iter().map(|e| e.activity).collect::<Vec<_>>(),
        [thread_own, ACTIVITY, thread_own]
    );
    assert!(
        events.iter().all(|e| e.related_activity.is_none()),
        "{events:?}"
    );
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn no_activity_given_is_the_threads_as_a_plain_write_takes_it() {
    let id = provider(32);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    registration.write(&INFO_EVENT, &[]).expect("plain");
    registration
        .write_transfer(&INFO_EVENT, None, None, &[])
        .expect("transfer");
    drop(registration);
    let events = session.stop().expect("stopped and read");
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0].activity, events[1].activity);
    assert_eq!(events[1].related_activity, None);
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn a_related_activity_is_recorded_without_an_activity_given() {
    let id = provider(33);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    registration.write(&INFO_EVENT, &[]).expect("plain");
    registration
        .write_transfer(&INFO_EVENT, None, Some(&RELATED), &[])
        .expect("transfer");
    drop(registration);
    let events = session.stop().expect("stopped and read");
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[1].activity, events[0].activity, "the thread's");
    assert_eq!(events[1].related_activity, Some(RELATED));
}

#[test]
#[ignore = "starts a trace session, which needs an elevated process or the Performance Log Users group: run with --include-ignored, as CI does"]
fn threads_writing_activities_of_their_own_each_record_theirs() {
    let id = provider(34);
    let mut session = session();
    session.enable(id, level::VERBOSE, 0xff).expect("enabled");
    let registration = Registration::register(id).expect("registered");
    std::thread::scope(|scope| {
        for t in 0..4_u8 {
            let registration = &registration;
            scope.spawn(move || {
                let activity =
                    Guid::from_u128(0x0a11_0000_0000_4000_8000_0000_0000_0100 + u128::from(t));
                for i in 0..25_u8 {
                    registration
                        .write_transfer(
                            &INFO_EVENT,
                            Some(&activity),
                            None,
                            &[Field::U8(t), Field::U8(i)],
                        )
                        .expect("written");
                }
            });
        }
    });
    drop(registration);
    let events = session.stop().expect("stopped and read");
    assert_eq!(events.len(), 100, "every event");
    for event in &events {
        let expected =
            Guid::from_u128(0x0a11_0000_0000_4000_8000_0000_0000_0100 + u128::from(event.data[0]));
        assert_eq!(event.activity, expected, "{event:?}");
    }
}
