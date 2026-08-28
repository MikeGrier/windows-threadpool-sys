// Copyright (c) 2026 Mike Grier

use std::io;

#[cold]
#[inline(never)]
pub(crate) fn panic_failure(error: io::Error) -> ! {
    panic!("SetThreadToken failed to restore the previous thread token: {error}");
}
