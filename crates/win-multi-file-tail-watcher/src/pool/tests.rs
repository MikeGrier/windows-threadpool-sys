// Copyright (c) 2026 Mike Grier
use std::collections::HashSet;
use std::sync::Barrier;

use io_buffers::{IoBuf, IoBufMut};

use super::{BufferPool, Chunk, MAX_BUFFER_LEN, PoolBuffer, PoolError, check};
use crate::test_support::{is_signalled, signalled_within};

fn pool(buffer_len: usize, count: usize) -> BufferPool {
    BufferPool::new(buffer_len, count).expect("a valid pool")
}

fn fill(buffer: &mut PoolBuffer, bytes: &[u8]) {
    // SAFETY: `bytes` fits the buffer, which this test owns exclusively.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.stable_mut_ptr(), bytes.len());
    }
}

fn chunk(pool: &BufferPool, bytes: &[u8]) -> Chunk {
    let mut buffer = pool.take().expect("a buffer");
    fill(&mut buffer, bytes);
    buffer.into_chunk(bytes.len())
}

#[test]
fn dimensions_out_of_range_are_refused_before_anything_is_allocated() {
    assert!(matches!(check(0, 1), Err(PoolError::BufferLen(0))));
    assert!(matches!(
        check(MAX_BUFFER_LEN + 1, 1),
        Err(PoolError::BufferLen(len)) if len == MAX_BUFFER_LEN + 1
    ));
    assert!(matches!(check(1, 0), Err(PoolError::NoBuffers)));
    assert!(check(1, 1).is_ok());
    assert!(check(MAX_BUFFER_LEN, 1).is_ok());
    assert!(matches!(
        BufferPool::new(0, 4),
        Err(PoolError::BufferLen(0))
    ));
    assert!(matches!(BufferPool::new(64, 0), Err(PoolError::NoBuffers)));
}

#[test]
fn a_count_that_cannot_be_reserved_is_an_allocation_error() {
    assert!(matches!(
        BufferPool::new(1, usize::MAX / 2),
        Err(PoolError::Allocation(_))
    ));
}

#[test]
fn a_new_pool_has_every_buffer_available_at_its_length() {
    let pool = pool(4096, 8);
    assert_eq!(pool.buffer_len(), 4096);
    assert_eq!(pool.count(), 8);
    assert_eq!(pool.available(), 8);
    let buffer = pool.take().expect("a buffer");
    assert_eq!(buffer.bytes_len(), 4096);
    assert_eq!(pool.available(), 7);
}

#[test]
fn every_byte_starts_zeroed() {
    let pool = pool(256, 3);
    let buffers: Vec<_> = (0..3).map(|_| pool.take().expect("a buffer")).collect();
    for buffer in buffers {
        assert!(buffer.into_chunk(256).iter().all(|&byte| byte == 0));
    }
}

#[test]
fn takes_succeed_up_to_the_count_and_then_are_refused() {
    let pool = pool(16, 5);
    let lent: Vec<_> = (0..5).map(|_| pool.take().expect("a buffer")).collect();
    assert!(pool.take().is_none());
    assert!(pool.take().is_none());
    assert_eq!(pool.available(), 0);
    drop(lent);
    assert_eq!(pool.available(), 5);
}

#[test]
fn a_dropped_buffer_goes_back_whether_or_not_it_became_a_chunk() {
    let pool = pool(16, 2);
    drop(pool.take().expect("a buffer"));
    assert_eq!(pool.available(), 2);
    let chunk = chunk(&pool, b"abc");
    assert_eq!(pool.available(), 1);
    drop(chunk);
    assert_eq!(pool.available(), 2);
}

#[test]
fn the_buffers_are_distinct_and_do_not_overlap() {
    let pool = pool(100, 6);
    let lent: Vec<_> = (0..6).map(|_| pool.take().expect("a buffer")).collect();
    let mut spans: Vec<_> = lent
        .iter()
        .map(|buffer| buffer.stable_ptr() as usize)
        .collect();
    spans.sort_unstable();
    assert_eq!(spans.iter().copied().collect::<HashSet<_>>().len(), 6);
    for pair in spans.windows(2) {
        assert!(pair[1] - pair[0] >= 100, "buffers overlap");
    }
}

#[test]
fn a_buffer_given_back_is_lent_again_without_a_new_allocation() {
    let pool = pool(64, 1);
    let first = pool.take().expect("a buffer");
    let address = first.stable_ptr();
    drop(first);
    assert_eq!(pool.take().expect("again").stable_ptr(), address);
}

#[test]
fn the_address_is_stable_across_moves_and_the_same_for_reading_and_writing() {
    let pool = pool(64, 1);
    let mut buffer = pool.take().expect("a buffer");
    let address = buffer.stable_ptr();
    assert_eq!(buffer.stable_mut_ptr().cast_const(), address);
    let moved = Box::new(buffer);
    assert_eq!(moved.stable_ptr(), address);
    let back = *moved;
    assert_eq!(back.stable_ptr(), address);
    assert_eq!(back.bytes_len(), 64);
}

#[test]
fn a_chunk_shows_the_bytes_read_and_nothing_past_them() {
    let pool = pool(32, 1);
    let first = chunk(&pool, b"hello, tail");
    assert_eq!(&*first, b"hello, tail");
    assert_eq!(first.len(), 11);
    assert_eq!(first.as_ref(), b"hello, tail");
    drop(first);
    // The same buffer again, with stale bytes past what this read wrote.
    let second = chunk(&pool, b"hi");
    assert_eq!(&*second, b"hi");
}

#[test]
fn a_chunk_may_be_empty_or_the_whole_buffer() {
    let pool = pool(8, 2);
    let empty = pool.take().expect("a buffer").into_chunk(0);
    assert!(empty.is_empty());
    let full = chunk(&pool, b"12345678");
    assert_eq!(&*full, b"12345678");
}

#[test]
#[should_panic(expected = "9 bytes read into a buffer of 8")]
fn a_chunk_longer_than_its_buffer_panics() {
    let pool = pool(8, 1);
    let _ = pool.take().expect("a buffer").into_chunk(9);
}

#[test]
fn the_doorbell_is_rung_only_by_the_first_return_after_a_refused_take() {
    let pool = pool(8, 2);
    let first = pool.take().expect("a buffer");
    drop(first);
    assert!(!is_signalled(pool.doorbell()), "no take was refused");

    let a = pool.take().expect("a buffer");
    let b = pool.take().expect("a buffer");
    assert!(pool.take().is_none());
    assert!(!is_signalled(pool.doorbell()), "refusing does not ring");
    drop(a);
    assert!(is_signalled(pool.doorbell()));
    pool.reset_doorbell();
    drop(b);
    assert!(
        !is_signalled(pool.doorbell()),
        "the starvation was answered"
    );
}

#[test]
fn after_a_reset_a_second_refusal_is_answered_by_a_second_ring() {
    let pool = pool(8, 1);
    for _ in 0..3 {
        let held = pool.take().expect("a buffer");
        assert!(pool.take().is_none());
        drop(held);
        assert!(is_signalled(pool.doorbell()));
        pool.reset_doorbell();
        assert!(!is_signalled(pool.doorbell()));
    }
}

#[test]
fn a_chunk_dropped_on_another_thread_gives_its_buffer_back_and_rings() {
    let pool = pool(8, 1);
    let chunk = chunk(&pool, b"x");
    assert!(pool.take().is_none());
    std::thread::spawn(move || drop(chunk))
        .join()
        .expect("dropper");
    assert!(signalled_within(pool.doorbell(), 1_000));
    assert_eq!(pool.available(), 1);
}

#[test]
fn lent_buffers_may_outlive_the_pool() {
    let pool = pool(8, 2);
    let chunk = chunk(&pool, b"kept");
    let buffer = pool.take().expect("a buffer");
    drop(pool);
    assert_eq!(&*chunk, b"kept");
    drop(buffer);
    drop(chunk);
}

#[test]
fn many_threads_taking_and_giving_back_lose_and_gain_no_buffer() {
    const THREADS: usize = 8;
    let pool = pool(16, 4);
    let start = Barrier::new(THREADS);
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                start.wait();
                for n in 0..2_000u32 {
                    if let Some(mut buffer) = pool.take() {
                        fill(&mut buffer, &n.to_le_bytes());
                        let chunk = buffer.into_chunk(4);
                        assert_eq!(&*chunk, n.to_le_bytes());
                    }
                }
            });
        }
    });
    assert_eq!(pool.available(), 4);
}

#[test]
fn errors_say_what_was_wrong() {
    assert!(PoolError::BufferLen(0).to_string().contains("0 bytes"));
    assert_eq!(PoolError::NoBuffers.to_string(), "a pool of no buffers");
    let io = PoolError::Io(std::io::Error::from_raw_os_error(5));
    assert!(std::error::Error::source(&io).is_some());
    assert!(std::error::Error::source(&PoolError::NoBuffers).is_none());
    let Err(allocation) = BufferPool::new(1, usize::MAX / 2) else {
        panic!("reserved");
    };
    assert!(allocation.to_string().contains("could not be allocated"));
    assert!(std::error::Error::source(&allocation).is_some());
}

#[test]
fn a_pool_and_its_buffers_are_send_and_sync() {
    fn both<V: Send + Sync>() {}
    both::<BufferPool>();
    both::<PoolBuffer>();
    both::<Chunk>();
}
