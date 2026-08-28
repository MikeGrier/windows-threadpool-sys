// Copyright (c) Mike Grier.

//! Owned, marshalable parameter sets for synchronous Win32 namespace calls.
//!
//! Win32's namespace and metadata surface -- opening, querying, closing -- is
//! synchronous-only. A call that blocks on a dead network path blocks the thread
//! that made it, and no overlapped form exists. This crate makes such a call
//! **capturable as a value**: an owned parameter set built on one thread and
//! executed faithfully on another.
//!
//! It schedules nothing. It is the catalogue-plus-faithful-execution layer,
//! testable with no ring, no pool, and no async anywhere near it.
//!
//! # What a request is, and is not
//!
//! A request carries call **parameters**. It does not carry the impersonation
//! token or any other thread-scoped state the call runs under -- that belongs to
//! [`windows-thread-ambient-sys`][ambient], which this crate does not depend on.
//! The two are siblings rather than a stack: a request can be executed with no
//! captured context at all, and a context is useful to work that never opens a
//! file. Whoever owns both pairs them at the submission site.
//!
//! A request also chooses **no delivery model**. An opened handle comes back
//! plain and unassociated, because associating it with a completion port
//! irreversibly forecloses `IoRing` use, and that choice belongs to a layer that
//! knows the handle's destination.
//!
//! [ambient]: https://docs.rs/windows-thread-ambient-sys
//!
//! # Faithful means unaltered
//!
//! An entry reports the raw Win32 outcome. `ERROR_FILE_NOT_FOUND` means a
//! missing directory from an open, an empty directory from a first query, and a
//! genuine failure from a later one; only a consumer can tell those apart, so
//! nothing here normalises or reclassifies.
//!
//! The code is also snapshotted before any cleanup can overwrite it, because
//! `GetLastError` is volatile thread state that a `Drop` or a buffer release
//! will happily clobber. That guarantee is a primitive rather than a rule each
//! entry remembers: see [`outcome`].
//!
//! # A path is copied; a handle is duplicated
//!
//! Several entries take a handle rather than a path, and a request owns a
//! **duplicate** of any handle it names. The distinction matters and is easy to
//! get backwards: a path is a value and is copied, while a handle is a reference
//! to a kernel object, so duplicating it *shares that object* rather than
//! cloning it.
//!
//! A request is therefore self-contained with respect to **lifetime** -- it
//! cannot be left pointing at a handle its originator closed -- and is **not**
//! isolated with respect to **state**. Measured: a duplicated handle continues
//! the source's directory enumeration rather than starting its own, while
//! closing the duplicate leaves the source usable and single-shot metadata
//! queries disturb nothing. An independent traversal needs a fresh open, not a
//! duplicate.
//!
//! # Scope
//!
//! One entry per Win32 call; a consumer needing two makes two requests and
//! sequences them itself. The round-one entry list is audited from three real
//! consumers rather than chosen by taste, and its omissions are deliberate and
//! written down. See `DESIGN-NOTES.md` in the crate root.
//!
//! # Example
//!
//! Capture the parameters on the submitting thread, where a failure is still
//! the caller's to see and the process current directory still means what the
//! caller thinks it means, then use them on a worker that saw none of it:
//!
//! ```
//! use std::fs;
//! use std::os::windows::io::AsHandle;
//! use std::thread;
//!
//! use windows_namespace_request_sys::{CapturedHandle, prepare};
//! use wtf_string::Wtf16String;
//!
//! let path = std::env::temp_dir().join(format!("wnrs-doc-{}.tmp", std::process::id()));
//! fs::write(&path, b"example")?;
//!
//! // Resolved here, not on the worker: the process current directory is
//! // shared mutable state that any thread can change in between.
//! let text = path.to_str().expect("a temporary path is valid UTF-8");
//! let prepared = prepare(&Wtf16String::from(text))?;
//! assert_eq!(prepared.as_wtf16().to_string_lossy(), text);
//!
//! // An owned duplicate, so the captured parameters cannot be left pointing
//! // at a handle the caller has since closed.
//! let file = fs::File::open(&path)?;
//! let captured = CapturedHandle::capture(file.as_handle())?;
//! drop(file);
//!
//! let length = thread::spawn(move || {
//!     fs::File::from(captured.into_owned_handle()).metadata().map(|m| m.len())
//! })
//! .join()
//! .expect("the worker did not panic")?;
//!
//! assert_eq!(length, b"example".len() as u64);
//! # fs::remove_file(&path)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![cfg(windows)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod buffer;
pub mod close;
pub mod file_info;
pub mod final_path;
pub mod handle;
pub mod open;
pub mod open_by_id;
pub mod outcome;
pub mod path;
pub mod query;
pub mod request;
pub mod security;
pub mod watch;

pub use buffer::AlignedBuffer;
pub use close::{CloseFn, CloseRequest};
pub use file_info::QueryFileInformationByHandle;
pub use final_path::{FinalPathError, FinalPathFlags, QueryFinalPath};
pub use handle::{CapturedHandle, HandleCaptureError, HandleCaptureFailure};
pub use open::OpenFile;
pub use open_by_id::{FileIdentifier, OpenFileByIdentifier};
pub use outcome::{Outcome, Win32Error};
pub use path::{PathError, PathFailure, PreparedPath, prepare};
pub use query::{FileInformationClass, QueryFileInformation};
pub use request::{ConsumingRequest, Request};
/// Compiles the README's examples, so a contract change breaks the build
/// rather than silently teaching the old answer.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

#[cfg(test)]
mod tests;

pub use security::{
    AclState, SecurityAttributes, SecurityCaptureError, SecurityCaptureFailure, SecurityDescriptor,
};
pub use watch::{ChangeNotification, NotifyFilter, WatchDirectory};
