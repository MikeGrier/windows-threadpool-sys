// Copyright (c) 2026 Mike Grier
//! A client that holds what it was given stops the reader at the size it chose, and nothing is
//! lost when it lets go (TW-9.5, TW-D-16).
//!
//! Two of the three stops are reached from outside: every buffer held in a chunk, and a client
//! queue nobody has drained. The third, a full ring, is not something a client holds, and is
//! covered by the option set whose ring is smaller than its pool.

use std::time::{Duration, Instant};

use win_multi_file_tail_watcher::PollError;
use win_multi_file_tail_watcher::reader::{TailReader, TailReaderOptions};

use crate::harness::{
    Client, TempDir, crlf, for_lines, hold_data, script, spawn, wait_until_rung, wtf,
};

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
    // Nothing happens is not something to wait for, so this gives the reader time to do something it
    // should not. It can only fail if the reader delivers; a slow host makes it weaker, not flaky.
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
    assert_eq!(bytes, content(40), "every byte, once, put back by range");
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
        "every byte, once, put back by range"
    );
    assert!(reader.take_failure().is_none());
}

#[test]
fn an_undrained_client_queue_holds_the_poll_open_and_loses_nothing() {
    let dir = TempDir::new("held-queue");
    let path = written(&dir, 40);

    let mut options = TailReaderOptions::default();
    options.buffer_len = 8;
    options.buffer_count = 64;
    options.client_sq_capacity = 2;
    let reader = TailReader::<u32>::new(&options, 64).expect("a reader");
    let mut client = Client::new(&reader, options.buffer_len);
    let _registration = reader.register(&wtf(&path), 0, FILE).expect("register");

    // Far more than two reads are wanted, and nobody takes any. How full the queue is cannot be
    // seen through the public API, and a drain is not a count of it: taking an entry can let the
    // reader write the one it was holding, and the drain goes on to take that too. So what is
    // asserted is what an undrained queue of two does to a poll of about a hundred reads whatever
    // the timing: it cannot be complete, so it is in flight and another is refused. That the stop
    // is at exactly the capacity is asserted by the engine's own test,
    // `a_full_client_sq_stops_the_engine_and_ringing_back_continues_it`, which drives it
    // synchronously.
    assert_eq!(reader.read_sq().capacity(), 2);
    reader.poll().expect("start a poll");
    wait_until_rung(&reader);
    assert!(
        matches!(reader.poll(), Err(PollError::InFlight)),
        "a poll that cannot be queued in full is not complete"
    );

    // Room is made, the reader is told, and it goes on from where it stopped.
    let polled = client.finish_poll();
    assert!(polled.other.is_empty(), "{:?}", polled.other);
    let (bytes, _) = client.assemble(FILE);
    assert_eq!(bytes, content(40), "every byte, once, put back by range");
    assert!(reader.take_failure().is_none());
}
