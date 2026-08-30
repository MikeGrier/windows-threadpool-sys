// Copyright (c) Mike Grier
//! Spike: what does `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` actually order?
//!
//! Q1 Does the barrier hold -- does a drained flush complete after every write
//!    pushed before it?
//! Q2 Is "preceding" scoped to the current submission batch, or to every
//!    outstanding op on the ring?
//! Q3 One-sided wait, or FULL barrier? May a write pushed AFTER the drained
//!    flush complete BEFORE it? io_uring's IOSQE_IO_DRAIN is a full barrier
//!    (it defers subsequent requests too), which would make cross-epoch
//!    pipelining impossible.
//!
//! A first attempt used buffered writes and could not discriminate: every
//! completion arrived in submission order even with NO drain flag, because
//! buffered writes land in the page cache and finish in issue order. There is
//! no reordering opportunity to detect.
//!
//! So this version forces real device I/O with FILE_FLAG_NO_BUFFERING and
//! makes the two phases deliberately asymmetric: phase A is 32 x 1 MiB (slow),
//! phase B is 32 x 4 KiB (fast). If later ops may overtake, the tiny phase-B
//! writes will finish while the big phase-A writes and the flush are still in
//! flight. NO_BUFFERING is a measurement instrument here, not a design
//! recommendation -- it is what creates the reordering opportunity.

use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, S_FALSE, S_OK};
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingFlushFile, BuildIoRingWriteFile, CREATE_ALWAYS, CloseIoRing, CreateFileW,
    CreateIoRing, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
    FILE_FLUSH_DEFAULT, FILE_SHARE_READ, FILE_WRITE_FLAGS_NONE, IORING_BUFFER_REF,
    IORING_BUFFER_REF_0, IORING_CAPABILITIES, IORING_CQE, IORING_CREATE_ADVISORY_FLAGS_NONE,
    IORING_CREATE_FLAGS, IORING_CREATE_REQUIRED_FLAGS_NONE, IORING_HANDLE_REF,
    IORING_HANDLE_REF_0, IORING_REF_RAW, IORING_VERSION_3, IOSQE_FLAGS_DRAIN_PRECEDING_OPS,
    IOSQE_FLAGS_NONE, PopIoRingCompletion, QueryIoRingCapabilities, SubmitIoRing,
};

const PHASE_OPS: usize = 32;
/// Phase A: large, so these writes are genuinely slow.
const BIG_LEN: u32 = 1024 * 1024;
/// Phase B: tiny, so these writes are genuinely fast and *could* overtake.
const SMALL_LEN: u32 = 4096;
/// NO_BUFFERING demands sector alignment for buffer, offset, and length.
const ALIGN: usize = 4096;

mod tag {
    /// Writes pushed BEFORE the flush.
    pub const PHASE_A: usize = 1_000;
    /// The flush itself.
    pub const FLUSH: usize = 5_000;
    /// Writes pushed AFTER the flush.
    pub const PHASE_B: usize = 9_000;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A sector-aligned buffer, since NO_BUFFERING requires one.
struct Aligned {
    storage: Vec<u8>,
    offset: usize,
}

impl Aligned {
    fn new(len: usize, fill: u8) -> Self {
        let storage = vec![fill; len + ALIGN];
        let base = storage.as_ptr() as usize;
        let offset = (ALIGN - (base % ALIGN)) % ALIGN;
        Self { storage, offset }
    }

    fn ptr(&mut self) -> *mut u8 {
        // SAFETY: `offset` is within the over-allocation by construction.
        unsafe { self.storage.as_mut_ptr().add(self.offset) }
    }
}

struct Ring {
    handle: *mut c_void,
    file: HANDLE,
    /// A second, unrelated file, to separate a ring-level barrier from
    /// filesystem-level serialization on the flushed file.
    other: HANDLE,
}

impl Ring {
    fn file_ref(&self) -> IORING_HANDLE_REF {
        Self::handle_ref(self.file)
    }

    fn other_ref(&self) -> IORING_HANDLE_REF {
        Self::handle_ref(self.other)
    }

    fn handle_ref(file: HANDLE) -> IORING_HANDLE_REF {
        IORING_HANDLE_REF {
            Kind: IORING_REF_RAW,
            Handle: IORING_HANDLE_REF_0 { Handle: file },
        }
    }

    fn push_write_to(
        &self,
        target: IORING_HANDLE_REF,
        buffer: *mut u8,
        len: u32,
        offset: u64,
        user_data: usize,
    ) {
        let buffer_ref = IORING_BUFFER_REF {
            Kind: IORING_REF_RAW,
            Buffer: IORING_BUFFER_REF_0 {
                Address: buffer.cast::<c_void>(),
            },
        };
        // SAFETY: live ring and file; `buffer` is valid for `len` bytes and
        // outlives every op (each case is drained before the buffers move).
        let hr = unsafe {
            BuildIoRingWriteFile(
                self.handle,
                target,
                buffer_ref,
                len,
                offset,
                FILE_WRITE_FLAGS_NONE,
                user_data,
                IOSQE_FLAGS_NONE,
            )
        };
        assert_eq!(hr, S_OK, "BuildIoRingWriteFile failed: 0x{:08X}", hr as u32);
    }

    fn push_write(&self, buffer: *mut u8, len: u32, offset: u64, user_data: usize) {
        let buffer_ref = IORING_BUFFER_REF {
            Kind: IORING_REF_RAW,
            Buffer: IORING_BUFFER_REF_0 {
                Address: buffer.cast::<c_void>(),
            },
        };
        // SAFETY: live ring and file; `buffer` is valid for `len` bytes and
        // outlives every op (each case is drained before the buffers move).
        let hr = unsafe {
            BuildIoRingWriteFile(
                self.handle,
                self.file_ref(),
                buffer_ref,
                len,
                offset,
                FILE_WRITE_FLAGS_NONE,
                user_data,
                IOSQE_FLAGS_NONE,
            )
        };
        assert_eq!(hr, S_OK, "BuildIoRingWriteFile failed: 0x{:08X}", hr as u32);
    }

    fn push_flush(&self, user_data: usize, drain: bool) {
        let flags = if drain {
            IOSQE_FLAGS_DRAIN_PRECEDING_OPS
        } else {
            IOSQE_FLAGS_NONE
        };
        // SAFETY: live ring and file.
        let hr = unsafe {
            BuildIoRingFlushFile(
                self.handle,
                self.file_ref(),
                FILE_FLUSH_DEFAULT,
                user_data,
                flags,
            )
        };
        assert_eq!(hr, S_OK, "BuildIoRingFlushFile failed: 0x{:08X}", hr as u32);
    }

    fn submit(&self) {
        let mut submitted = 0_u32;
        // SAFETY: live ring; valid out-pointer; no wait.
        let hr = unsafe { SubmitIoRing(self.handle, 0, 0, &raw mut submitted) };
        assert_eq!(hr, S_OK, "SubmitIoRing failed: 0x{:08X}", hr as u32);
    }

    /// Pop `expected` completions, recording each one's user_data and the time
    /// it was observed relative to `origin`.
    fn collect(&self, expected: usize, origin: Instant) -> Vec<(usize, Duration)> {
        let mut order = Vec::with_capacity(expected);
        while order.len() < expected {
            let mut cqe = IORING_CQE {
                UserData: 0,
                ResultCode: 0,
                Information: 0,
            };
            // SAFETY: live ring; valid out-pointer.
            let hr = unsafe { PopIoRingCompletion(self.handle, &raw mut cqe) };
            if hr == S_FALSE {
                assert!(
                    origin.elapsed().as_secs() < 120,
                    "timed out with {} of {expected} completions",
                    order.len()
                );
                let mut submitted = 0_u32;
                // SAFETY: live ring; zero new SQEs, so this only waits/reaps.
                let hr = unsafe { SubmitIoRing(self.handle, 1, 100, &raw mut submitted) };
                assert!(hr == S_OK || hr == S_FALSE, "wait failed: 0x{:08X}", hr as u32);
                continue;
            }
            assert_eq!(hr, S_OK, "PopIoRingCompletion failed: 0x{:08X}", hr as u32);
            assert_eq!(
                cqe.ResultCode, S_OK,
                "operation {} failed: 0x{:08X}",
                cqe.UserData, cqe.ResultCode as u32
            );
            order.push((cqe.UserData, origin.elapsed()));
        }
        order
    }
}

fn analyse(order: &[(usize, Duration)]) {
    let flush_at = order
        .iter()
        .position(|&(u, _)| u == tag::FLUSH)
        .expect("flush completion missing");
    let flush_time = order[flush_at].1;

    let a_after = order
        .iter()
        .enumerate()
        .filter(|&(index, &(u, _))| index > flush_at && (tag::PHASE_A..tag::FLUSH).contains(&u))
        .count();
    let b_before = order
        .iter()
        .enumerate()
        .filter(|&(index, &(u, _))| index < flush_at && u >= tag::PHASE_B)
        .count();

    let last_a = order
        .iter()
        .filter(|&&(u, _)| (tag::PHASE_A..tag::FLUSH).contains(&u))
        .map(|&(_, t)| t)
        .max()
        .unwrap_or_default();
    let first_b = order
        .iter()
        .filter(|&&(u, _)| u >= tag::PHASE_B)
        .map(|&(_, t)| t)
        .min()
        .unwrap_or_default();

    println!("  flush completed at position {flush_at}/{}, t+{flush_time:?}", order.len());
    println!("  last  phase-A completion : t+{last_a:?}");
    println!("  first phase-B completion : t+{first_b:?}");
    println!("  phase-A completing AFTER the flush : {a_after}/{PHASE_OPS}");
    println!("  phase-B completing BEFORE the flush: {b_before}/{PHASE_OPS}");
    print!("  -> ");
    match (a_after, b_before) {
        (0, 0) => println!("FULL BARRIER -- nothing crossed in either direction"),
        (0, _) => println!("ONE-SIDED -- preceding waited for, later ops overtook"),
        (_, 0) => println!("later ops deferred but preceding NOT waited for (surprising)"),
        (_, _) => println!("NO ORDERING OBSERVED"),
    }
}

/// Does the ring execute operations concurrently and complete them out of
/// order AT ALL? Without this, every ordering result below is vacuous: a ring
/// that serializes needs no barrier to look like one.
///
/// Same asymmetry, no flush: 32 x 1 MiB then 32 x 4 KiB. If the tiny writes do
/// not overtake the big ones here, nothing can be concluded about the flag.
fn concurrency_check(ring: &Ring, big: &mut [Aligned], small: &mut [Aligned]) {
    println!("
== CONCURRENCY CHECK (no flush, no flags) ==");
    let origin = Instant::now();

    for (index, buffer) in big.iter_mut().enumerate().take(PHASE_OPS) {
        let offset = (index as u64) * (BIG_LEN as u64);
        ring.push_write(buffer.ptr(), BIG_LEN, offset, tag::PHASE_A + index);
    }
    let small_base = (PHASE_OPS as u64) * (BIG_LEN as u64) * 2;
    for (index, buffer) in small.iter_mut().enumerate().take(PHASE_OPS) {
        let offset = small_base + (index as u64) * (SMALL_LEN as u64);
        ring.push_write(buffer.ptr(), SMALL_LEN, offset, tag::PHASE_B + index);
    }
    ring.submit();

    let order = ring.collect(PHASE_OPS * 2, origin);
    let last_big_at = order
        .iter()
        .rposition(|&(u, _)| (tag::PHASE_A..tag::FLUSH).contains(&u))
        .unwrap_or(0);
    let small_before = order
        .iter()
        .take(last_big_at)
        .filter(|&&(u, _)| u >= tag::PHASE_B)
        .count();

    println!("  all {} completions in {:?}", order.len(), origin.elapsed());
    println!("  4 KiB writes completing before the last 1 MiB write: {small_before}/{PHASE_OPS}");
    if small_before == 0 {
        println!("  -> NO REORDERING OBSERVABLE. Completions are strictly in submission");
        println!("     order even with no ordering flag, so this harness cannot tell a");
        println!("     barrier from the ring's own behaviour. Results below are VACUOUS.");
    } else {
        println!("  -> reordering happens; the barrier cases below are meaningful.");
    }
}

fn run_case(
    ring: &Ring,
    big: &mut [Aligned],
    small: &mut [Aligned],
    label: &str,
    drain: bool,
    split_submits: bool,
    phase_b_elsewhere: bool,
) {
    println!("
== {label} ==");
    println!(
        "   drain flag: {drain}; submission: {}; phase B on: {}",
        if split_submits { "three separate submits" } else { "one submit" },
        if phase_b_elsewhere { "a DIFFERENT file" } else { "the flushed file" }
    );

    let origin = Instant::now();

    for (index, buffer) in big.iter_mut().enumerate().take(PHASE_OPS) {
        let offset = (index as u64) * (BIG_LEN as u64);
        ring.push_write(buffer.ptr(), BIG_LEN, offset, tag::PHASE_A + index);
    }
    if split_submits {
        ring.submit();
    }

    ring.push_flush(tag::FLUSH, drain);
    if split_submits {
        ring.submit();
    }

    // Placed far past phase A's region so the two never touch the same blocks.
    let small_base = (PHASE_OPS as u64) * (BIG_LEN as u64) * 2;
    let target = if phase_b_elsewhere { ring.other_ref() } else { ring.file_ref() };
    for (index, buffer) in small.iter_mut().enumerate().take(PHASE_OPS) {
        let offset = if phase_b_elsewhere {
            (index as u64) * (SMALL_LEN as u64)
        } else {
            small_base + (index as u64) * (SMALL_LEN as u64)
        };
        ring.push_write_to(target, buffer.ptr(), SMALL_LEN, offset, tag::PHASE_B + index);
    }
    ring.submit();

    let order = ring.collect(PHASE_OPS * 2 + 1, origin);
    println!("  all {} completions in {:?}", order.len(), origin.elapsed());
    analyse(&order);
}

fn main() {
    let mut caps = IORING_CAPABILITIES::default();
    // SAFETY: valid out-pointer.
    let rc = unsafe { QueryIoRingCapabilities(&raw mut caps) };
    assert_eq!(rc, S_OK, "QueryIoRingCapabilities failed");
    println!("== environment ==");
    println!("max version : {}", caps.MaxVersion);
    println!(
        "phase A     : {PHASE_OPS} x {} KiB (slow)   phase B: {PHASE_OPS} x {} KiB (fast)",
        BIG_LEN / 1024,
        SMALL_LEN / 1024
    );
    println!("file        : FILE_FLAG_NO_BUFFERING (forces real device I/O)");

    let path = std::env::temp_dir().join(format!("ioring-drain-spike-{}.tmp", std::process::id()));
    let path_w = wide(&path.to_string_lossy());
    // SAFETY: null-terminated wide path; standard create.
    let file = unsafe {
        CreateFileW(
            path_w.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            ptr::null(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            ptr::null_mut(),
        )
    };
    assert!(!file.is_null(), "CreateFileW failed");

    let other_path =
        std::env::temp_dir().join(format!("ioring-drain-spike-other-{}.tmp", std::process::id()));
    let other_w = wide(&other_path.to_string_lossy());
    // SAFETY: null-terminated wide path; standard create.
    let other = unsafe {
        CreateFileW(
            other_w.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            ptr::null(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            ptr::null_mut(),
        )
    };
    assert!(!other.is_null(), "CreateFileW (other) failed");

    let mut handle: *mut c_void = ptr::null_mut();
    let flags = IORING_CREATE_FLAGS {
        Required: IORING_CREATE_REQUIRED_FLAGS_NONE,
        Advisory: IORING_CREATE_ADVISORY_FLAGS_NONE,
    };
    // SAFETY: valid out-pointer; sizes within reported caps.
    let rc = unsafe { CreateIoRing(IORING_VERSION_3, flags, 256, 512, &raw mut handle) };
    assert_eq!(rc, S_OK, "CreateIoRing failed: 0x{:08X}", rc as u32);
    let ring = Ring { handle, file, other };

    let mut big: Vec<Aligned> = (0..PHASE_OPS)
        .map(|index| Aligned::new(BIG_LEN as usize, index as u8))
        .collect();
    let mut small: Vec<Aligned> = (0..PHASE_OPS)
        .map(|index| Aligned::new(SMALL_LEN as usize, index as u8))
        .collect();

    // Pre-write the whole extent so the measured cases are pure OVERWRITES.
    // Extending writes (and writes past NTFS's valid-data-length, which the
    // filesystem must zero-fill) are serialized by the filesystem, which would
    // masquerade as ordering that the ring did not impose.
    {
        let total = (PHASE_OPS as u64) * (BIG_LEN as u64) * 2 + (PHASE_OPS as u64) * 1024 * 1024;
        let chunks = (total / (BIG_LEN as u64)) as usize;
        println!("
pre-writing {} MiB so measured cases are overwrites...", total / (1024 * 1024));
        let origin = Instant::now();
        for index in 0..chunks {
            let buffer = &mut big[index % PHASE_OPS];
            ring.push_write(
                buffer.ptr(),
                BIG_LEN,
                (index as u64) * (BIG_LEN as u64),
                tag::PHASE_A + index,
            );
            // Keep well inside the 256-entry submission queue.
            if index % 16 == 15 {
                ring.submit();
            }
        }
        ring.submit();
        let done = ring.collect(chunks, origin);
        println!("  {} chunks in {:?}", done.len(), origin.elapsed());

        // Same for the second file, so phase-B writes there are overwrites too.
        let other_ref = ring.other_ref();
        for index in 0..4 {
            let buffer = &mut big[index];
            ring.push_write_to(
                other_ref,
                buffer.ptr(),
                BIG_LEN,
                (index as u64) * (BIG_LEN as u64),
                tag::PHASE_A + index,
            );
        }
        ring.submit();
        ring.collect(4, origin);
    }

    // Is there any reordering to detect? If not, everything below is vacuous.
    concurrency_check(&ring, &mut big, &mut small);

    // Control: with no drain flag and this asymmetry, the tiny phase-B writes
    // SHOULD overtake. If they do not even here, the drained results below
    // prove nothing.
    run_case(&ring, &mut big, &mut small, "CONTROL (no drain flag)", false, false, false);
    run_case(&ring, &mut big, &mut small, "DRAIN, single submit", true, false, false);
    run_case(&ring, &mut big, &mut small, "DRAIN, separate submits", true, true, false);

    // The decisive case for Q3. If phase B is deferred even when it targets an
    // unrelated file, the barrier is ring-wide. If it overtakes here but not
    // above, what looked like a ring barrier was the filesystem serializing on
    // the flushed file.
    run_case(&ring, &mut big, &mut small, "DRAIN, phase B on ANOTHER file", true, false, true);
    run_case(&ring, &mut big, &mut small, "CONTROL, phase B on ANOTHER file", false, false, true);

    // SAFETY: every case drained to completion above.
    unsafe {
        let rc = CloseIoRing(ring.handle);
        assert_eq!(rc, S_OK, "CloseIoRing failed: 0x{:08X}", rc as u32);
        CloseHandle(file);
        CloseHandle(other);
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&other_path);
    println!("\ndone.");
}
