//! Where does Rust's allocator stop using the heap, and does chunk size matter?
//!
//! Two questions, prompted by a review rule of thumb: avoid contiguous
//! allocations over 64 KB from the general heap, because most heaps move large
//! blocks to a separate space anyway and the threshold is a useful place to be
//! asked "does this really need to be contiguous?".
//!
//! On Windows 64 KB is not an arbitrary round number -- it is the virtual
//! memory allocation granularity, so a reservation cannot share its 64 KB
//! region with anything else.
//!
//! # Q1 -- where is the knee?
//!
//! For each allocation, ask `VirtualQuery` which reservation it belongs to and
//! count the distinct `AllocationBase` values across many allocations of one
//! size. Blocks carved out of a heap segment share their segment's base; a
//! block given its own `VirtualAlloc` region has a base of its own.
//!
//! Two earlier attempts failed and are recorded because both look reasonable:
//!
//! - **Testing for exact 64 KB alignment** reported "none" at every size up to
//!   4 MB. `HeapAlloc`'s large-block path does call `VirtualAlloc`, but writes
//!   a header at the start of the region and returns a pointer past it, so a
//!   large block is aligned *plus a constant* and never exactly aligned.
//! - **Counting distinct offsets within a 64 KB region** declined with size
//!   (64 at 4 KB, 15 at 1 MB) but never reached 1, so it could not say where
//!   the change happened. Suggestive is not decisive.
//!
//! `VirtualQuery` answers directly and needs no inference.
//!
//! # Q2 -- does it cost anything to stay under it?
//!
//! Time a fixed zero-fill using chunks of each size. If a small chunk fills as
//! fast as a large one, then respecting the rule is free and the chunk should
//! be small. If throughput climbs with chunk size, the rule has a price and the
//! price is what decides.

use std::fs::File;
use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
use std::io::Write;
use std::time::Instant;

const KB: usize = 1024;
const SIZES: &[usize] = &[
    4 * KB,
    16 * KB,
    32 * KB,
    64 * KB,
    128 * KB,
    256 * KB,
    512 * KB,
    1024 * KB,
    4096 * KB,
];

/// Allocations per size for Q1. Enough that an accidental alignment cannot
/// look like a threshold.
const SAMPLES: usize = 64;

/// Bytes filled per chunk size in Q2. Large enough to be dominated by the
/// filling rather than by opening the file.
const FILL_TOTAL: usize = 64 * 1024 * 1024;

const FILL_REPEATS: usize = 7;

fn main() {
    println!("Q1: do allocations of this size get their own reservation?\n");
    println!(
        "{:>10}  {:>20}  {:>16}  {:>22}",
        "size", "distinct bases/64", "median region", "verdict"
    );

    for &size in SIZES {
        // Held until all are made, so the allocator cannot hand back the same
        // address every time and flatter the result.
        let mut live: Vec<Vec<u8>> = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let mut v: Vec<u8> = Vec::with_capacity(size);
            // Touch a byte so the allocation is real rather than deferred.
            v.push(0);
            live.push(v);
        }

        let mut bases: Vec<usize> = Vec::with_capacity(SAMPLES);
        let mut regions: Vec<usize> = Vec::with_capacity(SAMPLES);
        for v in &live {
            let mut info = MEMORY_BASIC_INFORMATION::default();
            // SAFETY: `v` is a live allocation this thread owns, and `info` is
            // a live local of exactly the size passed.
            let got = unsafe {
                VirtualQuery(
                    v.as_ptr().cast(),
                    &raw mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            assert_ne!(got, 0, "VirtualQuery failed");
            bases.push(info.AllocationBase as usize);
            regions.push(info.RegionSize);
        }
        bases.sort_unstable();
        bases.dedup();
        regions.sort_unstable();
        let median_region = regions[regions.len() / 2];

        let verdict = if bases.len() == SAMPLES {
            "own reservation each"
        } else if bases.len() * 4 >= SAMPLES {
            "mostly own"
        } else {
            "shares a heap segment"
        };
        println!(
            "{:>8} KB  {:>20}  {:>13} KB  {:>22}",
            size / KB,
            bases.len(),
            median_region / KB,
            verdict
        );
        drop(live);
    }

    println!("\nQ2: does a smaller chunk fill more slowly?\n");
    println!(
        "{:>10}  {:>14}  {:>16}",
        "chunk", "median ms", "MiB/s (median)"
    );

    let path = std::env::temp_dir().join(format!("chunk-probe-{}.tmp", std::process::id()));
    for &chunk_len in SIZES {
        let mut samples = Vec::with_capacity(FILL_REPEATS);
        for _ in 0..FILL_REPEATS {
            let chunk = vec![0_u8; chunk_len];
            let t = Instant::now();
            let mut file = File::create(&path).expect("create");
            let mut written = 0;
            while written < FILL_TOTAL {
                let take = chunk.len().min(FILL_TOTAL - written);
                file.write_all(&chunk[..take]).expect("write");
                written += take;
            }
            file.flush().expect("flush");
            drop(file);
            samples.push(t.elapsed().as_micros());
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        let mib_s = (FILL_TOTAL as f64 / (1024.0 * 1024.0)) / (median as f64 / 1_000_000.0);
        println!(
            "{:>8} KB  {:>14.1}  {:>16.0}",
            chunk_len / KB,
            median as f64 / 1000.0,
            mib_s
        );
    }
    let _ = std::fs::remove_file(&path);

    println!(
        "\nRead Q2 across the rows, not against a target. What decides the chunk size is\n\
         whether throughput is still climbing at the point the allocation stops being an\n\
         ordinary heap block."
    );
}
