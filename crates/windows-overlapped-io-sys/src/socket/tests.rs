// Copyright (c) 2026 Mike Grier
use crate::{BlockingSocket, CompletionPort};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::windows::io::OwnedSocket;

#[test]
fn iocp_socket_recv_and_send_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let client = TcpStream::connect(addr).expect("connect");
    let (mut server, _peer) = listener.accept().expect("accept");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate_socket(OwnedSocket::from(client), 0)
        .expect("associate_socket");

    // Receive: submit, have the peer send, then dequeue and claim.
    let recv_token = endpoint
        .recv(vec![0_u8; 64])
        .expect("submit recv")
        .expect_pending("this socket is not in skip-on-success mode");
    server.write_all(b"ping").expect("peer write");
    let completion = port.get(5_000).expect("get").expect("recv completion");
    let (mut buffer, result) = match recv_token.claim(&completion) {
        Ok(pair) => pair,
        Err(_) => panic!("recv completion did not match its token"),
    };
    let received = result.expect("recv result");
    buffer.truncate(received);
    assert_eq!(buffer, b"ping");
    assert_eq!(port.outstanding(), 0);

    // Send: our side sends, the peer reads it back.
    let send_token = endpoint
        .send(b"pong".to_vec())
        .expect("submit send")
        .expect_pending("this socket is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("send completion");
    let (_data, result) = match send_token.claim(&completion) {
        Ok(pair) => pair,
        Err(_) => panic!("send completion did not match its token"),
    };
    assert_eq!(result.expect("send result"), 4);
    let mut got = [0_u8; 4];
    server.read_exact(&mut got).expect("peer read");
    assert_eq!(&got, b"pong");
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
}

#[test]
fn blocking_socket_recv_and_send_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let client = TcpStream::connect(addr).expect("connect");
    let (mut server, _peer) = listener.accept().expect("accept");

    let blocking = BlockingSocket::new(OwnedSocket::from(client));

    // The peer sends first, so the blocking receive has data and cannot deadlock.
    server.write_all(b"ping").expect("peer write");
    let mut buffer = vec![0_u8; 64];
    let received = blocking.recv(&mut buffer).expect("recv");
    assert_eq!(received, 4);
    assert_eq!(&buffer[..received], b"ping");

    let sent = blocking.send(b"pong").expect("send");
    assert_eq!(sent, 4);
    let mut got = [0_u8; 4];
    server.read_exact(&mut got).expect("peer read");
    assert_eq!(&got, b"pong");
}

// --- buffer length limits ---

/// `WSABUF` carries its byte count as a `u32`, so a longer buffer cannot be
/// described to Winsock. Capping would transfer a prefix and report success,
/// which is the defect `checked_len` replaced -- the same one already fixed for
/// the file and device adapters.
#[test]
fn checked_len_rejects_lengths_beyond_u32() {
    use crate::socket::checked_len;

    assert_eq!(checked_len(0, "receive buffer").expect("empty fits"), 0);
    assert_eq!(
        checked_len(u32::MAX as usize, "receive buffer").expect("the largest fitting length"),
        u32::MAX
    );

    #[cfg(target_pointer_width = "64")]
    {
        let too_long = u32::MAX as usize + 1;
        let error = checked_len(too_long, "receive buffer").expect_err("must not cap");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            error.to_string().contains("receive buffer"),
            "the error should name the offending buffer: {error}"
        );
        assert!(
            checked_len(too_long, "send buffer").is_err(),
            "the send buffer has the same limit"
        );
    }
}

/// A connected loopback pair, returning the client socket and the server end,
/// which must be kept alive for the connection to stay up.
#[cfg(target_pointer_width = "64")]
fn connected_pair() -> (OwnedSocket, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let client = TcpStream::connect(addr).expect("connect");
    let (server, _peer) = listener.accept().expect("accept");
    (OwnedSocket::from(client), server)
}

/// The submitting path validates before building the operation rather than
/// inside its submission closure.
#[cfg(target_pointer_width = "64")]
#[test]
fn submitted_recv_rejects_an_oversized_length() {
    let (client, _server) = connected_pair();
    let port = CompletionPort::new(0).expect("create port");
    let socket = port.associate_socket(client, 0).expect("associate socket");

    let error = socket
        .recv(crate::buf::OversizedBuffer)
        .expect_err("an unrepresentable length must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the request should be rejected before reaching Winsock: {error}"
    );
}
