// Copyright (c) 2026 Mike Grier
//! The pairing, over the dump of a real trace and over dumps made for each way a pairing breaks.
//!
//! `trace.csv` is `tracerpt`'s CSV dump of a trace of `emit_every_event`, recorded elevated on 2026-10-21 with the manifest
//! installed (DE-D-6): one inconsistency and four pairs, whose ends follow their begins by at least 20, 30, 40 and 50 ms, the
//! two flushes ending on another thread than they began on.

use std::time::Duration;

use super::{Broken, NO_ACTIVITY, Pair, pair, read, values};

const TRACE: &str = include_str!("trace.csv");

/// The dump's header, as `tracerpt` writes it, padding and all.
const HEADER: &str = "  Event Name,       Type,     Event ID,    Version,    Channel,      Level,     Opcode,       Task,            Keyword,        PID,        TID,     Processor Number,  Instance ID,   Parent Instance ID,                              Activity ID,                      Related Activity ID,           Clock-Time, Kernel(ms),   User(ms), User Data";

const A: &str = "{0795ed5d-9da0-0002-189f-908e285ddd01}";
const B: &str = "{0795ed5d-9da0-0002-199f-908e285ddd01}";

/// A row of the provider: a lock wait's begin (13) or end (14) unless `id` says otherwise.
fn row(id: u16, opcode: u8, task: u16, activity: &str, clock: u64, data: &str) -> String {
    format!(
        "durable-ioring-events, Start, {id}, 0, 0, 4, {opcode}, {task}, 0x1, 0x10, 0x20, 2, , , {activity}, , {clock}, 0, 0, {data}"
    )
}

fn dump(rows: &[String]) -> String {
    let mut text = String::from(HEADER);
    for row in rows {
        text.push('\n');
        text.push_str(row);
    }
    text
}

fn paired(rows: &[String]) -> (Vec<Pair>, Vec<Broken>) {
    pair(&read(&dump(rows)).expect("read"))
}

#[test]
fn the_traces_four_pairs_are_paired_and_nothing_breaks() {
    let (pairs, broken) = pair(&read(TRACE).expect("read"));
    assert_eq!(broken, []);
    assert_eq!(pairs.len(), 4, "{pairs:?}");
}

#[test]
fn each_pair_of_the_trace_took_at_least_the_wait_the_example_made() {
    let (pairs, _) = pair(&read(TRACE).expect("read"));
    // By task and pair id: the lock wait (3), the file and domain flushes (2), and the seal (1).
    let waits = [(3, "1", 20), (2, "3", 30), (2, "4", 40), (1, "2", 50)];
    for (task, id, ms) in waits {
        let found = pairs
            .iter()
            .find(|p| p.task == task && p.pair == id)
            .unwrap_or_else(|| panic!("task {task} pair {id} in {pairs:?}"));
        assert!(found.took >= Duration::from_millis(ms), "{found}");
    }
}

#[test]
fn the_traces_durations_are_the_gaps_between_the_clock_times() {
    let (pairs, _) = pair(&read(TRACE).expect("read"));
    let lock = pairs.iter().find(|p| p.task == 3).expect("the lock wait");
    // 134371033678956115 - 134371033678751420 ticks of 100 ns.
    assert_eq!(lock.took, Duration::from_nanos(204_695 * 100));
}

#[test]
fn the_traces_flushes_end_on_another_thread_in_the_activity_they_began_in() {
    let (pairs, _) = pair(&read(TRACE).expect("read"));
    let flushes: Vec<_> = pairs.iter().filter(|p| p.task == 2).collect();
    assert_eq!(flushes.len(), 2);
    for flush in flushes {
        assert_ne!(flush.began_on, flush.ended_on, "{flush}");
    }
}

#[test]
fn the_traces_pairs_are_four_activities() {
    let (pairs, _) = pair(&read(TRACE).expect("read"));
    let mut activities: Vec<_> = pairs.iter().map(|p| p.activity.clone()).collect();
    activities.sort();
    activities.dedup();
    assert_eq!(activities.len(), 4);
    assert!(!activities.contains(&NO_ACTIVITY.to_owned()));
}

#[test]
fn the_trace_is_read_as_the_providers_nine_events_and_no_others() {
    let rows = read(TRACE).expect("read");
    assert_eq!(rows.len(), 9);
    assert_eq!(rows[0].id, 1, "the inconsistency first");
    assert_eq!(rows[0].data, ["7", "a write completed twice", "lineage 3"]);
}

#[test]
fn a_pair_is_displayed_with_its_task_and_its_duration_in_milliseconds() {
    let (pairs, _) = pair(&read(TRACE).expect("read"));
    let lock = pairs.iter().find(|p| p.task == 3).expect("the lock wait");
    assert_eq!(
        lock.to_string(),
        format!(
            "LockWait pair 1 in activity {A}: 20.470 ms, begun on thread 0x00008DE4 and ended on 0x00008DE4"
        )
    );
}

#[test]
fn a_start_and_its_stop_are_one_pair_whatever_comes_between() {
    let (pairs, broken) = paired(&[
        row(13, 1, 3, A, 100, "4, \"engine\", 1"),
        row(13, 1, 3, B, 150, "4, \"engine\", 2"),
        row(14, 2, 3, A, 400, "4, \"engine\", 1"),
        row(14, 2, 3, B, 450, "4, \"engine\", 2"),
    ]);
    assert_eq!(broken, []);
    assert_eq!(
        pairs.iter().map(|p| p.took).collect::<Vec<_>>(),
        [Duration::from_nanos(30_000); 2]
    );
}

#[test]
fn a_stop_with_no_start_breaks_the_pairing() {
    let (pairs, broken) = paired(&[row(14, 2, 3, A, 400, "4, \"engine\", 1")]);
    assert_eq!(pairs, []);
    assert_eq!(
        broken,
        [Broken::StopWithoutStart {
            activity: A.to_owned()
        }]
    );
}

#[test]
fn a_start_never_stopped_is_left_open() {
    let (_, broken) = paired(&[row(13, 1, 3, A, 100, "4, \"engine\", 1")]);
    assert_eq!(
        broken,
        [Broken::LeftOpen {
            activity: A.to_owned()
        }]
    );
}

#[test]
fn an_activity_started_twice_breaks_the_pairing() {
    let (_, broken) = paired(&[
        row(13, 1, 3, A, 100, "4, \"engine\", 1"),
        row(13, 1, 3, A, 200, "4, \"engine\", 1"),
        row(14, 2, 3, A, 400, "4, \"engine\", 1"),
    ]);
    assert_eq!(
        broken,
        [Broken::StartedTwice {
            activity: A.to_owned()
        }]
    );
}

#[test]
fn a_stop_of_another_task_breaks_the_pairing() {
    let (_, broken) = paired(&[
        row(13, 1, 3, A, 100, "4, \"engine\", 1"),
        row(8, 2, 1, A, 400, "2, 1, \"9\", 1, \"durable\""),
    ]);
    assert_eq!(
        broken,
        [Broken::TaskDiffers {
            activity: A.to_owned(),
            began: 3,
            ended: 1
        }]
    );
}

#[test]
fn a_stop_naming_another_pair_breaks_the_pairing() {
    let (_, broken) = paired(&[
        row(13, 1, 3, A, 100, "4, \"engine\", 1"),
        row(14, 2, 3, A, 400, "4, \"engine\", 9"),
    ]);
    assert_eq!(
        broken,
        [Broken::PairDiffers {
            activity: A.to_owned(),
            began: Some("1".into()),
            ended: Some("9".into())
        }]
    );
}

#[test]
fn a_stop_before_its_start_breaks_the_pairing() {
    let (_, broken) = paired(&[
        row(13, 1, 3, A, 400, "4, \"engine\", 1"),
        row(14, 2, 3, A, 100, "4, \"engine\", 1"),
    ]);
    assert_eq!(
        broken,
        [Broken::EndedBeforeBegun {
            activity: A.to_owned()
        }]
    );
}

#[test]
fn a_pairs_event_in_no_activity_breaks_the_pairing() {
    let (_, broken) = paired(&[row(13, 1, 3, NO_ACTIVITY, 100, "4, \"engine\", 1")]);
    assert_eq!(broken, [Broken::NoActivity { id: 13 }]);
}

#[test]
fn an_event_that_is_not_a_pair_is_not_paired() {
    let (pairs, broken) = paired(&[row(1, 0, 0, NO_ACTIVITY, 100, "7, \"what\", \"detail\"")]);
    assert_eq!((pairs, broken), (vec![], vec![]));
}

#[test]
fn another_providers_rows_are_skipped() {
    let other =
        row(13, 1, 3, A, 100, "4, \"engine\", 1").replace("durable-ioring-events", "EventTrace");
    assert_eq!(read(&dump(&[other])).expect("read"), []);
}

#[test]
fn a_dump_without_the_columns_is_refused() {
    assert!(read("Event Name, Opcode").is_err());
    assert!(read("").is_err());
}

#[test]
fn a_dump_with_crlf_lines_reads_as_one_with_lf() {
    let rows = [
        row(13, 1, 3, A, 100, "4, \"engine\", 1"),
        row(14, 2, 3, A, 400, "4, \"engine\", 1"),
    ];
    let lf = dump(&rows);
    assert_eq!(
        read(&lf.replace('\n', "\r\n")).expect("read"),
        read(&lf).expect("read")
    );
}

#[test]
fn user_data_splits_at_commas_outside_strings() {
    assert_eq!(values("4, \"a, b\", 1").expect("read"), ["4", "a, b", "1"]);
}

#[test]
fn user_data_reads_a_doubled_quote_as_one() {
    assert_eq!(values("\"say \"\"hi\"\"\"").expect("read"), ["say \"hi\""]);
}

#[test]
fn user_data_keeps_a_binary_field_as_written() {
    assert_eq!(
        values("5, 0x7373642D34, 4").expect("read"),
        ["5", "0x7373642D34", "4"]
    );
}

#[test]
fn user_data_that_is_empty_is_no_values() {
    assert_eq!(values("").expect("read"), Vec::<String>::new());
}

#[test]
fn an_unterminated_string_is_refused() {
    assert!(values("\"open").is_err());
}
