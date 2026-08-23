// Copyright (c) 2026 Mike Grier
//! `ring-copy`: engine (M7.3) -- one domain's read-then-write loop over its
//! own ring, its own registered `VirtualAllocExNuma` buffer, and its own
//! byte range, pinned to its plan's processors.

use std::io;
use std::ops::Range;
use std::ptr;
use std::time::{Duration, Instant};

use windows_ioring_sys::{Batch, IoRing, PushOptions, RegisteredBuffers, RegisteredSpan, Token};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;
use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadGroupAffinity};

use crate::buffer::NumaBuffer;
use crate::plan::DomainPlan;

/// How long a single push-and-wait may block before this sample gives up on
/// it, rather than hanging forever on a stuck device.
const OP_TIMEOUT_MS: u32 = 30_000;

/// What one domain's copy pass accomplished.
pub struct DomainReport {
    pub label: String,
    pub bytes_copied: u64,
    pub elapsed: Duration,
}

/// Copy `byte_range` of `source` to the same range of `destination`, entirely
/// on the calling thread, pinned to `plan`'s processors and reading through a
/// single registered buffer placed on `numa_node` (M7.3).
///
/// # Errors
///
/// Returns any error from pinning this thread, allocating or registering the
/// buffer, or any push, submit, or completion along the way.
pub fn copy_domain(
    plan: &DomainPlan,
    source: HANDLE,
    destination: HANDLE,
    byte_range: Range<u64>,
    chunk_len: usize,
    numa_node: Option<u32>,
) -> io::Result<DomainReport> {
    affinitize(plan.group, plan.mask)?;

    let mut ring = IoRing::new(8, 8)?;
    let buffer = NumaBuffer::new(chunk_len, numa_node)?;
    let registration = register_buffer(&mut ring, buffer)?;

    let start = Instant::now();
    let mut offset = byte_range.start;
    let mut bytes_copied = 0_u64;
    while offset < byte_range.end {
        let requested = u32::try_from((byte_range.end - offset).min(chunk_len as u64))
            .expect("chunk_len fits in u32");
        let read_span = RegisteredSpan {
            buffer_index: 0,
            offset: 0,
            len: requested,
        };

        // A short read is real, not an error: stop at whatever the source
        // actually had rather than writing uninitialized tail bytes or
        // spinning forever on an offset that never advances.
        let transferred = submit_one(&mut ring, |batch| {
            batch.read_registered(source, &registration, read_span, offset, PushOptions::new())
        })?;
        if transferred == 0 {
            break;
        }

        let write_span = RegisteredSpan {
            buffer_index: 0,
            offset: 0,
            len: transferred,
        };
        submit_one(&mut ring, |batch| {
            batch.write_registered(
                destination,
                &registration,
                write_span,
                offset,
                PushOptions::new(),
            )
        })?;

        offset += u64::from(transferred);
        bytes_copied += u64::from(transferred);
    }

    drop(registration);
    Ok(DomainReport {
        label: plan.label.clone(),
        bytes_copied,
        elapsed: start.elapsed(),
    })
}

/// Pin the calling thread to exactly `group`/`mask` (M7.2's plan is only
/// meaningful if the thread actually runs there).
fn affinitize(group: u16, mask: usize) -> io::Result<()> {
    let affinity = GROUP_AFFINITY {
        Mask: mask,
        Group: group,
        Reserved: [0; 3],
    };
    // SAFETY: `GetCurrentThread` is a pseudo-handle needing no closing;
    // `affinity` is a fully initialized in-argument, and no previous-affinity
    // out-pointer is requested.
    let ok = unsafe { SetThreadGroupAffinity(GetCurrentThread(), &affinity, ptr::null_mut()) };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Register `buffer` with `ring` and block until the registration is
/// confirmed, once per domain (M7.3: "registered once per ring").
fn register_buffer(
    ring: &mut IoRing,
    buffer: NumaBuffer,
) -> io::Result<RegisteredBuffers<NumaBuffer>> {
    let pending = {
        let mut batch = Batch::new(ring);
        let pending = batch.register_buffers(vec![buffer])?;
        batch.submit_and_wait(1, OP_TIMEOUT_MS)?;
        pending
    };
    let completion = ring.try_pop()?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "no completion for buffer registration",
        )
    })?;
    match pending.claim_if(&completion) {
        Ok(result) => result,
        Err(_) => Err(io::Error::other(
            "completion did not match the pending buffer registration",
        )),
    }
}

/// Push one op via `push`, submit and wait for it, then claim its completion,
/// returning the transferred byte count.
fn submit_one<F>(ring: &mut IoRing, push: F) -> io::Result<u32>
where
    F: FnOnce(&mut Batch<'_>) -> io::Result<Token<windows_ioring_sys::RegisteredUse>>,
{
    let token = {
        let mut batch = Batch::new(ring);
        let token = push(&mut batch)?;
        batch.submit_and_wait(1, OP_TIMEOUT_MS)?;
        token
    };
    let completion = ring.try_pop()?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "no completion after submit_and_wait",
        )
    })?;
    let transferred = completion.result()?;
    token
        .claim_if(&completion)
        .map_err(|_| io::Error::other("completion did not match the token this call submitted"))?;
    u32::try_from(transferred)
        .map_err(|_| io::Error::other("transferred byte count does not fit in u32"))
}
