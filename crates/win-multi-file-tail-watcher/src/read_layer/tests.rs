// Copyright (c) 2026 Mike Grier
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::ReadLayer;
use crate::Watcher;
use crate::pool::BufferPool;
use crate::read_engine::ReadEngine;
use crate::read_sq::ReadEntry;
use crate::test_support::{TempDir, signalled_within, wtf};

const BUFFER_LEN: usize = 8;

pub(super) struct Piece {
    pub(super) range: Range<u64>,
    pub(super) bytes: Vec<u8>,
}

#[derive(Default)]
pub(super) struct Taken {
    pub(super) pieces: BTreeMap<u32, Vec<Piece>>,
    pub(super) poll_dones: usize,
}

impl Taken {
    pub(super) fn drain(&mut self, layer: &ReadLayer<u32>) {
        for entry in layer.read_sq().drain() {
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

    /// The bytes of `value` from `from`, which must be delivered once, in pieces that meet.
    pub(super) fn bytes_from(&mut self, value: u32, from: u64) -> Vec<u8> {
        let pieces = self.pieces.entry(value).or_default();
        pieces.sort_by_key(|piece| piece.range.start);
        let mut at = from;
        let mut bytes = Vec::new();
        for piece in pieces.iter() {
            assert_eq!(piece.range.start, at, "file {value}: a gap or a repeat");
            bytes.extend_from_slice(&piece.bytes);
            at = piece.range.end;
        }
        bytes
    }
}

pub(super) fn layer_with(
    watcher_capacity: usize,
    buffers: usize,
    client_capacity: usize,
) -> ReadLayer<u32> {
    let watcher = Watcher::new(watcher_capacity).expect("watcher");
    let pool = BufferPool::new(BUFFER_LEN, buffers).expect("pool");
    let engine = ReadEngine::new(watcher, pool, client_capacity, 16, 32).expect("engine");
    ReadLayer::new(engine).expect("layer")
}

fn layer() -> ReadLayer<u32> {
    layer_with(64, 8, 64)
}

pub(super) fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|n| u8::try_from(n % 251).expect("below 251"))
        .collect()
}

/// Wait for the client's doorbell, reset it, then take what is in the SQ, as the discipline is
/// (TW-D-6), and say the client has made room.
fn take(layer: &ReadLayer<u32>, taken: &mut Taken) {
    signalled_within(layer.read_sq().doorbell(), 100);
    layer.read_sq().reset_doorbell();
    taken.drain(layer);
    layer.read_sq().ring_resume();
}

pub(super) fn take_until(layer: &ReadLayer<u32>, taken: &mut Taken, poll_dones: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while taken.poll_dones < poll_dones {
        assert!(Instant::now() < deadline, "the layer never finished");
        take(layer, taken);
    }
}

pub(super) fn runs(layer: &ReadLayer<u32>) -> usize {
    layer.core.runs.load(Ordering::Relaxed)
}

fn append(path: &PathBuf, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .expect("append");
}

#[test]
fn a_poll_is_read_and_delivered_with_no_further_call() {
    let dir = TempDir::new("rl-one");
    let layer = layer();
    let content = pattern(40);
    let path = dir.file("a.log", &content);
    let _registration = layer.register(&wtf(&path), 0, 7).expect("register");
    layer.poll().expect("poll");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    assert_eq!(taken.bytes_from(7, 0), content);
    assert!(layer.take_failure().is_none());
}

#[test]
fn a_poll_with_nothing_to_read_delivers_only_poll_done() {
    let dir = TempDir::new("rl-none");
    let layer = layer();
    let path = dir.file("a.log", b"abc");
    let _registration = layer.register(&wtf(&path), 3, 7).expect("register");
    layer.poll().expect("poll");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    assert!(taken.pieces.is_empty());
}

#[test]
fn several_files_are_each_read_in_full() {
    let dir = TempDir::new("rl-files");
    let layer = layer();
    let mut registrations = Vec::new();
    let mut expected = Vec::new();
    for n in 0..20_u32 {
        let content = pattern(5 + 7 * usize::try_from(n).expect("small"));
        let path = dir.file(format!("f{n}.log"), &content);
        registrations.push(layer.register(&wtf(&path), 0, n).expect("register"));
        expected.push(content);
    }
    layer.poll().expect("poll");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    for (n, content) in (0..20_u32).zip(expected) {
        assert_eq!(taken.bytes_from(n, 0), content, "file {n}");
    }
}

#[test]
fn a_client_that_stops_draining_stalls_the_layer_until_it_rings_back() {
    let dir = TempDir::new("rl-client");
    let layer = layer_with(64, 16, 2);
    let content = pattern(100);
    let path = dir.file("a.log", &content);
    let _registration = layer.register(&wtf(&path), 0, 7).expect("register");
    layer.poll().expect("poll");
    // The client does not drain, so the SQ fills and the layer stops, and a stopped layer is not
    // run again until something it waits for happens.
    assert!(signalled_within(layer.read_sq().doorbell(), 10_000));
    std::thread::sleep(Duration::from_millis(100));
    let stalled = runs(&layer);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(runs(&layer), stalled, "a stalled layer runs nothing");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    assert_eq!(taken.bytes_from(7, 0), content);
}

#[test]
fn a_returned_chunk_continues_the_reads_with_no_ring_from_the_client() {
    let dir = TempDir::new("rl-pool");
    let layer = layer_with(64, 1, 64);
    let content = pattern(40);
    let path = dir.file("a.log", &content);
    let _registration = layer.register(&wtf(&path), 0, 7).expect("register");
    layer.poll().expect("poll");
    // The only buffer goes to the first chunk, which the client keeps.
    assert!(signalled_within(layer.read_sq().doorbell(), 10_000));
    layer.read_sq().reset_doorbell();
    let mut held: Vec<_> = layer.read_sq().drain().collect();
    assert_eq!(held.len(), 1);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        layer.read_sq().drain().next().is_none(),
        "nothing is read without a buffer"
    );
    // Returning it is all the client does.
    let mut taken = Taken::default();
    let ReadEntry::Data(first) = held.remove(0) else {
        panic!("the first entry is data");
    };
    let (value, range, chunk) = first.into_parts();
    taken.pieces.entry(value).or_default().push(Piece {
        range,
        bytes: chunk.to_vec(),
    });
    drop(chunk);
    // The client's SQ has room, so it rings nothing: the dropped chunks are all that continues it.
    let deadline = Instant::now() + Duration::from_secs(30);
    while taken.poll_dones < 1 {
        assert!(Instant::now() < deadline, "the layer never finished");
        signalled_within(layer.read_sq().doorbell(), 100);
        layer.read_sq().reset_doorbell();
        taken.drain(&layer);
    }
    assert_eq!(taken.bytes_from(7, 0), content);
}

#[test]
fn a_poll_parked_on_a_full_watcher_sq_goes_on_as_the_layer_drains_it() {
    let dir = TempDir::new("rl-parked");
    let layer = layer_with(2, 8, 64);
    let mut registrations = Vec::new();
    let mut expected = Vec::new();
    for n in 0..12_u32 {
        let content = pattern(20);
        let path = dir.file(format!("f{n}.log"), &content);
        registrations.push(layer.register(&wtf(&path), 0, n).expect("register"));
        expected.push(content);
    }
    layer.poll().expect("poll");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    for (n, content) in (0..12_u32).zip(expected) {
        assert_eq!(taken.bytes_from(n, 0), content, "file {n}");
    }
}

#[test]
fn nothing_runs_between_polls() {
    let dir = TempDir::new("rl-idle");
    let layer = layer();
    let path = dir.file("a.log", &pattern(40));
    let _registration = layer.register(&wtf(&path), 0, 7).expect("register");
    layer.poll().expect("poll");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    std::thread::sleep(Duration::from_millis(100));
    let settled = runs(&layer);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(runs(&layer), settled, "an idle layer runs nothing");
}

#[test]
fn a_second_poll_reads_only_what_the_file_gained() {
    let dir = TempDir::new("rl-second");
    let layer = layer();
    let first = pattern(40);
    let path = dir.file("a.log", &first);
    let _registration = layer.register(&wtf(&path), 0, 7).expect("register");
    layer.poll().expect("poll");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    let more = vec![b'z'; 25];
    append(&path, &more);
    layer.poll().expect("a poll after poll-done");
    take_until(&layer, &mut taken, 2);
    let mut all = first;
    all.extend_from_slice(&more);
    assert_eq!(taken.bytes_from(7, 0), all);
}

#[test]
fn registering_through_the_layer_gives_each_registration_its_id() {
    let dir = TempDir::new("rl-reg");
    let layer = layer();
    let path = dir.file("a.log", b"abc");
    let first = layer.register(&wtf(&path), 0, 1).expect("register");
    let second = layer.register(&wtf(&path), 0, 2).expect("register");
    assert_ne!(first.id(), second.id());
}

#[test]
fn dropping_an_idle_layer_returns() {
    let dir = TempDir::new("rl-drop");
    let path = dir.file("a.log", &pattern(40));
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let layer = layer();
        let _registration = layer.register(&wtf(&path), 0, 7).expect("register");
        layer.poll().expect("poll");
        let mut taken = Taken::default();
        take_until(&layer, &mut taken, 1);
        drop(layer);
        done.send(()).expect("the test is waiting");
    });
    finished
        .recv_timeout(Duration::from_secs(30))
        .expect("dropping the layer did not return");
}

#[test]
fn a_new_layer_with_nothing_to_do_runs_the_engine_once_for_its_setup_and_then_not() {
    let layer = layer();
    std::thread::sleep(Duration::from_millis(100));
    let settled = runs(&layer);
    assert!(settled >= 1, "the ring's setup signal drives it once");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(runs(&layer), settled);
}

#[test]
fn shutdown_refuses_polls_and_may_be_repeated() {
    let dir = TempDir::new("rl-shut");
    let layer = layer();
    let path = dir.file("a.log", &pattern(40));
    let _registration = layer.register(&wtf(&path), 0, 7).expect("register");
    layer.shutdown();
    layer.shutdown();
    assert!(matches!(layer.poll(), Err(crate::PollError::Shutdown)));
    // Registering still works, as for the watcher.
    let _again = layer.register(&wtf(&path), 0, 8).expect("register");
}

#[test]
fn shutdown_does_not_wait_for_a_busy_engine() {
    let layer = layer();
    let busy = layer.core.engine();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            layer.shutdown();
            done.send(()).expect("the test is waiting");
        });
        finished
            .recv_timeout(Duration::from_secs(10))
            .expect("shutdown waited for the engine's lock");
    });
    drop(busy);
}

#[test]
fn shutdown_while_stalled_on_kept_chunks_delivers_poll_done_once_they_return() {
    let dir = TempDir::new("rl-shut-held");
    let layer = layer_with(64, 4, 8);
    let mut registrations = Vec::new();
    for n in 0..12_u32 {
        let path = dir.file(format!("f{n}.log"), &pattern(40));
        registrations.push(layer.register(&wtf(&path), 0, n).expect("register"));
    }
    layer.poll().expect("poll");
    let mut held = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while held.len() < 4 {
        assert!(Instant::now() < deadline, "the layer never delivered");
        signalled_within(layer.read_sq().doorbell(), 100);
        layer.read_sq().reset_doorbell();
        held.extend(layer.read_sq().drain());
    }
    layer.shutdown();
    assert!(matches!(layer.poll(), Err(crate::PollError::Shutdown)));
    let mut taken = Taken::default();
    for entry in held {
        let ReadEntry::Data(data) = entry else {
            panic!("only data was kept");
        };
        let (value, range, chunk) = data.into_parts();
        taken.pieces.entry(value).or_default().push(Piece {
            range,
            bytes: chunk.to_vec(),
        });
    }
    // Every chunk is back, nothing more is read, and the poll-done is delivered all the same.
    take_until(&layer, &mut taken, 1);
    let total: usize = taken
        .pieces
        .values()
        .flatten()
        .map(|piece| piece.bytes.len())
        .sum();
    assert!(total >= 32, "what was in flight was delivered: {total}");
    assert!(total < 12 * 40, "reads stopped: {total}");
    assert!(layer.take_failure().is_none());
}

#[test]
fn the_first_error_a_run_returns_is_kept_for_the_owner_and_taken_once() {
    use crate::read_engine::Stop;
    let layer = layer();
    assert!(layer.take_failure().is_none());
    layer.core.record(Err(std::io::Error::other("the first")));
    layer.core.record(Err(std::io::Error::other("the second")));
    layer.core.record(Ok(Stop::Idle));
    let kept = layer.take_failure().expect("an error was kept");
    assert_eq!(kept.to_string(), "the first", "the first stays");
    assert!(layer.take_failure().is_none(), "and is taken once");
    layer.core.record(Err(std::io::Error::other("the third")));
    assert_eq!(
        layer.take_failure().expect("a later one").to_string(),
        "the third"
    );
}
