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

pub mod poison;
pub mod witness;

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use windows_sys::Win32::System::Environment::GetEnvironmentVariableW;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_DECOMMIT, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
    VirtualFree, VirtualProtect,
};
use windows_sys::Win32::System::Performance::QueryPerformanceCounter;

/// Windows' page size. Fixed at 4 KiB on every architecture this repository
/// targets; a wrong value here would be a correctness bug rather than a
/// performance one, so it is asserted against the OS in `tests.rs` rather than
/// assumed.
const PAGE: usize = 4096;

/// Windows' allocation granularity: the alignment `VirtualAlloc` guarantees
/// for a fresh reservation. This is what makes the right-alignment arithmetic
/// below sound for any `align` up to and including this value.
const ALLOCATION_GRANULARITY: usize = 64 * 1024;

/// Sentinel for "the seed has not been established yet".
///
/// Zero is a legitimate seed a caller might pin deliberately, so it cannot
/// double as the sentinel; `u64::MAX` is used instead and refused as an
/// explicit choice.
const SEED_UNSET: u64 = u64::MAX;

/// The run seed, shared by every `GuardAlloc` in the process.
///
/// A `static` rather than a field because it must be resolvable from the very
/// first allocation, which happens before `main` and therefore before any test
/// could configure it.
static SEED: AtomicU64 = AtomicU64::new(SEED_UNSET);

/// Read [`poison::SEED_VAR`] without allocating.
///
/// `std::env::var` allocates, and this runs inside the global allocator, where
/// allocating would recurse into the very call that is trying to finish. So
/// the environment block is read straight into a stack buffer instead.
fn seed_from_environment() -> Option<u64> {
    // Wide, null-terminated, on the stack. Long enough for any `u64` in
    // decimal or hex with room to spare; a longer value is not a valid seed
    // anyway.
    let mut name = [0_u16; 32];
    for (slot, ch) in name.iter_mut().zip(poison::SEED_VAR.encode_utf16()) {
        *slot = ch;
    }
    let mut value = [0_u16; 64];

    // SAFETY: both buffers are live stack arrays, `name` is null-terminated by
    // construction (it is zero-initialised and the variable name is shorter
    // than it), and the length passed matches `value`.
    let written = unsafe {
        GetEnvironmentVariableW(
            name.as_ptr(),
            value.as_mut_ptr(),
            u32::try_from(value.len()).unwrap_or(0),
        )
    };
    // Zero means unset; a value at least as long as the buffer means it was
    // truncated, which is not a seed worth guessing at.
    if written == 0 || written as usize >= value.len() {
        return None;
    }

    parse_seed(&value[..written as usize])
}

/// Parse a seed from the environment variable's text.
///
/// # Why this is separate from reading the variable
///
/// Everything interesting about a seed is here -- the `0x` prefix, the radix
/// that follows from it, and the overflow that must refuse rather than wrap --
/// and none of it can be exercised through [`seed_from_environment`], which
/// reads a *process-global* variable. Setting one from a test would be visible
/// to every other test in the process, because this workspace runs tests as
/// threads rather than processes, so such a test could not be written safely
/// even once.
///
/// A mutation run made that concrete: the prefix arm, the truncation check, and
/// both comparisons in the guard above all survived, and not one of them could
/// have been reached from a test. Splitting the pure half out is what makes
/// them reachable; the impure half that remains is a single Win32 call with no
/// branch of its own.
fn parse_seed(digits: &[u16]) -> Option<u64> {
    const ZERO: u16 = b'0' as u16;
    const LOWER_X: u16 = b'x' as u16;
    const UPPER_X: u16 = b'X' as u16;

    let (digits, radix) = match digits {
        [ZERO, LOWER_X | UPPER_X, rest @ ..] => (rest, 16),
        _ => (digits, 10),
    };
    if digits.is_empty() {
        return None;
    }

    let mut accumulated = 0_u64;
    for unit in digits {
        let digit = char::from_u32(u32::from(*unit))?.to_digit(radix)?;
        accumulated = accumulated
            .checked_mul(u64::from(radix))?
            .checked_add(u64::from(digit))?;
    }
    Some(accumulated)
}

/// Establish the run seed exactly once, and return it.
fn seed() -> u64 {
    let current = SEED.load(Ordering::Relaxed);
    if current != SEED_UNSET {
        return current;
    }

    let chosen = seed_from_environment().unwrap_or_else(|| {
        // `SystemTime::now` allocates on some paths and is the wrong tool
        // inside an allocator; the performance counter is a bare syscall.
        let mut ticks = 0_i64;
        // SAFETY: `ticks` is a live local of the expected type.
        unsafe { QueryPerformanceCounter(&raw mut ticks) };
        // Mixed so that a low-resolution or slow-moving counter still yields a
        // seed whose bytes do not resemble a small integer.
        poison::word(ticks.cast_unsigned(), 0)
    });
    // A pinned seed of `u64::MAX` would be indistinguishable from "unset", so
    // it is nudged rather than silently ignored.
    let chosen = if chosen == SEED_UNSET { 0 } else { chosen };

    // Whoever wins the race establishes the seed; everyone else adopts it, so
    // every allocation in the process shares one pattern space.
    match SEED.compare_exchange(SEED_UNSET, chosen, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) => chosen,
        Err(established) => established,
    }
}

/// A global allocator that places every allocation against a guard page,
/// never reuses a freed address, and fills fresh memory with a tracked poison
/// pattern.
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

    /// This run's poison seed.
    ///
    /// Pin it with the `WINDOWS_GUARD_ALLOC_SEED` environment variable
    /// (decimal, or hex with a `0x` prefix) to reproduce a run exactly.
    pub fn seed(&self) -> u64 {
        seed()
    }

    /// Print the seed, once per process, so a failing run can be replayed.
    ///
    /// This is what reconciles a varying poison pattern with the requirement
    /// that tests be reproducible: the pattern changes between runs, and the
    /// line below is what turns any particular run back into a fixed one.
    /// Call it from a test rather than from the allocator, which must not
    /// perform I/O while satisfying an allocation.
    pub fn announce_seed(&self) {
        use std::sync::Once;
        static ANNOUNCED: Once = Once::new();
        ANNOUNCED.call_once(|| {
            let seed = self.seed();
            println!(
                "windows-guard-alloc: poison seed {seed:#018x} -- \
                 re-run with {}={seed:#x} to reproduce",
                poison::SEED_VAR
            );
        });
    }

    /// Whether `bytes`, taken from `offset` bytes into an allocation, is still
    /// the untouched poison that allocation started with.
    ///
    /// The allocation identifies *itself*: the first eight bytes name their
    /// own ordinal, so a caller does not have to have snapshotted the buffer
    /// beforehand. Returns the offset of the first byte that has changed.
    ///
    /// `None` for a region shorter than eight bytes at `offset` zero, or one
    /// whose leading word is not poison at all -- in both cases there is
    /// nothing to identify the allocation with.
    pub fn poison_check(&self, whole: &[u8], offset: usize, len: usize) -> PoisonCheck {
        if whole.len() < 8 {
            return PoisonCheck::Unidentifiable;
        }
        let mut leading = [0_u8; 8];
        leading.copy_from_slice(&whole[..8]);
        let seed = self.seed();
        let Some(ordinal) = poison::identify(
            seed,
            u64::from_le_bytes(leading),
            self.total_allocations() as u64,
        ) else {
            return PoisonCheck::Unidentifiable;
        };
        let end = offset.saturating_add(len).min(whole.len());
        let region = &whole[offset.min(whole.len())..end];
        match poison::first_mismatch(seed, ordinal, offset, region) {
            None => PoisonCheck::Pristine { ordinal },
            Some(at) => PoisonCheck::Overwritten {
                ordinal,
                at: offset + at,
            },
        }
    }
}

/// The outcome of a [`GuardAlloc::poison_check`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoisonCheck {
    /// The region is untouched poison from the named allocation.
    Pristine { ordinal: u64 },
    /// Something wrote into the region; `at` is the first changed byte,
    /// measured from the start of the allocation.
    Overwritten { ordinal: u64, at: usize },
    /// The allocation could not be identified, so nothing can be concluded.
    ///
    /// Deliberately **not** folded into `Overwritten`: "I cannot tell" and
    /// "this was definitely written" are different answers, and reporting the
    /// second when the first is true would manufacture a defect.
    Unidentifiable,
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
        // `fetch_add` returns the previous value, which is this allocation's
        // ordinal -- the number its poison pattern is derived from.
        let ordinal = self.total.fetch_add(1, Ordering::Relaxed) as u64;

        // SAFETY: `data_offset` returns a value in `0..=data - size`, so the
        // whole allocation lies inside the committed, writable region.
        let ptr = unsafe { base.add(data_offset(size, align, data)) };

        // Poison the bytes the caller is about to receive. Fresh pages are
        // already zero, so without this an untouched buffer is indistinguishable
        // from one someone wrote zeros into -- and a kernel that writes outside
        // the span it was given would leave no trace.
        // SAFETY: `ptr` addresses `size` writable bytes, established above.
        unsafe { poison::fill(ptr, size, seed(), ordinal) };

        ptr
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

        // Note what is deliberately *not* done here: the block is not poisoned
        // before release. M15.2 originally called for poisoning freed blocks
        // too, and implementing it showed that to be dead code -- decommitting
        // makes those bytes unreadable, so poison written there could never be
        // observed by anything. It would cost a full memset per free and buy a
        // guarantee strictly weaker than the one already in force.
        //
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
