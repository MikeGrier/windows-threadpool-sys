// Copyright (c) 2026 Mike Grier
//! A binary that commits a chosen memory violation under
//! [`GuardAlloc`](windows_guard_alloc::GuardAlloc), so
//! the `faults` integration test can observe what the allocator does about it.
//!
//! This exists because an access violation cannot be caught in-process: the
//! only way to assert "this faults" is to run it somewhere else and look at the
//! exit code. It is a **test fixture**, not a demonstration -- nothing here is
//! a pattern to copy.

#[global_allocator]
static ALLOC: windows_guard_alloc::GuardAlloc = windows_guard_alloc::GuardAlloc::new();

/// Exit code for a violation that was *not* caught. Distinct from `0` so the
/// test can tell "survived the violation" apart from "did not attempt one".
const UNDETECTED: i32 = 42;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();

    // Prove the allocator works at all before asking it to reject anything. If
    // ordinary allocation were broken, a fault below would prove nothing about
    // guard pages -- it would just mean the allocator is unusable.
    let mut warmup: Vec<u8> = Vec::with_capacity(64);
    warmup.extend_from_slice(b"the quick brown fox");
    assert_eq!(warmup.len(), 19);
    drop(warmup);

    assert!(
        ALLOC.total_allocations() > 0,
        "the guard allocator is not installed, so this fixture would prove nothing"
    );

    match mode.as_str() {
        // The control. Nothing illegal happens, so the process must exit
        // cleanly -- otherwise a "fault" in the other modes might just mean
        // the allocator is broken.
        "clean" => {
            let v: Vec<u8> = vec![0xAB; 200];
            let sum: usize = v.iter().map(|b| *b as usize).sum();
            println!("clean: sum={sum}");
        }

        // Issue #48's shape: hold a pointer past the free and read through it,
        // the way the kernel does when it reads a registration array at submit
        // time rather than at build time.
        "uaf" => {
            let v: Vec<u8> = vec![0xAB; 32];
            let ptr = v.as_ptr();
            drop(v);
            // SAFETY: deliberately unsound. The whole point is that the
            // allocator must make this fault instead of returning stale bytes.
            let byte = unsafe { std::ptr::read_volatile(ptr) };
            println!("UNDETECTED: read {byte:#x} out of freed memory");
            std::process::exit(UNDETECTED);
        }

        // One byte past the end, which is what right-alignment against the
        // guard page is for.
        "overrun" => {
            let v: Vec<u8> = vec![0u8; 32];
            let ptr = v.as_ptr();
            // SAFETY: deliberately unsound, as above.
            let byte = unsafe { std::ptr::read_volatile(ptr.add(32)) };
            println!("UNDETECTED: read {byte:#x} past the end");
            std::process::exit(UNDETECTED);
        }

        // A write past the end. Separate from the read case because a write
        // into a guard page and a read out of one are different access types,
        // and only checking one would leave the other unproven.
        "overrun-write" => {
            let mut v: Vec<u8> = vec![0u8; 32];
            let ptr = v.as_mut_ptr();
            // SAFETY: deliberately unsound, as above.
            unsafe { std::ptr::write_volatile(ptr.add(32), 0xFF) };
            println!("UNDETECTED: wrote past the end");
            std::process::exit(UNDETECTED);
        }

        // A large allocation, to prove the guard is placed correctly when the
        // allocation spans several pages -- the case where recovering the
        // reservation base by masking to a page boundary would have been wrong.
        "overrun-large" => {
            let v: Vec<u8> = vec![0u8; 9000];
            let ptr = v.as_ptr();
            // SAFETY: deliberately unsound, as above.
            let byte = unsafe { std::ptr::read_volatile(ptr.add(9000)) };
            println!("UNDETECTED: read {byte:#x} past the end of a large allocation");
            std::process::exit(UNDETECTED);
        }

        other => {
            eprintln!("unknown mode {other:?}");
            std::process::exit(2);
        }
    }
}
