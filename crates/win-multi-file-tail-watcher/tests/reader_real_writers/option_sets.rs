// Copyright (c) 2026 Mike Grier
//! Every byte delivered once, under each set of sizes, from children that write while the reader
//! polls (TW-9.5).
//!
//! The sets are the defaults, one well above them of the size a build service would choose, the
//! tiny set the in-crate tests use, and one whose ring is smaller than its pool, so that the ring
//! and not the pool is what limits the reads in flight.

use std::time::Duration;

use win_multi_file_tail_watcher::reader::{TailReader, TailReaderOptions};

use crate::harness::{Client, TempDir, crlf, for_lines, script, spawn, wtf};

/// What child `index` writes for row `i`: a number, or `%i` for the loop variable of the script.
fn row(index: usize, i: impl std::fmt::Display) -> String {
    format!("f{index} row {i} 0123456789abcdef")
}

fn expected(index: usize, rows: u64) -> Vec<u8> {
    crlf(&(1..=rows).map(|i| row(index, i)).collect::<Vec<_>>())
}

fn options(
    len: usize,
    count: usize,
    client_sq: usize,
    ring_sq: u32,
    ring_cq: u32,
) -> TailReaderOptions {
    // `#[non_exhaustive]`: a caller starts from the defaults and assigns.
    let mut options = TailReaderOptions::default();
    options.buffer_len = len;
    options.buffer_count = count;
    options.client_sq_capacity = client_sq;
    options.ring_sq_size = ring_sq;
    options.ring_cq_size = ring_cq;
    options
}

/// `children` writers, each appending `rows` rows to a file of its own as fast as it can, while the
/// reader polls. Afterwards every file has been delivered whole, once, from the first byte.
fn deliver_every_byte_once(
    tag: &str,
    options: &TailReaderOptions,
    watcher_sq: usize,
    children: usize,
    rows: u64,
) {
    let dir = TempDir::new(tag);
    let reader = TailReader::<u32>::new(options, watcher_sq).expect("a reader");
    let mut client = Client::new(&reader, options.buffer_len);

    let mut writers = Vec::new();
    let mut registrations = Vec::new();
    for index in 0..children {
        let path = dir.path(&format!("out{index}.log"));
        let body = format!("echo {}", row(index, "%i"));
        let value = u32::try_from(index).expect("fits");
        writers.push(spawn(
            &script(&[for_lines(1, rows, &body)]),
            &path,
            None,
            false,
        ));
        registrations.push(reader.register(&wtf(&path), 0, value).expect("register"));
    }

    // Poll while they write, then until a poll finds nothing more once they have all finished.
    while !writers.iter_mut().all(|w| w.exited()) {
        let polled = client.poll_once();
        assert!(polled.other.is_empty(), "{:?}", polled.other);
        std::thread::sleep(Duration::from_millis(2));
    }
    for writer in &mut writers {
        assert!(writer.wait().success());
    }
    for _ in 0..50 {
        let polled = client.poll_once();
        assert!(polled.other.is_empty(), "{:?}", polled.other);
        if polled.data == 0 {
            break;
        }
    }

    for index in 0..children {
        let value = u32::try_from(index).expect("fits");
        let (bytes, _) = client.assemble(value);
        assert_eq!(
            bytes,
            expected(index, rows),
            "file {index}, under {options:?}"
        );
    }
    assert!(reader.take_failure().is_none());
    drop(registrations);
}

#[test]
fn every_byte_is_delivered_once_under_the_default_options() {
    deliver_every_byte_once("default", &TailReaderOptions::default(), 64, 6, 300);
}

#[test]
fn every_byte_is_delivered_once_under_options_well_above_the_defaults() {
    // Large buffers, many of them, and a large client queue and ring: the size a build service
    // would choose. A file of a few kilobytes is one read, and the queue never fills.
    let options = options(65_536, 256, 4096, 4096, 8192);
    deliver_every_byte_once("large", &options, 4096, 6, 300);
}

#[test]
fn every_byte_is_delivered_once_under_the_tiny_options() {
    // Four buffers of eight bytes and a client queue of two: the pool is empty or the client queue
    // is full at almost every step, and the reader goes on from each. The ring's completion queue
    // is larger than the pool, so this set does not reach the ring-full stop; the next test does.
    let options = options(8, 4, 2, 8, 16);
    deliver_every_byte_once("tiny", &options, 2, 4, 80);
}

#[test]
fn every_byte_is_delivered_once_when_the_ring_is_smaller_than_the_pool() {
    // Sixty-four buffers but a completion queue of sixteen, so the ring holds fewer reads than
    // the pool could lend.
    let options = options(8, 64, 64, 8, 16);
    deliver_every_byte_once("ring", &options, 64, 4, 80);
}
