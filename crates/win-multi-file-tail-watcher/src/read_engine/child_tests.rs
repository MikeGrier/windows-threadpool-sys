// Copyright (c) 2026 Mike Grier
//! The reading engine over real files written by child processes through a handle their parent
//! opened for them, which is how a parent gives a child its stdout file, through a pool of a few
//! buffers and a client SQ of two slots.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{Advance, ReadEngine, Stop};
use crate::pool::BufferPool;
use crate::read_sq::ReadEntry;
use crate::test_support::{Children, TempDir, start_writer, wtf};
use crate::{Registration, Watcher};

// What each pass of a child's loop appends to its file.
const LINE: &str = "123456789ab\r\n";
const LINE_LEN: u64 = LINE.len() as u64;

// Buffers shorter than a line, so reads straddle lines.
const BUFFER_LEN: usize = 8;

struct Piece {
    range: Range<u64>,
    bytes: Vec<u8>,
}

/// What the client has taken from the engine: the pieces of each file by value, and how many
/// poll-done entries.
#[derive(Default)]
struct Taken {
    pieces: BTreeMap<u32, Vec<Piece>>,
    poll_dones: usize,
}

impl Taken {
    /// Take everything in the client's SQ, giving each chunk's buffer back.
    fn drain(&mut self, engine: &ReadEngine<u32>) {
        for entry in engine.read_sq().drain() {
            match entry {
                ReadEntry::Data(data) => {
                    let (value, range, chunk) = data.into_parts();
                    self.pieces.entry(value).or_default().push(Piece {
                        range,
                        bytes: chunk.to_vec(),
                    });
                }
                ReadEntry::PollDone => self.poll_dones += 1,
                other => panic!("only data and poll-done were expected, not {other:?}"),
            }
        }
    }

    /// Check that each file's pieces meet with no gap or overlap from offset 0, and are exactly the
    /// bytes the file holds, which is each byte delivered once.
    fn assert_each_file_delivered_once(&mut self, files: &[(u32, PathBuf)]) {
        for (value, path) in files {
            let pieces = self.pieces.entry(*value).or_default();
            pieces.sort_by_key(|piece| piece.range.start);
            let mut at = 0;
            let mut bytes = Vec::new();
            for piece in pieces.iter() {
                assert_eq!(piece.range.start, at, "file {value}: a gap or a repeat");
                assert_eq!(
                    u64::try_from(piece.bytes.len()).expect("small"),
                    piece.range.end - piece.range.start,
                    "file {value}: a range labelled with other than its bytes"
                );
                bytes.extend_from_slice(&piece.bytes);
                at = piece.range.end;
            }
            assert_eq!(
                bytes,
                std::fs::read(path).expect("read the file"),
                "file {value}"
            );
        }
    }
}

/// Run the engine, taking what it delivers, until it has nothing to do and nothing in flight.
fn pump(engine: &mut ReadEngine<u32>, taken: &mut Taken) {
    for _ in 0..1_000_000 {
        let stop = engine.run().expect("run");
        taken.drain(engine);
        match stop {
            Stop::Idle if engine.in_flight() == 0 => return,
            Stop::ClientFull => {}
            _ => engine.wait_for_completion(5_000).expect("wait"),
        }
    }
    panic!("the engine never settled");
}

struct Fixture {
    engine: ReadEngine<u32>,
    registrations: Vec<Registration<super::Tracked<u32>>>,
    writers: Vec<(u32, PathBuf)>,
    children: Children,
}

impl Fixture {
    /// `count` children, each appending `passes` lines to a file of its own, which is registered
    /// at offset 0, read through `buffers` buffers of `BUFFER_LEN` bytes and a client SQ of two.
    fn new(dir: &TempDir, count: u32, passes: u64, buffers: usize) -> Self {
        let watcher = Watcher::new(1024).expect("watcher");
        let pool = BufferPool::new(BUFFER_LEN, buffers).expect("pool");
        let engine = ReadEngine::new(watcher, pool, 2, 16, 32).expect("engine");
        let mut fixture = Self {
            engine,
            registrations: Vec::new(),
            writers: Vec::new(),
            children: Children(Vec::new()),
        };
        for value in 0..count {
            let path = dir.0.join(format!("w{value}.log"));
            fixture
                .children
                .0
                .push(start_writer(&path, LINE, passes, false));
            fixture.registrations.push(
                fixture
                    .engine
                    .register(&wtf(&path), 0, value)
                    .expect("register"),
            );
            fixture.writers.push((value, path));
        }
        fixture
    }

    fn running(&mut self) -> bool {
        self.children
            .0
            .iter_mut()
            .any(|child| child.try_wait().expect("try_wait").is_none())
    }

    fn finish(&mut self) {
        for child in &mut self.children.0 {
            assert!(child.wait().expect("wait").success());
        }
    }
}

#[test]
fn what_children_write_is_reassembled_by_range_each_byte_delivered_once() {
    const COUNT: u32 = 200;
    const PASSES: u64 = 30;
    let dir = TempDir::new("re-child-whole");
    let mut fixture = Fixture::new(&dir, COUNT, PASSES, 4);
    let mut taken = Taken::default();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut polls = 0;
    loop {
        // Asked before the poll: a poll made after the last child is gone has seen it all.
        let running = fixture.running();
        fixture.engine.watcher().poll_here();
        polls += 1;
        pump(&mut fixture.engine, &mut taken);
        if !running {
            break;
        }
        assert!(Instant::now() < deadline, "the children never finished");
    }
    fixture.finish();
    taken.assert_each_file_delivered_once(&fixture.writers);
    assert_eq!(taken.poll_dones, polls, "one poll-done for each poll");
    for (value, _) in &fixture.writers {
        let total: u64 = taken.pieces[value]
            .iter()
            .map(|piece| piece.range.end - piece.range.start)
            .sum();
        assert_eq!(total, PASSES * LINE_LEN, "file {value} is read to its end");
    }
}

#[test]
fn chunks_the_client_keeps_stop_the_reads_and_dropping_them_resumes_them() {
    const PASSES: u64 = 20;
    let dir = TempDir::new("re-child-held");
    let mut fixture = Fixture::new(&dir, 1, PASSES, 4);
    fixture.finish();
    fixture.engine.watcher().poll_here();

    // The client takes each entry and keeps its chunk, so the buffers do not come back.
    let mut kept = Vec::new();
    for _ in 0..10_000 {
        let stop = fixture.engine.run().expect("run");
        kept.extend(fixture.engine.read_sq().drain());
        if stop == Stop::PoolEmpty && fixture.engine.in_flight() == 0 && kept.len() == 4 {
            break;
        }
        assert_ne!(kept.len(), 5, "more entries than there are buffers");
        fixture.engine.wait_for_completion(5_000).expect("wait");
    }
    assert_eq!(kept.len(), 4);
    assert_eq!(fixture.engine.pool().available(), 0);
    assert_eq!(fixture.engine.run().expect("run"), Stop::PoolEmpty);
    assert_eq!(fixture.engine.in_flight(), 0, "no read without a buffer");
    assert!(fixture.engine.read_sq().drain().next().is_none());

    // Dropping what was kept gives the buffers back, and the reads go on.
    let mut taken = Taken::default();
    for entry in kept {
        let ReadEntry::Data(data) = entry else {
            panic!("only data was kept");
        };
        let (value, range, chunk) = data.into_parts();
        taken.pieces.entry(value).or_default().push(Piece {
            range,
            bytes: chunk.to_vec(),
        });
    }
    assert_eq!(fixture.engine.pool().available(), 4);
    pump(&mut fixture.engine, &mut taken);
    taken.assert_each_file_delivered_once(&fixture.writers);
    assert_eq!(taken.poll_dones, 1);
}

#[test]
fn registrations_dropped_while_their_reads_are_in_flight_are_safe_and_the_reads_complete() {
    const COUNT: u32 = 60;
    const PASSES: u64 = 10;
    let dir = TempDir::new("re-child-dropped");
    let mut fixture = Fixture::new(&dir, COUNT, PASSES, 4);
    fixture.finish();
    fixture.engine.watcher().poll_here();

    // Take attentions and submit their reads, without popping, until the pool is empty.
    loop {
        match fixture.engine.advance().expect("advance") {
            Advance::Progress => {}
            Advance::Blocked(stop) => {
                assert_eq!(stop, Stop::PoolEmpty);
                break;
            }
            Advance::Drained => panic!("sixty files do not fit four buffers"),
        }
    }
    assert!(fixture.engine.in_flight() > 0, "reads are in flight");

    // Every registration goes, and with it the watcher's hold on each file.
    fixture.registrations.clear();
    assert_eq!(fixture.engine.watcher().registered(), 0);
    assert!(fixture.engine.in_flight() > 0);

    let mut taken = Taken::default();
    pump(&mut fixture.engine, &mut taken);
    taken.assert_each_file_delivered_once(&fixture.writers);
    assert_eq!(taken.poll_dones, 1);
}
