// Copyright (c) 2026 Mike Grier
use std::io;
use std::ops::Range;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

use super::{TailReader, TailRegistration};
use crate::pool::PoolError;
use crate::read_engine::{EngineError, Stop};
use crate::reader::{Cause, OptionsError, ReadEntry, TailReaderError, TailReaderOptions};
use crate::test_support::{RecordingSink, TempDir, code, signalled_within, wtf};
use crate::{CreateError, Diagnostic, DiagnosticKind, OpenError, PollError, SqCapacityError};

// Far longer than any step takes, and shorter than a sabotage run's bound, so a reader that stops
// delivering fails by assertion.
const WAIT: Duration = Duration::from_secs(6);

/// Small, valid sizes, so a short file needs several reads. No two of them are equal, so a pair
/// passed the wrong way round shows.
fn small() -> TailReaderOptions {
    TailReaderOptions {
        buffer_len: 8,
        buffer_count: 16,
        client_sq_capacity: 32,
        ring_sq_size: 16,
        ring_cq_size: 64,
    }
}

fn reader(options: &TailReaderOptions) -> TailReader<u32> {
    match TailReader::new(options, 64) {
        Ok(reader) => reader,
        Err(error) => panic!("no reader from {options:?}: {error}"),
    }
}

fn pattern(len: usize) -> Vec<u8> {
    (0..len)
        .map(|n| u8::try_from(n % 251).expect("below 251"))
        .collect()
}

/// What one poll delivered: the data by value, and anything else that was not a poll-done.
#[derive(Default)]
struct Poll {
    pieces: Vec<(u32, Range<u64>, Vec<u8>)>,
    other: Vec<String>,
}

impl Poll {
    /// The bytes delivered for `value`, in file order, and where they start. Checks that the pieces
    /// meet with no gap or overlap.
    fn bytes_of(&self, value: u32) -> (u64, Vec<u8>) {
        let mut pieces: Vec<_> = self.pieces.iter().filter(|p| p.0 == value).collect();
        pieces.sort_by_key(|p| p.1.start);
        let start = pieces.first().map_or(0, |p| p.1.start);
        let mut at = start;
        let mut bytes = Vec::new();
        for (_, range, piece) in pieces {
            assert_eq!(range.start, at, "a gap or a repeat in the bytes of {value}");
            bytes.extend_from_slice(piece);
            at = range.end;
        }
        (start, bytes)
    }
}

/// Poll `reader` and take everything up to the poll-done, through the doorbell as a client does.
fn poll_through(reader: &TailReader<u32>) -> Poll {
    reader.poll().expect("poll");
    take_until_poll_done(reader)
}

/// Take entries, waiting on the doorbell, until a poll-done arrives.
fn take_until_poll_done(reader: &TailReader<u32>) -> Poll {
    let mut taken = Poll::default();
    let started = Instant::now();
    loop {
        assert!(started.elapsed() < WAIT, "no poll-done arrived");
        if !signalled_within(reader.read_sq().doorbell(), 1_000) {
            continue;
        }
        reader.read_sq().reset_doorbell();
        for entry in reader.read_sq().drain() {
            match entry {
                ReadEntry::Data(data) => {
                    let (value, range, chunk) = data.into_parts();
                    assert_eq!(
                        u64::try_from(chunk.len()).expect("fits"),
                        range.end - range.start,
                        "the range is the length of the chunk"
                    );
                    taken.pieces.push((value, range, chunk.to_vec()));
                }
                ReadEntry::PollDone => return taken,
                other => taken.other.push(format!("{other:?}")),
            }
        }
    }
}

fn assert_send<T: Send>() {}
fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn the_reader_and_a_registration_can_be_used_across_threads() {
    assert_send_sync::<TailReader<u32>>();
    assert_send::<TailRegistration<u32>>();
}

#[test]
fn a_reader_is_made_from_the_defaults() {
    let reader = TailReader::<u32>::new(&TailReaderOptions::default(), 64).expect("a reader");
    assert!(reader.take_failure().is_none());
    assert_eq!(
        reader.read_sq().capacity(),
        TailReaderOptions::DEFAULT_CLIENT_SQ_CAPACITY,
        "the client queue is the size the options asked for"
    );
}

#[test]
fn the_pool_is_the_size_and_count_the_options_ask_for() {
    // 40 bytes through buffers of 8 is five reads, and of 16 would be three.
    let dir = TempDir::new("tr-pool");
    let path = dir.file("a.log", &pattern(40));
    for (len, expected) in [(8, 5), (16, 3), (40, 1), (3, 14)] {
        let mut options = small();
        options.buffer_len = len;
        let reader = reader(&options);
        let _registration = reader.register(&wtf(&path), 0, 1).expect("register");
        assert_eq!(
            poll_through(&reader).pieces.len(),
            expected,
            "buffers of {len} bytes"
        );
    }
    for count in [1, 2, 16, 33] {
        let mut options = small();
        options.buffer_count = count;
        assert_eq!(
            reader(&options).layer().engine_for_test().pool().count(),
            count
        );
    }
}

#[test]
fn the_ring_sizes_are_the_ones_the_options_ask_for() {
    // The reads in flight are bounded by the completion queue the ring gives: the larger of the
    // request and twice the submission queue, so the two are told apart by a request above twice.
    for (sq, cq, gives) in [(16, 64, 64), (8, 256, 256), (32, 64, 64), (16, 32, 32)] {
        let mut options = small();
        options.ring_sq_size = sq;
        options.ring_cq_size = cq;
        let reader = reader(&options);
        assert_eq!(
            reader.layer().engine_for_test().cq_size_for_test(),
            gives,
            "a ring of {sq} and {cq}"
        );
    }
}

#[test]
fn the_client_queue_is_the_size_the_options_ask_for() {
    for capacity in [2, 4, 16, 256, 4096] {
        let mut options = small();
        options.client_sq_capacity = capacity;
        assert_eq!(reader(&options).read_sq().capacity(), capacity);
    }
}

#[test]
fn a_refused_size_is_the_options_error_and_names_the_option() {
    let cases: [(TailReaderOptions, &str); 5] = [
        (
            TailReaderOptions {
                buffer_len: 0,
                ..small()
            },
            "buffer_len",
        ),
        (
            TailReaderOptions {
                buffer_count: 0,
                ..small()
            },
            "buffer_count",
        ),
        (
            TailReaderOptions {
                client_sq_capacity: 3,
                ..small()
            },
            "client_sq_capacity",
        ),
        (
            TailReaderOptions {
                ring_sq_size: 4,
                ..small()
            },
            "ring_sq_size",
        ),
        (
            TailReaderOptions {
                ring_cq_size: 16,
                ..small()
            },
            "ring_cq_size",
        ),
    ];
    for (options, field) in cases {
        let Err(error) = TailReader::<u32>::new(&options, 64) else {
            panic!("{options:?} should be refused");
        };
        let TailReaderError::Options(refusal) = &error else {
            panic!("{options:?}: {error:?}");
        };
        assert!(refusal.cause().is_some(), "{options:?}");
        assert!(error.to_string().contains(field), "{error}");
        let source = std::error::Error::source(&error).expect("a source");
        assert!(source.downcast_ref::<OptionsError>().is_some());
    }
}

#[test]
fn a_refused_size_is_reported_before_a_bad_watcher_capacity() {
    let options = TailReaderOptions {
        buffer_count: 0,
        ..small()
    };
    let Err(TailReaderError::Options(OptionsError::BufferCount {
        cause: Cause::Zero, ..
    })) = TailReader::<u32>::new(&options, 3)
    else {
        panic!("the options are checked first");
    };
}

#[test]
fn a_watcher_capacity_it_cannot_have_is_reported_as_the_watchers() {
    for capacity in [0, 1, 3, 100] {
        let Err(error) = TailReader::<u32>::new(&small(), capacity) else {
            panic!("a watcher capacity of {capacity} should be refused");
        };
        let TailReaderError::Watcher(CreateError::Capacity(SqCapacityError { requested, .. })) =
            &error
        else {
            panic!("{error:?}");
        };
        assert_eq!(*requested, capacity);
    }
    assert!(TailReader::<u32>::new(&small(), 2).is_ok());
}

#[test]
fn a_watcher_queue_the_allocator_will_not_give_is_the_watchers_allocation_failure() {
    // A capacity the queue accepts, so it is not a rule refusal: the memory is what is missing.
    let Err(error) = TailReader::<u32>::new(&small(), 1 << 50) else {
        panic!("no machine holds a queue of 2^50 entries");
    };
    let TailReaderError::Watcher(CreateError::Allocation(SqCapacityError { requested, .. })) =
        &error
    else {
        panic!("{error:?}");
    };
    assert_eq!(*requested, 1 << 50);
    assert!(error.to_string().contains("memory"), "{error}");
}

#[test]
fn a_client_queue_the_allocator_will_not_give_is_the_client_queues_allocation_failure() {
    let options = TailReaderOptions {
        client_sq_capacity: 1 << 50,
        ..small()
    };
    assert!(
        options.validate().is_ok(),
        "the options accept any capacity the queue does: the allocator answers when it is made"
    );
    let Err(error) = TailReader::<u32>::new(&options, 64) else {
        panic!("no machine holds a queue of 2^50 entries");
    };
    let TailReaderError::ClientSq(CreateError::Allocation(SqCapacityError { requested, .. })) =
        &error
    else {
        panic!("{error:?}");
    };
    assert_eq!(*requested, 1 << 50);
    assert!(error.to_string().contains("memory"), "{error}");
}

#[test]
fn buffers_the_allocator_will_not_give_are_an_error_not_an_abort() {
    let options = TailReaderOptions {
        buffer_count: usize::MAX,
        ..small()
    };
    assert!(
        options.validate().is_ok(),
        "the options accept any count: the allocator answers when the layer is made"
    );
    let Err(error) = TailReader::<u32>::new(&options, 64) else {
        panic!("a count of usize::MAX cannot be allocated");
    };
    assert!(matches!(error, TailReaderError::Allocation(_)), "{error:?}");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn what_a_pool_could_not_do_is_translated_to_the_cause() {
    let reserve = Vec::<u8>::new().try_reserve(usize::MAX).unwrap_err();
    assert!(matches!(
        TailReaderError::from_pool(PoolError::Allocation(reserve)),
        TailReaderError::Allocation(_)
    ));
    let io = TailReaderError::from_pool(PoolError::Io(io::Error::from_raw_os_error(5)));
    let TailReaderError::Io(inner) = io else {
        panic!("a doorbell the system refused is Io");
    };
    assert_eq!(code(&inner), Some(ERROR_ACCESS_DENIED));
}

#[test]
#[should_panic(expected = "the options refused this size with the pool's own check")]
fn a_pool_that_refuses_a_size_the_options_passed_is_a_defect() {
    let _ = TailReaderError::from_pool(PoolError::NoBuffers);
}

#[test]
fn what_an_engine_could_not_do_is_translated_to_the_cause() {
    let ring = TailReaderError::from_engine(EngineError::Ring(io::Error::from_raw_os_error(50)));
    let TailReaderError::Ring(inner) = ring else {
        panic!("a ring that could not be made is Ring");
    };
    assert_eq!(inner.raw_os_error(), Some(50));

    let sq = TailReaderError::from_engine(EngineError::Sq(CreateError::Io(
        io::Error::from_raw_os_error(5),
    )));
    assert!(matches!(sq, TailReaderError::ClientSq(CreateError::Io(_))));
}

#[test]
fn every_error_names_its_cause_and_carries_its_source() {
    let reserve = Vec::<u8>::new().try_reserve(usize::MAX).unwrap_err();
    let cases: [(TailReaderError, &str); 6] = [
        (
            TailReaderError::Options(OptionsError::BufferLen {
                given: 0,
                cause: Cause::Zero,
            }),
            "refused",
        ),
        (
            TailReaderError::Watcher(CreateError::Io(io::Error::from_raw_os_error(5))),
            "watcher",
        ),
        (
            TailReaderError::ClientSq(CreateError::Io(io::Error::from_raw_os_error(5))),
            "client queue",
        ),
        (TailReaderError::Allocation(reserve), "allocated"),
        (
            TailReaderError::Ring(io::Error::from_raw_os_error(50)),
            "ring",
        ),
        (
            TailReaderError::Io(io::Error::from_raw_os_error(5)),
            "set itself up",
        ),
    ];
    for (error, words) in cases {
        let message = error.to_string();
        assert!(message.contains(words), "{message:?} should say {words:?}");
        assert!(
            std::error::Error::source(&error).is_some(),
            "{message:?} carries its source"
        );
    }
}

#[test]
fn a_registered_file_is_read_and_its_poll_ends_in_a_poll_done() {
    let dir = TempDir::new("tr-read");
    let content = pattern(40);
    let path = dir.file("a.log", &content);
    let reader = reader(&small());
    let _registration = reader.register(&wtf(&path), 0, 7).expect("register");

    let poll = poll_through(&reader);
    assert_eq!(poll.bytes_of(7), (0, content));
    assert_eq!(poll.pieces.len(), 5, "40 bytes in buffers of 8");
    assert!(poll.other.is_empty(), "{:?}", poll.other);
}

#[test]
fn growth_after_a_poll_is_read_by_the_next_from_where_the_last_ended() {
    let dir = TempDir::new("tr-grow");
    let path = dir.file("a.log", &pattern(24));
    let reader = reader(&small());
    let _registration = reader.register(&wtf(&path), 0, 3).expect("register");

    let first = poll_through(&reader);
    assert_eq!(first.bytes_of(3), (0, pattern(24)));

    let more = pattern(24 + 19);
    std::fs::write(&path, &more).expect("grow");
    let second = poll_through(&reader);
    assert_eq!(second.bytes_of(3), (24, more[24..].to_vec()));
    assert!(
        poll_through(&reader).pieces.is_empty(),
        "no growth, no data"
    );
}

#[test]
fn a_registration_at_an_offset_reads_from_there() {
    let dir = TempDir::new("tr-offset");
    let content = pattern(30);
    let path = dir.file("a.log", &content);
    let reader = reader(&small());
    let registration = reader.register(&wtf(&path), 11, 9).expect("register");
    assert_eq!(registration.start_offset(), 11);
    assert_eq!(registration.value(), 9);

    assert_eq!(
        poll_through(&reader).bytes_of(9),
        (11, content[11..].to_vec())
    );
}

#[test]
fn an_offset_past_the_length_is_refused_and_one_at_the_length_is_not() {
    let dir = TempDir::new("tr-past");
    let path = dir.file("a.log", &pattern(10));
    let reader = reader(&small());
    let Err(OpenError::OffsetBeyondLength { offset, len }) = reader.register(&wtf(&path), 11, 1)
    else {
        panic!("an offset of 11 in a file of 10 bytes");
    };
    assert_eq!((offset, len), (11, 10));
    assert!(reader.register(&wtf(&path), 10, 1).is_ok());
}

#[test]
fn a_missing_file_is_refused_with_its_win32_code() {
    let dir = TempDir::new("tr-missing");
    let reader = reader(&small());
    let Err(OpenError::Io(error)) = reader.register(&wtf(&dir.0.join("absent.log")), 0, 1) else {
        panic!("a missing file");
    };
    assert_eq!(code(&error), Some(2));
}

#[test]
fn registrations_have_distinct_ids_that_only_grow_and_each_keeps_its_value() {
    let dir = TempDir::new("tr-ids");
    let path = dir.file("a.log", &pattern(4));
    let reader = reader(&small());
    let first = reader.register(&wtf(&path), 0, 100).expect("register");
    let second = reader.register(&wtf(&path), 0, 200).expect("register");
    assert!(second.id() > first.id());
    assert_eq!((first.value(), second.value()), (100, 200));

    let poll = poll_through(&reader);
    assert_eq!(poll.bytes_of(100).1, pattern(4));
    assert_eq!(poll.bytes_of(200).1, pattern(4));
}

#[test]
fn dropping_a_registration_unregisters_the_file() {
    let dir = TempDir::new("tr-drop");
    let path = dir.file("a.log", &pattern(16));
    let reader = reader(&small());
    let kept = reader.register(&wtf(&path), 0, 1).expect("register");
    let dropped = reader.register(&wtf(&path), 0, 2).expect("register");
    drop(dropped);

    let poll = poll_through(&reader);
    assert_eq!(poll.bytes_of(1).1, pattern(16));
    assert!(
        poll.pieces.iter().all(|p| p.0 != 2),
        "nothing for the registration that was dropped"
    );
    drop(kept);
}

#[test]
fn a_registration_may_outlive_the_reader() {
    let dir = TempDir::new("tr-outlive");
    let path = dir.file("a.log", &pattern(4));
    let reader = reader(&small());
    let registration = reader.register(&wtf(&path), 0, 5).expect("register");
    drop(reader);
    assert_eq!(registration.value(), 5);
    drop(registration);
    std::fs::remove_file(&path).expect("the file is closed, so it can be removed");
}

#[test]
fn a_poll_with_nothing_registered_is_a_poll_done_alone() {
    let reader = reader(&small());
    let poll = poll_through(&reader);
    assert!(poll.pieces.is_empty());
    assert!(poll.other.is_empty());
}

#[test]
fn a_poll_after_a_poll_done_is_accepted() {
    let dir = TempDir::new("tr-again");
    let path = dir.file("a.log", &pattern(4));
    let reader = reader(&small());
    let _registration = reader.register(&wtf(&path), 0, 1).expect("register");
    for _ in 0..5 {
        let _ = poll_through(&reader);
    }
}

#[test]
fn shutdown_refuses_later_polls_and_may_be_asked_again() {
    let reader = reader(&small());
    reader.shutdown();
    reader.shutdown();
    assert!(matches!(reader.poll(), Err(PollError::Shutdown)));
}

#[test]
fn shutdown_after_a_poll_started_still_delivers_its_poll_done() {
    let dir = TempDir::new("tr-shutdown");
    let path = dir.file("a.log", &pattern(64));
    let reader = reader(&small());
    let _registration = reader.register(&wtf(&path), 0, 1).expect("register");
    reader.poll().expect("poll");
    reader.shutdown();
    // Whatever was read before the stop arrives, and the poll-done is always delivered.
    let _ = take_until_poll_done(&reader);
    assert!(matches!(reader.poll(), Err(PollError::Shutdown)));
}

#[test]
fn a_reader_is_dropped_with_entries_still_queued() {
    let dir = TempDir::new("tr-queued");
    let path = dir.file("a.log", &pattern(64));
    let reader = reader(&small());
    let _registration = reader.register(&wtf(&path), 0, 1).expect("register");
    reader.poll().expect("poll");
    assert!(signalled_within(reader.read_sq().doorbell(), 6_000));
    drop(reader);
}

#[test]
fn the_first_failure_a_run_returned_is_kept_and_taken_once() {
    let reader = reader(&small());
    assert!(reader.take_failure().is_none());
    reader
        .layer()
        .record_for_test(Err(io::Error::from_raw_os_error(5)));
    reader
        .layer()
        .record_for_test(Err(io::Error::from_raw_os_error(6)));
    reader.layer().record_for_test(Ok(Stop::Idle));
    let first = reader.take_failure().expect("a failure");
    assert_eq!(code(&first), Some(5), "the first is the one kept");
    assert!(reader.take_failure().is_none(), "taken once");
}

#[test]
fn the_diagnostic_sink_given_is_the_one_the_reader_reports_to() {
    let sink = RecordingSink::wanting();
    let reader: TailReader<u32> =
        TailReader::with_diagnostics(&small(), 64, sink.clone()).expect("a reader");
    reader
        .layer()
        .engine_for_test()
        .watcher()
        .diagnose(DiagnosticKind::ProbeFailed, || Diagnostic::ProbeFailed {
            registration: 4,
            win32_code: Some(5),
        });
    assert_eq!(
        sink.seen(),
        [Diagnostic::ProbeFailed {
            registration: 4,
            win32_code: Some(5)
        }]
    );
}

#[test]
fn a_sink_that_wants_nothing_is_asked_and_given_nothing() {
    let sink = RecordingSink::unwilling();
    let reader: TailReader<u32> =
        TailReader::with_diagnostics(&small(), 64, sink.clone()).expect("a reader");
    reader
        .layer()
        .engine_for_test()
        .watcher()
        .diagnose(DiagnosticKind::ProbeFailed, || {
            panic!("no event is built for a sink that does not want one")
        });
    assert_eq!(sink.asked(), [DiagnosticKind::ProbeFailed]);
    assert!(sink.seen().is_empty());
}

#[test]
fn the_items_format_for_debugging() {
    let dir = TempDir::new("tr-debug");
    let path = dir.file("a.log", &pattern(4));
    let reader = reader(&small());
    let registration = reader.register(&wtf(&path), 0, 1).expect("register");
    assert!(format!("{reader:?}").contains("TailReader"));
    let shown = format!("{registration:?}");
    assert!(
        shown.contains("TailRegistration") && shown.contains("id"),
        "{shown}"
    );
}
