// Copyright (c) 2026 Mike Grier
use super::{Data, ReadEntry, ReadSq, Shrink};
use crate::pool::{BufferPool, Chunk};
use crate::sq::{PushFailure, Terminal, queue};
use crate::test_support::is_signalled;
use crate::{CreateError, SqCapacityError};

use io_buffers::IoBufMut;

fn sq(capacity: usize) -> ReadSq<u32> {
    ReadSq::new(capacity).expect("a valid capacity")
}

fn pool(count: usize) -> BufferPool {
    BufferPool::new(16, count).expect("a pool")
}

fn chunk(pool: &BufferPool, bytes: &[u8]) -> Chunk {
    let mut buffer = pool.take().expect("a buffer");
    // SAFETY: `bytes` fits the buffer, which this test owns exclusively.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.stable_mut_ptr(), bytes.len());
    }
    buffer.into_chunk(bytes.len())
}

fn data(pool: &BufferPool, value: u32, offset: u64, bytes: &[u8]) -> ReadEntry<u32> {
    ReadEntry::Data(Data::new(value, offset, chunk(pool, bytes)))
}

fn capacity_error(capacity: usize) -> SqCapacityError {
    match ReadSq::<u32>::new(capacity) {
        Err(CreateError::Capacity(error)) => error,
        other => panic!("capacity {capacity}: {other:?}"),
    }
}

#[test]
fn capacities_are_refused_and_accepted_by_the_watchers_rule() {
    for capacity in [0, 1, 3, 5, 6, 100] {
        let expected = queue::<u32>(capacity).expect_err("refused for the watcher");
        assert_eq!(capacity_error(capacity), expected, "capacity {capacity}");
    }
    for capacity in [2, 4, 64, 1024] {
        assert!(queue::<u32>(capacity).is_ok());
        assert_eq!(sq(capacity).capacity(), capacity);
    }
}

#[test]
fn a_data_entrys_range_is_computed_from_its_chunk() {
    let pool = pool(3);
    let entry = Data::new(7u32, 100, chunk(&pool, b"abcde"));
    assert_eq!(*entry.value(), 7);
    assert_eq!(entry.range(), 100..105);
    assert_eq!(&**entry.chunk(), b"abcde");
    let empty = Data::new(8u32, 42, chunk(&pool, b""));
    assert_eq!(empty.range(), 42..42);
    let far = Data::new(9u32, u64::MAX - 3, chunk(&pool, b"xyz"));
    assert_eq!(far.range(), u64::MAX - 3..u64::MAX);
}

#[test]
#[should_panic(expected = "a file range ends before u64::MAX")]
fn a_data_entry_whose_range_would_overflow_panics() {
    let pool = pool(1);
    let _ = Data::new(0u32, u64::MAX - 1, chunk(&pool, b"ab"));
}

#[test]
fn into_parts_hands_back_the_value_range_and_chunk() {
    let pool = pool(1);
    let (value, range, chunk) = Data::new(3u32, 10, chunk(&pool, b"xyz")).into_parts();
    assert_eq!((value, range), (3, 10..13));
    assert_eq!(&*chunk, b"xyz");
    assert_eq!(pool.available(), 0);
    drop(chunk);
    assert_eq!(pool.available(), 1);
}

#[test]
fn entries_of_every_kind_are_drained_in_the_order_written() {
    let pool = pool(4);
    let sq = sq(8);
    sq.push(data(&pool, 1, 0, b"hello")).expect("room");
    sq.push(ReadEntry::Shrink(Shrink {
        value: 2,
        last_known: 50,
        current: 10,
    }))
    .expect("room");
    sq.push(ReadEntry::Terminal(Terminal {
        value: 3,
        win32_code: 6,
        last_known: 4,
    }))
    .expect("room");
    sq.push(data(&pool, 1, 5, b" world")).expect("room");
    sq.push(ReadEntry::PollDone).expect("room");

    let entries: Vec<_> = sq.drain().collect();
    assert_eq!(entries.len(), 5);
    assert!(
        matches!(&entries[0], ReadEntry::Data(d) if d.range() == (0..5) && &**d.chunk() == b"hello")
    );
    assert!(matches!(
        &entries[1],
        ReadEntry::Shrink(Shrink {
            value: 2,
            last_known: 50,
            current: 10
        })
    ));
    assert!(matches!(
        &entries[2],
        ReadEntry::Terminal(Terminal {
            value: 3,
            win32_code: 6,
            last_known: 4
        })
    ));
    assert!(matches!(&entries[3], ReadEntry::Data(d) if d.range() == (5..11)));
    assert!(matches!(entries[4], ReadEntry::PollDone));
    assert_eq!(sq.drain().count(), 0);
}

#[test]
fn a_full_sq_hands_the_entry_back_with_its_chunk_still_lent() {
    let pool = pool(3);
    let sq = sq(2);
    sq.push(data(&pool, 1, 0, b"a")).expect("room");
    sq.push(data(&pool, 1, 1, b"b")).expect("room");
    let refused = match sq.push(data(&pool, 1, 2, b"c")) {
        Err(PushFailure::Full(entry)) => entry,
        other => panic!("{other:?}"),
    };
    assert_eq!(pool.available(), 0, "the refused chunk is still lent");
    assert!(matches!(&refused, ReadEntry::Data(d) if d.range() == (2..3)));
    drop(refused);
    assert_eq!(pool.available(), 1);
    assert_eq!(sq.drain().count(), 2);
    sq.push(ReadEntry::PollDone).expect("room again");
}

#[test]
fn chunks_go_back_to_the_pool_when_their_entries_are_dropped() {
    let pool = pool(4);
    let sq = sq(4);
    for n in 0..4u8 {
        sq.push(data(&pool, 1, u64::from(n), &[n])).expect("room");
    }
    assert_eq!(pool.available(), 0);
    let drained: Vec<_> = sq.drain().take(2).collect();
    assert_eq!(pool.available(), 0, "drained but still held");
    drop(drained);
    assert_eq!(pool.available(), 2);
    drop(sq);
    assert_eq!(
        pool.available(),
        4,
        "entries left in a dropped SQ give theirs back"
    );
}

#[test]
fn the_two_doorbells_are_separate_and_start_unrung() {
    let sq = sq(2);
    assert!(!is_signalled(sq.doorbell()));
    assert!(!is_signalled(sq.resume_doorbell()));
    sq.ring_doorbell();
    assert!(is_signalled(sq.doorbell()));
    assert!(!is_signalled(sq.resume_doorbell()));
    sq.ring_resume();
    sq.reset_doorbell();
    assert!(!is_signalled(sq.doorbell()));
    assert!(is_signalled(sq.resume_doorbell()));
    sq.reset_resume();
    assert!(!is_signalled(sq.resume_doorbell()));
}

#[test]
fn a_doorbell_stays_rung_for_every_waiter_until_reset() {
    let sq = sq(2);
    sq.ring_doorbell();
    sq.ring_doorbell();
    for _ in 0..3 {
        assert!(is_signalled(sq.doorbell()));
    }
    sq.reset_doorbell();
    sq.reset_doorbell();
    assert!(!is_signalled(sq.doorbell()));
}

#[test]
fn writers_on_many_threads_each_land_every_entry() {
    const THREADS: u32 = 4;
    const EACH: u32 = 100;
    let sq = sq(1024);
    std::thread::scope(|scope| {
        for thread in 0..THREADS {
            let writer = sq.writer();
            scope.spawn(move || {
                for n in 0..EACH {
                    writer
                        .push(ReadEntry::Shrink(Shrink {
                            value: thread,
                            last_known: u64::from(n) + 1,
                            current: u64::from(n),
                        }))
                        .expect("room");
                }
            });
        }
    });
    let mut seen = vec![Vec::new(); THREADS as usize];
    for entry in sq.drain() {
        let ReadEntry::Shrink(shrink) = entry else {
            panic!("only shrinks were written");
        };
        seen[shrink.value as usize].push(shrink.current);
    }
    for per_thread in seen {
        assert_eq!(
            per_thread,
            (0..u64::from(EACH)).collect::<Vec<_>>(),
            "in each writer's order"
        );
    }
}

#[test]
fn the_sq_and_its_entries_can_cross_threads() {
    fn send<V: Send>() {}
    fn sync<V: Sync>() {}
    send::<ReadEntry<u32>>();
    send::<ReadSq<u32>>();
    sync::<ReadSq<u32>>();
}
