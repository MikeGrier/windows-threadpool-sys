// Copyright (c) 2026 Mike Grier
//! A registration with no session listening (WE-D-2): what the pages say happens, which is what a process that no one
//! traces sees. Events that reach a session are `tests/session.rs`, and the call's failure is `tests/limit.rs`, which
//! needs a process of its own.

use std::thread;

use super::{Registration, WriteError};
use crate::{EventDescriptor, Field, Guid, level};
use win_error_codes::Win32Error;

/// A provider no session has been told of, one for each test, so that tests in one process do not share an id.
const fn provider(n: u128) -> Guid {
    Guid::from_u128(0x7a3c_91e0_5d24_4b8f_a1c6_0000_0000_0000 | n)
}

fn event() -> EventDescriptor {
    EventDescriptor {
        id: 1,
        level: level::INFO,
        keyword: 1,
        ..EventDescriptor::default()
    }
}

#[test]
fn a_provider_is_registered_and_unregistered() {
    let registration = Registration::register(provider(1)).expect("registered");
    assert_eq!(registration.unregister(), Ok(()));
}

#[test]
fn dropping_a_registration_unregisters_it_so_the_provider_can_be_registered_again() {
    for _ in 0..20 {
        let registration = Registration::register(provider(2)).expect("registered");
        drop(registration);
    }
}

#[test]
fn a_provider_registered_twice_is_two_registrations() {
    let a = Registration::register(provider(3)).expect("registered");
    let b = Registration::register(provider(3)).expect("registered again");
    assert_ne!(a.handle, b.handle);
    drop(a);
    assert_eq!(b.write(&event(), &[]), Ok(()));
}

#[test]
fn several_providers_are_registered_at_once() {
    let all: Vec<Registration> = (0..40)
        .map(|n| Registration::register(provider(100 + n)).expect("registered"))
        .collect();
    let mut handles: Vec<i64> = all.iter().map(|r| r.handle).collect();
    handles.sort_unstable();
    handles.dedup();
    assert_eq!(handles.len(), 40);
}

#[test]
fn a_registration_is_a_nonzero_handle() {
    let registration = Registration::register(provider(4)).expect("registered");
    assert_ne!(registration.handle, 0);
}

#[test]
fn with_no_session_nothing_is_enabled() {
    // Observed: the page says false "if ETW can quickly determine that no session is listening", and that true can be
    // returned where it cannot, so this is evidence of what a provider no session has enabled reports.
    let registration = Registration::register(provider(5)).expect("registered");
    assert!(!registration.is_enabled(&event()));
    assert!(!registration.is_level_enabled(level::INFO, 1));
    assert!(!registration.is_level_enabled(level::LOG_ALWAYS, u64::MAX));
    assert!(!registration.is_level_enabled(level::VERBOSE, 0));
}

#[test]
fn with_no_session_a_write_does_nothing_and_succeeds() {
    let registration = Registration::register(provider(6)).expect("registered");
    assert_eq!(registration.write(&event(), &[]), Ok(()));
    assert_eq!(
        registration.write(
            &event(),
            &[Field::U64(1), Field::Utf16Terminated(&[0x68, 0x69])]
        ),
        Ok(())
    );
}

#[test]
fn every_kind_of_field_is_written_with_no_failure() {
    let registration = Registration::register(provider(7)).expect("registered");
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
        Field::Guid(Guid::from_u128(9)),
        Field::Bytes(&[1, 2]),
        Field::Utf16(&text),
        Field::Utf16Terminated(&text),
        Field::BytesTerminated(b"ok"),
    ];
    assert_eq!(registration.write(&event(), &fields), Ok(()));
}

#[test]
fn a_registration_is_shared_between_threads_that_write_through_it() {
    fn shares<T: Send + Sync>() {}
    shares::<Registration>();
    let registration = Registration::register(provider(8)).expect("registered");
    thread::scope(|scope| {
        for n in 0..8_u32 {
            let registration = &registration;
            scope.spawn(move || {
                for i in 0..50_u32 {
                    registration
                        .write(&event(), &[Field::U32(n), Field::U32(i)])
                        .expect("written");
                    let _ = registration.is_enabled(&event());
                }
            });
        }
    });
}

#[test]
fn a_registration_moves_between_threads_and_is_dropped_there() {
    let registration = Registration::register(provider(9)).expect("registered");
    thread::spawn(move || {
        assert_eq!(registration.write(&event(), &[]), Ok(()));
    })
    .join()
    .expect("the thread finished");
}

#[test]
fn a_write_with_a_zero_handle_would_do_nothing_but_the_type_never_holds_one() {
    // A registration is only made from a successful call, whose handle the page says is nonzero, and `unregister`
    // takes the handle out of the value it consumes.
    let registration = Registration::register(provider(10)).expect("registered");
    assert_ne!(registration.handle, 0);
}

#[test]
fn a_refusal_names_the_field_and_is_not_the_calls() {
    let error = WriteError::BlockTooLong { field: 2, len: 5 };
    assert_eq!(
        error.to_string(),
        "field 2 is 5 bytes, more than an EVENT_DATA_DESCRIPTOR's size, a ULONG, can hold"
    );
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn too_many_blocks_reads_as_a_sentence_with_no_source() {
    let error = WriteError::TooManyBlocks { count: 7 };
    assert_eq!(
        error.to_string(),
        "the fields are 7 blocks, more than EventWrite's count, a ULONG, can hold"
    );
    assert!(std::error::Error::source(&error).is_none());
}

#[test]
fn a_calls_failure_is_its_code_and_is_the_errors_source() {
    let error = WriteError::Call(Win32Error::from_code(87));
    assert!(error.to_string().starts_with("EventWrite failed: "));
    let source = std::error::Error::source(&error).expect("the call's code");
    assert_eq!(source.to_string(), Win32Error::from_code(87).to_string());
}
