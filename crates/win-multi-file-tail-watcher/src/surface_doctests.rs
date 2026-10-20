// Copyright (c) 2026 Mike Grier
//! Documentation examples that exist only to be run: each shows a method of the watcher that the
//! reading layer needs and that is not part of the crate's public surface (TW-9.4).
//!
//! Each example is written to compile if the method were public, with valid arguments, so a method
//! made public by mistake turns its example from a refusal into a success and fails it. They are
//! compiled only for `cargo test --doc`.

/// `Watcher::register_with` lets the reading layer attach what a read needs to a registration. A
/// caller registers through the reading layer, or through `Watcher::register`.
///
/// ```compile_fail,E0624
/// use win_multi_file_tail_watcher::Watcher;
/// use wtf_string::Wtf16String;
///
/// let watcher = Watcher::<u8>::new(8).unwrap();
/// let path = Wtf16String::from(std::ffi::OsStr::new("a.log"));
/// let _ = watcher.register_with(&path, 0, |_, _| 0u8);
/// ```
struct RegisterWithStaysInternal;

/// `Watcher::stop_handle` lets the reading layer request shutdown from where it holds only the
/// handle. A caller shuts a watcher down with `Watcher::shutdown`.
///
/// ```compile_fail,E0624
/// use win_multi_file_tail_watcher::Watcher;
///
/// let watcher = Watcher::<u8>::new(8).unwrap();
/// let _ = watcher.stop_handle();
/// ```
struct StopHandleStaysInternal;

/// `Watcher::mark_terminal` is how the reading layer records that a read, and not a probe, found a
/// registration can never be read again. A caller is told by a terminal entry.
///
/// ```compile_fail,E0624
/// use win_multi_file_tail_watcher::Watcher;
///
/// let watcher = Watcher::<u8>::new(8).unwrap();
/// let _ = watcher.mark_terminal(0, 6);
/// ```
struct MarkTerminalStaysInternal;

/// `Watcher::diagnose` is how the reading layer reports through the watcher's sink. A caller
/// supplies the sink and receives what it is given.
///
/// ```compile_fail,E0624
/// use win_multi_file_tail_watcher::{Diagnostic, DiagnosticKind, Watcher};
///
/// let watcher = Watcher::<u8>::new(8).unwrap();
/// watcher.diagnose(DiagnosticKind::ProbeFailed, || Diagnostic::ProbeFailed {
///     registration: 0,
///     win32_code: None,
/// });
/// ```
struct DiagnoseStaysInternal;
