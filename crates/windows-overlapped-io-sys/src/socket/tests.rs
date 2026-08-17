// Copyright (c) 2026 Mike Grier
use crate::CompletionPort;
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
    let recv_token = endpoint.recv(64).expect("submit recv");
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
    let send_token = endpoint.send(b"pong".to_vec()).expect("submit send");
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
