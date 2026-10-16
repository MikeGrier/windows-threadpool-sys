// Copyright (c) 2026 Mike Grier

use std::num::NonZeroU32;
use std::ops::Range;

use super::{Plan, PlannedRead, Reads, plan};
use crate::read_sq::{ReadEntry, Shrink};
use crate::sq::{Attention, SqEntry, Terminal};

fn len(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).expect("a test buffer length is not zero")
}

fn attention(last_known: u64, current: u64) -> SqEntry<&'static str> {
    SqEntry::Attention(Attention {
        value: "file",
        last_known,
        current,
    })
}

fn reads(last_known: u64, current: u64, buffer_len: u32) -> Reads<&'static str> {
    match plan(attention(last_known, current), len(buffer_len)) {
        Plan::Reads(reads) => reads,
        other => panic!("growth from {last_known} to {current} planned {other:?}"),
    }
}

fn read(offset: u64, len: u32) -> PlannedRead {
    PlannedRead { offset, len }
}

/// Every read of the plan, checked to cover `last_known..current` in order with no gap or
/// overlap, and none longer than a buffer.
fn all_reads(last_known: u64, current: u64, buffer_len: u32) -> Vec<PlannedRead> {
    let planned: Vec<_> = reads(last_known, current, buffer_len).collect();
    let mut at = last_known;
    for read in &planned {
        assert_eq!(read.offset, at, "reads are contiguous and in order");
        assert!(read.len >= 1 && read.len <= buffer_len, "{read:?}");
        at = read.range().end;
    }
    assert_eq!(at, current, "the reads end where the growth does");
    planned
}

#[test]
fn growth_shorter_than_a_buffer_is_one_read_of_the_growth() {
    assert_eq!(all_reads(0, 100, 4096), [read(0, 100)]);
}

#[test]
fn growth_of_exactly_one_buffer_is_one_full_read() {
    assert_eq!(all_reads(0, 4096, 4096), [read(0, 4096)]);
}

#[test]
fn one_byte_past_a_buffer_is_a_second_read_of_one_byte() {
    assert_eq!(all_reads(0, 4097, 4096), [read(0, 4096), read(4096, 1)]);
}

#[test]
fn growth_of_whole_buffers_is_that_many_full_reads() {
    assert_eq!(
        all_reads(0, 3 * 4096, 4096),
        [read(0, 4096), read(4096, 4096), read(8192, 4096)]
    );
}

#[test]
fn reads_start_at_the_length_last_reported() {
    assert_eq!(
        all_reads(10_000, 10_300, 128),
        [read(10_000, 128), read(10_128, 128), read(10_256, 44)]
    );
}

#[test]
fn a_one_byte_buffer_reads_one_byte_at_a_time() {
    assert_eq!(all_reads(7, 10, 1), [read(7, 1), read(8, 1), read(9, 1)]);
}

#[test]
fn every_growth_and_buffer_length_is_covered_exactly() {
    for last_known in [0, 1, 4095, 4096, 1 << 40] {
        for growth in 1..=40 {
            for buffer_len in 1..=17 {
                let planned = all_reads(last_known, last_known + growth, buffer_len);
                let expected = growth.div_ceil(u64::from(buffer_len));
                assert_eq!(
                    planned.len() as u64,
                    expected,
                    "{last_known} {growth} {buffer_len}"
                );
            }
        }
    }
}

#[test]
fn growth_past_u32_is_split_at_the_largest_buffer() {
    let max = u32::MAX;
    let growth = u64::from(max) * 2 + 5;
    // Bounded, so a plan that splits wrongly fails here rather than walking billions of reads.
    let planned: Vec<_> = reads(0, growth, max).take(4).collect();
    assert_eq!(
        planned,
        [
            read(0, max),
            read(u64::from(max), max),
            read(u64::from(max) * 2, 5)
        ]
    );
}

#[test]
fn growth_ending_at_u64_max_ends_its_last_read_there() {
    let planned = all_reads(u64::MAX - 10, u64::MAX, 4);
    assert_eq!(planned.last(), Some(&read(u64::MAX - 2, 2)));
    assert_eq!(
        planned.last().map(|read| read.range()),
        Some(u64::MAX - 2..u64::MAX)
    );
}

#[test]
fn front_leaves_the_read_planned_and_next_takes_it() {
    let mut reads = reads(0, 10, 4);
    assert_eq!(reads.front(), Some(read(0, 4)));
    assert_eq!(
        reads.front(),
        Some(read(0, 4)),
        "a read not taken stays planned"
    );
    assert_eq!(reads.next(), Some(read(0, 4)));
    assert_eq!(reads.front(), Some(read(4, 4)));
    assert_eq!(reads.remaining(), 4..10);
}

#[test]
fn remaining_shrinks_by_each_read_taken() {
    let mut reads = reads(100, 110, 3);
    let mut seen: Vec<Range<u64>> = vec![reads.remaining()];
    while reads.next().is_some() {
        seen.push(reads.remaining());
    }
    assert_eq!(seen, [100..110, 103..110, 106..110, 109..110, 110..110]);
}

#[test]
fn a_finished_plan_stays_finished() {
    let mut reads = reads(0, 2, 2);
    assert_eq!(reads.next(), Some(read(0, 2)));
    for _ in 0..3 {
        assert_eq!(reads.front(), None);
        assert_eq!(reads.next(), None);
    }
    assert!(reads.remaining().is_empty());
}

#[test]
fn an_unchanged_length_plans_no_reads() {
    let mut reads = reads(50, 50, 4096);
    assert_eq!(reads.front(), None);
    assert_eq!(reads.next(), None);
}

#[test]
fn the_largest_range_is_planned_without_walking_it() {
    let mut reads = reads(0, u64::MAX, 1);
    assert_eq!(reads.next(), Some(read(0, 1)));
    assert_eq!(reads.front(), Some(read(1, 1)));
    assert_eq!(reads.remaining(), 1..u64::MAX);
}

#[test]
fn the_plan_keeps_the_registration_s_value() {
    assert_eq!(*reads(0, 1, 1).value(), "file");
}

#[test]
fn a_shrink_is_delivered_with_no_read() {
    match plan(attention(900, 300), len(4096)) {
        Plan::Deliver(ReadEntry::Shrink(shrink)) => assert_eq!(
            shrink,
            Shrink {
                value: "file",
                last_known: 900,
                current: 300,
            }
        ),
        other => panic!("a shrink planned {other:?}"),
    }
}

#[test]
fn a_shrink_by_one_byte_or_to_nothing_is_still_a_shrink() {
    for (last_known, current) in [(1, 0), (u64::MAX, 0), (u64::MAX, u64::MAX - 1), (5, 4)] {
        match plan(attention(last_known, current), len(1)) {
            Plan::Deliver(ReadEntry::Shrink(shrink)) => {
                assert_eq!((shrink.last_known, shrink.current), (last_known, current));
            }
            other => panic!("{last_known} to {current} planned {other:?}"),
        }
    }
}

#[test]
fn a_terminal_entry_passes_through_unchanged() {
    let terminal = Terminal {
        value: "file",
        win32_code: 6,
        last_known: 1234,
    };
    match plan(SqEntry::Terminal(terminal.clone()), len(4096)) {
        Plan::Deliver(ReadEntry::Terminal(passed)) => assert_eq!(passed, terminal),
        other => panic!("a terminal entry planned {other:?}"),
    }
}

#[test]
fn poll_done_is_planned_as_poll_done() {
    assert!(matches!(
        plan(SqEntry::<&str>::PollDone, len(4096)),
        Plan::PollDone
    ));
}
