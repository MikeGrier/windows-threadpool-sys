// Copyright (c) 2026 Mike Grier
//! A client that holds what it was given stops the reader at the size it chose, and nothing is
//! lost when it lets go (TW-9.5, TW-D-16).
//!
//! Two of the three stops are reached from outside: every buffer held in a chunk, and a client
//! queue nobody has drained. The third, a full ring, is not something a client holds, and is
//! covered by the option set whose ring is smaller than its pool.

use std::time::{Duration, Instant};

use win_multi_file_tail_watcher::PollError;
use win_multi_file_tail_watcher::reader::{ReadEntry, TailReader, TailReaderOptions};

use crate::harness::{Client, TempDir, crlf, for_lines, hold_data, script, spawn, wtf};

const FILE: u32 = 3;

fn content(rows: u64) -> Vec<u8> {
    crlf(
        &(1..=rows)
            .map(|i| format!("held row {i} abcdef"))
            .collect::<Vec<_>>(),
    )
}

/// A file of `rows` rows, written by a child that has finished.
fn written(dir: &TempDir, rows: u64) -> std::path::PathBuf {
    let path = dir.path("held.log");
    let mut writer = spawn(
        &script(&[for_lines(1, rows, "echo held row %i abcdef")]),
        &path,
        None,
        false,
    );
    assert!(writer.wait().success());
    path
}

#[test]
fn holding_every_buffer_stops_reads_until_one_is_dropped_and_loses_nothing() {
    let dir = TempDir::new("held-pool");
    let path = written(&dir, 40);

    let mut options = TailReaderOptions::default();
    options.buffer_len = 8;
    options.buffer_count = 4;
    let reader = TailReader::<u32>::new(&options, 64).expect("a reader");
    let mut client = Client::new(&reader, options.buffer_len);
    let _registration = reader.register(&wtf(&path), 0, FILE).expect("register");

    // Four chunks are every buffer there is: the reader has nothing more to read into.
    reader.poll().expect("start a poll");
    let held = hold_data(&reader, 4);
    std::thread::sleep(Duration::from_millis(500));
    let sq = reader.read_sq();
    sq.reset_doorbell();
    assert!(
        sq.drain().next().is_none(),
        "no read, and no poll-done, while every buffer is held"
    );

    // Let go of them, keeping their bytes, and the rest follows, ending in the poll-done.
    for data in held {
        client.add_piece(*data.value(), data.range(), data.chunk().to_vec());
    }
    let polled = client.finish_poll();
    assert!(polled.other.is_empty(), "{:?}", polled.other);

    let (bytes, _) = client.assemble(FILE);
    assert_eq!(bytes, content(40), "every byte, once, in order");
    assert!(reader.take_failure().is_none());
}

#[test]
fn a_poll_before_the_previous_one_is_complete_is_refused_and_the_next_is_accepted_once_it_is() {
    let dir = TempDir::new("held-poll");
    let path = written(&dir, 40);

    let mut options = TailReaderOptions::default();
    options.buffer_len = 8;
    options.buffer_count = 4;
    let reader = TailReader::<u32>::new(&options, 64).expect("a reader");
    let mut client = Client::new(&reader, options.buffer_len);
    let _registration = reader.register(&wtf(&path), 0, FILE).expect("register");

    // Every buffer is held, so the reads stop. The watcher has long since announced everything, but
    // the poll is not complete until its poll-done is queued, and that waits for the reads.
    reader.poll().expect("start a poll");
    let held = hold_data(&reader, 4);
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        assert!(
            matches!(reader.poll(), Err(PollError::InFlight)),
            "a poll was accepted before the previous one was complete"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    for data in held {
        client.add_piece(*data.value(), data.range(), data.chunk().to_vec());
    }
    let polled = client.finish_poll();
    assert!(polled.other.is_empty(), "{:?}", polled.other);

    // The poll-done has been read, so the next poll is accepted at once, and finds nothing new.
    reader.poll().expect("the previous poll is complete");
    assert_eq!(client.finish_poll().data, 0);
    assert_eq!(
        client.assemble(FILE).0,
        content(40),
        "every byte, once, in order"
    );
    assert!(reader.take_failure().is_none());
}

#[test]
fn an_undrained_client_queue_stops_the_reader_at_its_capacity_and_loses_nothing() {
    let dir = TempDir::new("held-queue");
    let path = written(&dir, 40);

    let mut options = TailReaderOptions::default();
    options.buffer_len = 8;
    options.buffer_count = 64;
    options.client_sq_capacity = 2;
    let reader = TailReader::<u32>::new(&options, 64).expect("a reader");
    let mut client = Client::new(&reader, options.buffer_len);
    let _registration = reader.register(&wtf(&path), 0, FILE).expect("register");

    // Far more than two reads are wanted, and nobody takes any.
    reader.poll().expect("start a poll");
    std::thread::sleep(Duration::from_millis(500));

    // The queue holds what it can: two entries, though dozens of reads have been wanted.
    let sq = reader.read_sq();
    assert_eq!(sq.capacity(), 2);
    let mut first = 0;
    for entry in sq.drain() {
        let ReadEntry::Data(data) = entry else {
            panic!("a data entry was expected, not {entry:?}");
        };
        client.add_piece(*data.value(), data.range(), data.chunk().to_vec());
        first += 1;
    }
    assert_eq!(first, 2, "the queue holds its capacity and no more");

    // Room has been made, and the reader goes on from where it stopped.
    sq.ring_resume();
    let polled = client.finish_poll();
    assert!(polled.other.is_empty(), "{:?}", polled.other);

    let (bytes, _) = client.assemble(FILE);
    assert_eq!(bytes, content(40), "every byte, once, in order");
    assert!(reader.take_failure().is_none());
}
