// Copyright (c) 2026 Mike Grier
//! The Windows path's mapping of a completion to an outcome (DI-D-50).

use std::io;

use super::op_outcome;
use crate::error_code::{ErrorCode, WINDOWS_IORING};
use crate::ids::DioringIds;
use crate::types::Outcome;

type O = Outcome<DioringIds<u64>>;

/// A write that transferred every byte it asked for completed.
#[test]
fn a_full_write_is_transferred() {
    assert!(matches!(
        op_outcome::<DioringIds<u64>>(Ok(8), true, 8),
        O::Transferred(8)
    ));
}

/// A write that transferred fewer bytes than it asked for is a fault under the `Fault` policy: a
/// target never completes a write short, and no layer gave a code for it.
#[test]
fn a_short_write_is_a_fault_with_no_code() {
    let O::Failed(fault) = op_outcome::<DioringIds<u64>>(Ok(3), true, 8) else {
        panic!("a short write is a fault");
    };
    assert_eq!(fault.source, WINDOWS_IORING);
    assert_eq!(fault.code, None);
    assert!(
        fault.error.to_string().contains("3 of 8"),
        "{}",
        fault.error
    );
}

/// A write that transferred nothing is short too.
#[test]
fn a_write_that_transferred_nothing_is_a_fault() {
    assert!(matches!(
        op_outcome::<DioringIds<u64>>(Ok(0), true, 8),
        O::Failed(_)
    ));
}

/// A read may complete short: the obligation is a write's.
#[test]
fn a_short_read_is_transferred() {
    assert!(matches!(
        op_outcome::<DioringIds<u64>>(Ok(3), false, 0),
        O::Transferred(3)
    ));
}

/// A failed completion is the ring's fault, with the code it carries.
#[test]
fn a_failed_completion_is_the_rings_fault_with_its_code() {
    let O::Failed(fault) =
        op_outcome::<DioringIds<u64>>(Err(io::Error::from_raw_os_error(1117)), true, 8)
    else {
        panic!("a failed completion is a fault");
    };
    assert_eq!(fault.source, WINDOWS_IORING);
    assert_eq!(fault.code, Some(ErrorCode::from_win32(1117)));
}

/// A failed completion with no code relays the error with none: the code is optional (DI-D-50).
#[test]
fn a_failure_with_no_code_is_relayed_without_one() {
    let O::Failed(fault) = op_outcome::<DioringIds<u64>>(Err(io::Error::other("no code")), true, 8)
    else {
        panic!("a failed completion is a fault");
    };
    assert_eq!(fault.code, None);
}

/// A transfer longer than a `u32` is counted as `u32::MAX` rather than wrapping.
#[test]
fn a_transfer_beyond_u32_saturates() {
    let huge = usize::try_from(u64::from(u32::MAX) + 1).expect("a 64-bit target");
    assert!(matches!(
        op_outcome::<DioringIds<u64>>(Ok(huge), false, 0),
        O::Transferred(u32::MAX)
    ));
}
