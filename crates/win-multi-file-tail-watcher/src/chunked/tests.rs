// Copyright (c) 2026 Mike Grier
use std::sync::Arc;

use super::{CHUNK_BYTES, Chunked};

fn per_chunk<T>() -> usize {
    Chunked::<T>::PER_CHUNK
}

fn filled(count: usize) -> Chunked<usize> {
    let mut buffer = Chunked::new();
    for n in 0..count {
        buffer.push(n);
    }
    buffer
}

fn contents(buffer: &Chunked<usize>) -> Vec<usize> {
    buffer.iter().copied().collect()
}

#[test]
fn a_new_buffer_is_empty_and_allocates_nothing() {
    let buffer = Chunked::<u64>::new();
    assert_eq!(buffer.len(), 0);
    assert!(buffer.is_empty());
    assert_eq!(buffer.get(0), None);
    assert_eq!(buffer.iter().count(), 0);
    assert!(buffer.chunk_addresses().is_empty());
}

#[test]
fn items_come_back_by_index_and_in_order_within_one_chunk() {
    let buffer = filled(10);
    assert_eq!(buffer.len(), 10);
    assert!(!buffer.is_empty());
    for n in 0..10 {
        assert_eq!(buffer.get(n), Some(&n));
    }
    assert_eq!(contents(&buffer), (0..10).collect::<Vec<_>>());
}

#[test]
fn items_across_several_chunks_keep_their_index_and_order() {
    let per = per_chunk::<usize>();
    let count = 3 * per + 7;
    let buffer = filled(count);
    assert_eq!(buffer.len(), count);
    assert_eq!(buffer.chunk_addresses().len(), 4);
    for n in 0..count {
        assert_eq!(buffer.get(n), Some(&n), "index {n}");
    }
    assert_eq!(contents(&buffer), (0..count).collect::<Vec<_>>());
}

#[test]
fn the_chunk_edges_are_where_they_should_be() {
    let per = per_chunk::<usize>();
    let buffer = filled(2 * per + 1);
    for n in [per - 1, per, per + 1, 2 * per - 1, 2 * per] {
        assert_eq!(buffer.get(n), Some(&n));
    }
    assert_eq!(buffer.get(2 * per + 1), None);
    assert_eq!(buffer.get(usize::MAX), None);
    assert_eq!(filled(per).chunk_addresses().len(), 1);
    assert_eq!(filled(per + 1).chunk_addresses().len(), 2);
}

#[test]
fn items_per_chunk_follow_from_the_item_size() {
    assert_eq!(per_chunk::<u8>(), CHUNK_BYTES);
    assert_eq!(per_chunk::<u32>(), CHUNK_BYTES / 4);
    assert_eq!(per_chunk::<u64>(), CHUNK_BYTES / 8);
    assert_eq!(per_chunk::<(u64, usize, u64)>(), 2730);
    assert_eq!(per_chunk::<[u8; 1000]>(), 65);
    assert_eq!(per_chunk::<[u8; CHUNK_BYTES]>(), 1);
    assert_eq!(per_chunk::<[u8; 40_000]>(), 1);
}

fn no_chunk_is_larger_than_the_limit<T>(count: usize, make: impl Fn() -> T) {
    let mut buffer = Chunked::new();
    for _ in 0..count {
        buffer.push(make());
    }
    for bytes in buffer.chunk_allocation_bytes() {
        assert!(bytes <= CHUNK_BYTES, "{bytes} bytes");
    }
}

#[test]
fn no_allocation_is_larger_than_64_kib_whatever_the_item() {
    no_chunk_is_larger_than_the_limit(200_000, || 0u8);
    no_chunk_is_larger_than_the_limit(100_000, || 0u32);
    no_chunk_is_larger_than_the_limit(40_000, || (0u64, 0usize, 0u64));
    no_chunk_is_larger_than_the_limit(300, || [0u8; 1000]);
    no_chunk_is_larger_than_the_limit(5, || [0u8; 40_000]);
    no_chunk_is_larger_than_the_limit(3, || [0u8; CHUNK_BYTES]);
}

#[test]
fn forty_thousand_poll_entries_take_fifteen_chunks() {
    let mut buffer = Chunked::<(u64, usize, u64)>::new();
    for n in 0..40_000u64 {
        buffer.push((n, 0, n));
    }
    assert_eq!(buffer.chunk_addresses().len(), 15);
}

#[test]
fn pushing_never_moves_an_item_already_stored() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(per);
    let before = buffer.get(0).map(std::ptr::from_ref);
    let first_chunk = buffer.chunk_addresses()[0];
    for n in per..3 * per {
        buffer.push(n);
    }
    assert_eq!(buffer.get(0).map(std::ptr::from_ref), before);
    assert_eq!(buffer.chunk_addresses()[0], first_chunk);
}

#[test]
fn clear_empties_and_keeps_every_chunk() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(2 * per + 5);
    let addresses = buffer.chunk_addresses();
    buffer.clear();
    assert_eq!(buffer.len(), 0);
    assert!(buffer.is_empty());
    assert_eq!(buffer.get(0), None);
    assert_eq!(buffer.iter().count(), 0);
    assert_eq!(buffer.chunk_addresses(), addresses);
}

#[test]
fn a_refill_after_clear_allocates_nothing() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(2 * per + 5);
    let addresses = buffer.chunk_addresses();
    for round in 1..=20 {
        buffer.clear();
        let count = (round * 977) % (2 * per + 5);
        for n in 0..count {
            buffer.push(n);
        }
        assert_eq!(
            contents(&buffer),
            (0..count).collect::<Vec<_>>(),
            "round {round}"
        );
        assert_eq!(buffer.chunk_addresses(), addresses, "round {round}");
    }
}

#[test]
fn clear_drops_the_items() {
    let tag = Arc::new(());
    let mut buffer = Chunked::new();
    for _ in 0..100 {
        buffer.push(Arc::clone(&tag));
    }
    assert_eq!(Arc::strong_count(&tag), 101);
    buffer.clear();
    assert_eq!(Arc::strong_count(&tag), 1);
}

#[test]
fn truncate_keeps_the_front_and_drops_the_rest() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(3 * per);
    buffer.truncate(2 * per + 3);
    assert_eq!(buffer.len(), 2 * per + 3);
    assert_eq!(contents(&buffer), (0..2 * per + 3).collect::<Vec<_>>());
    assert_eq!(buffer.get(2 * per + 3), None);
    buffer.truncate(per);
    assert_eq!(contents(&buffer), (0..per).collect::<Vec<_>>());
    buffer.truncate(0);
    assert!(buffer.is_empty());
}

#[test]
fn truncate_to_or_past_the_length_does_nothing() {
    let mut buffer = filled(10);
    buffer.truncate(10);
    buffer.truncate(11);
    buffer.truncate(usize::MAX);
    assert_eq!(contents(&buffer), (0..10).collect::<Vec<_>>());
}

#[test]
fn truncate_drops_exactly_the_items_it_cuts() {
    let tag = Arc::new(());
    let mut buffer = Chunked::new();
    for _ in 0..50 {
        buffer.push(Arc::clone(&tag));
    }
    buffer.truncate(20);
    assert_eq!(Arc::strong_count(&tag), 21);
}

#[test]
fn pushing_after_a_truncate_continues_from_the_cut_across_a_chunk_edge() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(2 * per);
    buffer.truncate(per - 2);
    for n in 0..5 {
        buffer.push(1000 + n);
    }
    assert_eq!(buffer.len(), per + 3);
    assert_eq!(buffer.get(per - 3), Some(&(per - 3)));
    for n in 0..5 {
        assert_eq!(buffer.get(per - 2 + n), Some(&(1000 + n)));
    }
}

#[test]
fn retain_keeps_the_accepted_items_in_order_across_chunks() {
    let per = per_chunk::<usize>();
    let count = 3 * per + 11;
    let mut buffer = filled(count);
    buffer.retain(|n| n % 3 == 0);
    let expected: Vec<_> = (0..count).filter(|n| n % 3 == 0).collect();
    assert_eq!(buffer.len(), expected.len());
    assert_eq!(contents(&buffer), expected);
    for (index, value) in expected.iter().enumerate() {
        assert_eq!(buffer.get(index), Some(value));
    }
}

#[test]
fn retain_keeping_everything_changes_nothing() {
    let mut buffer = filled(per_chunk::<usize>() + 9);
    buffer.retain(|_| true);
    assert_eq!(
        contents(&buffer),
        (0..per_chunk::<usize>() + 9).collect::<Vec<_>>()
    );
}

#[test]
fn retain_keeping_nothing_empties_it_and_keeps_the_chunks() {
    let mut buffer = filled(2 * per_chunk::<usize>() + 1);
    let addresses = buffer.chunk_addresses();
    buffer.retain(|_| false);
    assert!(buffer.is_empty());
    assert_eq!(buffer.chunk_addresses(), addresses);
}

#[test]
fn retain_at_the_ends_and_around_the_chunk_edges() {
    let per = per_chunk::<usize>();
    let count = 2 * per + 4;
    for keep in [
        vec![0usize],
        vec![count - 1],
        vec![per - 1, per],
        vec![per - 1, per, 2 * per - 1, 2 * per],
        vec![0, count - 1],
    ] {
        let mut buffer = filled(count);
        buffer.retain(|n| keep.contains(n));
        assert_eq!(contents(&buffer), keep);
    }
}

#[test]
fn retain_keeps_a_run_and_drops_the_runs_on_either_side() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(3 * per);
    buffer.retain(|&n| (per - 5..2 * per + 5).contains(&n));
    assert_eq!(
        contents(&buffer),
        (per - 5..2 * per + 5).collect::<Vec<_>>()
    );
}

#[test]
fn retain_drops_each_rejected_item_exactly_once() {
    let tag = Arc::new(());
    let mut buffer = Chunked::new();
    for n in 0..300usize {
        buffer.push((n, Arc::clone(&tag)));
    }
    buffer.retain(|(n, _)| n % 4 == 0);
    assert_eq!(buffer.len(), 75);
    assert_eq!(Arc::strong_count(&tag), 76);
}

#[test]
fn retain_works_without_the_items_being_copy_or_default() {
    let mut buffer = Chunked::new();
    for n in 0..5000 {
        buffer.push(format!("item {n}"));
    }
    buffer.retain(|s| s.ends_with('7'));
    assert_eq!(buffer.len(), 500);
    assert_eq!(buffer.get(0).map(String::as_str), Some("item 7"));
    assert_eq!(buffer.get(499).map(String::as_str), Some("item 4997"));
}

#[test]
fn retain_sees_each_item_once_in_order() {
    let mut buffer = filled(per_chunk::<usize>() + 100);
    let mut seen = Vec::new();
    buffer.retain(|&n| {
        seen.push(n);
        true
    });
    assert_eq!(seen, (0..per_chunk::<usize>() + 100).collect::<Vec<_>>());
}

#[test]
fn a_buffer_is_usable_after_retain_and_after_clear_again() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(2 * per);
    buffer.retain(|n| n % 2 == 1);
    buffer.push(7);
    assert_eq!(buffer.len(), per + 1);
    assert_eq!(buffer.get(per), Some(&7));
    buffer.clear();
    buffer.push(1);
    assert_eq!(contents(&buffer), [1]);
}

#[test]
fn it_crosses_threads_when_its_items_do() {
    fn send_sync<X: Send + Sync>() {}
    send_sync::<Chunked<u64>>();
}

#[test]
fn drain_each_hands_over_every_item_in_order_across_chunks() {
    let per = per_chunk::<usize>();
    let count = 2 * per + 9;
    let mut buffer = filled(count);
    let mut seen = Vec::new();
    buffer.drain_each(|n| seen.push(n));
    assert_eq!(seen, (0..count).collect::<Vec<_>>());
}

#[test]
fn drain_each_leaves_the_buffer_empty_and_its_chunks_in_place() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(2 * per + 9);
    let addresses = buffer.chunk_addresses();
    buffer.drain_each(|_| {});
    assert_eq!(buffer.len(), 0);
    assert!(buffer.is_empty());
    assert_eq!(buffer.get(0), None);
    assert_eq!(buffer.iter().count(), 0);
    assert_eq!(buffer.chunk_addresses(), addresses);
}

#[test]
fn drain_each_on_an_empty_buffer_calls_nothing() {
    let mut buffer = Chunked::<usize>::new();
    buffer.drain_each(|_| panic!("nothing to hand over"));
    let mut cleared = filled(5);
    cleared.clear();
    cleared.drain_each(|_| panic!("nothing to hand over"));
}

#[test]
fn drain_each_drops_each_item_as_its_call_returns() {
    let tag = Arc::new(());
    let mut buffer = Chunked::new();
    for _ in 0..50 {
        buffer.push(Arc::clone(&tag));
    }
    let mut remaining = 51;
    buffer.drain_each(|item| {
        assert_eq!(Arc::strong_count(&tag), remaining);
        drop(item);
        remaining -= 1;
    });
    assert_eq!(Arc::strong_count(&tag), 1);
}

#[test]
fn a_buffer_is_refilled_after_drain_each_with_no_new_allocation() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(2 * per + 3);
    let addresses = buffer.chunk_addresses();
    buffer.drain_each(|_| {});
    for n in 0..per + 4 {
        buffer.push(n);
    }
    assert_eq!(contents(&buffer), (0..per + 4).collect::<Vec<_>>());
    assert_eq!(buffer.chunk_addresses(), addresses);
}

#[test]
fn clear_restores_a_buffer_whose_drain_each_panicked() {
    let per = per_chunk::<usize>();
    let mut buffer = filled(2 * per);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        buffer.drain_each(|n| assert!(n < per + 5, "stop part-way"));
    }));
    assert!(outcome.is_err());
    buffer.clear();
    assert!(buffer.is_empty());
    for n in 0..per + 2 {
        buffer.push(n);
    }
    assert_eq!(contents(&buffer), (0..per + 2).collect::<Vec<_>>());
}
