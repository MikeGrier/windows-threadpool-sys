// Copyright (c) Mike Grier.
use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::Instant;

use windows_overlapped_io_sys::{
    AssociatedEndpoint, CompletionPort, FileIo, OperationId, Started, UnassociatedEndpoint,
};

use crate::payload::Payload;
use crate::{BlockResult, Config, InputKind, checksum_checked, failed, fill_fixture};

const FILE_COMPLETION_KEY: usize = 1;

pub(crate) struct Job {
    pub id: usize,
    pub buffer: Payload,
    pub submitted: Instant,
    pub read_done: Instant,
}

pub(crate) struct Finished {
    pub buffer: Payload,
    pub result: BlockResult,
}

impl Job {
    pub fn process(
        self,
        passes: u32,
        queued: bool,
        check: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Finished> {
        #[cfg(test)]
        self.buffer.record_processing()?;
        let started = Instant::now();
        let hash = checksum_checked(&self.buffer, passes, check)?;
        let completed = Instant::now();
        Ok(Finished {
            buffer: self.buffer,
            result: BlockResult {
                id: self.id,
                hash,
                latency_ns: completed.duration_since(self.submitted).as_nanos() as u64,
                read_ns: self.read_done.duration_since(self.submitted).as_nanos() as u64,
                compute_ns: completed.duration_since(started).as_nanos() as u64,
                queue_ns: if queued {
                    started.duration_since(self.read_done).as_nanos() as u64
                } else {
                    0
                },
            },
        })
    }
}

struct Pending {
    token: FileIo<Payload>,
    id: usize,
    bytes: usize,
    submitted: Instant,
}

pub(crate) struct Reader<'a> {
    endpoint: Option<AssociatedEndpoint<'a>>,
    port: Option<&'a CompletionPort>,
    pub free: Vec<Payload>,
    pending: HashMap<OperationId, Pending>,
    ready: VecDeque<Job>,
    next: usize,
    stride: usize,
    blocks: usize,
    file_bytes: u64,
    block_bytes: usize,
    pub expected: usize,
    pub results: Vec<BlockResult>,
    pub peak_io: usize,
    pub peak_leased: usize,
    pub submitted: usize,
    pub buffer_pressure_observations: usize,
    pub read_limit: usize,
    pool_size: usize,
}

impl<'a> Reader<'a> {
    pub fn new(
        port: Option<&'a CompletionPort>,
        config: &Config,
        file_bytes: u64,
        blocks: usize,
        lane: usize,
        lanes: usize,
        platform: &dyn crate::platform::Platform,
    ) -> io::Result<Self> {
        let endpoint = if config.input == InputKind::BufferedFile {
            let source = UnassociatedEndpoint::open(&config.file, true, false, 0)?;
            Some(
                port.expect("file reader has a completion port")
                    .associate(source, FILE_COMPLETION_KEY)?,
            )
        } else {
            None
        };
        let expected = (blocks - lane).div_ceil(lanes);
        let pool_size = config.buffers() / lanes;
        Ok(Self {
            endpoint,
            port,
            free: (0..pool_size)
                .map(|_| platform.allocate(config.block_bytes, config.payload_node))
                .collect::<io::Result<_>>()?,
            pending: HashMap::with_capacity(pool_size),
            ready: VecDeque::with_capacity(pool_size),
            next: lane,
            stride: lanes,
            blocks,
            file_bytes,
            block_bytes: config.block_bytes,
            expected,
            results: Vec::with_capacity(expected),
            peak_io: 0,
            peak_leased: 0,
            submitted: 0,
            buffer_pressure_observations: 0,
            read_limit: config.depth / lanes,
            pool_size,
        })
    }

    pub fn submit_available(
        &mut self,
        mut check: impl FnMut() -> io::Result<()>,
    ) -> io::Result<bool> {
        let mut progress = false;
        while self.next < self.blocks
            && !self.free.is_empty()
            && self.pending.len() < self.read_limit
            && (self.endpoint.is_some() || self.ready.len() < self.read_limit)
        {
            check()?;
            let id = self.next;
            let offset = id as u64 * self.block_bytes as u64;
            let bytes = (self.file_bytes - offset).min(self.block_bytes as u64) as usize;
            let mut buffer = self.free.pop().expect("checked nonempty");
            buffer.truncate(bytes);
            let submitted = Instant::now();
            let started = if let Some(endpoint) = &self.endpoint {
                endpoint.read(buffer, offset)?
            } else {
                for (index, chunk) in buffer.chunks_mut(crate::CHECKSUM_POLL_BYTES).enumerate() {
                    check()?;
                    fill_fixture(chunk, offset + (index * crate::CHECKSUM_POLL_BYTES) as u64);
                }
                Started::Completed {
                    payload: buffer,
                    bytes_transferred: bytes,
                }
            };
            match started {
                Started::Pending(token) => {
                    self.pending.insert(
                        token.id(),
                        Pending {
                            token,
                            id,
                            bytes,
                            submitted,
                        },
                    );
                    self.peak_io = self.peak_io.max(self.pending.len());
                }
                Started::Completed {
                    payload,
                    bytes_transferred,
                } => {
                    if bytes_transferred != bytes {
                        return Err(failed("short immediate read"));
                    }
                    self.ready.push_back(Job {
                        id,
                        buffer: payload,
                        submitted,
                        read_done: Instant::now(),
                    });
                }
            }
            self.next += self.stride;
            self.submitted += 1;
            self.peak_leased = self.peak_leased.max(self.pool_size - self.free.len());
            progress = true;
        }
        if self.next < self.blocks && self.free.is_empty() {
            self.buffer_pressure_observations += 1;
        }
        Ok(progress)
    }

    pub fn poll(&mut self) -> io::Result<Option<Job>> {
        if let Some(job) = self.ready.pop_front() {
            return Ok(Some(job));
        }
        let Some(port) = self.port else {
            return Ok(None);
        };
        let Some(completion) = port.get(0)? else {
            return Ok(None);
        };
        let id = completion
            .id()
            .ok_or_else(|| failed("unexpected non-I/O completion"))?;
        let pending = self
            .pending
            .remove(&id)
            .ok_or_else(|| failed("unknown completion identity"))?;
        let (buffer, result) = pending
            .token
            .claim(&completion)
            .map_err(|_| failed("completion did not match its operation"))?;
        if result? != pending.bytes {
            return Err(failed(format!("short read at block {}", pending.id)));
        }
        Ok(Some(Job {
            id: pending.id,
            buffer,
            submitted: pending.submitted,
            read_done: Instant::now(),
        }))
    }

    pub fn reclaim(&mut self, mut finished: Finished) {
        finished.buffer.resize(self.block_bytes, 0);
        self.free.push(finished.buffer);
        self.results.push(finished.result);
    }

    pub fn done(&self) -> bool {
        self.results.len() == self.expected
    }
}
