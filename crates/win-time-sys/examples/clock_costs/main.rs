// Copyright (c) 2026 Mike Grier
//! What each clock costs to read, and how finely it moves, on the machine this runs on (`WT-1.6`).
//!
//! ```text
//! cargo run --release -p win-time-sys --example clock_costs
//! ```
//!
//! Each clock is read in batches of consecutive reads, timed with `Instant`; the cost per read is a
//! batch's time divided by its reads, reported as the least, the median and the most over the
//! batches. Its step is the smallest change seen between consecutive readings that differ. Two
//! readings from `std` are included for reference. The output is what was observed here and makes
//! no comparison: what a reading costs depends on the processor, the hypervisor and how the
//! performance counter is backed, and this measures only the machine it runs on.

use std::fmt;
use std::hint::black_box;
use std::io::{self, Write as _};
use std::time::{Duration, Instant, SystemTime};

use win_time_sys::{
    Clock, CoarseSystemClock, InterruptClock, PerformanceClock, PerformanceCounter,
    PreciseInterruptClock, PreciseSystemClock, PreciseUnbiasedInterruptClock, TimePoint, Timeline,
    UnbiasedInterruptClock,
};

#[cfg(test)]
mod tests;

/// Reads timed together, so the timing's own cost is spread across them.
const READS_PER_BATCH: u32 = 10_000;
/// Batches per clock; odd, so the median is one of them.
const BATCHES: usize = 31;
/// Distinct steps to see before reporting the smallest.
const STEPS: usize = 5;
/// How long to wait for those steps before reporting what was seen.
const STEP_BUDGET: Duration = Duration::from_millis(250);

/// One clock's observations.
struct Row {
    name: &'static str,
    /// Least, median and most nanoseconds per read.
    cost: Summary,
    /// The smallest step seen, if the clock moved at all within the budget.
    step: Option<Duration>,
}

/// Least, median and most of some samples.
#[derive(Debug, PartialEq)]
struct Summary {
    least: f64,
    median: f64,
    most: f64,
}

/// The least, the median -- the lower of the two middle samples, for an even count -- and the most.
fn summarize(samples: &mut [f64]) -> Option<Summary> {
    samples.sort_by(f64::total_cmp);
    Some(Summary {
        least: *samples.first()?,
        median: samples[(samples.len() - 1) / 2],
        most: *samples.last()?,
    })
}

/// The smallest positive difference between consecutive readings, in readings' own units.
fn smallest_step(readings: &[u128]) -> Option<u128> {
    readings
        .windows(2)
        .filter_map(|pair| pair[1].checked_sub(pair[0]).filter(|step| *step > 0))
        .min()
}

/// Nanoseconds per read of `read`, over each batch.
fn cost<R>(read: impl Fn() -> R) -> Summary {
    for _ in 0..READS_PER_BATCH {
        black_box(read());
    }
    let mut samples: Vec<f64> = (0..BATCHES)
        .map(|_| {
            let started = Instant::now();
            for _ in 0..READS_PER_BATCH {
                black_box(read());
            }
            started.elapsed().as_nanos() as f64 / f64::from(READS_PER_BATCH)
        })
        .collect();
    summarize(&mut samples).expect("at least one batch")
}

/// The smallest step of `nanos`, a reading in nanoseconds from some fixed origin.
fn step(nanos: impl Fn() -> u128) -> Option<Duration> {
    let started = Instant::now();
    let mut readings = vec![nanos()];
    while readings.len() <= STEPS && started.elapsed() < STEP_BUDGET {
        let next = nanos();
        if next != *readings.last().expect("one reading") {
            readings.push(next);
        }
    }
    smallest_step(&readings)
        .map(|step| Duration::from_nanos(u64::try_from(step).unwrap_or(u64::MAX)))
}

/// A reading of `clock`, in nanoseconds since its timeline's zero.
fn clock_nanos<C: Clock>(clock: &C) -> u128 {
    let since_zero = clock.now() - TimePoint::from_ticks(0);
    since_zero.abs_duration().as_nanos()
}

fn clock_row<C: Clock>(name: &'static str, clock: C) -> Row {
    Row {
        name,
        cost: cost(|| clock.now()),
        step: step(|| clock_nanos(&clock)),
    }
}

fn rows() -> Vec<Row> {
    let mut rows = vec![
        clock_row("InterruptClock", InterruptClock),
        clock_row("PreciseInterruptClock", PreciseInterruptClock),
        clock_row("UnbiasedInterruptClock", UnbiasedInterruptClock),
        clock_row(
            "PreciseUnbiasedInterruptClock",
            PreciseUnbiasedInterruptClock,
        ),
        clock_row("CoarseSystemClock", CoarseSystemClock),
        clock_row("PreciseSystemClock", PreciseSystemClock),
        clock_row("PerformanceClock", PerformanceClock),
    ];
    let origin = Instant::now();
    rows.push(Row {
        name: "std Instant (reference)",
        cost: cost(Instant::now),
        step: step(|| origin.elapsed().as_nanos()),
    });
    rows.push(Row {
        name: "std SystemTime (reference)",
        cost: cost(SystemTime::now),
        step: step(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos())
        }),
    });
    rows
}

/// The report, written to `out`: the one place this probe produces output.
fn report(out: &mut impl fmt::Write, frequency: u64, rows: &[Row]) -> fmt::Result {
    writeln!(out, "win-time-sys clock costs, observed on this machine")?;
    writeln!(out, "performance counter frequency: {frequency} Hz")?;
    writeln!(
        out,
        "{READS_PER_BATCH} reads per batch, {BATCHES} batches; ns per read as least / median / most"
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "{:<32} {:>10} {:>10} {:>10}   {:>14}",
        "clock", "least", "median", "most", "smallest step"
    )?;
    for row in rows {
        let step = row
            .step
            .map_or_else(|| "none seen".to_owned(), |step| format!("{step:?}"));
        writeln!(
            out,
            "{:<32} {:>10.1} {:>10.1} {:>10.1}   {:>14}",
            row.name, row.cost.least, row.cost.median, row.cost.most, step
        )?;
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let frequency = PerformanceCounter::ticks_per_second().get();
    let rows = rows();
    let mut text = String::new();
    report(&mut text, frequency, &rows).expect("writing to a String does not fail");
    io::stdout().lock().write_all(text.as_bytes())
}
