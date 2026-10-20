// Copyright (c) 2026 Mike Grier
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{Attention, SqCapacityError, SqEntry, SqPushError, Terminal, queue};
use crate::{CreateError, Watcher};

fn attention(value: u32, last_known: u64, current: u64) -> SqEntry<u32> {
    SqEntry::Attention(Attention {
        value,
        last_known,
        current,
    })
}

#[test]
fn a_valid_capacity_is_reported() {
    for capacity in [2usize, 4, 64, 1024] {
        let (_writer, reader) = queue::<u32>(capacity).expect("valid");
        assert_eq!(reader.capacity(), capacity);
    }
}

#[test]
fn an_invalid_capacity_names_the_request_and_the_bounds() {
    for requested in [0usize, 1, 3, 5, 100, 1000] {
        let error = queue::<u32>(requested).expect_err("invalid").capacity();
        assert_eq!(error.requested, requested);
        assert_eq!(error.min_valid, 2);
        assert!(error.max_valid.is_power_of_two());
        assert!(error.to_string().contains(&requested.to_string()));
    }
}

#[test]
fn the_capacity_error_is_a_std_error() {
    fn is_error<E: std::error::Error>(_: &E) {}
    let error: SqCapacityError = queue::<u32>(3).expect_err("invalid").capacity();
    is_error(&error);
}

#[test]
fn entries_come_out_in_the_order_they_went_in() {
    let (writer, reader) = queue(8).expect("queue");
    let written = [
        attention(1, 0, 10),
        attention(2, 10, 4),
        SqEntry::PollDone,
        attention(3, 4, 4096),
    ];
    for entry in written.clone() {
        writer.push(entry).expect("room");
    }
    let read: Vec<_> = std::iter::from_fn(|| reader.pop()).collect();
    assert_eq!(read, written);
}

#[test]
fn an_empty_queue_yields_nothing() {
    let (_writer, reader) = queue::<u32>(4).expect("queue");
    assert_eq!(reader.pop(), None);
    assert_eq!(reader.pop(), None);
}

#[test]
fn a_full_queue_refuses_and_hands_the_entry_back() {
    let (writer, reader) = queue(4).expect("queue");
    for n in 0..4u32 {
        writer.push(attention(n, 0, 1)).expect("room");
    }
    let refused = writer.push(attention(99, 7, 8)).expect_err("full");
    assert_eq!(refused, SqPushError::Full(attention(99, 7, 8)));
    assert_eq!(reader.pop(), Some(attention(0, 0, 1)));
    writer.push(attention(99, 7, 8)).expect("room again");
}

#[test]
fn it_holds_exactly_its_capacity() {
    for capacity in [2usize, 8, 32] {
        let (writer, _reader) = queue(capacity).expect("queue");
        for n in 0..capacity {
            writer
                .push(SqEntry::<usize>::PollDone)
                .unwrap_or_else(|_| panic!("slot {n}"));
        }
        assert!(matches!(
            writer.push(SqEntry::PollDone),
            Err(SqPushError::Full(_))
        ));
    }
}

#[test]
fn many_laps_of_a_small_queue_keep_order() {
    let (writer, reader) = queue(4).expect("queue");
    let mut next_out = 0u32;
    for n in 0..1000u32 {
        writer.push(attention(n, 0, 0)).expect("room");
        if n % 3 == 2 {
            while let Some(entry) = reader.pop() {
                assert_eq!(entry, attention(next_out, 0, 0));
                next_out += 1;
            }
        }
    }
    while let Some(entry) = reader.pop() {
        assert_eq!(entry, attention(next_out, 0, 0));
        next_out += 1;
    }
    assert_eq!(next_out, 1000);
}

#[test]
fn a_writer_with_no_reader_is_told_so_and_gets_the_entry_back() {
    let (writer, reader) = queue(4).expect("queue");
    drop(reader);
    let refused = writer.push(attention(5, 1, 2)).expect_err("closed");
    assert_eq!(refused, SqPushError::Closed(attention(5, 1, 2)));
}

#[test]
fn cloned_writers_from_many_threads_lose_nothing_and_keep_each_ones_order() {
    const WRITERS: u32 = 4;
    const EACH: u32 = 100;
    let (writer, reader) = queue(512).expect("queue");
    std::thread::scope(|scope| {
        for id in 0..WRITERS {
            let writer = writer.clone();
            scope.spawn(move || {
                for n in 0..EACH {
                    writer.push(attention(id, 0, u64::from(n))).expect("room");
                }
            });
        }
    });
    let mut next = [0u64; WRITERS as usize];
    let mut total = 0;
    while let Some(SqEntry::Attention(entry)) = reader.pop() {
        assert_eq!(
            entry.current, next[entry.value as usize],
            "writer {}",
            entry.value
        );
        next[entry.value as usize] += 1;
        total += 1;
    }
    assert_eq!(total, WRITERS * EACH);
}

#[test]
fn a_watcher_refuses_a_bad_capacity() {
    for requested in [0usize, 1, 3, 100] {
        let error = Watcher::<u32>::new(requested).expect_err("invalid");
        assert!(matches!(error, CreateError::Capacity(e) if e.requested == requested));
    }
    assert!(Watcher::<u32>::new(2).is_ok());
}

#[test]
fn a_watchers_drain_is_empty_until_something_is_written() {
    let watcher = Watcher::<u32>::new(8).expect("watcher");
    assert_eq!(watcher.drain().count(), 0);
}

#[test]
fn what_the_crate_writes_the_caller_drains_in_order_and_once() {
    let watcher = Watcher::new(8).expect("watcher");
    let writer = watcher.sq_writer();
    writer.push(attention(1, 0, 5)).expect("room");
    writer.push(SqEntry::PollDone).expect("room");
    let drained: Vec<_> = watcher.drain().collect();
    assert_eq!(drained, [attention(1, 0, 5), SqEntry::PollDone]);
    assert_eq!(watcher.drain().count(), 0);
}

#[test]
fn draining_some_makes_room_for_as_many() {
    let watcher = Watcher::new(4).expect("watcher");
    let writer = watcher.sq_writer();
    for n in 0..4 {
        writer.push(attention(n, 0, 0)).expect("room");
    }
    assert!(matches!(
        writer.push(attention(4, 0, 0)),
        Err(SqPushError::Full(_))
    ));
    {
        let mut drain = watcher.drain();
        assert_eq!(drain.next(), Some(attention(0, 0, 0)));
        assert_eq!(drain.next(), Some(attention(1, 0, 0)));
    }
    writer.push(attention(4, 0, 0)).expect("one");
    writer.push(attention(5, 0, 0)).expect("two");
    assert!(writer.push(attention(6, 0, 0)).is_err());
    let rest: Vec<_> = watcher.drain().collect();
    assert_eq!(rest.len(), 4);
}

#[test]
fn an_entry_outlives_the_registration_it_describes() {
    let tag = Arc::new(());
    let watcher = Watcher::new(4).expect("watcher");
    let writer = watcher.sq_writer();
    writer
        .push(SqEntry::Attention(Attention {
            value: Arc::clone(&tag),
            last_known: 3,
            current: 9,
        }))
        .expect("room");
    assert_eq!(Arc::strong_count(&tag), 2);
    let entry = watcher.drain().next().expect("entry");
    assert_eq!(Arc::strong_count(&tag), 2);
    drop(entry);
    assert_eq!(Arc::strong_count(&tag), 1);
}

#[test]
fn writers_and_a_draining_caller_run_together_without_loss() {
    const WRITERS: u32 = 4;
    const EACH: u32 = 200;
    let watcher = Watcher::<u32>::new(16).expect("watcher");
    let mut seen = vec![0u32; WRITERS as usize];
    // The writers give up at the same deadline, so a stalled drain fails the test instead of
    // leaving the scope waiting on writers that can never get room.
    let deadline = Instant::now() + Duration::from_secs(8);
    std::thread::scope(|scope| {
        for id in 0..WRITERS {
            let writer = watcher.sq_writer();
            scope.spawn(move || {
                for n in 0..EACH {
                    let mut entry = attention(id, 0, u64::from(n));
                    while let Err(SqPushError::Full(back)) = writer.push(entry) {
                        entry = back;
                        if Instant::now() > deadline {
                            return;
                        }
                        std::thread::yield_now();
                    }
                }
            });
        }
        let mut total = 0;
        while total < WRITERS * EACH {
            assert!(Instant::now() < deadline, "stalled at {total}");
            for entry in watcher.drain() {
                let SqEntry::Attention(entry) = entry else {
                    panic!("only attentions were written");
                };
                assert_eq!(
                    u32::try_from(entry.current).expect("small"),
                    seen[entry.value as usize]
                );
                seen[entry.value as usize] += 1;
                total += 1;
            }
            std::thread::yield_now();
        }
    });
    assert!(seen.iter().all(|&count| count == EACH));
}

#[test]
fn entries_compare_by_kind_and_content() {
    assert_eq!(attention(1, 2, 3), attention(1, 2, 3));
    assert_ne!(attention(1, 2, 3), attention(1, 2, 4));
    assert_ne!(attention(1, 2, 3), attention(1, 3, 3));
    assert_ne!(attention(1, 2, 3), attention(2, 2, 3));
    assert_ne!(attention(0, 0, 0), SqEntry::PollDone);
}

fn terminal(value: u32, win32_code: u32, last_known: u64) -> SqEntry<u32> {
    SqEntry::Terminal(Terminal {
        value,
        win32_code,
        last_known,
    })
}

#[test]
fn a_terminal_entry_carries_the_value_the_code_and_the_last_length() {
    let SqEntry::Terminal(entry) = terminal(7, 6, 1234) else {
        panic!("a terminal entry");
    };
    assert_eq!(
        (entry.value, entry.win32_code, entry.last_known),
        (7, 6, 1234)
    );
}

#[test]
fn terminal_entries_compare_by_every_field_and_by_kind() {
    assert_eq!(terminal(1, 6, 10), terminal(1, 6, 10));
    assert_ne!(terminal(1, 6, 10), terminal(2, 6, 10));
    assert_ne!(terminal(1, 6, 10), terminal(1, 1006, 10));
    assert_ne!(terminal(1, 6, 10), terminal(1, 6, 11));
    assert_ne!(terminal(1, 6, 10), attention(1, 6, 10));
    assert_ne!(terminal(1, 6, 10), SqEntry::PollDone);
    assert_eq!(terminal(1, 6, 10).clone(), terminal(1, 6, 10));
}

#[test]
fn the_three_kinds_come_out_in_the_order_they_went_in() {
    let (writer, reader) = queue(8).expect("queue");
    let written = [
        attention(1, 0, 10),
        terminal(2, 6, 4),
        SqEntry::PollDone,
        terminal(3, 1006, 0),
        attention(4, 10, 11),
    ];
    for entry in written.clone() {
        writer.push(entry).expect("room");
    }
    let read: Vec<_> = std::iter::from_fn(|| reader.pop()).collect();
    assert_eq!(read, written);
}

#[test]
fn a_full_queue_hands_a_terminal_entry_back_intact() {
    let (writer, _reader) = queue(2).expect("queue");
    writer.push(attention(0, 0, 1)).expect("room");
    writer.push(attention(1, 0, 1)).expect("room");
    let refused = writer.push(terminal(9, 6, 77)).expect_err("full");
    assert_eq!(refused, SqPushError::Full(terminal(9, 6, 77)));
}

#[test]
fn a_closed_queue_hands_a_terminal_entry_back_intact() {
    let (writer, reader) = queue(4).expect("queue");
    drop(reader);
    let refused = writer.push(terminal(9, 1006, 5)).expect_err("closed");
    assert_eq!(refused, SqPushError::Closed(terminal(9, 1006, 5)));
}

#[test]
fn a_terminal_entry_is_drained_by_the_caller_among_the_others() {
    let watcher = Watcher::new(8).expect("watcher");
    let writer = watcher.sq_writer();
    writer.push(attention(1, 0, 5)).expect("room");
    writer.push(terminal(2, 6, 5)).expect("room");
    writer.push(SqEntry::PollDone).expect("room");
    let drained: Vec<_> = watcher.drain().collect();
    assert_eq!(
        drained,
        [attention(1, 0, 5), terminal(2, 6, 5), SqEntry::PollDone]
    );
}

#[test]
fn a_terminal_entry_owns_its_value_independently_of_the_registration() {
    let tag = Arc::new(());
    let (writer, reader) = queue(4).expect("queue");
    writer
        .push(SqEntry::Terminal(Terminal {
            value: Arc::clone(&tag),
            win32_code: 6,
            last_known: 3,
        }))
        .expect("room");
    assert_eq!(Arc::strong_count(&tag), 2);
    drop(reader.pop().expect("entry"));
    assert_eq!(Arc::strong_count(&tag), 1);
}
