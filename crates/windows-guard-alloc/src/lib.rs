// Copyright (c) 2026 Mike Grier
//! A guard-page global allocator for Windows test builds.
//!
//! # What it is for
//!
//! Making two classes of memory bug **deterministic** instead of lucky:
//!
//! - a **heap overrun** -- the allocation abuts a `PAGE_NOACCESS` guard page,
//!   so one byte past the end faults;
//! - a **use-after-free** -- a freed block is decommitted and its address is
//!   never handed out again, so any later touch faults.
//!
//! Without instrumentation neither is reliably visible. Measured on a probe
//! against the system allocator, a read through a pointer to a freed 32-byte
//! `Vec` returned a plausible byte and the process exited `0`. That is not a
//! near miss; it is the mechanism by which a real defect in this repository
//! escaped review, shipped, and had to be yanked -- see
//! `windows-ioring-sys` D-32, where the kernel read a freed
//! `IORING_BUFFER_INFO` array and the failure surfaced as a survivable
//! `ERROR_NOACCESS` purely because the freed pages happened to still be
//! mapped.
//!
//! # Why not Application Verifier / PageHeap
//!
//! Because it works but cannot be pointed at `cargo test`. Full PageHeap is
//! enabled per **image file name** under `Image File Execution Options`, and
//! cargo rehashes test binaries: one test target produced six distinct
//! `registration-<hash>.exe` names in a single day's work. CI would have to
//! enumerate, register and unregister every built binary on every job, and
//! would degrade *silently to instrumenting nothing* whenever the enumeration
//! missed one. This allocator lives inside the binary, so renaming cannot
//! defeat it, and it needs no administrator rights, no SDK, and no cleanup.
//! See `windows-ioring-sys` D-37 for the measurements.
//!
//! # The cost, stated plainly
//!
//! One reservation per allocation, a minimum of two pages each, and **address
//! space is never reused**. A long-running process would exhaust its address
//! space; the commit is released on free, so it is virtual address space
//! rather than physical memory that grows without bound. That trade is correct
//! for a test binary and wrong for anything else, which is why this crate is
//! `publish = false`.
//!
//! # Installing it
//!
//! A `#[global_allocator]` must be declared by the binary, so every test
//! target that wants this opts in explicitly:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOC: windows_guard_alloc::GuardAlloc = windows_guard_alloc::GuardAlloc::new();
//! ```
//!
//! **Then assert it is actually installed**, with
//! [`GuardAlloc::total_allocations`]. An instrument nobody checks is
//! indistinguishable from one that is not there, and forgetting the attribute
//! is silent -- the tests still pass, and instrument nothing.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_DECOMMIT, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
    VirtualFree, VirtualProtect,
};

/// Windows' page size. Fixed at 4 KiB on every architecture this repository
/// targets; a wrong value here would be a correctness bug rather than a
/// performance one, so it is asserted against the OS in `tests.rs` rather than
/// assumed.
const PAGE: usize = 4096;

/// Windows' allocation granularity: the alignment `VirtualAlloc` guarantees
/// for a fresh reservation. This is what makes the right-alignment arithmetic
/// below sound for any `align` up to and including this value.
const ALLOCATION_GRANULARITY: usize = 64 * 1024;

/// A global allocator that places every allocation against a guard page and
/// never reuses a freed address.
///
/// See the module documentation for what it catches and what it costs.
pub struct GuardAlloc {
    live: AtomicUsize,
    total: AtomicUsize,
}

impl GuardAlloc {
    /// Create the allocator. `const` so it can initialise a
    /// `#[global_allocator]` static.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            live: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
        }
    }

    /// Allocations made and not yet freed.
    pub fn live_allocations(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }

    /// Allocations made since process start.
    ///
    /// Use this to prove the allocator is installed. It is non-zero before
    /// `main` runs on any realistic program, so a test asserting it is
    /// non-zero fails loudly when the `#[global_allocator]` attribute is
    /// missing -- which is otherwise a silent no-op.
    pub fn total_allocations(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }
}

impl Default for GuardAlloc {
    fn default() -> Self {
        Self::new()
    }
}

/// How far into the reservation the allocation begins.
///
/// Right-aligned so the allocation's **end** abuts the guard page, which is
/// what turns a one-byte overrun into a fault rather than landing in slack.
/// The offset is rounded *down* to `align`, so the returned pointer is aligned
/// whenever the reservation base is -- and `VirtualAlloc` bases a reservation
/// at [`ALLOCATION_GRANULARITY`], so that holds for every `align` up to it.
///
/// For a larger `align` than that, no offset can be relied on, so the
/// allocation is placed at the base instead. It is still guarded; it merely
/// has slack before the guard page, so a small overrun may not fault. Such
/// alignments do not occur in this repository's code, and pretending otherwise
/// would be worse than saying so.
fn data_offset(size: usize, align: usize, data_bytes: usize) -> usize {
    if align > ALLOCATION_GRANULARITY {
        return 0;
    }
    (data_bytes - size) & !(align - 1)
}

/// Bytes of committed, writable space a `size`-byte allocation occupies,
/// excluding its guard page.
fn data_bytes(size: usize) -> usize {
    size.max(1).div_ceil(PAGE) * PAGE
}

// SAFETY: `alloc` returns either null or a pointer to `layout.size()` writable
// bytes aligned to `layout.align()`, obtained from a fresh reservation that no
// other allocation shares. `dealloc` is only ever called with a pointer and
// layout this allocator produced, and it decommits exactly that reservation
// without releasing the address, so the pointer can never be handed out again.
unsafe impl GlobalAlloc for GuardAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1);
        let align = layout.align().max(1);
        let data = data_bytes(size);

        let Some(total) = data.checked_add(PAGE) else {
            return std::ptr::null_mut();
        };

        // SAFETY: a fresh reservation at an address of the kernel's choosing;
        // the result is null-checked before any use.
        let base = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                total,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        if base.is_null() {
            return std::ptr::null_mut();
        }
        let base = base.cast::<u8>();

        // The trailing page becomes the guard. Committed first (above) then
        // protected, rather than reserved-only, so that touching it raises an
        // access violation rather than a commit-on-demand.
        let mut previous = PAGE_READWRITE;
        // SAFETY: `guard` is the last page of the reservation just made, and
        // `previous` is a live local.
        let protected = unsafe {
            let guard = base.add(data);
            VirtualProtect(guard.cast(), PAGE, PAGE_NOACCESS, &raw mut previous)
        };
        if protected == 0 {
            // Without the guard page this allocation would be silently
            // uninstrumented, which is worse than failing: it would report
            // success while checking nothing.
            // SAFETY: `base` is our own reservation and nothing has been
            // handed out, so releasing it here is unobservable.
            unsafe {
                VirtualFree(
                    base.cast(),
                    0,
                    windows_sys::Win32::System::Memory::MEM_RELEASE,
                )
            };
            return std::ptr::null_mut();
        }

        self.live.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(1, Ordering::Relaxed);

        // SAFETY: `data_offset` returns a value in `0..=data - size`, so the
        // whole allocation lies inside the committed, writable region.
        unsafe { base.add(data_offset(size, align, data)) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size().max(1);
        let align = layout.align().max(1);
        let data = data_bytes(size);

        // Recover the reservation base by undoing the offset arithmetic, not
        // by masking the pointer to a page boundary: for an allocation larger
        // than a page the pointer is not in the first page, and masking would
        // silently decommit the wrong region.
        // SAFETY: `ptr` came from this allocator's `alloc` with this same
        // layout, so this reverses exactly the addition made there.
        let base = unsafe { ptr.sub(data_offset(size, align, data)) };

        // Decommit rather than release: the address stays reserved, so it can
        // never be handed out again and any later touch faults. Releasing it
        // would return the range to the allocator and reintroduce exactly the
        // stale-pointer-reads-plausible-bytes case this exists to remove.
        //
        // The guard page is decommitted along with it; it is inside the same
        // reservation and there is nothing left to guard once the data is
        // gone.
        // SAFETY: `base` addresses this allocation's own reservation, and
        // `data + PAGE` is the extent committed for it.
        unsafe { VirtualFree(base.cast(), data + PAGE, MEM_DECOMMIT) };

        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests;
