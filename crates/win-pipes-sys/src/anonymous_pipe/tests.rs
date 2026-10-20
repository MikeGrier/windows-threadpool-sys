// Copyright (c) 2026 Mike Grier
//! Tests for `create_pipe`.
//!
//! What is asserted is what the page says, `ms.date` 12/05/2018. A test that depends on something
//! the page does not say names it as an observation.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::thread;

use win_authorization_sys::SecurityAttributes;
use win_handles_sys::{HandleFlags, get_handle_information};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType};

use super::{AnonymousPipe, create_pipe};

fn files(pipe: AnonymousPipe) -> (File, File) {
    (File::from(pipe.read), File::from(pipe.write))
}

fn inheritable(handle: &OwnedHandle) -> bool {
    get_handle_information(handle)
        .expect("flags")
        .contains(HandleFlags::INHERIT)
}

#[test]
fn bytes_written_to_the_write_end_are_read_from_the_read_end() {
    let (mut read, mut write) = files(create_pipe(0, None).expect("a pipe"));
    write.write_all(b"ping").expect("write");
    let mut received = [0u8; 4];
    read.read_exact(&mut received).expect("read");
    assert_eq!(&received, b"ping");
}

#[test]
fn every_size_gives_a_working_pipe() {
    // Zero is the page's "default buffer size"; the rest are suggestions the system may round, so
    // the write is made from its own thread: a size too small to hold it would otherwise block the
    // writer, as the page says, with nobody reading.
    for size in [0u32, 1, 2, 63, 64, 512, 4096, 4097, 65_536, 1 << 20] {
        let (mut read, mut write) = files(create_pipe(size, None).expect("a pipe"));
        let writer = thread::spawn(move || write.write_all(b"abcdef").expect("write"));
        let mut received = [0u8; 6];
        read.read_exact(&mut received).expect("read");
        writer.join().expect("the writer finished");
        assert_eq!(&received, b"abcdef", "size {size}");
    }
}

#[test]
fn the_two_ends_are_different_handles() {
    let pipe = create_pipe(0, None).expect("a pipe");
    assert_ne!(pipe.read.as_raw_handle(), pipe.write.as_raw_handle());
    assert!(!pipe.read.as_raw_handle().is_null());
    assert!(!pipe.write.as_raw_handle().is_null());
}

#[test]
fn bytes_arrive_in_the_order_written() {
    let (mut read, mut write) = files(create_pipe(4096, None).expect("a pipe"));
    let sent: Vec<u8> = (0..=255u8).cycle().take(2000).collect();
    for chunk in sent.chunks(7) {
        write.write_all(chunk).expect("write");
    }
    let mut received = vec![0u8; sent.len()];
    read.read_exact(&mut received).expect("read");
    assert_eq!(received, sent);
}

#[test]
fn a_write_larger_than_the_buffer_completes_once_a_reader_drains_it() {
    // The page: a write that finds the pipe full "does not return until another process or thread
    // uses ReadFile to make more buffer space available".
    let (mut read, mut write) = files(create_pipe(64, None).expect("a pipe"));
    let sent: Vec<u8> = (0..=255u8).cycle().take(100_000).collect();
    let writer = {
        let sent = sent.clone();
        thread::spawn(move || write.write_all(&sent).expect("write"))
    };
    let mut received = vec![0u8; sent.len()];
    read.read_exact(&mut received).expect("read");
    writer.join().expect("the writer finished");
    assert_eq!(received, sent);
}

#[test]
fn closing_the_write_end_ends_the_reads() {
    // Observed, and not on the page: a read that finds the write end closed reads zero bytes, which
    // `File` reports as the end of the stream.
    let (mut read, mut write) = files(create_pipe(0, None).expect("a pipe"));
    write.write_all(b"last").expect("write");
    drop(write);
    let mut rest = Vec::new();
    read.read_to_end(&mut rest).expect("read to the end");
    assert_eq!(rest, b"last");
}

#[test]
fn closing_the_read_end_fails_the_writes() {
    // Observed, and not on the page.
    let (read, mut write) = files(create_pipe(0, None).expect("a pipe"));
    drop(read);
    let error = write.write_all(b"x").expect_err("nobody is reading");
    assert_eq!(error.kind(), ErrorKind::BrokenPipe);
}

#[test]
fn each_end_is_a_pipe_to_the_file_type_query() {
    let pipe = create_pipe(0, None).expect("a pipe");
    // SAFETY: both handles are open for the call.
    unsafe {
        assert_eq!(GetFileType(pipe.read.as_raw_handle()), FILE_TYPE_PIPE);
        assert_eq!(GetFileType(pipe.write.as_raw_handle()), FILE_TYPE_PIPE);
    }
}

#[test]
fn the_read_end_does_not_write_and_the_write_end_does_not_read() {
    // Observed, and not on the page, which names only what each handle is for.
    let (mut read, mut write) = files(create_pipe(0, None).expect("a pipe"));
    assert!(read.write(b"x").is_err());
    assert!(write.read(&mut [0u8; 1]).is_err());
}

#[test]
fn no_attributes_give_ends_that_cannot_be_inherited() {
    let pipe = create_pipe(0, None).expect("a pipe");
    assert!(!inheritable(&pipe.read));
    assert!(!inheritable(&pipe.write));
}

#[test]
fn attributes_that_inherit_give_ends_that_can_be() {
    let attributes = SecurityAttributes::new(None, true);
    let pipe = create_pipe(0, Some(&attributes)).expect("a pipe");
    assert!(inheritable(&pipe.read));
    assert!(inheritable(&pipe.write));
}

#[test]
fn attributes_that_do_not_inherit_give_ends_that_cannot_be() {
    let attributes = SecurityAttributes::new(None, false);
    let pipe = create_pipe(0, Some(&attributes)).expect("a pipe");
    assert!(!inheritable(&pipe.read));
    assert!(!inheritable(&pipe.write));
}

#[test]
fn pipes_made_one_after_another_are_independent() {
    let (mut read_a, mut write_a) = files(create_pipe(0, None).expect("a pipe"));
    let (mut read_b, mut write_b) = files(create_pipe(0, None).expect("a pipe"));
    write_a.write_all(b"a").expect("write");
    write_b.write_all(b"b").expect("write");
    let mut received = [0u8; 1];
    read_b.read_exact(&mut received).expect("read");
    assert_eq!(&received, b"b");
    read_a.read_exact(&mut received).expect("read");
    assert_eq!(&received, b"a");
}

#[test]
fn the_largest_size_is_a_call_result_and_not_a_panic() {
    // The page does not name a limit, so this does not assert which result the call gives: it
    // asserts that either one is a result, and that a pipe made is a working one.
    match create_pipe(u32::MAX, None) {
        Ok(pipe) => {
            let (mut read, mut write) = files(pipe);
            write.write_all(b"x").expect("write");
            let mut received = [0u8; 1];
            read.read_exact(&mut received).expect("read");
        }
        Err(error) => assert!(error.raw_os_error().is_some(), "{error}"),
    }
}
