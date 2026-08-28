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
//! nothing here normalises or reclassifies. `GetLastError` is read before any
//! restoration runs, so the error is an output of the operation rather than
//! something left on a thread.
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

#![cfg(windows)]
#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]
