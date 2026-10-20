// Copyright (c) 2026 Mike Grier
//! Tests for `peek_named_pipe`.
//!
//! What is asserted is what the page says, `ms.date` 12/05/2018. A test of something the page does not
//! say, or says otherwise than what was observed, is named an observation.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicU32, Ordering};

use super::{PeekedPipe, peek_named_pipe};
use crate::{
    NamedPipeOptions, PipeMode, PipeOpenMode, PipeStateUpdate, create_named_pipe, create_pipe,
    set_named_pipe_handle_state,
};

const FILE_SHARE_ALL: u32 = 7;

fn unique_name(tag: &str) -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!(
        r"\\.\pipe\win-pipes-sys-peek-test-{}-{tag}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn options(pipe_mode: PipeMode) -> NamedPipeOptions {
    NamedPipeOptions {
        open_mode: PipeOpenMode::ACCESS_DUPLEX,
        pipe_mode,
        max_instances: 1,
        out_buffer_size: 4096,
        in_buffer_size: 4096,
        default_timeout_ms: 0,
    }
}

/// A message pipe whose server end has written `messages`, and the client end, in message-read mode.
fn message_pipe_with(messages: &[&[u8]]) -> (OwnedHandle, File) {
    let name = unique_name("message");
    let server = create_named_pipe(
        &name,
        &options(PipeMode::TYPE_MESSAGE | PipeMode::READMODE_MESSAGE | PipeMode::WAIT),
        None,
    )
    .expect("a pipe");
    let client = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .open(&name)
        .expect("a client");
    set_mode(&client, PipeMode::READMODE_MESSAGE);
    let mut writer = File::from(server.try_clone().expect("a duplicate"));
    for message in messages {
        writer.write_all(message).expect("server write");
    }
    (server, client)
}

fn set_mode(pipe: &impl std::os::windows::io::AsHandle, mode: PipeMode) {
    set_named_pipe_handle_state(
        pipe,
        &PipeStateUpdate {
            mode: Some(mode),
            ..PipeStateUpdate::default()
        },
    )
    .expect("set the mode");
}

#[test]
fn an_empty_pipe_peeks_as_nothing_and_returns_at_once() {
    // The page: "The function always returns immediately in a single-threaded application, even if
    // there is no data in the pipe."
    let pipe = create_pipe(0, None).expect("a pipe");
    let mut buffer = [0u8; 8];
    let peeked = peek_named_pipe(&pipe.read, Some(&mut buffer)).expect("a peek");
    assert_eq!(peeked, PeekedPipe::default());
    assert_eq!(buffer, [0u8; 8]);
}

#[test]
fn what_is_peeked_is_not_removed() {
    let pipe = create_pipe(0, None).expect("a pipe");
    File::from(pipe.write.try_clone().expect("a duplicate"))
        .write_all(b"hello")
        .expect("write");

    for _ in 0..3 {
        let mut buffer = [0u8; 5];
        let peeked = peek_named_pipe(&pipe.read, Some(&mut buffer)).expect("a peek");
        assert_eq!(&buffer, b"hello");
        assert_eq!(peeked.bytes_read, 5);
        assert_eq!(peeked.total_bytes_available, 5);
    }
    let mut read = File::from(pipe.read);
    let mut taken = [0u8; 5];
    read.read_exact(&mut taken).expect("read");
    assert_eq!(&taken, b"hello", "the reads took what the peeks left");
}

#[test]
fn a_buffer_shorter_than_the_data_reads_that_many_and_counts_the_rest() {
    let pipe = create_pipe(0, None).expect("a pipe");
    File::from(pipe.write.try_clone().expect("a duplicate"))
        .write_all(b"hello")
        .expect("write");
    for size in 0..=5usize {
        let mut buffer = [0xffu8; 5];
        let peeked = peek_named_pipe(&pipe.read, Some(&mut buffer[..size])).expect("a peek");
        assert_eq!(peeked.bytes_read as usize, size);
        assert_eq!(&buffer[..size], &b"hello"[..size]);
        assert_eq!(peeked.total_bytes_available, 5);
        assert!(buffer[size..].iter().all(|&byte| byte == 0xff), "untouched");
    }
}

#[test]
fn no_buffer_asks_only_for_the_counts() {
    let pipe = create_pipe(0, None).expect("a pipe");
    File::from(pipe.write.try_clone().expect("a duplicate"))
        .write_all(b"abc")
        .expect("write");
    let peeked = peek_named_pipe(&pipe.read, None).expect("a peek");
    assert_eq!(peeked.bytes_read, 0);
    assert_eq!(peeked.total_bytes_available, 3);
}

#[test]
fn the_available_count_follows_what_is_written_and_read() {
    let pipe = create_pipe(0, None).expect("a pipe");
    let mut write = File::from(pipe.write.try_clone().expect("a duplicate"));
    let available = |pipe: &OwnedHandle| {
        peek_named_pipe(pipe, None)
            .expect("a peek")
            .total_bytes_available
    };
    assert_eq!(available(&pipe.read), 0);
    write.write_all(&[1u8; 10]).expect("write");
    assert_eq!(available(&pipe.read), 10);
    write.write_all(&[2u8; 5]).expect("write");
    assert_eq!(available(&pipe.read), 15);
    let mut read = File::from(pipe.read.try_clone().expect("a duplicate"));
    read.read_exact(&mut [0u8; 12]).expect("read");
    assert_eq!(available(&pipe.read), 3);
}

#[test]
fn a_message_pipe_peeks_one_message_and_counts_what_is_left_of_it() {
    // The page: in message-read mode the call "reads the next message in the pipe. If the message is
    // larger than nBufferSize, the function returns TRUE after reading the specified number of bytes.
    // In this situation, lpBytesLeftThisMessage will receive the number of bytes remaining".
    let (_server, client) = message_pipe_with(&[b"abc", b"defgh"]);

    let mut buffer = [0u8; 2];
    let peeked = peek_named_pipe(&client, Some(&mut buffer)).expect("a peek");
    assert_eq!(&buffer, b"ab");
    assert_eq!(peeked.bytes_read, 2);
    assert_eq!(peeked.bytes_left_this_message, 1);
    assert_eq!(peeked.total_bytes_available, 8, "both messages");

    let mut buffer = [0u8; 16];
    let peeked = peek_named_pipe(&client, Some(&mut buffer)).expect("a peek");
    assert_eq!(&buffer[..3], b"abc", "the first message only");
    assert_eq!(peeked.bytes_read, 3);
    assert_eq!(peeked.bytes_left_this_message, 0);
}

#[test]
fn a_handle_changed_to_byte_read_mode_is_still_peeked_in_message_mode() {
    // The page: "If you change the mode to PIPE_READMODE_BYTE with SetNamedPipeHandleState, ReadFile
    // will read in byte mode, but PeekNamedPipe will continue to read in message mode."
    let (_server, mut client) = message_pipe_with(&[b"abc", b"defgh"]);
    set_mode(&client, PipeMode::READMODE_BYTE);

    let mut buffer = [0u8; 16];
    let peeked = peek_named_pipe(&client, Some(&mut buffer)).expect("a peek");
    assert_eq!(peeked.bytes_read, 3, "one message");

    let mut taken = [0u8; 16];
    assert_eq!(client.read(&mut taken).expect("a read"), 8, "all the bytes");
}

#[test]
fn the_wait_mode_has_no_effect() {
    // The page: "The wait mode of a named pipe handle (blocking or nonblocking) has no effect on the
    // function."
    let name = unique_name("nowait");
    let server = create_named_pipe(
        &name,
        &options(PipeMode::TYPE_BYTE | PipeMode::NOWAIT),
        None,
    )
    .expect("a pipe");
    let client = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_ALL)
        .open(&name)
        .expect("a client");
    File::from(server.try_clone().expect("a duplicate"))
        .write_all(b"xy")
        .expect("write");
    set_mode(&client, PipeMode::NOWAIT);
    assert_eq!(
        peek_named_pipe(&client, None)
            .expect("a peek")
            .total_bytes_available,
        2
    );
    set_mode(&client, PipeMode::WAIT);
    assert_eq!(
        peek_named_pipe(&client, None)
            .expect("a peek")
            .total_bytes_available,
        2
    );
}

#[test]
fn the_write_end_of_an_anonymous_pipe_cannot_be_peeked() {
    // The page: the handle "must have GENERIC_READ access to the pipe", and for an anonymous pipe is
    // "the read end".
    let pipe = create_pipe(0, None).expect("a pipe");
    assert!(peek_named_pipe(&pipe.write, None).is_err());
}

#[test]
fn a_pipe_whose_writer_has_gone_still_shows_what_it_held() {
    // Observed: data written before the write end closed is still peekable.
    let pipe = create_pipe(0, None).expect("a pipe");
    let mut write = File::from(pipe.write);
    write.write_all(b"last").expect("write");
    drop(write);
    let mut buffer = [0u8; 4];
    let peeked = peek_named_pipe(&pipe.read, Some(&mut buffer)).expect("a peek");
    assert_eq!(&buffer, b"last");
    assert_eq!(peeked.total_bytes_available, 4);
}

#[test]
fn an_empty_pipe_whose_writer_has_gone_is_the_calls_error() {
    // Observed: the call fails once the write end is closed and nothing is left in the pipe.
    let pipe = create_pipe(0, None).expect("a pipe");
    drop(pipe.write);
    assert!(peek_named_pipe(&pipe.read, None).is_err());
}

#[test]
fn the_left_this_message_count_of_a_byte_pipe_is_the_calls_and_not_the_pages_zero() {
    // Observed, against the page, which says the value "will be zero for byte-type named pipes or for
    // anonymous pipes": for an anonymous pipe peeked with a buffer shorter than the data it was
    // `0 - bytes_read` as a u32, and with no buffer it was zero. The wrapper reports what the call
    // gave and does not correct it.
    let pipe = create_pipe(0, None).expect("a pipe");
    File::from(pipe.write.try_clone().expect("a duplicate"))
        .write_all(b"hello")
        .expect("write");

    let none = peek_named_pipe(&pipe.read, None).expect("a peek");
    assert_eq!(none.bytes_left_this_message, 0);

    let mut buffer = [0u8; 3];
    let some = peek_named_pipe(&pipe.read, Some(&mut buffer)).expect("a peek");
    assert_eq!(
        some.bytes_left_this_message,
        0u32.wrapping_sub(some.bytes_read)
    );
}
