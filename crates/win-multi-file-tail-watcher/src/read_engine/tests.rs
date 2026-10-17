// Copyright (c) 2026 Mike Grier
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_INVALID, ERROR_HANDLE_EOF, ERROR_INVALID_HANDLE,
};

use super::{
    Advance, Current, EngineError, Groups, Pending, ReadEngine, Stop, Submitted, Tracked, split,
    win32_code,
};
use crate::pool::{BufferPool, PoolBuffer};
use crate::read_plan::{Plan, plan};
use crate::read_sq::ReadEntry;
use crate::sq::{Attention, SqEntry, Terminal};
use crate::test_support::{RecordingSink, TempDir, is_signalled, wtf};
use crate::{CreateError, Diagnostic, DiagnosticKind, HeldFile, Watcher};

/// What the client sees of an entry, with the chunk given back to the pool as it is read.
#[derive(Debug, PartialEq, Eq)]
enum Got {
    Data {
        value: u32,
        range: Range<u64>,
        bytes: Vec<u8>,
    },
    Shrink {
        value: u32,
        last_known: u64,
        current: u64,
    },
    Terminal {
        value: u32,
        win32_code: u32,
        last_known: u64,
    },
    PollDone,
}

fn got(entry: ReadEntry<u32>) -> Got {
    match entry {
        ReadEntry::Data(data) => {
            let (value, range, chunk) = data.into_parts();
            Got::Data {
                value,
                range,
                bytes: chunk.to_vec(),
            }
        }
        ReadEntry::Shrink(shrink) => Got::Shrink {
            value: shrink.value,
            last_known: shrink.last_known,
            current: shrink.current,
        },
        ReadEntry::Terminal(terminal) => Got::Terminal {
            value: terminal.value,
            win32_code: terminal.win32_code,
            last_known: terminal.last_known,
        },
        ReadEntry::PollDone => Got::PollDone,
    }
}

fn engine_with(
    buffer_len: usize,
    buffers: usize,
    client_capacity: usize,
    ring_sq: u32,
    ring_cq: u32,
    sink: Option<Arc<RecordingSink>>,
) -> ReadEngine<u32> {
    let watcher = match sink {
        Some(sink) => Watcher::with_diagnostics(1024, sink),
        None => Watcher::new(1024),
    }
    .expect("a watcher");
    let pool = BufferPool::new(buffer_len, buffers).expect("a pool");
    match ReadEngine::new(watcher, pool, client_capacity, ring_sq, ring_cq) {
        Ok(engine) => engine,
        Err(error) => panic!("no engine: {error}"),
    }
}

fn engine(buffer_len: usize, buffers: usize) -> ReadEngine<u32> {
    engine_with(buffer_len, buffers, 256, 64, 128, None)
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|n| u8::try_from(n % 251).expect("below 251"))
        .collect()
}

fn append(path: &PathBuf, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .expect("append");
}

fn drain_got(engine: &ReadEngine<u32>) -> Vec<Got> {
    engine.read_sq().drain().map(got).collect()
}

/// Run the engine, taking what it delivers, until it has nothing left to do and nothing in flight.
fn settle(engine: &mut ReadEngine<u32>) -> Vec<Got> {
    let mut out = Vec::new();
    for _ in 0..100_000 {
        let stop = engine.run().expect("run");
        out.extend(drain_got(engine));
        match stop {
            Stop::Idle if engine.in_flight() == 0 => return out,
            Stop::ClientFull => {}
            _ => engine.wait_for_completion(5_000).expect("wait"),
        }
    }
    panic!("the engine never settled");
}

/// The bytes delivered for `value`, put back in file order, and where they start. Checks as it goes
/// that the pieces meet with no gap or overlap, and that each is labelled with exactly its bytes.
fn reassembled(got: &[Got], value: u32) -> (u64, Vec<u8>) {
    let mut pieces: Vec<_> = got
        .iter()
        .filter_map(|entry| match entry {
            Got::Data {
                value: v,
                range,
                bytes,
            } if *v == value => Some((range.clone(), bytes)),
            _ => None,
        })
        .collect();
    pieces.sort_by_key(|(range, _)| range.start);
    let start = pieces.first().map_or(0, |(range, _)| range.start);
    let mut at = start;
    let mut out = Vec::new();
    for (range, bytes) in pieces {
        assert_eq!(range.start, at, "the pieces meet with no gap or overlap");
        assert_eq!(
            u64::try_from(bytes.len()).expect("small"),
            range.end - range.start,
            "a piece is labelled with exactly its bytes"
        );
        out.extend_from_slice(bytes);
        at = range.end;
    }
    (start, out)
}

fn poll_dones(got: &[Got]) -> Vec<usize> {
    got.iter()
        .enumerate()
        .filter_map(|(at, entry)| (*entry == Got::PollDone).then_some(at))
        .collect()
}

fn push(engine: &ReadEngine<u32>, entry: SqEntry<Tracked<u32>>) {
    engine
        .watcher()
        .sq_writer()
        .push(entry)
        .expect("the watcher's SQ has room");
}

fn announce(engine: &ReadEngine<u32>, tracked: &Tracked<u32>, last_known: u64, current: u64) {
    push(
        engine,
        SqEntry::Attention(Attention {
            value: tracked.clone(),
            last_known,
            current,
        }),
    );
}

/// A buffer lent for a read of `len` bytes at `offset`, and what the ring would hold for it, as if
/// the read had been submitted.
fn lent(
    engine: &mut ReadEngine<u32>,
    tracked: &Tracked<u32>,
    offset: u64,
    len: u32,
) -> (PoolBuffer, Pending<u32>) {
    let group = engine.groups.open();
    engine.groups.started(group);
    let mut buffer = engine.pool().take().expect("a buffer");
    buffer.limit_to(usize::try_from(len).expect("small"));
    let pending = Pending {
        registration: tracked.registration,
        value: tracked.value,
        offset,
        len,
        group,
        _keepalive: tracked.keepalive.clone(),
    };
    (buffer, pending)
}

#[test]
fn a_growth_within_one_buffer_is_one_data_entry_then_poll_done() {
    let dir = TempDir::new("re-one");
    let mut engine = engine(64, 4);
    let content = pattern(40);
    let path = dir.file("a.log", &content);
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    assert_eq!(
        settle(&mut engine),
        [
            Got::Data {
                value: 7,
                range: 0..40,
                bytes: content
            },
            Got::PollDone
        ]
    );
}

#[test]
fn a_growth_over_several_buffers_arrives_in_pieces_that_reassemble() {
    let dir = TempDir::new("re-many");
    let mut engine = engine(16, 8);
    let content = pattern(200);
    let path = dir.file("a.log", &content);
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    let got = settle(&mut engine);
    assert_eq!(reassembled(&got, 7), (0, content));
    let pieces = got.len() - 1;
    assert_eq!(pieces, 13, "200 bytes in buffers of 16 is thirteen reads");
    assert_eq!(got.last(), Some(&Got::PollDone));
    assert_eq!(poll_dones(&got).len(), 1);
    for entry in &got[..pieces] {
        let Got::Data { bytes, .. } = entry else {
            panic!("only data before poll-done, found {entry:?}");
        };
        assert!(bytes.len() <= 16);
    }
}

#[test]
fn reading_begins_at_the_starting_offset() {
    let dir = TempDir::new("re-offset");
    let mut engine = engine(16, 4);
    let content = pattern(50);
    let path = dir.file("a.log", &content);
    let _registration = engine.register(&wtf(&path), 10, 7).expect("register");
    engine.watcher().poll_here();
    let got = settle(&mut engine);
    assert_eq!(reassembled(&got, 7), (10, content[10..].to_vec()));
}

#[test]
fn several_files_are_each_read_in_full_and_labelled_with_their_values() {
    let dir = TempDir::new("re-files");
    let mut engine = engine(16, 6);
    let mut expected = Vec::new();
    let mut registrations = Vec::new();
    for n in 0..5_u32 {
        let content = pattern(5 + 37 * usize::try_from(n).expect("small"));
        let path = dir.file(format!("f{n}.log"), &content);
        registrations.push(engine.register(&wtf(&path), 0, n).expect("register"));
        expected.push(content);
    }
    engine.watcher().poll_here();
    let got = settle(&mut engine);
    for (n, content) in (0..5_u32).zip(expected) {
        assert_eq!(reassembled(&got, n), (0, content), "file {n}");
    }
    assert_eq!(poll_dones(&got), [got.len() - 1]);
}

#[test]
fn a_poll_with_no_changes_delivers_only_poll_done() {
    let dir = TempDir::new("re-none");
    let mut engine = engine(16, 4);
    let path = dir.file("a.log", b"abcdef");
    let _registration = engine.register(&wtf(&path), 6, 7).expect("register");
    engine.watcher().poll_here();
    assert_eq!(settle(&mut engine), [Got::PollDone]);
}

#[test]
fn a_second_poll_reads_only_what_the_file_gained() {
    let dir = TempDir::new("re-second");
    let mut engine = engine(16, 4);
    let first = pattern(40);
    let path = dir.file("a.log", &first);
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    let got = settle(&mut engine);
    assert_eq!(reassembled(&got, 7), (0, first.clone()));
    let more = vec![b'z'; 25];
    append(&path, &more);
    engine.watcher().poll_here();
    let got = settle(&mut engine);
    assert_eq!(reassembled(&got, 7), (40, more));
    assert_eq!(poll_dones(&got), [got.len() - 1]);
}

#[test]
fn poll_done_follows_every_read_of_its_poll_though_two_polls_were_queued_together() {
    let dir = TempDir::new("re-order");
    let mut engine = engine(16, 8);
    let path = dir.file("a.log", &pattern(100));
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    append(&path, &pattern(50));
    engine.watcher().poll_here();
    let got = settle(&mut engine);
    let dones = poll_dones(&got);
    assert_eq!(dones.len(), 2);
    for (at, entry) in got.iter().enumerate() {
        if let Got::Data { range, .. } = entry {
            let poll_done = if range.end <= 100 { dones[0] } else { dones[1] };
            assert!(at < poll_done, "{range:?} came after its poll-done");
        }
    }
    assert_eq!(reassembled(&got, 7).1.len(), 150);
}

#[test]
fn a_shrink_is_delivered_with_no_read() {
    let dir = TempDir::new("re-shrink");
    let mut engine = engine(16, 4);
    let path = dir.file("a.log", &pattern(100));
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    settle(&mut engine);
    OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_len(30))
        .expect("truncate");
    engine.watcher().poll_here();
    assert_eq!(
        settle(&mut engine),
        [
            Got::Shrink {
                value: 7,
                last_known: 100,
                current: 30
            },
            Got::PollDone
        ]
    );
}

#[test]
fn a_terminal_entry_passes_through_with_its_value_and_code() {
    let dir = TempDir::new("re-terminal");
    let mut engine = engine(16, 4);
    let path = dir.file("a.log", b"abc");
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    push(
        &engine,
        SqEntry::Terminal(Terminal {
            value: registration.value(),
            win32_code: ERROR_ACCESS_DENIED,
            last_known: 3,
        }),
    );
    push(&engine, SqEntry::PollDone);
    assert_eq!(
        settle(&mut engine),
        [
            Got::Terminal {
                value: 7,
                win32_code: ERROR_ACCESS_DENIED,
                last_known: 3
            },
            Got::PollDone
        ]
    );
}

#[test]
fn a_short_read_is_labelled_with_the_bytes_actually_read() {
    let dir = TempDir::new("re-short");
    let mut engine = engine(64, 4);
    let content = pattern(10);
    let path = dir.file("a.log", &content);
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    announce(&engine, &registration.value(), 0, 30);
    push(&engine, SqEntry::PollDone);
    assert_eq!(
        settle(&mut engine),
        [
            Got::Data {
                value: 7,
                range: 0..10,
                bytes: content
            },
            Got::PollDone
        ]
    );
}

#[test]
fn a_read_is_limited_to_what_was_announced() {
    let dir = TempDir::new("re-limit");
    let mut engine = engine(64, 4);
    let content = pattern(100);
    let path = dir.file("a.log", &content);
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    announce(&engine, &registration.value(), 10, 40);
    push(&engine, SqEntry::PollDone);
    assert_eq!(
        settle(&mut engine),
        [
            Got::Data {
                value: 7,
                range: 10..40,
                bytes: content[10..40].to_vec()
            },
            Got::PollDone
        ]
    );
}

#[test]
fn a_read_wholly_past_the_end_delivers_nothing_and_is_not_a_failure() {
    let dir = TempDir::new("re-past");
    let sink = RecordingSink::wanting();
    let mut engine = engine_with(64, 4, 256, 64, 128, Some(sink.clone()));
    let path = dir.file("a.log", &pattern(10));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    announce(&engine, &registration.value(), 50, 60);
    push(&engine, SqEntry::PollDone);
    assert_eq!(settle(&mut engine), [Got::PollDone]);
    assert!(sink.seen().is_empty(), "{:?}", sink.seen());
    assert_eq!(engine.pool().available(), 4, "the buffer went back");
}

#[test]
fn reads_go_on_for_a_registration_dropped_after_its_attention_was_written() {
    let dir = TempDir::new("re-dropped");
    let mut engine = engine(16, 4);
    let content = pattern(70);
    let path = dir.file("a.log", &content);
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    drop(registration);
    assert_eq!(engine.watcher().registered(), 0);
    let got = settle(&mut engine);
    assert_eq!(reassembled(&got, 7), (0, content));
}

#[test]
fn the_means_to_read_is_taken_from_the_file_when_it_is_opened() {
    let dir = TempDir::new("re-tracked");
    let engine = engine(16, 4);
    let path = dir.file("a.log", &pattern(40));
    let first = engine.register(&wtf(&path), 0, 1).expect("register");
    let second = engine.register(&wtf(&path), 0, 2).expect("register");
    for registration in [&first, &second] {
        let tracked = registration.value();
        assert_eq!(tracked.registration, registration.id());
    }
    let (a, b) = (first.value(), second.value());
    assert_ne!(a.registration, b.registration);
    assert_ne!(a.file, b.file, "two registrations hold two handles");
    assert!(!a.keepalive.same_handle(&b.keepalive));
    // The observer outlives the registration, which is what keeps a read's handle open.
    drop(first);
    assert_eq!(a.keepalive.file_len().expect("length"), 40);
}

#[test]
fn a_tracked_value_names_the_handle_its_file_holds() {
    let dir = TempDir::new("re-handle");
    let path = dir.file("a.log", b"abc");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    let tracked = Tracked::new(5_u32, &held, 9);
    assert_eq!(tracked.value, 5);
    assert_eq!(tracked.registration, 9);
    assert!(tracked.keepalive.same_handle(&held.observer()));
    assert_eq!(
        tracked.file,
        std::os::windows::io::AsRawHandle::as_raw_handle(held.endpoint_handle()).addr()
    );
}

#[test]
fn a_failed_read_is_reported_and_delivers_nothing() {
    let dir = TempDir::new("re-failed");
    let sink = RecordingSink::wanting();
    let mut engine = engine_with(16, 2, 8, 8, 16, Some(sink.clone()));
    let path = dir.file("a.log", &pattern(40));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let tracked = registration.value();
    let (buffer, pending) = lent(&mut engine, &tracked, 32, 8);
    engine.finish_read(
        buffer,
        pending,
        Err(io::Error::from_raw_os_error(
            ERROR_ACCESS_DENIED.cast_signed(),
        )),
    );
    assert_eq!(
        sink.seen(),
        [Diagnostic::ReadFailed {
            registration: registration.id(),
            offset: 32,
            len: 8,
            win32_code: Some(ERROR_ACCESS_DENIED),
        }]
    );
    assert_eq!(sink.asked(), [DiagnosticKind::ReadFailed]);
    assert!(engine.undelivered.is_none());
    assert_eq!(engine.pool().available(), 2, "the buffer went back");
    assert!(
        engine
            .groups
            .list
            .front()
            .is_some_and(|g| g.outstanding == 0),
        "the read is settled"
    );
}

#[test]
fn a_failure_with_no_code_is_reported_without_one() {
    let dir = TempDir::new("re-nocode");
    let sink = RecordingSink::wanting();
    let mut engine = engine_with(16, 2, 8, 8, 16, Some(sink.clone()));
    let path = dir.file("a.log", &pattern(40));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let (buffer, pending) = lent(&mut engine, &registration.value(), 0, 4);
    engine.finish_read(buffer, pending, Err(io::Error::other("no code")));
    assert_eq!(
        sink.seen(),
        [Diagnostic::ReadFailed {
            registration: registration.id(),
            offset: 0,
            len: 4,
            win32_code: None,
        }]
    );
}

#[test]
fn end_of_file_reported_as_an_error_reads_nothing_and_is_no_failure() {
    let dir = TempDir::new("re-eof");
    let sink = RecordingSink::wanting();
    let mut engine = engine_with(16, 2, 8, 8, 16, Some(sink.clone()));
    let path = dir.file("a.log", &pattern(4));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let (buffer, pending) = lent(&mut engine, &registration.value(), 100, 8);
    engine.finish_read(
        buffer,
        pending,
        Err(io::Error::from_raw_os_error(ERROR_HANDLE_EOF.cast_signed())),
    );
    assert!(sink.seen().is_empty());
    assert!(engine.undelivered.is_none());
    assert_eq!(engine.pool().available(), 2);
}

#[test]
fn a_read_that_returns_nothing_delivers_nothing() {
    let dir = TempDir::new("re-zero");
    let mut engine = engine(16, 2);
    let path = dir.file("a.log", &pattern(4));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let (buffer, pending) = lent(&mut engine, &registration.value(), 0, 8);
    engine.finish_read(buffer, pending, Ok(0));
    assert!(engine.undelivered.is_none());
    assert_eq!(engine.pool().available(), 2);
}

#[test]
fn a_read_that_returns_bytes_becomes_the_one_entry_awaiting_delivery() {
    let dir = TempDir::new("re-bytes");
    let mut engine = engine(16, 2);
    let path = dir.file("a.log", &pattern(4));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    for (len, read) in [(8_u32, 8_usize), (8, 3), (1, 1)] {
        let (buffer, pending) = lent(&mut engine, &registration.value(), 100, len);
        engine.finish_read(buffer, pending, Ok(read));
        let Some(ReadEntry::Data(data)) = engine.undelivered.take() else {
            panic!("a read of bytes makes a data entry");
        };
        assert_eq!(data.value(), &7);
        assert_eq!(
            data.range(),
            100..100 + u64::try_from(read).expect("small"),
            "labelled with the bytes read, not the bytes asked for"
        );
        assert_eq!(data.chunk().len(), read);
    }
}

#[test]
#[should_panic(expected = "once the entry before it is delivered")]
fn settling_a_read_while_an_entry_is_undelivered_panics() {
    let dir = TempDir::new("re-twice");
    let mut engine = engine(16, 4);
    let path = dir.file("a.log", &pattern(4));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let tracked = registration.value();
    let (buffer, pending) = lent(&mut engine, &tracked, 0, 4);
    engine.finish_read(buffer, pending, Ok(4));
    let (buffer, pending) = lent(&mut engine, &tracked, 4, 4);
    engine.finish_read(buffer, pending, Ok(4));
}

#[test]
fn a_win32_code_is_found_in_an_os_error_and_absent_from_others() {
    assert_eq!(
        win32_code(&io::Error::from_raw_os_error(5)),
        Some(5),
        "an OS error carries its code"
    );
    assert_eq!(
        win32_code(&io::Error::from_raw_os_error(
            ERROR_HANDLE_EOF.cast_signed()
        )),
        Some(ERROR_HANDLE_EOF)
    );
    assert_eq!(win32_code(&io::Error::other("none")), None);
    assert_eq!(win32_code(&io::ErrorKind::NotFound.into()), None);
}

#[test]
fn an_empty_pool_stops_the_reads_and_a_returned_buffer_continues_them() {
    let dir = TempDir::new("re-pool");
    let mut engine = engine(8, 1);
    let content = pattern(40);
    let path = dir.file("a.log", &content);
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    // The only buffer is lent to the first read, and then held by the client in its chunk.
    let first = loop {
        let stop = engine.run().expect("run");
        assert_ne!(stop, Stop::ClientFull);
        if let Some(entry) = engine.read_sq().drain().next() {
            break entry;
        }
        engine.wait_for_completion(5_000).expect("wait");
    };
    assert_eq!(engine.pool().available(), 0);
    assert_eq!(engine.run().expect("run"), Stop::PoolEmpty);
    assert_eq!(engine.in_flight(), 0, "nothing is read without a buffer");
    assert!(!is_signalled(engine.pool().doorbell()));
    let first = got(first);
    assert!(
        is_signalled(engine.pool().doorbell()),
        "the chunk's drop rang"
    );
    // A run resets it before it takes, and nothing comes back during the run.
    assert_eq!(engine.run().expect("run"), Stop::PoolEmpty);
    assert!(
        !is_signalled(engine.pool().doorbell()),
        "a run resets the pool's doorbell"
    );
    let mut all = vec![first];
    all.extend(settle(&mut engine));
    assert_eq!(reassembled(&all, 7), (0, content));
    assert_eq!(poll_dones(&all), [all.len() - 1]);
}

#[test]
fn a_full_client_sq_stops_the_engine_and_ringing_back_continues_it() {
    let dir = TempDir::new("re-client");
    let mut engine = engine_with(8, 16, 2, 16, 32, None);
    let content = pattern(100);
    let path = dir.file("a.log", &content);
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    let mut all = Vec::new();
    let mut full_stops = 0;
    for _ in 0..1_000 {
        let stop = engine.run().expect("run");
        assert!(
            !is_signalled(engine.read_sq().resume_doorbell()),
            "a run resets the resume doorbell before it looks"
        );
        match stop {
            Stop::ClientFull => {
                full_stops += 1;
                assert!(is_signalled(engine.read_sq().doorbell()));
                let drained = drain_got(&engine);
                assert_eq!(drained.len(), engine.read_sq().capacity());
                all.extend(drained);
                engine.read_sq().ring_resume();
            }
            Stop::Idle if engine.in_flight() == 0 => {
                all.extend(drain_got(&engine));
                break;
            }
            _ => engine.wait_for_completion(5_000).expect("wait"),
        }
    }
    assert!(full_stops > 0, "thirteen reads do not fit an SQ of two");
    assert_eq!(reassembled(&all, 7), (0, content));
    assert_eq!(poll_dones(&all), [all.len() - 1]);
}

#[test]
fn a_full_client_sq_stops_with_the_entry_it_refused_and_loses_none() {
    let dir = TempDir::new("re-keep");
    let mut engine = engine_with(8, 16, 2, 16, 32, None);
    let path = dir.file("a.log", &pattern(8));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let tracked = registration.value();
    // Fill the client's SQ so the engine's first entry is refused.
    for _ in 0..2 {
        engine.read_sq().push(ReadEntry::PollDone).expect("room");
    }
    announce(&engine, &tracked, 0, 8);
    let stop = loop {
        let stop = engine.run().expect("run");
        if stop == Stop::ClientFull {
            break stop;
        }
        engine.wait_for_completion(5_000).expect("wait");
    };
    assert_eq!(stop, Stop::ClientFull);
    assert!(matches!(engine.undelivered, Some(ReadEntry::Data(_))));
    let mut all = drain_got(&engine);
    engine.read_sq().ring_resume();
    all.extend(settle(&mut engine));
    let datas: Vec<_> = all
        .iter()
        .filter(|entry| matches!(entry, Got::Data { .. }))
        .collect();
    assert_eq!(datas.len(), 1, "the refused entry was kept, once");
}

/// Make the attention the engine's current plan, as taking it from the watcher would.
fn plan_current(
    engine: &mut ReadEngine<u32>,
    tracked: &Tracked<u32>,
    last_known: u64,
    current: u64,
) {
    let (entry, target) = split(SqEntry::Attention(Attention {
        value: tracked.clone(),
        last_known,
        current,
    }));
    let Plan::Reads(reads) = plan(entry, engine.buffer_len) else {
        panic!("growth plans reads");
    };
    let group = engine.groups.open();
    engine.current = Some(Current {
        reads,
        target: target.expect("an attention has a target"),
        group,
    });
}

#[test]
fn reads_in_flight_never_exceed_the_completion_queue() {
    let dir = TempDir::new("re-cq");
    let mut engine = engine_with(8, 80, 256, 8, 4, None);
    let content = pattern(8 * 64);
    let path = dir.file("a.log", &content);
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let tracked = registration.value();
    assert!(
        engine.cq_size < 64,
        "more reads than the queue holds: {}",
        engine.cq_size
    );
    // Nothing is popped between the two passes, as when the device is slower than the engine. The
    // first leaves the second starting part way through the queue, with room for fewer reads than
    // its SQ holds, so a batch that did not count its own pushes would overfill the queue.
    plan_current(&mut engine, &tracked, 0, 32);
    assert!(matches!(
        engine.submit_reads().expect("submit"),
        Submitted::Done
    ));
    assert_eq!(engine.in_flight(), 4);
    plan_current(&mut engine, &tracked, 32, 8 * 64);
    assert!(matches!(
        engine
            .submit_reads()
            .expect("a pass never overfills the queue"),
        Submitted::Blocked(Stop::RingFull)
    ));
    assert_eq!(engine.in_flight(), engine.cq_size);
    // What was planned and not submitted is kept, and goes on as the queue empties.
    assert!(engine.current.is_some());
    let all = settle(&mut engine);
    assert_eq!(reassembled(&all, 7), (0, content));
}
#[test]
fn a_ring_sq_that_fills_is_submitted_and_the_reads_go_on() {
    let dir = TempDir::new("re-sq");
    let mut engine = engine_with(8, 32, 64, 2, 64, None);
    let content = pattern(8 * 20);
    let path = dir.file("a.log", &content);
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    assert_eq!(
        engine.run().expect("run"),
        Stop::Idle,
        "twenty reads through a queue of two, in one run"
    );
    let mut all = drain_got(&engine);
    all.extend(settle(&mut engine));
    assert_eq!(reassembled(&all, 7), (0, content));
}

#[test]
fn the_doorbell_is_rung_when_entries_are_written_and_not_before() {
    let dir = TempDir::new("re-bell");
    let mut engine = engine(16, 4);
    let path = dir.file("a.log", &pattern(20));
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    assert!(!is_signalled(engine.read_sq().doorbell()));
    assert_eq!(
        engine.run().expect("run"),
        Stop::Idle,
        "nothing yet to read"
    );
    assert!(
        !is_signalled(engine.read_sq().doorbell()),
        "a run that wrote nothing rings nothing"
    );
    engine.watcher().poll_here();
    settle(&mut engine);
    // `settle` drained, so it is the rung doorbell it left behind that is seen.
    assert!(is_signalled(engine.read_sq().doorbell()));
    engine.read_sq().reset_doorbell();
    assert!(!is_signalled(engine.read_sq().doorbell()));
}

#[test]
fn an_engine_whose_sq_capacity_is_invalid_says_so() {
    let watcher = Watcher::<Tracked<u32>>::new(4).expect("watcher");
    let pool = BufferPool::new(8, 1).expect("pool");
    let result = ReadEngine::new(watcher, pool, 3, 8, 8);
    assert!(
        matches!(result, Err(EngineError::Sq(CreateError::Capacity(_)))),
        "a capacity of three"
    );
}

#[test]
fn an_engine_whose_ring_cannot_be_made_says_so() {
    let watcher = Watcher::<Tracked<u32>>::new(4).expect("watcher");
    let pool = BufferPool::new(8, 1).expect("pool");
    let result = ReadEngine::new(watcher, pool, 4, u32::MAX, u32::MAX);
    let Err(EngineError::Ring(error)) = result else {
        panic!("a ring of that size cannot be made");
    };
    assert!(!error.to_string().is_empty());
}

#[test]
fn engine_errors_say_what_was_wrong() {
    let sq = EngineError::Sq(CreateError::Io(io::Error::other("doorbell"))).to_string();
    assert!(sq.contains("SQ") && sq.contains("doorbell"), "{sq}");
    let ring = EngineError::Ring(io::Error::other("ring")).to_string();
    assert!(ring.contains("ring"), "{ring}");
    assert!(std::error::Error::source(&EngineError::Ring(io::Error::other("x"))).is_some());
}

#[test]
fn an_open_group_is_reused_until_it_is_closed() {
    let mut groups = Groups::default();
    let first = groups.open();
    assert_eq!(groups.open(), first);
    groups.close_open();
    let second = groups.open();
    assert_eq!(second, first + 1);
    assert_eq!(groups.open(), second);
}

#[test]
fn a_group_is_ready_only_when_closed_and_every_read_is_settled() {
    let mut groups = Groups::default();
    assert!(!groups.ready(), "no group at all");
    let id = groups.open();
    groups.started(id);
    groups.started(id);
    assert!(!groups.ready(), "open, with reads outstanding");
    groups.close_open();
    assert!(!groups.ready(), "closed, with reads outstanding");
    groups.settled(id);
    assert!(!groups.ready(), "one read still outstanding");
    groups.settled(id);
    assert!(groups.ready());
    groups.pop_ready();
    assert!(!groups.ready());
}

#[test]
fn an_open_group_with_nothing_outstanding_is_not_ready_until_closed() {
    let mut groups = Groups::default();
    groups.open();
    assert!(!groups.ready());
    groups.close_open();
    assert!(groups.ready());
}

#[test]
fn a_poll_with_no_reads_gets_a_closed_group_of_its_own() {
    let mut groups = Groups::default();
    groups.close_open();
    assert!(groups.ready());
    groups.pop_ready();
    assert!(!groups.ready());
    let id = groups.open();
    assert_eq!(id, 1, "ids go on from the group that was delivered");
}

#[test]
fn polls_are_delivered_in_order_though_a_later_one_finishes_first() {
    let mut groups = Groups::default();
    let older = groups.open();
    groups.started(older);
    groups.close_open();
    let newer = groups.open();
    groups.started(newer);
    groups.close_open();
    groups.settled(newer);
    assert!(!groups.ready(), "the older poll's read is outstanding");
    groups.settled(older);
    assert!(groups.ready());
    groups.pop_ready();
    assert!(groups.ready(), "and then the newer, already finished");
    groups.pop_ready();
    assert!(!groups.ready());
}

#[test]
fn a_group_keeps_its_id_while_earlier_ones_are_delivered() {
    let mut groups = Groups::default();
    let a = groups.open();
    groups.close_open();
    let b = groups.open();
    groups.started(b);
    groups.pop_ready();
    groups.settled(b);
    assert_eq!((a, b), (0, 1));
    groups.close_open();
    assert!(groups.ready());
}

#[test]
fn splitting_an_attention_separates_the_value_from_the_means_to_read() {
    let dir = TempDir::new("re-split");
    let path = dir.file("a.log", b"abc");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    let tracked = Tracked::new(5_u32, &held, 9);
    let (entry, target) = split(SqEntry::Attention(Attention {
        value: tracked,
        last_known: 3,
        current: 8,
    }));
    assert_eq!(
        entry,
        SqEntry::Attention(Attention {
            value: 5,
            last_known: 3,
            current: 8,
        })
    );
    let target = target.expect("an attention has a target");
    assert_eq!(target.registration, 9);
    assert!(target.keepalive.same_handle(&held.observer()));
}

#[test]
fn splitting_a_terminal_entry_unwraps_its_value_and_has_no_target() {
    let dir = TempDir::new("re-split-t");
    let path = dir.file("a.log", b"abc");
    let held = HeldFile::open(&wtf(&path)).expect("open");
    let (entry, target) = split(SqEntry::Terminal(Terminal {
        value: Tracked::new(5_u32, &held, 9),
        win32_code: 6,
        last_known: 3,
    }));
    assert_eq!(
        entry,
        SqEntry::Terminal(Terminal {
            value: 5,
            win32_code: 6,
            last_known: 3,
        })
    );
    assert!(target.is_none());
}

#[test]
fn splitting_poll_done_is_poll_done_with_no_target() {
    let (entry, target) = split(SqEntry::<Tracked<u32>>::PollDone);
    assert_eq!(entry, SqEntry::PollDone);
    assert!(target.is_none());
}

#[test]
fn a_run_resets_the_watchers_doorbell_before_it_looks() {
    let dir = TempDir::new("re-wbell");
    let mut engine = engine(16, 4);
    let path = dir.file("a.log", &pattern(20));
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    assert!(
        is_signalled(engine.watcher().doorbell()),
        "the poll rang it"
    );
    engine.run().expect("run");
    assert!(
        !is_signalled(engine.watcher().doorbell()),
        "a run resets it, and nothing rang it again"
    );
}

#[test]
fn entries_taken_from_the_watchers_sq_ring_its_resume_doorbell_once_the_run_ends() {
    let dir = TempDir::new("re-wresume");
    let mut engine = engine(16, 4);
    let path = dir.file("a.log", &pattern(20));
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    assert_eq!(engine.run().expect("run"), Stop::Idle);
    assert!(
        !is_signalled(engine.watcher().resume_doorbell()),
        "nothing was taken, so no room was made"
    );
    engine.watcher().poll_here();
    settle(&mut engine);
    assert!(
        is_signalled(engine.watcher().resume_doorbell()),
        "a poll parked on a full SQ is told it can go on"
    );
}

#[test]
fn a_terminal_read_failure_is_a_terminal_entry_on_the_client_s_sq_and_also_a_diagnostic() {
    for code in [ERROR_INVALID_HANDLE, ERROR_FILE_INVALID] {
        let dir = TempDir::new("re-terminal-read");
        let sink = RecordingSink::wanting();
        let mut engine = engine_with(16, 2, 8, 8, 16, Some(sink.clone()));
        let path = dir.file("a.log", &pattern(40));
        let registration = engine.register(&wtf(&path), 0, 7).expect("register");
        let (buffer, pending) = lent(&mut engine, &registration.value(), 24, 8);
        engine.finish_read(
            buffer,
            pending,
            Err(io::Error::from_raw_os_error(code.cast_signed())),
        );
        engine.groups.close_open();
        assert_eq!(
            settle(&mut engine),
            [
                Got::Terminal {
                    value: 7,
                    win32_code: code,
                    last_known: 24
                },
                Got::PollDone
            ],
            "the terminal entry comes before its poll's poll-done"
        );
        assert_eq!(
            sink.seen(),
            [Diagnostic::ReadFailed {
                registration: registration.id(),
                offset: 24,
                len: 8,
                win32_code: Some(code),
            }]
        );
        assert_eq!(engine.pool().available(), 2, "the buffer went back");
    }
}

#[test]
fn a_registration_is_made_terminal_once_by_its_first_terminal_read_failure() {
    let dir = TempDir::new("re-terminal-once");
    let sink = RecordingSink::wanting();
    let mut engine = engine_with(16, 4, 8, 8, 16, Some(sink.clone()));
    let path = dir.file("a.log", &pattern(40));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let mut got = Vec::new();
    for (offset, code) in [(0, ERROR_INVALID_HANDLE), (8, ERROR_FILE_INVALID)] {
        let (buffer, pending) = lent(&mut engine, &registration.value(), offset, 8);
        engine.finish_read(
            buffer,
            pending,
            Err(io::Error::from_raw_os_error(code.cast_signed())),
        );
        engine.groups.close_open();
        got.extend(settle(&mut engine));
    }
    assert_eq!(
        got,
        [
            Got::Terminal {
                value: 7,
                win32_code: ERROR_INVALID_HANDLE,
                last_known: 0
            },
            Got::PollDone,
            Got::PollDone
        ],
        "the second failure is not reported again, and keeps its poll-done"
    );
    assert_eq!(sink.seen().len(), 2, "both reached the sink");
    assert_eq!(registration.terminal_code(), Some(ERROR_INVALID_HANDLE));
}

#[test]
fn a_read_failure_of_a_registration_a_probe_made_terminal_is_not_reported_again() {
    let dir = TempDir::new("re-terminal-probed");
    let mut engine = engine(16, 2);
    let path = dir.file("a.log", &pattern(40));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    assert!(
        engine
            .watcher()
            .mark_terminal(registration.id(), ERROR_FILE_INVALID)
    );
    let (buffer, pending) = lent(&mut engine, &registration.value(), 0, 8);
    engine.finish_read(
        buffer,
        pending,
        Err(io::Error::from_raw_os_error(
            ERROR_INVALID_HANDLE.cast_signed(),
        )),
    );
    assert!(engine.undelivered.is_none());
    assert_eq!(registration.terminal_code(), Some(ERROR_FILE_INVALID));
}

#[test]
fn a_terminal_read_failure_of_a_registration_since_dropped_reaches_the_sink_alone() {
    let dir = TempDir::new("re-terminal-dropped");
    let sink = RecordingSink::wanting();
    let mut engine = engine_with(16, 2, 8, 8, 16, Some(sink.clone()));
    let path = dir.file("a.log", &pattern(40));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    let tracked = registration.value();
    drop(registration);
    let (buffer, pending) = lent(&mut engine, &tracked, 0, 8);
    engine.finish_read(
        buffer,
        pending,
        Err(io::Error::from_raw_os_error(
            ERROR_INVALID_HANDLE.cast_signed(),
        )),
    );
    assert!(engine.undelivered.is_none());
    assert_eq!(sink.seen().len(), 1);
}

#[test]
fn a_read_the_ring_refuses_is_a_failed_read_and_the_rest_of_its_plan_is_abandoned() {
    let dir = TempDir::new("re-refused");
    let sink = RecordingSink::wanting();
    let mut engine = engine_with(8, 8, 64, 64, 128, Some(sink.clone()));
    let path = dir.file("a.log", &pattern(64));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    plan_current(&mut engine, &registration.value(), 0, 64);
    // A handle no read can address, which the ring is not asked about until the read is built.
    engine.current.as_mut().expect("planned").target.file = usize::MAX;
    assert!(matches!(
        engine
            .advance()
            .expect("a refusal is not an error of the run"),
        Advance::Progress
    ));
    assert!(engine.current.is_none(), "the plan is abandoned");
    assert_eq!(engine.in_flight(), 0, "nothing was queued");
    let seen = sink.seen();
    assert_eq!(seen.len(), 1, "one read failed: {seen:?}");
    assert!(matches!(
        seen[0],
        Diagnostic::ReadFailed { registration: id, offset: 0, len: 8, .. } if id == registration.id()
    ));
    assert_eq!(engine.pool().available(), 8, "the buffer went back");
    // The ring refuses an invalid handle with the code a terminal read failure carries, so the
    // failure is delivered, once, ahead of the poll-done that was waiting on it.
    engine.groups.close_open();
    assert_eq!(
        settle(&mut engine),
        [
            Got::Terminal {
                value: 7,
                win32_code: ERROR_INVALID_HANDLE,
                last_known: 0
            },
            Got::PollDone
        ]
    );
}

#[test]
fn a_transient_read_failure_reaches_the_diagnostic_sink_alone() {
    let dir = TempDir::new("re-transient-read");
    let sink = RecordingSink::wanting();
    let mut engine = engine_with(16, 2, 8, 8, 16, Some(sink.clone()));
    let path = dir.file("a.log", &pattern(40));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    for failure in [
        io::Error::from_raw_os_error(ERROR_ACCESS_DENIED.cast_signed()),
        io::Error::other("no code"),
    ] {
        let (buffer, pending) = lent(&mut engine, &registration.value(), 0, 8);
        engine.finish_read(buffer, pending, Err(failure));
        assert!(engine.undelivered.is_none());
    }
    assert_eq!(sink.seen().len(), 2);
    assert_eq!(drain_got(&engine), []);
}

#[test]
fn after_a_stop_no_read_is_submitted_and_the_reads_in_flight_are_delivered_with_poll_done() {
    let dir = TempDir::new("re-stop");
    let mut engine = engine(8, 4);
    let content = pattern(64);
    let path = dir.file("a.log", &content);
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    // Four reads go in flight, the pool's four buffers, and four more are planned and not submitted.
    loop {
        match engine.advance().expect("advance") {
            Advance::Progress => {}
            Advance::Blocked(stop) => {
                assert_eq!(stop, Stop::PoolEmpty);
                break;
            }
            Advance::Drained => panic!("eight reads do not fit four buffers"),
        }
    }
    assert_eq!(engine.in_flight(), 4);
    engine.stop();
    let got = settle(&mut engine);
    let (start, bytes) = reassembled(&got, 7);
    assert_eq!((start, bytes), (0, content[..32].to_vec()));
    assert_eq!(
        poll_dones(&got),
        [got.len() - 1],
        "poll-done is delivered, last"
    );
    assert!(engine.current.is_none(), "what was planned is abandoned");
}

#[test]
fn after_a_stop_attentions_are_skipped_and_poll_done_and_terminal_entries_are_still_delivered() {
    let dir = TempDir::new("re-stop-skip");
    let mut engine = engine(8, 4);
    let path = dir.file("a.log", &pattern(64));
    let registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.stop();
    engine.watcher().poll_here();
    push(
        &engine,
        SqEntry::Terminal(Terminal {
            value: registration.value(),
            win32_code: ERROR_INVALID_HANDLE,
            last_known: 5,
        }),
    );
    assert_eq!(
        settle(&mut engine),
        [
            Got::PollDone,
            Got::Terminal {
                value: 7,
                win32_code: ERROR_INVALID_HANDLE,
                last_known: 5
            }
        ]
    );
    assert_eq!(engine.in_flight(), 0, "nothing was read");
}

#[test]
fn a_stop_that_lands_while_reads_are_being_pushed_ends_the_pushing_at_the_next_read() {
    // A value whose clone, which the engine makes for each read it pushes, requests the stop.
    struct Spy {
        stop: Arc<AtomicBool>,
        clones_left: Arc<AtomicUsize>,
    }
    impl Clone for Spy {
        fn clone(&self) -> Self {
            if self.clones_left.load(Ordering::SeqCst) > 0
                && self.clones_left.fetch_sub(1, Ordering::SeqCst) == 1
            {
                self.stop.store(true, Ordering::SeqCst);
            }
            Self {
                stop: Arc::clone(&self.stop),
                clones_left: Arc::clone(&self.clones_left),
            }
        }
    }
    let dir = TempDir::new("re-stop-mid-push");
    let watcher = Watcher::new(1024).expect("a watcher");
    let pool = BufferPool::new(8, 8).expect("a pool");
    let mut engine = match ReadEngine::<Spy>::new(watcher, pool, 64, 64, 128) {
        Ok(engine) => engine,
        Err(error) => panic!("no engine: {error}"),
    };
    let path = dir.file("a.log", &pattern(64));
    let clones_left = Arc::new(AtomicUsize::new(0));
    let spy = Spy {
        stop: engine.stop_flag(),
        clones_left: Arc::clone(&clones_left),
    };
    let _registration = engine.register(&wtf(&path), 0, spy).expect("register");
    engine.watcher().poll_here();
    // Takes the attention and plans its eight reads, submitting none.
    assert!(matches!(
        engine.advance().expect("advance"),
        Advance::Progress
    ));
    assert!(engine.current.is_some());
    // The second clone from here, the second read's, requests the stop.
    clones_left.store(2, Ordering::SeqCst);
    assert!(matches!(
        engine.submit_reads().expect("submit"),
        Submitted::Done
    ));
    assert_eq!(
        engine.in_flight(),
        2,
        "the read being pushed when the stop landed went in, and no read after it"
    );
}

#[test]
fn a_stop_leaves_the_watcher_working() {
    let dir = TempDir::new("re-stop-watcher");
    let engine = engine(8, 4);
    engine.stop();
    let path = dir.file("a.log", b"abc");
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();
    assert!(engine.watcher().drain().next().is_some());
}
