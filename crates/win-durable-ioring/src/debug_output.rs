// Copyright (c) 2026 Mike Grier
//! A diagnostic sink that writes each event to the debugger (DI-12.1.3, DI-D-51).

use durable_ioring::{Diagnostic, DiagnosticKind, DiagnosticSink, LINE_PREFIX};
use win_debugging_sys::output_debug_str;

#[cfg(test)]
mod tests;

/// A sink that writes every event as a line to the debugger, by `OutputDebugStringW`, prefixed `dioring:` as
/// [`StderrDiagnostics`](durable_ioring::StderrDiagnostics)'s lines are.
///
/// It wants every kind. A line is built for each event, whether or not a debugger is attached: with neither a
/// debugger nor an active system debugger the call "does nothing", and it gives no way to tell, so a consumer
/// that does not want the cost chooses another sink, or a [`FanOutDiagnostics`](durable_ioring::FanOutDiagnostics)
/// that leaves this one out.
///
/// The call takes a NUL-terminated string, so a NUL in an event's text, which a domain's name or an inconsistency's
/// detail can carry, is written as the two characters `\0` and the line is sent. Nothing else in the text is
/// changed. The call converts the line to the system's code page, and "some Unicode characters may not be
/// displayed correctly".
#[derive(Clone, Copy, Debug, Default)]
pub struct DebugOutputDiagnostics;

impl DiagnosticSink for DebugOutputDiagnostics {
    fn wants(&self, _: DiagnosticKind) -> bool {
        true
    }

    fn report(&self, event: &Diagnostic<'_>) {
        let sent = output_debug_str(&line(event));
        // The line has no NUL, which is the call's one refusal, so it is made.
        debug_assert!(sent.is_ok(), "the line had a NUL: {sent:?}");
    }
}

/// The line an event is written as: the prefix, the event's text, and no NUL.
fn line(event: &Diagnostic<'_>) -> String {
    format!("{LINE_PREFIX}{event}").replace('\0', "\\0")
}
