// Copyright (c) 2026 Mike Grier
//! What every test of this target shares: a temporary directory, the child processes that write,
//! the scripts they run, and the client's side of a poll, which uses public items only.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::ops::Range;
use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use win_multi_file_tail_watcher::reader::{Data, ReadEntry, TailReader};
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::WaitForSingleObject;
use wtf_string::Wtf16String;

/// Far longer than anything here takes, so a reader that stops delivering fails by assertion.
pub const BOUND: Duration = Duration::from_secs(60);

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        // A tag is not unique: two tests of one target may share it, and run at once.
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "win-multi-file-tail-watcher-reader-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).expect("create the test directory");
        Self(dir)
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn wtf(path: &Path) -> Wtf16String {
    Wtf16String::from(path.as_os_str())
}

/// A `cmd.exe` writing to files the parent opened and handed over as its stdout and stderr, which
/// is how a parent gives a child its output. Killed and reaped when it drops.
pub struct Writer {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Writer {
    /// Let a gated writer go on to its next phase.
    pub fn release(&mut self) {
        let stdin = self.stdin.as_mut().expect("a gated writer has a stdin");
        stdin.write_all(b"go\r\n").expect("release the writer");
        stdin.flush().expect("flush the release");
    }

    pub fn exited(&mut self) -> bool {
        self.child.try_wait().expect("poll the child").is_some()
    }

    /// Wait for the child to end, which is when everything it will write has been written.
    pub fn wait(&mut self) -> ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("poll the child") {
                return status;
            }
            assert!(started.elapsed() < BOUND, "the child did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start `script` under `cmd.exe`. Gated, it waits for [`Writer::release`] at each `gate()` in it.
pub fn spawn(script: &str, stdout: &Path, stderr: Option<&Path>, gated: bool) -> Writer {
    let open = |path: &Path| {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .expect("open the child's output file")
    };
    let mut child = Command::new("cmd")
        .raw_arg(format!("/q /s /c \"{script}\""))
        .stdin(if gated { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::from(open(stdout)))
        .stderr(match stderr {
            Some(path) => Stdio::from(open(path)),
            None => Stdio::null(),
        })
        .spawn()
        .expect("start cmd.exe");
    let stdin = child.stdin.take();
    Writer { child, stdin }
}

// The pieces of a script. Each is a command group, joined by `&`. The text of a line may not
// contain `& | < > ^ ( ) %` or an odd number of quotes, which `cmd.exe` would read as syntax.

/// Wait for a line on stdin.
pub fn gate() -> String {
    "(set /p x=)".to_owned()
}

/// Write `line` to stdout. The one space after `echo` is its delimiter, so a line that starts with
/// spaces keeps all of them.
pub fn echo(line: &str) -> String {
    format!("(echo {line})")
}

/// Write `line` to stderr.
pub fn echo_err(line: &str) -> String {
    format!("(echo {line})1>&2")
}

/// Run `body` for `%i` from `from` to `to`. `%i` may be used in the text of `body`.
pub fn for_lines(from: u64, to: u64, body: &str) -> String {
    format!("(for /l %i in ({from},1,{to}) do ({body}))")
}

pub fn script(parts: &[String]) -> String {
    parts.join("&")
}

/// The bytes a script's lines make: each ends in the CR and LF `echo` writes.
pub fn crlf(lines: &[String]) -> Vec<u8> {
    lines
        .iter()
        .flat_map(|l| format!("{l}\r\n").into_bytes())
        .collect()
}

fn signalled_within(handle: BorrowedHandle<'_>, millis: u32) -> bool {
    // SAFETY: a live event the reader owns and this borrows; waiting has no other precondition.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), millis) == WAIT_OBJECT_0 }
}

/// What one poll delivered, as the client counted it.
#[derive(Default, Debug)]
pub struct Polled {
    /// Data entries, and how many for each file.
    pub data: usize,
    pub by_value: BTreeMap<u32, usize>,
    /// Anything that was not data or the poll-done, which these tests do not expect.
    pub other: Vec<String>,
}

/// The file range a chunk came from, and its bytes.
type Piece = (Range<u64>, Vec<u8>);

/// The client of a [`TailReader`]: it takes what a poll delivers and keeps each file's pieces.
pub struct Client<'a> {
    reader: &'a TailReader<u32>,
    max_chunk: usize,
    pieces: BTreeMap<u32, Vec<Piece>>,
}

impl<'a> Client<'a> {
    /// `max_chunk` is the buffer size the reader was made with: no chunk may be longer.
    pub fn new(reader: &'a TailReader<u32>, max_chunk: usize) -> Self {
        Self {
            reader,
            max_chunk,
            pieces: BTreeMap::new(),
        }
    }

    /// Start a poll and take everything up to its poll-done.
    pub fn poll_once(&mut self) -> Polled {
        self.reader.poll().expect("start a poll");
        self.finish_poll()
    }

    /// Take entries, waiting on the doorbell as a client does, until a poll-done arrives.
    pub fn finish_poll(&mut self) -> Polled {
        let sq = self.reader.read_sq();
        let mut polled = Polled::default();
        let started = Instant::now();
        loop {
            assert!(started.elapsed() < BOUND, "no poll-done arrived");
            if !signalled_within(sq.doorbell(), 500) {
                continue;
            }
            // Reset first and then drain, so an entry written after the reset rings it again.
            sq.reset_doorbell();
            let mut done = false;
            for entry in sq.drain() {
                match entry {
                    ReadEntry::Data(data) => {
                        let value = *data.value();
                        self.add_piece(value, data.range(), data.chunk().to_vec());
                        *polled.by_value.entry(value).or_default() += 1;
                        polled.data += 1;
                    }
                    ReadEntry::PollDone => {
                        done = true;
                        break;
                    }
                    other => polled.other.push(format!("{other:?}")),
                }
            }
            // Room has been made, which a reader stopped on a full queue waits to be told.
            sq.ring_resume();
            if done {
                assert!(
                    sq.drain().next().is_none(),
                    "nothing follows the poll-done of a poll"
                );
                return polled;
            }
        }
    }

    /// Keep a piece of `value`'s file, checking it against the size the reader was made with.
    pub fn add_piece(&mut self, value: u32, range: Range<u64>, bytes: Vec<u8>) {
        assert_eq!(
            u64::try_from(bytes.len()).expect("fits"),
            range.end - range.start,
            "the range of a chunk is its length"
        );
        assert!(
            !bytes.is_empty() && bytes.len() <= self.max_chunk,
            "a chunk of {} bytes from buffers of {}",
            bytes.len(),
            self.max_chunk
        );
        self.pieces.entry(value).or_default().push((range, bytes));
    }

    /// How many bytes of `value`'s file have been delivered.
    pub fn delivered(&self, value: u32) -> u64 {
        self.pieces
            .get(&value)
            .map_or(0, |p| p.iter().map(|(r, _)| r.end - r.start).sum())
    }

    /// The bytes of `value`'s file put back in file order, and the range of each piece. The pieces
    /// must start at the beginning and meet, with no gap and no repeat: every byte delivered once.
    pub fn assemble(&self, value: u32) -> (Vec<u8>, Vec<Range<u64>>) {
        let mut pieces: Vec<_> = self.pieces.get(&value).into_iter().flatten().collect();
        pieces.sort_by_key(|(range, _)| range.start);
        let mut at = 0;
        let mut bytes = Vec::new();
        let mut ranges = Vec::new();
        for (range, piece) in pieces {
            assert_eq!(
                range.start, at,
                "file {value}: a gap or a repeat where {at} was expected"
            );
            bytes.extend_from_slice(piece);
            ranges.push(range.clone());
            at = range.end;
        }
        (bytes, ranges)
    }

    /// After the poll just finished, nothing rings the doorbell for `millis`: the reader does
    /// nothing between polls. A ring that lands just after the poll-done was drained, because it
    /// was made at the end of the run that wrote it, is let go by first.
    pub fn assert_quiet(&self, millis: u32) {
        let sq = self.reader.read_sq();
        std::thread::sleep(Duration::from_millis(100));
        sq.reset_doorbell();
        assert!(
            sq.drain().next().is_none(),
            "an entry with no poll to bring it"
        );
        assert!(
            !signalled_within(sq.doorbell(), millis),
            "the reader rang its doorbell with no poll running"
        );
    }
}

/// Take the entries a poll started elsewhere has written until `count` data entries are held,
/// without dropping any of them, and return them. For a test that keeps chunks.
pub fn hold_data(reader: &TailReader<u32>, count: usize) -> Vec<Data<u32>> {
    let sq = reader.read_sq();
    let mut held = Vec::new();
    let started = Instant::now();
    while held.len() < count {
        assert!(
            started.elapsed() < BOUND,
            "{count} data entries did not arrive"
        );
        if !signalled_within(sq.doorbell(), 500) {
            continue;
        }
        sq.reset_doorbell();
        for entry in sq.drain() {
            match entry {
                ReadEntry::Data(data) => held.push(data),
                other => panic!("a data entry was expected, not {other:?}"),
            }
        }
        sq.ring_resume();
    }
    held
}
