// Copyright (c) Mike Grier.

use std::cell::RefCell;
use std::io;
use std::marker::PhantomData;
use std::rc::Rc;

use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

const POINT_COUNT: usize = 6;

#[derive(Clone, Copy)]
pub(crate) enum FaultPoint {
    BackgroundInstall,
    MemoryInstall,
    RedirectionRevert,
    ErrorModeSet,
    TransactionSupport,
    TransactionSet,
}

impl FaultPoint {
    const fn index(self) -> usize {
        match self {
            Self::BackgroundInstall => 0,
            Self::MemoryInstall => 1,
            Self::RedirectionRevert => 2,
            Self::ErrorModeSet => 3,
            Self::TransactionSupport => 4,
            Self::TransactionSet => 5,
        }
    }
}

#[derive(Default)]
struct Injection {
    calls: [usize; POINT_COUNT],
    failures: [usize; POINT_COUNT],
}

thread_local! {
    static INJECTION: RefCell<Injection> = RefCell::new(Injection::default());
}

pub(crate) struct FaultScope {
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for FaultScope {
    fn drop(&mut self) {
        reset();
    }
}

pub(crate) fn fail(points: &[(FaultPoint, usize)]) -> FaultScope {
    reset();
    INJECTION.with_borrow_mut(|injection| {
        for (point, failures) in points {
            injection.failures[point.index()] = *failures;
        }
    });
    FaultScope {
        _not_send: PhantomData,
    }
}

pub(crate) fn hit(point: FaultPoint) -> Option<io::Error> {
    INJECTION.with_borrow_mut(|injection| {
        let index = point.index();
        injection.calls[index] += 1;
        if injection.failures[index] == 0 {
            return None;
        }
        injection.failures[index] -= 1;
        Some(io::Error::from_raw_os_error(
            i32::try_from(ERROR_ACCESS_DENIED).expect("ERROR_ACCESS_DENIED fits in i32"),
        ))
    })
}

pub(crate) fn calls(point: FaultPoint) -> usize {
    INJECTION.with_borrow(|injection| injection.calls[point.index()])
}

fn reset() {
    INJECTION.with_borrow_mut(|injection| *injection = Injection::default());
}
