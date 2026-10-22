// Copyright (c) 2026 Mike Grier
//! The call's failure to register (WE-D-2): a process that keeps registering is eventually refused, and the refusal is the
//! call's code.
//!
//! The page says "Each process can register up to 1,024 providers". On the machine this was written on the call allowed
//! about twice that and then refused with `ERROR_OUTOFMEMORY`, so this test asserts no number: the page's limit is not a
//! contract this crate can check ahead of the call, and a measurement is documentation and never a check. What is asserted
//! is that the failure is reached, that it arrives as the call's code, that what was registered still works, and that
//! letting it go makes room again.
//!
//! # Why this is an integration test
//!
//! It takes every registration a process may have, which another test in the same process would find gone, so it needs a
//! process of its own, which an integration test file is.

#![cfg(windows)]

use win_etw_sys::{EventDescriptor, Guid, Registration};

/// Far more than the page's limit or what was observed, so that a process that is refused at all is refused here.
const ATTEMPTS: u128 = 200_000;

#[test]
fn a_process_that_keeps_registering_is_refused_with_the_calls_code_and_recovers() {
    let mut held = Vec::new();
    let mut refused = None;
    for n in 0..ATTEMPTS {
        match Registration::register(Guid::from_u128(
            0x5b21_c0de_0001_4e8a_8f10_0000_0000_0000 | n,
        )) {
            Ok(registration) => held.push(registration),
            Err(code) => {
                refused = Some(code);
                break;
            }
        }
    }
    let code = refused.expect("the process was refused within the attempts");
    assert!(!code.is_success());
    assert!(
        !held.is_empty(),
        "nothing was registered before the refusal"
    );

    // What was held is still good, and letting it go makes room again.
    let event = EventDescriptor::default();
    assert_eq!(held[0].write(&event, &[]), Ok(()));
    assert_eq!(held[held.len() - 1].write(&event, &[]), Ok(()));
    drop(held);
    Registration::register(Guid::from_u128(1)).expect("room again once they are dropped");
}
