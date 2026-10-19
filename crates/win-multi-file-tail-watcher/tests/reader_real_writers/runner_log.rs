// Copyright (c) 2026 Mike Grier
//! The worst-case load: output with the cadence and the line lengths of a CI runner's job log
//! (TW-9.5, TW-D-16).
//!
//! The shape is from the [capture](../../bench-captures/2026-10-16-runner-log-shape.txt): bursts of
//! many lines of mixed length, with silences between them, and a few lines far longer than a small
//! buffer. The lines here are of four lengths, and the longer ones cross the boundary between
//! buffers, which is what this checks: a line cut across chunks comes back whole once the chunks of
//! the file are put back by their ranges. They are not delivered in file order, and are not joined
//! as they arrive.

use std::time::Duration;

use win_multi_file_tail_watcher::reader::{TailReader, TailReaderOptions};

use crate::harness::{Client, TempDir, crlf, echo, for_lines, gate, script, spawn, wtf};

const FILE: u32 = 7;

/// The four lines written for `i`: about 18, 76, 136 and 206 bytes.
fn lines_for(i: u64) -> [String; 4] {
    [
        format!("##[group]step {i}"),
        format!("{i}: Run the build for the project in the working directory of this step"),
        format!(
            "{i}: {}",
            "the quick brown fox jumps over the lazy dog "
                .repeat(3)
                .trim_end()
        ),
        format!(
            "{i}: {}",
            "the quick brown fox jumps over the lazy dog "
                .repeat(4)
                .trim_end()
        ),
    ]
}

/// The same four lines as the loop of a script writes them, with `%i` for `i`.
fn loop_body() -> String {
    let [a, b, c, d] = lines_for(0);
    let with_i = |line: String| line.replacen('0', "%i", 1);
    // The first `0` of each line is the number: none of the lines has an earlier one.
    [a, b, c, d]
        .into_iter()
        .map(|l| echo(&with_i(l)))
        .collect::<Vec<_>>()
        .join("&")
}

fn run(options: &TailReaderOptions) {
    let dir = TempDir::new("runner");
    let path = dir.path("job.log");

    // Three bursts with a silence before each: 120, 40 and 200 steps.
    let bursts = [(1, 120), (121, 160), (161, 360)];
    let mut parts = Vec::new();
    for (from, to) in bursts {
        parts.push(gate());
        parts.push(for_lines(from, to, &loop_body()));
    }
    let mut command = spawn(&script(&parts), &path, None, true);

    let reader = TailReader::<u32>::new(options, 64).expect("a reader");
    let mut client = Client::new(&reader, options.buffer_len);
    let _registration = reader.register(&wtf(&path), 0, FILE).expect("register");

    let mut expected = Vec::new();
    for (from, to) in bursts {
        // The silence before the burst: nothing is delivered and nothing rings.
        let polled = client.poll_once();
        assert_eq!(polled.data, 0);
        client.assert_quiet(200);

        command.release();
        let written: Vec<String> = (from..=to).flat_map(lines_for).collect();
        expected.extend(crlf(&written));

        // Poll while the burst is written, until every byte of it has arrived.
        let want = u64::try_from(expected.len()).expect("fits");
        let started = std::time::Instant::now();
        while client.delivered(FILE) < want {
            assert!(
                started.elapsed() < crate::harness::BOUND,
                "the burst did not arrive: {} of {want} bytes delivered, the file holds {:?}, and the \
                 child has {}exited",
                client.delivered(FILE),
                std::fs::read(&path).map(|bytes| bytes.len()),
                if command.exited() { "" } else { "not " }
            );
            let polled = client.poll_once();
            assert!(polled.other.is_empty(), "{:?}", polled.other);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(client.delivered(FILE), want, "no byte delivered twice");

        // And then it is idle again.
        let idle = client.poll_once();
        assert_eq!(idle.data, 0);
        client.assert_quiet(200);
    }
    assert!(command.wait().success());

    let (bytes, ranges) = client.assemble(FILE);
    assert_eq!(
        bytes, expected,
        "the log, every byte once, put back by range"
    );
    assert!(reader.take_failure().is_none());

    // Lines longer than a buffer were cut across chunks, and came back whole all the same.
    let mut crossed = 0;
    let mut at = 0u64;
    for line in expected.split_inclusive(|b| *b == b'\n') {
        let end = at + u64::try_from(line.len()).expect("fits");
        if line.len() > options.buffer_len {
            let cuts = ranges
                .iter()
                .filter(|r| r.start > at && r.start < end)
                .count();
            assert!(
                cuts >= 1,
                "a line longer than a buffer is cut across chunks"
            );
            crossed += 1;
        }
        at = end;
    }
    assert!(crossed > 0, "the log has lines longer than the buffer");
}

#[test]
fn a_job_log_is_delivered_whole_under_the_default_options() {
    run(&TailReaderOptions::default());
}

#[test]
fn a_job_log_is_delivered_whole_under_the_tiny_options() {
    // Eight-byte buffers: every line is cut across chunks, the pool and the client queue are
    // exhausted by each burst, and the log is still whole.
    let mut options = TailReaderOptions::default();
    options.buffer_len = 8;
    options.buffer_count = 4;
    options.client_sq_capacity = 2;
    options.ring_sq_size = 8;
    options.ring_cq_size = 16;
    run(&options);
}
