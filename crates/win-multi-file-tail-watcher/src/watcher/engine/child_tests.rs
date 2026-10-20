// Copyright (c) 2026 Mike Grier
//! The engine over real files written by child processes through a handle their parent opened for
//! them, which is how a parent gives a child its stdout file. Here and not in `tests/` because the
//! engine is not public; the poll on the thread pool (TW-3.1) is what an outside test will drive.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::ChildStdin;
use std::time::{Duration, Instant};

use super::{Engine, RunOutcome};
use crate::test_support::{Children, TempDir, start_writer, wtf};
use crate::{Attention, Registration, SqEntry, Watcher};

// What each pass of a child's loop appends to its file.
const LINE: &str = "123456789ab\r\n";
const LINE_LEN: u64 = LINE.len() as u64;
const PASSES: u64 = 5;

struct Fixture {
    watcher: Watcher<u32>,
    registrations: Vec<Option<Registration<u32>>>,
    // Value (the roster index) and path of each file a child writes.
    writers: Vec<(u32, PathBuf)>,
    // Value, path and length of each file nobody writes, registered at that length.
    statics: Vec<(u32, PathBuf, u64)>,
    children: Children,
    stdins: Vec<ChildStdin>,
}

impl Fixture {
    // `total` registrations, every `every`th of them a file a child writes.
    fn new(
        dir: &TempDir,
        total: u32,
        every: u32,
        gated: bool,
        passes: u64,
        capacity: usize,
    ) -> Self {
        let watcher = Watcher::new(capacity).expect("watcher");
        let mut fixture = Self {
            watcher,
            registrations: Vec::new(),
            writers: Vec::new(),
            statics: Vec::new(),
            children: Children(Vec::new()),
            stdins: Vec::new(),
        };
        for value in 0..total {
            if value % every == every - 1 {
                let path = dir.0.join(format!("w{value}.log"));
                let mut child = start_writer(&path, LINE, passes, gated);
                if gated {
                    fixture.stdins.push(child.stdin.take().expect("stdin"));
                }
                fixture.children.0.push(child);
                let registration = fixture
                    .watcher
                    .register(&wtf(&path), 0, value)
                    .expect("register a writer");
                fixture.registrations.push(Some(registration));
                fixture.writers.push((value, path));
            } else {
                let len = 20 + u64::from(value % 30);
                let path = dir.file(format!("s{value}.log"), &vec![b'x'; len as usize]);
                let registration = fixture
                    .watcher
                    .register(&wtf(&path), len, value)
                    .expect("register a static file");
                fixture.registrations.push(Some(registration));
                fixture.statics.push((value, path, len));
            }
        }
        fixture
    }

    // Let every gated child append one line.
    fn release(&mut self) {
        for stdin in &mut self.stdins {
            stdin.write_all(b"go\r\n").expect("release one pass");
            stdin.flush().expect("flush");
        }
    }

    // Wait until every writer's file is exactly `expected` long, failing at once if one overshoots.
    fn wait_lengths(&self, expected: u64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let lens: Vec<u64> = self
                .writers
                .iter()
                .map(|(_, path)| std::fs::metadata(path).expect("metadata").len())
                .collect();
            assert!(
                lens.iter().all(|&len| len <= expected),
                "a file passed {expected}: {lens:?}"
            );
            if lens.iter().all(|&len| len == expected) {
                return;
            }
            assert!(Instant::now() < deadline, "lengths stayed at {lens:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    // Close the children's stdin and wait for each to finish.
    fn finish(&mut self) {
        self.stdins.clear();
        for child in &mut self.children.0 {
            assert!(child.wait().expect("wait").success());
        }
    }
}

fn drained(watcher: &Watcher<u32>) -> Vec<SqEntry<u32>> {
    watcher.drain().collect()
}

// Run the engine to the end of a poll, draining after each stop, and count the stops.
fn poll_to_end(engine: &mut Engine<u32>, watcher: &Watcher<u32>) -> (Vec<SqEntry<u32>>, usize) {
    let mut entries = Vec::new();
    let mut stops = 0;
    loop {
        let outcome = engine.run(watcher);
        entries.extend(drained(watcher));
        match outcome {
            RunOutcome::Stopped => stops += 1,
            RunOutcome::Complete => return (entries, stops),
        }
    }
}

fn att(value: u32, last_known: u64, current: u64) -> SqEntry<u32> {
    SqEntry::Attention(Attention {
        value,
        last_known,
        current,
    })
}

#[test]
fn children_writing_to_inherited_handles_are_reported_a_pass_at_a_time() {
    let dir = TempDir::new("child-steps");
    let mut fixture = Fixture::new(&dir, 300, 12, true, PASSES, 8);
    let mut engine = Engine::new(&fixture.watcher);
    let late_path = dir.file("late.log", &[b'y'; 40]);
    let mut live: Vec<u32> = fixture.writers.iter().map(|(value, _)| *value).collect();
    let dropped = live[20];
    let mut late = None;

    let (entries, _) = poll_to_end(&mut engine, &fixture.watcher);
    assert_eq!(entries, [SqEntry::PollDone], "nothing has been written yet");

    for pass in 1..=PASSES {
        fixture.release();
        fixture.wait_lengths(pass * LINE_LEN);
        let (before, after) = ((pass - 1) * LINE_LEN, pass * LINE_LEN);

        let (entries, stops) = if pass == 3 {
            // A registration dropped and another made between a stop and its continuation.
            assert_eq!(engine.run(&fixture.watcher), RunOutcome::Stopped);
            let mut entries = drained(&fixture.watcher);
            fixture.registrations[dropped as usize] = None;
            live.retain(|&value| value != dropped);
            late = Some(
                fixture
                    .watcher
                    .register(&wtf(&late_path), 0, 1000)
                    .expect("register the late file"),
            );
            let (rest, stops) = poll_to_end(&mut engine, &fixture.watcher);
            entries.extend(rest);
            (entries, stops + 1)
        } else {
            poll_to_end(&mut engine, &fixture.watcher)
        };

        let mut expected: Vec<_> = live
            .iter()
            .map(|&value| att(value, before, after))
            .collect();
        // Reported once, by the first poll after it was registered, and silent after.
        if pass == 4 {
            expected.push(att(1000, 0, 40));
        }
        expected.push(SqEntry::PollDone);
        assert_eq!(entries, expected, "pass {pass}");
        assert!(stops >= 2, "pass {pass} stopped {stops} times");
    }
    drop(late);

    fixture.finish();
    let (entries, _) = poll_to_end(&mut engine, &fixture.watcher);
    assert_eq!(entries, [SqEntry::PollDone], "the children are done");

    // One file shrunk by its parent: an edge, reported as the smaller length.
    let (value, path, len) = fixture.statics[7].clone();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open")
        .set_len(3)
        .expect("shrink");
    let (entries, _) = poll_to_end(&mut engine, &fixture.watcher);
    assert_eq!(entries, [att(value, len, 3), SqEntry::PollDone]);
}

#[test]
fn polling_while_children_write_reports_each_files_growth_as_one_unbroken_chain() {
    const WRITES: u64 = 400;
    let dir = TempDir::new("child-concurrent");
    let mut fixture = Fixture::new(&dir, 200, 10, false, WRITES, 4);
    let mut engine = Engine::new(&fixture.watcher);
    let deadline = Instant::now() + Duration::from_secs(60);

    let mut entries = Vec::new();
    let mut polls = 0;
    loop {
        // Asked before the poll: a poll that is silent after the last child is gone has seen it all.
        let running = fixture
            .children
            .0
            .iter_mut()
            .any(|child| child.try_wait().expect("try_wait").is_none());
        let (got, _) = poll_to_end(&mut engine, &fixture.watcher);
        polls += 1;
        let silent = got.len() == 1;
        entries.extend(got);
        if !running && silent {
            break;
        }
        assert!(Instant::now() < deadline, "the children never finished");
    }

    let polls_done = entries
        .iter()
        .filter(|entry| matches!(entry, SqEntry::PollDone))
        .count();
    assert_eq!(polls_done, polls);
    for (value, _) in &fixture.writers {
        let chain: Vec<(u64, u64)> = entries
            .iter()
            .filter_map(|entry| match entry {
                SqEntry::Attention(a) if a.value == *value => Some((a.last_known, a.current)),
                _ => None,
            })
            .collect();
        assert!(!chain.is_empty(), "writer {value} was never reported");
        assert_eq!(chain[0].0, 0, "writer {value} starts from its offset");
        for pair in chain.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "writer {value}: a gap or a repeat");
        }
        assert!(chain.iter().all(|&(last, current)| current > last));
        assert_eq!(
            chain.last().expect("not empty").1,
            WRITES * LINE_LEN,
            "writer {value} ends at its full length"
        );
    }
    assert!(
        entries.iter().all(|entry| match entry {
            SqEntry::Attention(a) => fixture.writers.iter().any(|(value, _)| *value == a.value),
            SqEntry::PollDone => true,
            _ => false,
        }),
        "only writers are reported, and nothing is terminal"
    );
    fixture.finish();
}
