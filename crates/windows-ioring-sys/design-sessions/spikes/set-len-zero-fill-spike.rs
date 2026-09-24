//! Does `set_len` avoid the zero-fill, or force it? -- measured, not reasoned.
//!
//! # Why this exists
//!
//! `M25.3` opens the epoch log over a **zero-filled** extent -- a real write of
//! zeros -- and its first draft justified that by asserting `set_len` would
//! leave writes in the extending case. That was reasoned from documentation.
//! Review supplied a specific counter-recollection, from optimizing `.cab`
//! expansion some years earlier: that setting the length **too early forced**
//! the zero fill of large files rather than deferring it.
//!
//! Both statements are about the same mechanism and they cannot both be the
//! whole story, so this measures it. The recollection turned out to be right
//! and to have a sharper shape than either statement had: the forcing is real,
//! it is conditional on *where* the write lands, and when it fires it costs an
//! order of magnitude more than doing the zeroing up front.
//!
//! # What it runs
//!
//! Five cases per size, each on a fresh file, timed separately:
//!
//! 1. `set_len(n)` alone.
//! 2. A zero-fill: writing `n` bytes of zeros.
//! 3. `set_len(n)` then one sector at offset 0 -- no gap in front of it.
//! 4. `set_len(n)` then one sector at the END, offset `n - SECTOR` -- a whole
//!    file's worth of gap between the valid data length and the write.
//! 5. `set_len(n)` then the whole extent written sequentially. This is a log's
//!    pattern: every write lands exactly at the valid data length.
//!
//! # How to read it
//!
//! **Case 4 against case 3** is the discriminator for the forcing. Both start
//! from an identical `set_len`'d file and write one sector; only the gap in
//! front of the write differs.
//!
//! **Case 5 against case 2** is the discriminator for whether a *sequential*
//! writer pays anything for having set its length first.
//!
//! # What it cannot tell you
//!
//! Nothing here is a platform guarantee. These are wall-clock costs on one
//! machine, one filesystem, one device, and NTFS is free to change how it
//! tracks valid data length. What the figures support is a shape -- free,
//! free, free, catastrophic, free -- not a constant to design against.
//!
//! # Running it
//!
//! Standalone, no dependencies. Copy into a scratch crate's `src/main.rs` and
//! `cargo run --release`, the way `tools/run-numa-spikes.ps1` builds the NUMA
//! spikes. It writes up to 1 GiB files into `%TEMP%` and removes them.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::time::Instant;

const SECTOR: usize = 4096;

fn path(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("setlen-probe-{}-{tag}.tmp", std::process::id()))
}

fn fresh(tag: &str) -> (std::path::PathBuf, File) {
    let p = path(tag);
    let _ = std::fs::remove_file(&p);
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&p)
        .expect("create");
    (p, f)
}

fn ms(t: Instant) -> u128 {
    t.elapsed().as_millis()
}

fn main() {
    println!("set_len vs zero-fill: is the zeroing avoided, or moved?\n");
    println!(
        "{:>8}  {:>12}  {:>12}  {:>16}  {:>16}  {:>18}",
        "size", "set_len", "zero-fill", "set_len+head", "set_len+tail", "set_len+sequential"
    );

    for mib in [64_usize, 256, 1024] {
        let n = mib * 1024 * 1024;

        // 1. set_len alone.
        let (p1, f1) = fresh("setlen");
        let t = Instant::now();
        f1.set_len(n as u64).expect("set_len");
        drop(f1);
        let a = ms(t);

        // 2. A real zero-fill of the same extent.
        let (p2, mut f2) = fresh("zerofill");
        let zeros = vec![0_u8; 8 * 1024 * 1024];
        let t = Instant::now();
        let mut written = 0;
        while written < n {
            let take = zeros.len().min(n - written);
            f2.write_all(&zeros[..take]).expect("write");
            written += take;
        }
        f2.flush().expect("flush");
        drop(f2);
        let b = ms(t);

        // 3. set_len, then one sector at the FRONT. No gap between the valid
        //    data length and the write.
        let (p3, mut f3) = fresh("head");
        f3.set_len(n as u64).expect("set_len");
        let t = Instant::now();
        f3.seek(SeekFrom::Start(0)).expect("seek");
        f3.write_all(&vec![0xAB_u8; SECTOR]).expect("write");
        f3.flush().expect("flush");
        drop(f3);
        let c = ms(t);

        // 4. set_len, then one sector at the END. A whole file of gap.
        let (p4, mut f4) = fresh("tail");
        f4.set_len(n as u64).expect("set_len");
        let t = Instant::now();
        f4.seek(SeekFrom::Start((n - SECTOR) as u64)).expect("seek");
        f4.write_all(&vec![0xCD_u8; SECTOR]).expect("write");
        f4.flush().expect("flush");
        drop(f4);
        let d = ms(t);

        // 5. set_len, then fill the whole extent sequentially. This is a log's
        //    access pattern: every write lands exactly at the valid data
        //    length, so no write ever has a gap in front of it.
        let (p5, mut f5) = fresh("sequential");
        f5.set_len(n as u64).expect("set_len");
        let t = Instant::now();
        let mut written = 0;
        while written < n {
            let take = zeros.len().min(n - written);
            f5.write_all(&zeros[..take]).expect("write");
            written += take;
        }
        f5.flush().expect("flush");
        drop(f5);
        let e = ms(t);

        println!(
            "{:>6} MiB  {:>10} ms  {:>10} ms  {:>14} ms  {:>14} ms  {:>16} ms",
            mib, a, b, c, d, e
        );

        for p in [p1, p2, p3, p4, p5] {
            let _ = std::fs::remove_file(p);
        }
    }

    println!(
        "\nRead case 4 against case 3. Both begin from an identical set_len'd file and\n\
         write one sector. The only difference is how much unwritten extent lies between\n\
         the valid data length and the write.\n\n\
         Read case 5 against case 2. Both end with the whole extent written; case 5 had\n\
         its length set first. That is a log's pattern, where every write lands at the\n\
         valid data length and no write ever has a gap in front of it."
    );
}
