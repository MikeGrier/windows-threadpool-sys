// Copyright (c) 2026 Mike Grier
//! What the epoch-log sample's pre-allocation actually costs at its real sizes.
//!
//! The GiB figures in the cost spike answer "what does zeroing cost per unit".
//! This answers "what does this program actually pay", which is a different
//! question and the one that decides whether any of it matters here.

use std::fs::File;
use std::io::Write;
use std::time::Instant;

const STRIDE: usize = 4096;
/// Matches the sample's own fill chunk (`examples/epoch_log/logfile.rs`).
/// The measured operation includes the write loop's chunking, so a benchmark
/// that chunked differently would not be measuring the implementation it
/// reports on. This was 1 MiB until review caught the mismatch.
const FILL_CHUNK: usize = 64 * 1024;

/// Sizes this sample actually asks for, derived from main.rs's constants.
const CASES: &[(&str, usize)] = &[
    // RECORDS 24 + TAIL_RECORDS 3 + SLACK_BLOCKS 8
    ("log (35 blocks)", 35),
    // EPOCHS 32 * PER_EPOCH 64 + SLACK_BLOCKS 8
    ("one strategy file (2056 blocks)", 2056),
];

const REPEATS: usize = 50;

fn fill(path: &std::path::Path, len: usize) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    let chunk = vec![0_u8; FILL_CHUNK.min(len.max(1))];
    let mut written = 0;
    while written < len {
        let take = chunk.len().min(len - written);
        file.write_all(&chunk[..take])?;
        written += take;
    }
    file.flush()
}

fn main() {
    println!("What the epoch-log sample's zero-fill actually costs\n");
    println!(
        "{:<34} {:>12} {:>14} {:>14}",
        "case", "bytes", "median us", "max us"
    );

    let mut total_bytes = 0_usize;
    let mut total_us = 0_u128;

    for (label, blocks) in CASES {
        let len = blocks * STRIDE;
        let path = std::env::temp_dir().join(format!("prealloc-cost-{}.tmp", std::process::id()));

        // One untimed run so the file exists and any first-touch cost is not
        // charged to the first measurement.
        fill(&path, len).expect("fill");

        let mut samples = Vec::with_capacity(REPEATS);
        for _ in 0..REPEATS {
            let t = Instant::now();
            fill(&path, len).expect("fill");
            samples.push(t.elapsed().as_micros());
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        let max = *samples.last().expect("REPEATS is not zero");

        println!("{:<34} {:>12} {:>14} {:>14}", label, len, median, max);

        let _ = std::fs::remove_file(&path);

        // The sample creates one log and three strategy files per run.
        let copies = if *blocks == 35 { 1 } else { 3 };
        total_bytes += len * copies;
        total_us += median * copies as u128;
    }

    println!(
        "\nOne whole run of the sample pre-allocates {} bytes ({:.1} MiB) across four files,\n\
         for about {} us ({:.1} ms) of zero-filling in total.",
        total_bytes,
        total_bytes as f64 / (1024.0 * 1024.0),
        total_us,
        total_us as f64 / 1000.0
    );
}
