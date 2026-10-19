// Copyright (c) 2026 Mike Grier
//! The reading layer's entries, as a client outside the crate sees them (TW-9.3).
//!
//! A client cannot make a `ReadSq` or a `Chunk`: the layer does, and lends them. What can be shown
//! from outside is that every item the client reads is public, with the signature it is documented
//! to have. These tests compile only while that holds; a signature that changed, or an item made
//! private again, fails the build of this file. What the items do is tested through the layer, which
//! is public in TW-9.4 and run over real files in TW-9.5.
#![cfg(windows)]

use std::io;
use std::ops::Range;
use std::os::windows::io::BorrowedHandle;
use std::sync::Arc;

use win_multi_file_tail_watcher::reader::{
    Chunk, Data, ReadDrain, ReadEntry, ReadSq, Shrink, TailReader, TailReaderError,
    TailReaderOptions, TailRegistration,
};
use win_multi_file_tail_watcher::{DiagnosticSink, OpenError, PollError, Terminal};
use wtf_string::Wtf16Str;

fn assert_send<T: Send>() {}
fn assert_send_sync<T: Send + Sync>() {}
fn assert_iterator<I: Iterator<Item = ReadEntry<u32>>>() {}

#[test]
fn the_entries_the_client_reads_can_cross_threads_when_the_value_can() {
    // The layer writes them on a thread-pool thread and the client reads them on its own.
    assert_send::<ReadEntry<u32>>();
    assert_send::<Data<u32>>();
    assert_send::<Shrink<u32>>();
    assert_send::<Chunk>();
    assert_send_sync::<ReadSq<u32>>();
}

#[test]
fn the_drain_is_an_iterator_of_entries() {
    assert_iterator::<ReadDrain<'static, u32>>();
}

#[test]
fn the_documented_signatures_of_the_sq_and_its_doorbells() {
    let _: for<'a> fn(&'a ReadSq<u32>) -> ReadDrain<'a, u32> = ReadSq::<u32>::drain;
    let _: fn(&ReadSq<u32>) -> usize = ReadSq::<u32>::capacity;
    let _: for<'a> fn(&'a ReadSq<u32>) -> BorrowedHandle<'a> = ReadSq::<u32>::doorbell;
    let _: fn(&ReadSq<u32>) = ReadSq::<u32>::reset_doorbell;
    let _: for<'a> fn(&'a ReadSq<u32>) -> BorrowedHandle<'a> = ReadSq::<u32>::resume_doorbell;
    let _: fn(&ReadSq<u32>) = ReadSq::<u32>::ring_resume;
}

#[test]
fn the_documented_signatures_of_a_data_entry() {
    let _: for<'a> fn(&'a Data<u32>) -> &'a u32 = Data::<u32>::value;
    let _: fn(&Data<u32>) -> Range<u64> = Data::<u32>::range;
    let _: for<'a> fn(&'a Data<u32>) -> &'a Chunk = Data::<u32>::chunk;
    let _: fn(Data<u32>) -> (u32, Range<u64>, Chunk) = Data::<u32>::into_parts;
}

#[test]
fn a_chunk_is_a_view_of_the_bytes_it_holds() {
    fn bytes_of(chunk: &Chunk) -> &[u8] {
        chunk
    }
    fn as_ref_of(chunk: &Chunk) -> &[u8] {
        chunk.as_ref()
    }
    let _ = (bytes_of, as_ref_of);
}

/// A shrink has public fields, like the watcher's attention, so a client reads it by name.
#[test]
fn a_shrink_has_public_fields_and_is_equal_by_them() {
    let shrink = Shrink {
        value: 7_u32,
        last_known: 100,
        current: 40,
    };
    assert_eq!(shrink.clone(), shrink);
    assert_ne!(
        shrink,
        Shrink {
            value: 7,
            last_known: 100,
            current: 41
        }
    );
    let Shrink {
        value,
        last_known,
        current,
    } = shrink;
    assert_eq!((value, last_known, current), (7, 100, 40));
}

#[test]
fn an_entry_is_matched_with_a_wildcard_arm_and_formats_for_debugging() {
    let entries: Vec<ReadEntry<u32>> = vec![
        ReadEntry::Shrink(Shrink {
            value: 1,
            last_known: 9,
            current: 3,
        }),
        ReadEntry::Terminal(Terminal {
            value: 2,
            win32_code: 6,
            last_known: 4,
        }),
        ReadEntry::PollDone,
    ];
    let kinds: Vec<&str> = entries
        .iter()
        .map(|entry| match entry {
            ReadEntry::Data(_) => "data",
            ReadEntry::Shrink(_) => "shrink",
            ReadEntry::Terminal(_) => "terminal",
            ReadEntry::PollDone => "poll done",
            _ => "unknown",
        })
        .collect();
    assert_eq!(kinds, ["shrink", "terminal", "poll done"]);
    for entry in &entries {
        assert!(!format!("{entry:?}").is_empty());
    }
}

#[test]
fn the_documented_signatures_of_the_reader() {
    type Made = Result<TailReader<u32>, TailReaderError>;
    type Registered = Result<TailRegistration<u32>, OpenError>;
    let _: fn(&TailReaderOptions, usize) -> Made = TailReader::<u32>::new;
    let _: fn(&TailReaderOptions, usize, Arc<dyn DiagnosticSink>) -> Made =
        TailReader::<u32>::with_diagnostics;
    let _: fn(&TailReader<u32>, &Wtf16Str, u64, u32) -> Registered = TailReader::<u32>::register;
    let _: fn(&TailReader<u32>) -> Result<(), PollError> = TailReader::<u32>::poll;
    let _: for<'a> fn(&'a TailReader<u32>) -> &'a ReadSq<u32> = TailReader::<u32>::read_sq;
    let _: fn(&TailReader<u32>) = TailReader::<u32>::shutdown;
    let _: fn(&TailReader<u32>) -> Option<io::Error> = TailReader::<u32>::take_failure;
}

#[test]
fn the_documented_signatures_of_a_registration() {
    let _: fn(&TailRegistration<u32>) -> u64 = TailRegistration::<u32>::id;
    let _: fn(&TailRegistration<u32>) -> u32 = TailRegistration::<u32>::value;
    let _: fn(&TailRegistration<u32>) -> u64 = TailRegistration::<u32>::start_offset;
}

#[test]
fn the_reader_error_is_an_error_a_caller_can_hold_and_send() {
    fn assert_error<E: std::error::Error + Send + Sync + 'static>() {}
    assert_error::<TailReaderError>();
}

/// The error is open to further causes, so a caller matches it with a wildcard arm.
#[test]
fn the_reader_error_is_matched_with_a_wildcard_arm() {
    // The options are `#[non_exhaustive]`: a caller starts from the defaults and assigns.
    let mut options = TailReaderOptions::default();
    options.buffer_len = 0;
    let error = TailReader::<u32>::new(&options, 64).unwrap_err();
    let kind = match &error {
        TailReaderError::Options(_) => "options",
        TailReaderError::Watcher(_) => "watcher",
        TailReaderError::ClientSq(_) => "client queue",
        TailReaderError::Allocation(_) => "allocation",
        TailReaderError::Ring(_) => "ring",
        TailReaderError::Io(_) => "io",
        _ => "a cause this version does not know",
    };
    assert_eq!(kind, "options");
}
