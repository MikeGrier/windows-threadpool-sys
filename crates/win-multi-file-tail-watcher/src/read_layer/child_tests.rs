// Copyright (c) 2026 Mike Grier
//! The layer on the thread pool over real files written by child processes through a handle their
//! parent opened for them, through a pool of four small buffers and a client SQ of two slots.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::ReadLayer;
use super::tests::{Taken, layer_with, runs, take_until};
use crate::read_sq::ReadEntry;
use crate::test_support::{Children, TempDir, signalled_within, start_writer, wtf};
use crate::{Registration, read_engine::Tracked};

// What each pass of a child's loop appends to its file.
const LINE: &str = "123456789ab\r\n";
const LINE_LEN: u64 = LINE.len() as u64;

struct Writers {
    children: Children,
    files: Vec<(u32, PathBuf)>,
    _registrations: Vec<Registration<Tracked<u32>>>,
}

impl Writers {
    /// `count` children, each appending `passes` lines to a file of its own, registered at offset 0.
    fn start(dir: &TempDir, layer: &ReadLayer<u32>, count: u32, passes: u64) -> Self {
        let mut writers = Self {
            children: Children(Vec::new()),
            files: Vec::new(),
            _registrations: Vec::new(),
        };
        for value in 0..count {
            let path = dir.0.join(format!("w{value}.log"));
            writers
                .children
                .0
                .push(start_writer(&path, LINE, passes, false));
            writers
                ._registrations
                .push(layer.register(&wtf(&path), 0, value).expect("register"));
            writers.files.push((value, path));
        }
        writers
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

    /// Each file's bytes delivered once, in pieces that meet, and equal to what the file holds.
    fn assert_delivered_once(&self, taken: &mut Taken) {
        for (value, path) in &self.files {
            assert_eq!(
                taken.bytes_from(*value, 0),
                std::fs::read(path).expect("read the file"),
                "file {value}"
            );
        }
    }
}

#[test]
fn polls_on_the_thread_pool_deliver_what_children_write_each_byte_once() {
    const COUNT: u32 = 150;
    const PASSES: u64 = 30;
    let dir = TempDir::new("rl-child-polls");
    // A watcher SQ of eight, so polls park and are continued, a pool of four buffers of eight
    // bytes, and a client SQ of two.
    let layer = layer_with(8, 4, 2);
    let mut writers = Writers::start(&dir, &layer, COUNT, PASSES);
    let mut taken = Taken::default();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut polls = 0;
    loop {
        // Asked before the poll: a poll made after the last child is gone has seen it all.
        let running = writers.running();
        layer.poll().expect("poll");
        polls += 1;
        take_until(&layer, &mut taken, polls);
        if !running {
            break;
        }
        assert!(Instant::now() < deadline, "the children never finished");
    }
    writers.finish();
    writers.assert_delivered_once(&mut taken);
    assert_eq!(taken.poll_dones, polls, "one poll-done for each poll");
    for (value, _) in &writers.files {
        let total: u64 = taken.pieces[value]
            .iter()
            .map(|piece| piece.range.end - piece.range.start)
            .sum();
        assert_eq!(total, PASSES * LINE_LEN, "file {value} is read to its end");
    }
    assert!(layer.take_failure().is_none());
}

#[test]
fn chunks_the_client_keeps_stall_the_layer_and_dropping_them_continues_it() {
    const COUNT: u32 = 12;
    const PASSES: u64 = 20;
    let dir = TempDir::new("rl-child-held");
    let layer = layer_with(64, 4, 8);
    let mut writers = Writers::start(&dir, &layer, COUNT, PASSES);
    writers.finish();
    layer.poll().expect("poll");

    // The client takes entries and keeps their chunks until it holds one for each buffer.
    let mut held = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while held.len() < 4 {
        assert!(Instant::now() < deadline, "the layer never delivered");
        signalled_within(layer.read_sq().doorbell(), 100);
        layer.read_sq().reset_doorbell();
        held.extend(layer.read_sq().drain());
    }
    assert_eq!(held.len(), 4);
    // Out of buffers, the layer is stopped, and a stopped layer runs nothing.
    std::thread::sleep(Duration::from_millis(100));
    let stalled = runs(&layer);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(runs(&layer), stalled, "a stalled layer runs nothing");
    assert!(layer.read_sq().drain().next().is_none());

    // Dropping them is all the client does to continue it.
    let mut taken = Taken::default();
    for entry in held {
        let ReadEntry::Data(data) = entry else {
            panic!("only data was kept");
        };
        let (value, range, chunk) = data.into_parts();
        taken
            .pieces
            .entry(value)
            .or_default()
            .push(super::tests::Piece {
                range,
                bytes: chunk.to_vec(),
            });
    }
    take_until(&layer, &mut taken, 1);
    writers.assert_delivered_once(&mut taken);
}

#[test]
fn a_layer_dropped_after_its_polls_releases_the_children_files() {
    const COUNT: u32 = 20;
    let dir = TempDir::new("rl-child-drop");
    let layer = layer_with(8, 4, 2);
    let mut writers = Writers::start(&dir, &layer, COUNT, 10);
    writers.finish();
    layer.poll().expect("poll");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    writers.assert_delivered_once(&mut taken);
    drop(writers._registrations);
    drop(layer);
    // Every handle the layer held is closed, so each file can be replaced.
    for (_, path) in &writers.files {
        std::fs::remove_file(path).expect("a closed file can be deleted");
        std::fs::write(path, b"again").expect("and made again");
    }
}

#[test]
fn a_layer_dropped_with_reads_in_flight_returns_and_releases_the_files() {
    const COUNT: u32 = 20;
    let dir = TempDir::new("rl-child-inflight");
    let layer = layer_with(8, 4, 2);
    let mut writers = Writers::start(&dir, &layer, COUNT, 10);
    writers.finish();
    layer.poll().expect("poll");
    // The client's SQ of two fills and the layer stops with reads still held by its ring.
    assert!(signalled_within(layer.read_sq().doorbell(), 10_000));
    std::thread::sleep(Duration::from_millis(100));
    assert!(layer.core.engine().in_flight() > 0, "reads are in flight");
    drop(writers._registrations);
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(layer);
        done.send(()).expect("the test is waiting");
    });
    finished
        .recv_timeout(Duration::from_secs(30))
        .expect("dropping the layer with reads in flight did not return");
    for (_, path) in &writers.files {
        std::fs::remove_file(path).expect("a closed file can be deleted");
    }
}

// Shutdown from each state a read can be in, over files children write.

#[test]
fn shutdown_with_reads_in_flight_still_delivers_poll_done_and_drop_returns() {
    let dir = TempDir::new("rl-child-sd-inflight");
    let layer = layer_with(8, 4, 2);
    let mut writers = Writers::start(&dir, &layer, 20, 10);
    writers.finish();
    layer.poll().expect("poll");
    assert!(signalled_within(layer.read_sq().doorbell(), 10_000));
    std::thread::sleep(Duration::from_millis(100));
    assert!(layer.core.engine().in_flight() > 0, "reads are in flight");
    layer.shutdown();
    // The poll's reads that were in flight come back, then its poll-done.
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    assert_eq!(taken.poll_dones, 1);
    drop(writers._registrations);
    drop(layer);
    for (_, path) in &writers.files {
        std::fs::remove_file(path).expect("a closed file can be deleted");
    }
}

#[test]
fn shutdown_while_the_children_are_still_writing_ends_the_polls_and_releases_the_files() {
    let dir = TempDir::new("rl-child-sd-writing");
    let layer = layer_with(8, 4, 2);
    let mut writers = Writers::start(&dir, &layer, 40, 200);
    layer.poll().expect("poll");
    layer.shutdown();
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    assert!(matches!(layer.poll(), Err(crate::PollError::Shutdown)));
    drop(std::mem::take(&mut writers._registrations));
    drop(layer);
    writers.finish();
    for (_, path) in &writers.files {
        std::fs::remove_file(path).expect("a closed file can be deleted");
    }
}

#[test]
fn shutdown_while_stalled_on_kept_chunks_returns_and_the_chunks_may_be_dropped_after() {
    let dir = TempDir::new("rl-child-sd-held");
    let layer = layer_with(64, 4, 8);
    let mut writers = Writers::start(&dir, &layer, 12, 20);
    writers.finish();
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
    drop(writers._registrations);
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(layer);
        done.send(()).expect("the test is waiting");
    });
    finished
        .recv_timeout(Duration::from_secs(30))
        .expect("dropping a stalled layer did not return");
    // Chunks outlive the layer that lent them.
    drop(held);
    for (_, path) in &writers.files {
        std::fs::remove_file(path).expect("a closed file can be deleted");
    }
}

#[test]
fn shutdown_of_an_idle_layer_after_its_polls_releases_the_files() {
    let dir = TempDir::new("rl-child-sd-idle");
    let layer = layer_with(8, 4, 2);
    let mut writers = Writers::start(&dir, &layer, 10, 5);
    writers.finish();
    layer.poll().expect("poll");
    let mut taken = Taken::default();
    take_until(&layer, &mut taken, 1);
    writers.assert_delivered_once(&mut taken);
    layer.shutdown();
    layer.shutdown();
    drop(writers._registrations);
    drop(layer);
    for (_, path) in &writers.files {
        std::fs::remove_file(path).expect("a closed file can be deleted");
    }
}
