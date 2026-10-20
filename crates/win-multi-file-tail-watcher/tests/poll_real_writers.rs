// Copyright (c) 2026 Mike Grier
//! Polls on the thread pool over real files that child processes write, through a queue of two
//! slots: the smallest the queue can be, so that any poll with more than one change parks and is
//! resumed many times, which is where a poll that loses a ring or a change would show.
#![cfg(windows)]

use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use win_multi_file_tail_watcher::{PollError, Registration, SqEntry, Watcher};
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use wtf_string::Wtf16String;

// What each pass of a child's loop appends to its file.
const LINE: &str = "123456789ab\r\n";
const LINE_LEN: u64 = LINE.len() as u64;
const SLOTS: usize = 2;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "win-multi-file-tail-watcher-poll-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).expect("create the test directory");
        Self(dir)
    }

    fn file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, content).expect("create the test file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wtf(path: &Path) -> Wtf16String {
    Wtf16String::from(path.as_os_str())
}

struct Children(Vec<Child>);

// Sets the flag when it drops, so a failing test stops the thread that watches it.
struct StopOnDrop<'a>(&'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// A `cmd.exe` whose stdout is the file at `path`, opened here and inherited. Gated, it appends one
// line for each line read on its stdin; ungated, it appends `passes` lines as fast as it can.
fn start(path: &Path, passes: u64, gated: bool) -> Child {
    let stdout = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .expect("open the child's stdout file");
    let echo = format!("echo {}", LINE.trim_end());
    let body = if gated {
        format!("(set /p x=)&({echo})")
    } else {
        echo
    };
    Command::new("cmd")
        .raw_arg(format!("/q /c \"for /l %i in (1,1,{passes}) do ({body})\""))
        .stdin(if gated { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::null())
        .spawn()
        .expect("start cmd.exe")
}

struct Fixture {
    watcher: Watcher<u32>,
    registrations: Vec<Option<Registration<u32>>>,
    // Value (the roster index) and path of each file a child writes.
    writers: Vec<(u32, PathBuf)>,
    children: Children,
    stdins: Vec<ChildStdin>,
}

impl Fixture {
    // `total` registrations, every `every`th a file a child writes, the rest files nobody writes,
    // registered at their length.
    fn new(dir: &TempDir, total: u32, every: u32, passes: u64, gated: bool) -> Self {
        let mut fixture = Self {
            watcher: Watcher::new(SLOTS).expect("watcher"),
            registrations: Vec::new(),
            writers: Vec::new(),
            children: Children(Vec::new()),
            stdins: Vec::new(),
        };
        for value in 0..total {
            let registration = if value % every == every - 1 {
                let path = dir.0.join(format!("w{value}.log"));
                let mut child = start(&path, passes, gated);
                if gated {
                    fixture.stdins.push(child.stdin.take().expect("stdin"));
                }
                fixture.children.0.push(child);
                let registration = fixture
                    .watcher
                    .register(&wtf(&path), 0, value)
                    .expect("register a writer");
                fixture.writers.push((value, path));
                registration
            } else {
                let len = 20 + u64::from(value % 30);
                let path = dir.file(&format!("s{value}.log"), &vec![b'x'; len as usize]);
                fixture
                    .watcher
                    .register(&wtf(&path), len, value)
                    .expect("register a static file")
            };
            fixture.registrations.push(Some(registration));
        }
        fixture
    }

    fn release(&mut self) {
        for stdin in &mut self.stdins {
            stdin.write_all(b"go\r\n").expect("release one pass");
            stdin.flush().expect("flush");
        }
    }

    // Until every writer's file is exactly `expected` long, failing at once on an overshoot.
    fn wait_lengths(&self, expected: u64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let lens: Vec<u64> = self
                .writers
                .iter()
                .map(|(_, path)| std::fs::metadata(path).expect("metadata").len())
                .collect();
            assert!(lens.iter().all(|&len| len <= expected), "{lens:?}");
            if lens.iter().all(|&len| len == expected) {
                return;
            }
            assert!(Instant::now() < deadline, "lengths stayed at {lens:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn finish(&mut self) {
        self.stdins.clear();
        for child in &mut self.children.0 {
            assert!(child.wait().expect("wait").success());
        }
    }
}

fn rung(watcher: &Watcher<u32>, millis: u32) -> bool {
    // SAFETY: the doorbell is a live event for as long as the watcher is, and waiting on it has no
    // other precondition.
    unsafe { WaitForSingleObject(watcher.doorbell().as_raw_handle(), millis) == WAIT_OBJECT_0 }
}

// One round: wait for the doorbell, reset it, and read what it announced.
fn round(watcher: &Watcher<u32>) -> Vec<SqEntry<u32>> {
    assert!(rung(watcher, 30_000), "the doorbell was not rung");
    watcher.reset_doorbell();
    watcher.drain().collect()
}

// Rounds, ringing the resume doorbell after each, until poll-done has been read. Returns the
// entries and how many rounds it took.
fn finish_poll(
    watcher: &Watcher<u32>,
    mut entries: Vec<SqEntry<u32>>,
) -> (Vec<SqEntry<u32>>, usize) {
    let mut rounds = 1;
    while !entries
        .iter()
        .any(|entry| matches!(entry, SqEntry::PollDone))
    {
        watcher.ring_resume();
        entries.extend(round(watcher));
        rounds += 1;
        assert!(rounds < 100_000, "the poll never finished");
    }
    (entries, rounds)
}

fn poll_to_end(watcher: &Watcher<u32>) -> (Vec<SqEntry<u32>>, usize) {
    watcher.poll().expect("poll");
    let first = round(watcher);
    finish_poll(watcher, first)
}

fn attentions(entries: &[SqEntry<u32>]) -> Vec<(u32, u64, u64)> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            SqEntry::Attention(a) => Some((a.value, a.last_known, a.current)),
            _ => None,
        })
        .collect()
}

#[test]
fn children_writing_to_inherited_handles_are_reported_across_many_parks_and_resumes() {
    const PASSES: u64 = 5;
    let dir = TempDir::new("steps");
    let mut fixture = Fixture::new(&dir, 300, 5, PASSES, true);
    let late_path = dir.file("late.log", &[b'y'; 40]);
    let mut live: Vec<u32> = fixture.writers.iter().map(|(value, _)| *value).collect();
    let dropped = live[40];
    let mut late = None;

    let (entries, _) = poll_to_end(&fixture.watcher);
    assert_eq!(entries, [SqEntry::PollDone], "nothing has been written yet");

    for pass in 1..=PASSES {
        fixture.release();
        fixture.wait_lengths(pass * LINE_LEN);
        let (before, after) = ((pass - 1) * LINE_LEN, pass * LINE_LEN);

        let (entries, rounds) = if pass == 3 {
            // Parked on a full queue: a second poll is refused, and the roster changes under it.
            fixture.watcher.poll().expect("poll");
            let first = round(&fixture.watcher);
            assert_eq!(first.len(), SLOTS, "the queue held two");
            assert!(matches!(fixture.watcher.poll(), Err(PollError::InFlight)));
            fixture.registrations[dropped as usize] = None;
            live.retain(|&value| value != dropped);
            late = Some(
                fixture
                    .watcher
                    .register(&wtf(&late_path), 0, 1000)
                    .expect("register the late file"),
            );
            finish_poll(&fixture.watcher, first)
        } else {
            poll_to_end(&fixture.watcher)
        };

        let mut expected: Vec<(u32, u64, u64)> =
            live.iter().map(|&value| (value, before, after)).collect();
        // Seen by the first poll after it was registered, and silent after.
        if pass == 4 {
            expected.push((1000, 0, 40));
        }
        assert_eq!(attentions(&entries), expected, "pass {pass}");
        assert_eq!(entries.last(), Some(&SqEntry::PollDone));
        assert!(rounds > 20, "pass {pass} took {rounds} rounds");
    }
    drop(late);
    fixture.finish();

    let (entries, _) = poll_to_end(&fixture.watcher);
    assert_eq!(entries, [SqEntry::PollDone], "the children are done");

    // Between polls nothing runs: a ring starts no poll and the doorbell stays quiet.
    fixture.watcher.reset_doorbell();
    fixture.watcher.ring_resume();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!rung(&fixture.watcher, 0), "something rang between polls");
    assert_eq!(fixture.watcher.drain().count(), 0);
    fixture
        .watcher
        .poll()
        .expect("and the next poll is accepted");
    round(&fixture.watcher);
}

#[test]
fn polling_through_two_slots_while_children_write_and_registrations_come_and_go() {
    const WRITES: u64 = 300;
    let dir = TempDir::new("concurrent");
    let mut fixture = Fixture::new(&dir, 150, 5, WRITES, false);
    let originals = fixture.watcher.registered();
    let churn_path = dir.file("churn.log", b"0123456789");
    let stop = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(120);

    let mut entries = Vec::new();
    let mut polls = 0usize;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::SeqCst) {
                let registration = fixture
                    .watcher
                    .register(&wtf(&churn_path), 0, 9_000)
                    .expect("register");
                std::thread::sleep(Duration::from_micros(200));
                drop(registration);
            }
        });
        let _stop = StopOnDrop(&stop);
        loop {
            let running = fixture
                .children
                .0
                .iter_mut()
                .any(|child| child.try_wait().expect("try_wait").is_none());
            let (got, _) = poll_to_end(&fixture.watcher);
            polls += 1;
            entries.extend(got);
            if !running {
                break;
            }
            assert!(Instant::now() < deadline, "the children never finished");
        }
    });
    // The children are gone and the churn has stopped: what is left is the last of their writes.
    loop {
        let (got, _) = poll_to_end(&fixture.watcher);
        polls += 1;
        let quiet = attentions(&got).is_empty();
        entries.extend(got);
        if quiet {
            break;
        }
        assert!(polls < 1_000, "the poll never went quiet");
    }

    let done = entries
        .iter()
        .filter(|entry| matches!(entry, SqEntry::PollDone))
        .count();
    assert_eq!(done, polls);
    for (value, _) in &fixture.writers {
        let chain: Vec<(u64, u64)> = attentions(&entries)
            .into_iter()
            .filter(|&(v, _, _)| v == *value)
            .map(|(_, last, current)| (last, current))
            .collect();
        assert!(!chain.is_empty(), "writer {value} was never reported");
        assert_eq!(chain[0].0, 0, "writer {value} starts from its offset");
        for pair in chain.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "writer {value}: a gap or a repeat");
        }
        assert_eq!(
            chain.last().expect("not empty").1,
            WRITES * LINE_LEN,
            "writer {value} ends at its full length"
        );
    }
    for (value, last, current) in attentions(&entries) {
        let writer = fixture.writers.iter().any(|(v, _)| *v == value);
        if value == 9_000 {
            assert_eq!((last, current), (0, 10), "a registration made mid-poll");
        } else {
            assert!(writer, "{value}: only writers and the churn are reported");
        }
    }
    assert!(
        entries
            .iter()
            .all(|entry| matches!(entry, SqEntry::Attention(_) | SqEntry::PollDone)),
        "nothing is terminal"
    );
    assert_eq!(fixture.watcher.registered(), originals);
    fixture.finish();
}
