// Copyright (c) Mike Grier.

//! The completion-port probe's machine-readable row (`M8.12`).

use super::*;
use crate::ioring::NO_USABLE_IORING;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-completion-port";

/// The row a report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md: the fill
/// byte and the length every read asks for, then each read case by name with
/// its result code, the byte count its completion reported and the first byte
/// that landed, and whether the associated handle still completed a read
/// through the port. The result code is an HRESULT, published as `0x`
/// hexadecimal text, as the `IoRing` probe's row does.
///
/// A host with no usable `IoRing` emits the same row with `declined` naming
/// why and no reads, following topology's `report_unmeasured`.
///
/// Not pass or fail, valid, forecloses or coexists; each is a comparison over
/// the figures here.
#[must_use]
pub fn raw_row(support: &IoRingSupport<CompletionPortFinding>, build: &str) -> Row {
    let row = Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("fill", FILL)
        .with("read_len", FIXTURE_LEN);
    let IoRingSupport::Measured(finding) = support else {
        return row.with("declined", NO_USABLE_IORING);
    };
    row.with("declined", Value::Null)
        .with(
            "reads",
            Value::Object(vec![
                ("control_unassociated", read(finding.control_unassociated)),
                (
                    "after_iocp_association",
                    read(finding.after_iocp_association),
                ),
                (
                    "before_late_association",
                    read(finding.before_late_association),
                ),
                (
                    "after_late_association",
                    read(finding.after_late_association),
                ),
                ("before_threadpool_io", read(finding.before_threadpool_io)),
                ("after_threadpool_io", read(finding.after_threadpool_io)),
            ]),
        )
        .with("port_still_works", finding.port_still_works)
}

fn read(attempt: ReadAttempt) -> Value {
    Value::Object(vec![
        (
            "result_code",
            Value::from(format!("{:#010x}", attempt.result_code)),
        ),
        ("bytes", Value::from(attempt.bytes)),
        ("first_byte", Value::from(attempt.first_byte)),
    ])
}
