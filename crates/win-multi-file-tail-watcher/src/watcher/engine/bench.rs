// Copyright (c) 2026 Mike Grier
//! Timings of each pass against the size of the roster (TW-2.8), printed as `BENCH` lines.
//!
//! Ignored, so the ordinary test run does not pay for them. Run in release, one at a time:
//! `cargo test --release -p win-multi-file-tail-watcher bench -- --ignored --nocapture --test-threads=1`.

use std::collections::BTreeMap;
use std::os::windows::io::AsRawHandle;
use std::sync::Barrier;
use std::time::{Duration, Instant};

use windows_overlapped_io_sys::EndpointObserver;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
};

use super::{Engine, REPORT_BATCH, RunOutcome};
use crate::Registration;
use crate::chunked::Chunked;
use crate::test_support::{TempDir, wtf};
use crate::watcher::probe::Change;
use crate::watcher::{Entry, SNAPSHOT_BATCH, SnapshotItem, Watcher};

// The stated top end is about 20,000 files (DESIGN-NOTES.md); the larger sizes are headroom.
const POPULATIONS: [usize; 4] = [1_000, 5_000, 20_000, 50_000];

// Distinct files on disk, cycled, so a large population does not mean creating that many files.
const DISTINCT_FILES: usize = 4_096;

fn us(d: Duration) -> f64 {
    d.as_secs_f64() * 1e6
}

fn percentile(sorted: &[Duration], p: usize) -> Duration {
    sorted[(sorted.len() - 1) * p / 100]
}

// The spans between calls of the pause, which is where each batch's lock hold shows: the pause
// runs with the lock released and does nothing else.
struct Holds {
    last: Instant,
    spans: Vec<Duration>,
}

impl Holds {
    fn new() -> Self {
        Self {
            last: Instant::now(),
            spans: Vec::new(),
        }
    }

    fn mark(&mut self) {
        let now = Instant::now();
        self.spans.push(now - self.last);
        self.last = Instant::now();
    }

    fn line(mut self, pass: &str, n: usize, total: Duration) {
        self.spans.sort_unstable();
        let (p50, p99, max) = if self.spans.is_empty() {
            (Duration::ZERO, Duration::ZERO, Duration::ZERO)
        } else {
            (
                percentile(&self.spans, 50),
                percentile(&self.spans, 99),
                *self.spans.last().expect("not empty"),
            )
        };
        println!(
            "BENCH pass={pass} n={n} total_us={:.0} ns_per={:.0} batches={} hold_p50_us={:.1} hold_p99_us={:.1} hold_max_us={:.1}",
            us(total),
            total.as_nanos() as f64 / n as f64,
            self.spans.len(),
            us(p50),
            us(p99),
            us(max),
        );
    }
}

fn line(pass: &str, n: usize, total: Duration) {
    println!(
        "BENCH pass={pass} n={n} total_us={:.0} ns_per={:.0}",
        us(total),
        total.as_nanos() as f64 / n as f64,
    );
}

// A fixed permutation of `0..n`, so a run is repeatable.
fn permutation(n: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).collect();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in (1..n).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        order.swap(i, (state >> 33) as usize % (i + 1));
    }
    order
}

struct Population {
    _dir: TempDir,
    paths: Vec<std::path::PathBuf>,
}

fn population(n: usize) -> Population {
    population_of(n, n.min(DISTINCT_FILES))
}

fn population_of(n: usize, distinct: usize) -> Population {
    let dir = TempDir::new(&format!("bench-{n}-{distinct}"));
    let paths = (0..distinct)
        .map(|i| dir.file(format!("f{i}.log"), &vec![b'x'; 16 + i % 64]))
        .collect();
    Population { _dir: dir, paths }
}

fn register_all(
    watcher: &Watcher<u32>,
    files: &Population,
    n: usize,
) -> Vec<Option<Registration<u32>>> {
    (0..n)
        .map(|i| {
            let path = &files.paths[i % files.paths.len()];
            Some(watcher.register(&wtf(path), 0, i as u32).expect("register"))
        })
        .collect()
}

fn watcher_for(n: usize) -> Watcher<u32> {
    Watcher::new((n + 2).next_power_of_two()).expect("watcher")
}

fn timed<R>(work: impl FnOnce() -> R) -> (R, Duration) {
    let start = Instant::now();
    let result = work();
    (result, start.elapsed())
}

#[test]
#[ignore = "a timing run (TW-2.8), not a test"]
fn bench_the_passes_against_the_roster_size() {
    println!(
        "BENCH config snapshot_batch={SNAPSHOT_BATCH} report_batch={REPORT_BATCH} entry_bytes={} snapshot_item_bytes={} change_bytes={} distinct_files={DISTINCT_FILES}",
        size_of::<Entry<u32>>(),
        size_of::<SnapshotItem>(),
        size_of::<Change>(),
    );
    for n in POPULATIONS {
        let files = population(n);
        let watcher = watcher_for(n);

        let (mut registrations, took) = timed(|| register_all(&watcher, &files, n));
        line("register", n, took);

        // The snapshot, in the same batches the engine uses.
        let mut snapshot = Chunked::new();
        let mut holds = Holds::new();
        let (_, took) = timed(|| {
            holds.last = Instant::now();
            watcher.snapshot_batches(&mut snapshot, SNAPSHOT_BATCH, || holds.mark());
        });
        holds.line("snapshot", n, took);

        // The probe, every file a change (each is registered at offset 0 and not empty).
        let mut changed = Chunked::new();
        let (_, took) = timed(|| {
            watcher.probe_with(&mut snapshot, &mut changed, EndpointObserver::file_len);
        });
        line("probe_all_changed", n, took);
        assert_eq!(changed.len(), n);

        // The report, every change written, nothing refused.
        let mut engine = Engine::new(&watcher);
        engine.begin(&watcher, EndpointObserver::file_len);
        let mut holds = Holds::new();
        let (outcome, took) = timed(|| {
            holds.last = Instant::now();
            engine.finish(&watcher, REPORT_BATCH, || holds.mark())
        });
        assert_eq!(outcome, RunOutcome::Complete);
        holds.line("report_all_changed", n, took);
        assert_eq!(watcher.drain().count(), n + 1);

        // The steady state: a whole poll in which nothing changed.
        let (outcome, took) = timed(|| engine.run(&watcher));
        assert_eq!(outcome, RunOutcome::Complete);
        line("poll_nothing_changed", n, took);
        assert_eq!(watcher.drain().count(), 1);

        // Reporting through a small SQ: every stop and every continuation.
        let small = Watcher::<u32>::new(64).expect("watcher");
        let kept = register_all(&small, &files, n);
        let mut engine = Engine::new(&small);
        let (stops, took) = timed(|| {
            let mut stops = 0usize;
            loop {
                let outcome = engine.run(&small);
                small.drain().for_each(drop);
                match outcome {
                    RunOutcome::Stopped => stops += 1,
                    RunOutcome::Complete => break stops,
                }
            }
        });
        println!(
            "BENCH pass=poll_all_changed_sq64 n={n} total_us={:.0} ns_per={:.0} stops={stops}",
            us(took),
            took.as_nanos() as f64 / n as f64,
        );
        drop(kept);

        // The roster under churn: one registration dropped and another made, at random.
        let order = permutation(n);
        let cycles = n / 10;
        let (_, took) = timed(|| {
            for (cycle, &slot) in order.iter().take(cycles).enumerate() {
                registrations[slot] = None;
                let path = &files.paths[slot % files.paths.len()];
                registrations[slot] = Some(
                    watcher
                        .register(&wtf(path), 0, (n + cycle) as u32)
                        .expect("register"),
                );
            }
        });
        line("churn_replace_random", cycles, took);

        // Everything dropped, in random order.
        let (_, took) = timed(|| {
            for &slot in &order {
                registrations[slot] = None;
            }
        });
        line("drop_all_random", n, took);
        assert_eq!(watcher.registered(), 0);
    }
}

// The map with an entry-sized value and no file behind it, so the share of the figures above that
// is the map's own can be told from the file's.
#[test]
#[ignore = "a timing run (TW-2.8), not a test"]
fn bench_the_roster_map_alone() {
    type Value = [u8; size_of::<Entry<u32>>()];
    for n in POPULATIONS {
        let mut map: BTreeMap<u64, Value> = BTreeMap::new();
        let (_, took) = timed(|| {
            for id in 0..n as u64 {
                map.insert(id, [0; size_of::<Entry<u32>>()]);
            }
        });
        line("map_append", n, took);

        let mut holds = Holds::new();
        let (walked, took) = timed(|| {
            holds.last = Instant::now();
            let mut after = None;
            let mut walked = 0usize;
            loop {
                let from = after.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded);
                let mut scanned = 0;
                for (&id, _) in map
                    .range((from, std::ops::Bound::Unbounded))
                    .take(SNAPSHOT_BATCH)
                {
                    after = Some(id);
                    scanned += 1;
                }
                walked += scanned;
                if scanned < SNAPSHOT_BATCH {
                    break walked;
                }
                holds.mark();
            }
        });
        assert_eq!(walked, n);
        holds.line("map_walk", n, took);

        let order = permutation(n);
        let (_, took) = timed(|| {
            for &slot in &order {
                map.remove(&(slot as u64));
            }
        });
        line("map_remove_random", n, took);
    }
}

// The one kernel call `file_len` makes, issued directly: what is left of `file_len`'s time once
// the crate's own code is taken away.
fn bare_len(observer: &EndpointObserver) -> u64 {
    let mut info = FILE_STANDARD_INFO::default();
    // SAFETY: the borrow serves one synchronous query and issues no I/O, which is all `handle`
    // allows.
    let handle = unsafe { observer.handle() };
    // SAFETY: a live handle, and a buffer of exactly `FILE_STANDARD_INFO`'s size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle.as_raw_handle(),
            FileStandardInfo,
            (&raw mut info).cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    assert_ne!(ok, 0, "the query failed");
    info.EndOfFile as u64
}

fn crate_len(observer: &EndpointObserver) -> u64 {
    observer.file_len().expect("a length")
}

// Wall time from the moment every thread is released to the moment the last one is done; starting
// the threads is not in it.
fn run_across(
    observers: &[EndpointObserver],
    threads: usize,
    query: fn(&EndpointObserver) -> u64,
) -> Duration {
    let per = observers.len().div_ceil(threads);
    let parts = observers.chunks(per).count();
    let gate = Barrier::new(parts + 1);
    std::thread::scope(|scope| {
        let workers: Vec<_> = observers
            .chunks(per)
            .map(|part| {
                let gate = &gate;
                scope.spawn(move || {
                    gate.wait();
                    part.iter().map(query).sum::<u64>()
                })
            })
            .collect();
        gate.wait();
        let start = Instant::now();
        let total: u64 = workers
            .into_iter()
            .map(|worker| worker.join().expect("a worker"))
            .sum();
        let took = start.elapsed();
        assert!(total > 0);
        took
    })
}

#[test]
#[ignore = "a timing run (TW-2.8), not a test"]
fn bench_the_query_alone_and_across_threads() {
    const REPS: usize = 7;
    const THREADS: [usize; 6] = [1, 2, 4, 8, 12, 16];
    let n = 20_000;
    for (layout, distinct) in [("shared", DISTINCT_FILES), ("distinct", n)] {
        let files = population_of(n, distinct);
        let watcher = watcher_for(n);
        let _registrations = register_all(&watcher, &files, n);
        let mut snapshot = Chunked::new();
        watcher.snapshot(&mut snapshot);
        let mut observers = Vec::new();
        snapshot.drain_each(|item| observers.push(item.observer));
        assert_eq!(observers.len(), n);

        for (name, query) in [
            ("bare_call", bare_len as fn(&EndpointObserver) -> u64),
            ("file_len", crate_len),
        ] {
            for threads in THREADS {
                run_across(&observers, threads, query);
                let mut walls: Vec<Duration> = (0..REPS)
                    .map(|_| run_across(&observers, threads, query))
                    .collect();
                walls.sort_unstable();
                let median = walls[REPS / 2];
                println!(
                    "BENCH pass=query_{name} n={n} files={layout} threads={threads} wall_us_min={:.0} wall_us_median={:.0} wall_us_max={:.0} ns_per_median={:.0}",
                    us(walls[0]),
                    us(median),
                    us(walls[REPS - 1]),
                    median.as_nanos() as f64 / n as f64,
                );
            }
        }
    }
}
