// Copyright (c) 2026 Mike Grier
//! Integration test (`socket` feature): a loopback TCP send-and-receive
//! round-trip through the IOCP socket adapter, with no `unsafe` in the test's
//! I/O path. Payloads stay under the default socket buffer so each direction can
//! be driven sequentially without a concurrent reader.

#![cfg(all(windows, feature = "socket"))]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::windows::io::OwnedSocket;

use windows_overlapped_io_sys::{AssociatedSocket, BlockingSocket, CompletionPort};

const PAYLOAD: usize = 8192;

fn send_all(endpoint: &AssociatedSocket<'_>, port: &CompletionPort, data: &[u8]) {
    let mut sent = 0;
    while sent < data.len() {
        let token = endpoint
            .send(data[sent..].to_vec())
            .expect("submit send")
            .expect_pending("this socket is not in skip-on-success mode");
        let completion = port.get(5_000).expect("get").expect("send completion");
        let (_data, result) = match token.claim(&completion) {
            Ok(pair) => pair,
            Err(_) => panic!("send completion did not match its token"),
        };
        let n = result.expect("send result");
        assert!(n > 0, "send made no progress");
        sent += n;
    }
}

fn recv_exact(endpoint: &AssociatedSocket<'_>, port: &CompletionPort, total: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(total);
    while out.len() < total {
        let token = endpoint
            .recv(total - out.len())
            .expect("submit recv")
            .expect_pending("this socket is not in skip-on-success mode");
        let completion = port.get(5_000).expect("get").expect("recv completion");
        let (mut buffer, result) = match token.claim(&completion) {
            Ok(pair) => pair,
            Err(_) => panic!("recv completion did not match its token"),
        };
        let n = result.expect("recv result");
        assert!(n > 0, "peer closed before sending all bytes");
        buffer.truncate(n);
        out.extend_from_slice(&buffer);
    }
    out
}

#[test]
fn socket_adapter_round_trips_over_loopback_tcp() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let client = TcpStream::connect(addr).expect("connect");
    let (mut server, _peer) = listener.accept().expect("accept");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate_socket(OwnedSocket::from(client), 0)
        .expect("associate_socket");

    let outbound: Vec<u8> = (0..PAYLOAD).map(|i| (i % 251) as u8).collect();
    let inbound: Vec<u8> = (0..PAYLOAD).map(|i| (i % 241) as u8).collect();

    // Our side sends; the peer reads it back and verifies.
    send_all(&endpoint, &port, &outbound);
    let mut peer_got = vec![0_u8; PAYLOAD];
    server.read_exact(&mut peer_got).expect("peer read");
    assert_eq!(peer_got, outbound);
    assert_eq!(port.outstanding(), 0);

    // The peer sends; our side receives and verifies.
    server.write_all(&inbound).expect("peer write");
    let got = recv_exact(&endpoint, &port, PAYLOAD);
    assert_eq!(got, inbound);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
}

fn blocking_send_all(socket: &BlockingSocket, data: &[u8]) {
    let mut sent = 0;
    while sent < data.len() {
        let n = socket.send(&data[sent..]).expect("blocking send");
        assert!(n > 0, "send made no progress");
        sent += n;
    }
}

fn blocking_recv_exact(socket: &BlockingSocket, total: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(total);
    while out.len() < total {
        let (buffer, n) = socket.recv(total - out.len()).expect("blocking recv");
        assert!(n > 0, "peer closed before sending all bytes");
        out.extend_from_slice(&buffer);
    }
    out
}

#[test]
fn blocking_socket_round_trips_over_loopback_tcp() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let client = TcpStream::connect(addr).expect("connect");
    let (mut server, _peer) = listener.accept().expect("accept");

    let socket = BlockingSocket::new(OwnedSocket::from(client));

    let outbound: Vec<u8> = (0..PAYLOAD).map(|i| (i % 251) as u8).collect();
    let inbound: Vec<u8> = (0..PAYLOAD).map(|i| (i % 241) as u8).collect();

    // Our side sends; the peer reads it back and verifies.
    blocking_send_all(&socket, &outbound);
    let mut peer_got = vec![0_u8; PAYLOAD];
    server.read_exact(&mut peer_got).expect("peer read");
    assert_eq!(peer_got, outbound);

    // The peer sends (fits the receive buffer), then our side receives it.
    server.write_all(&inbound).expect("peer write");
    let got = blocking_recv_exact(&socket, PAYLOAD);
    assert_eq!(got, inbound);
}
