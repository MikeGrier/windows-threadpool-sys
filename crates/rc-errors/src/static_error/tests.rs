// Copyright (c) 2026 Mike Grier

use super::StaticError;
use alloc::format;
use alloc::string::ToString;
use core::error::Error;

static REFUSED: StaticError = StaticError::new("refused");
const AT_CAPACITY: StaticError = StaticError::new("at capacity");

const fn assert_traits<T: Copy + Eq + Send + Sync + core::fmt::Debug + 'static>() {}
const _: () = assert_traits::<StaticError>();
const _: () = assert!(AT_CAPACITY.message().len() == 11);

#[test]
fn it_is_made_in_a_static_and_in_a_const() {
    assert_eq!(REFUSED.message(), "refused");
    assert_eq!(AT_CAPACITY.message(), "at capacity");
}

#[test]
fn it_displays_its_message() {
    assert_eq!(REFUSED.to_string(), "refused");
    assert_eq!(format!("{AT_CAPACITY}"), "at capacity");
}

#[test]
fn it_debugs_as_its_message_field() {
    assert_eq!(
        format!("{REFUSED:?}"),
        "StaticError { message: \"refused\" }"
    );
}

#[test]
fn it_has_no_source() {
    assert!(REFUSED.source().is_none());
}

#[test]
fn its_own_equality_compares_messages() {
    assert_eq!(StaticError::new("same"), StaticError::new("same"));
    assert_ne!(StaticError::new("one"), StaticError::new("other"));
}

#[test]
fn an_empty_message_is_allowed() {
    let empty = StaticError::new("");
    assert_eq!(empty.to_string(), "");
}

#[test]
fn it_is_reached_as_a_static_trait_object() {
    let error: &'static (dyn Error + Send + Sync) = &REFUSED;
    assert_eq!(error.to_string(), "refused");
    assert_eq!(error.downcast_ref::<StaticError>(), Some(&REFUSED));
}
