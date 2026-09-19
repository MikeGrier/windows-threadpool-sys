// Copyright (c) Mike Grier.
#![cfg(windows)]

mod experiment;
mod platform;
mod reader;

use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use windows_topology_sys::ProcessorId;

pub use experiment::{Capture, run};

const MAX_BUFFERS: usize = 1024;
const MAX_POOL_BYTES: usize = 256 * 1024 * 1024;
const MAX_BLOCKS: usize = 1_000_000;
const MAX_PASSES: u32 = 1024;
const MAX_REPETITIONS: usize = 60;
const MAX_TIMEOUT_MS: u64 = 300_000;
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const CHECKSUM_POLL_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub file: PathBuf,
    pub block_bytes: usize,
    pub depth: usize,
    #[serde(default)]
    pub buffer_count: Option<usize>,
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    pub queue_capacity: usize,
    pub checksum_passes: u32,
    pub repetitions: usize,
    pub timeout_ms: u64,
    pub processors: Option<[ProcessorId; 2]>,
}

impl Config {
    pub fn buffers(&self) -> usize {
        self.buffer_count.unwrap_or(self.depth)
    }

    pub fn validate(&self, file_bytes: u64) -> io::Result<usize> {
        if file_bytes > MAX_FILE_BYTES {
            return Err(invalid("fixture exceeds the experiment's 1 GiB limit"));
        }
        if self.block_bytes == 0 || self.block_bytes > u32::MAX as usize {
            return Err(invalid("block_bytes must fit a nonzero u32"));
        }
        if self.depth < 2 || !self.depth.is_power_of_two() || self.depth > MAX_BUFFERS {
            return Err(invalid("depth must be a power of two in 2..=1024"));
        }
        let buffers = self.buffers();
        if buffers < self.depth || !buffers.is_power_of_two() || buffers > MAX_BUFFERS {
            return Err(invalid(
                "buffer_count must be a power of two in depth..=1024",
            ));
        }
        if !(1..=MAX_BUFFERS).contains(&self.batch_size) {
            return Err(invalid("batch_size must be in 1..=1024"));
        }
        if self.queue_capacity == 0
            || !self.queue_capacity.is_power_of_two()
            || self.queue_capacity > buffers
        {
            return Err(invalid(
                "queue_capacity must be a power of two no greater than buffer_count",
            ));
        }
        if self
            .block_bytes
            .checked_mul(buffers)
            .is_none_or(|n| n > MAX_POOL_BYTES)
        {
            return Err(invalid("payload pool exceeds 256 MiB"));
        }
        if !(1..=MAX_PASSES).contains(&self.checksum_passes)
            || !(1..=MAX_REPETITIONS).contains(&self.repetitions)
            || !(1..=MAX_TIMEOUT_MS).contains(&self.timeout_ms)
        {
            return Err(invalid(
                "passes, repetitions or timeout outside experiment limits",
            ));
        }
        let blocks = file_bytes.div_ceil(self.block_bytes as u64);
        if blocks < 2 || blocks > MAX_BLOCKS as u64 {
            return Err(invalid("fixture must contain 2..=1000000 blocks"));
        }
        Ok(blocks as usize)
    }
}

fn default_batch_size() -> usize {
    1
}

pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

pub(crate) fn failed(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

// FNV-1a is an experiment checksum, not authentication or a collision-free proof.
const HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn checksum(bytes: &[u8], passes: u32) -> u64 {
    checksum_checked(bytes, passes, || Ok(())).expect("infallible checksum budget")
}

pub(crate) fn checksum_checked(
    bytes: &[u8],
    passes: u32,
    mut check: impl FnMut() -> io::Result<()>,
) -> io::Result<u64> {
    let mut value = HASH_OFFSET;
    for _ in 0..passes {
        check()?;
        for chunk in std::hint::black_box(bytes).chunks(CHECKSUM_POLL_BYTES) {
            check()?;
            for &byte in chunk {
                value = (value ^ u64::from(byte)).wrapping_mul(HASH_PRIME);
            }
        }
    }
    Ok(std::hint::black_box(value))
}

pub fn write_fixture(out: &mut impl Write, bytes: u64) -> io::Result<()> {
    let mut chunk = [0_u8; 8192];
    let mut offset = 0_u64;
    while offset < bytes {
        let count = (bytes - offset).min(chunk.len() as u64) as usize;
        for (i, value) in chunk[..count].iter_mut().enumerate() {
            let position = offset + i as u64;
            *value = (position.wrapping_mul(31) ^ (position >> 8) ^ (position >> 17)) as u8;
        }
        out.write_all(&chunk[..count])?;
        offset += count as u64;
    }
    out.flush()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Arrangement {
    Direct,
    Pipeline,
    Independent,
}

pub fn trial_order(repetition: usize) -> [Arrangement; 3] {
    use Arrangement::{Direct as A, Independent as C, Pipeline as B};
    const ORDERS: [[Arrangement; 3]; 6] = [
        [A, B, C],
        [B, C, A],
        [C, A, B],
        [A, C, B],
        [C, B, A],
        [B, A, C],
    ];
    ORDERS[repetition % ORDERS.len()]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Comparison {
    pub arrangement: Arrangement,
    pub reversed: bool,
}

impl Comparison {
    pub fn processors(self, pair: [ProcessorId; 2]) -> [ProcessorId; 2] {
        if self.reversed {
            [pair[1], pair[0]]
        } else {
            pair
        }
    }
}

pub fn comparison_order(repetition: usize) -> [Comparison; 6] {
    let arrangements = trial_order(0);
    let first_row = [0, 1, 5, 2, 4, 3];
    first_row.map(|offset| {
        let treatment = (offset + repetition % first_row.len()) % first_row.len();
        Comparison {
            arrangement: arrangements[treatment % arrangements.len()],
            reversed: treatment >= arrangements.len(),
        }
    })
}

#[derive(Clone, Debug)]
pub(crate) struct BlockResult {
    id: usize,
    hash: u64,
    latency_ns: u64,
    queue_ns: u64,
    read_ns: u64,
    compute_ns: u64,
}

pub(crate) fn verify(results: &mut [BlockResult], expected: &[u64]) -> io::Result<()> {
    if results.len() != expected.len() {
        return Err(failed("missing or additional block results"));
    }
    results.sort_unstable_by_key(|result| result.id);
    for (id, (result, hash)) in results.iter().zip(expected).enumerate() {
        if result.id != id || result.hash != *hash {
            return Err(failed(format!(
                "incorrect, duplicate or missing result at block {id}"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Distribution {
    pub count: usize,
    pub p50_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
}

impl Distribution {
    pub(crate) fn from_values(mut values: Vec<u64>) -> Self {
        values.sort_unstable();
        let count = values.len();
        let percentile = |percent: usize| {
            count
                .checked_sub(1)
                .map_or(0, |_| values[(count * percent).div_ceil(100) - 1])
        };
        Self {
            count,
            p50_ns: percentile(50),
            p99_ns: percentile(99),
            max_ns: values.last().copied().unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests;
