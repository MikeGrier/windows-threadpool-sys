// Copyright (c) 2026 Mike Grier
//! Construction's registration paths (DI-7.2): what `Dioring::new` returns when its ring refuses a
//! registration, fails its completion, never completes it, or will not take its completion event.
//!
//! The backend's own hand-back is `windows_backend`'s tests; this is what the instance makes of it:
//! every refusal is a [`SetupRefusal::Ring`], and the files, the buffers where the kernel was not
//! given them, and the provider all come back. Each runs with the ring crate's safe call-fault
//! responder registered on the ring as it is created, through [`Construction`]: construction
//! finishes before the consumer can name the ring, so a responder registered afterwards cannot
//! answer the calls it makes.

use std::io;
use std::time::Duration;

use crate::InterruptStamps;
use win_shared_os_owned_handle::SharedHandle;
use windows_ioring_sys::sys::{Call, CallFaultControl};
use windows_ioring_sys::{InjectedFailure, IoRingErrorExt};

use super::{Ring, SetupError, SetupRefusal, TempFile, file, serves, setup};
use crate::windows_backend::Construction;

/// `HRESULT_FROM_WIN32(ERROR_IO_DEVICE)`, as `InjectedFailure::Win32(1117)` spells it.
const DEVICE: i32 = 0x8007_045D_u32.cast_signed();

const LONG: Duration = Duration::from_secs(5);

fn device() -> InjectedFailure {
    InjectedFailure::Win32(1117)
}

/// The `HRESULT` a native report carries, or a plain error's OS code.
fn code(error: &io::Error) -> Option<i32> {
    error
        .as_ioring_error()
        .map(|native| native.code())
        .or_else(|| error.raw_os_error())
}

/// What a refused instance handed back, and the handles it was given.
struct Refused {
    error: SetupError<u64, Vec<u8>>,
    originals: Vec<SharedHandle>,
    control: CallFaultControl,
}

impl Refused {
    /// The ring's error behind the refusal.
    fn ring_error(&self) -> &io::Error {
        match &self.error.reason {
            SetupRefusal::Ring(error) => error,
            other => panic!("refused for the wrong reason: {other:?}"),
        }
    }

    /// Every file, the same handle as it was given and in order; the provider; and, once all that
    /// is dropped, no ring holding any of the files.
    fn assert_everything_but_the_buffers_came_back(self, temps: &[&TempFile]) -> CallFaultControl {
        let Self {
            error,
            originals,
            control,
        } = self;
        assert_eq!(error.files.len(), originals.len());
        for (back, original) in error.files.iter().zip(&originals) {
            assert!(
                back.target.handle().same_handle(original),
                "the same handle comes back, in order"
            );
        }
        assert!(error.provider.is_some(), "the provider comes back");
        drop((error, originals));
        for temp in temps {
            assert!(!temp.is_held(), "no ring is left holding a file");
        }
        control
    }
}

/// Construct an instance over `temps` and `buffers`, `arm`ed before the ring exists, waiting
/// `bound` for each registration; it is refused.
fn refused_with(
    temps: &[&TempFile],
    buffers: Vec<Vec<u8>>,
    bound: Duration,
    arm: impl FnOnce(&CallFaultControl),
) -> Refused {
    let (construction, control) = Construction::faulted(bound);
    arm(&control);
    let files: Vec<_> = temps
        .iter()
        .enumerate()
        .map(|(i, temp)| file(i as u64 + 1, temp, &[]))
        .collect();
    let originals = files.iter().map(|f| f.target.handle().clone()).collect();
    let error = match Ring::with_construction(
        setup(files, buffers, serves(&["P"])),
        InterruptStamps,
        construction,
    ) {
        Ok(_) => panic!("construction should have been refused"),
        Err(error) => error,
    };
    Refused {
        error,
        originals,
        control,
    }
}

#[test]
fn a_refused_file_registration_build_fails_construction_and_everything_comes_back() {
    let temp = TempFile::new("dc-files-build");
    let refused = refused_with(&[&temp], vec![vec![7; 4]], LONG, |control| {
        control.fail_next(Call::BuildRegisterFiles, 1, device());
    });
    assert_eq!(code(refused.ring_error()), Some(DEVICE));
    assert_eq!(refused.error.buffers, Some(vec![vec![7; 4]]));
    refused.assert_everything_but_the_buffers_came_back(&[&temp]);
}

#[test]
fn a_refused_buffer_registration_build_fails_construction_and_the_buffers_come_back() {
    let temp = TempFile::new("dc-buffers-build");
    let refused = refused_with(&[&temp], vec![vec![1; 8], vec![2; 16]], LONG, |control| {
        control.fail_next(Call::BuildRegisterBuffers, 1, device());
    });
    assert_eq!(code(refused.ring_error()), Some(DEVICE));
    assert_eq!(refused.error.buffers, Some(vec![vec![1; 8], vec![2; 16]]));
    refused.assert_everything_but_the_buffers_came_back(&[&temp]);
}

#[test]
fn a_registration_whose_submit_fails_fails_construction() {
    let temp = TempFile::new("dc-submit");
    let refused = refused_with(&[&temp], vec![vec![7; 4]], LONG, |control| {
        control.fail_next(Call::Submit, 1, device());
    });
    assert_eq!(code(refused.ring_error()), Some(DEVICE));
    assert_eq!(refused.error.buffers, Some(vec![vec![7; 4]]));
    let control = refused.assert_everything_but_the_buffers_came_back(&[&temp]);
    assert_eq!(control.failures(Call::Submit).len(), 1);
}

#[test]
fn a_registration_whose_completion_fails_fails_construction() {
    let temp = TempFile::new("dc-completion");
    let refused = refused_with(&[&temp], vec![vec![7; 4]], LONG, |control| {
        control.fail_next_completions(1, device());
    });
    assert_eq!(code(refused.ring_error()), Some(DEVICE));
    assert_eq!(refused.error.buffers, Some(vec![vec![7; 4]]));
    refused.assert_everything_but_the_buffers_came_back(&[&temp]);
}

#[test]
fn a_buffer_registration_whose_completion_fails_keeps_the_buffers_with_the_ring() {
    let refused = refused_with(&[], vec![vec![7; 4]], LONG, |control| {
        control.fail_next_completions(1, device());
    });
    assert_eq!(code(refused.ring_error()), Some(DEVICE));
    assert!(
        refused.error.buffers.is_none(),
        "the kernel was given them, and cannot be shown to have given them back"
    );
    refused.assert_everything_but_the_buffers_came_back(&[]);
}

#[test]
fn a_registration_that_does_not_complete_within_the_bound_fails_construction() {
    let temp = TempFile::new("dc-stall");
    let refused = refused_with(
        &[&temp],
        vec![vec![7; 4]],
        Duration::from_millis(50),
        |control| control.stall_submits(Duration::from_millis(600)),
    );
    let error = refused.ring_error();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    assert!(error.to_string().contains("50ms"), "{error}");
    assert_eq!(refused.error.buffers, Some(vec![vec![7; 4]]));
    refused.assert_everything_but_the_buffers_came_back(&[&temp]);
}

#[test]
fn a_completion_event_that_cannot_be_attached_fails_construction_with_the_buffers_kept() {
    let temp = TempFile::new("dc-event");
    let refused = refused_with(&[&temp], vec![vec![7; 4]], LONG, |control| {
        control.fail_next(Call::SetCompletionEvent, 1, device());
    });
    assert_eq!(code(refused.ring_error()), Some(DEVICE));
    assert!(
        refused.error.buffers.is_none(),
        "both registrations had completed, so the kernel holds the buffers"
    );
    let control = refused.assert_everything_but_the_buffers_came_back(&[&temp]);
    assert_eq!(control.failures(Call::SetCompletionEvent).len(), 1);
}
