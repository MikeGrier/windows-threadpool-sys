// Copyright (c) 2026 Mike Grier
//! Integration test (`socket` feature): `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`
//! end to end on a socket, which M12.2 made reachable for the first time.
//!
//! The shape mirrors `skip_on_success_adapters.rs`: whether a given request
//! completes synchronously is Winsock's call, not something a caller can
//! compel, so the skip-mode test asserts the invariants that must hold for
//! whichever arm it observes. The default-mode socket, where `Pending` *is*
//! guaranteed, is asserted exactly -- that contrast is what gives the tolerant
//! test its meaning.

#![cfg(all(windows, feature = "socket"))]

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::os::windows::io::OwnedSocket;

use windows_overlapped_io_sys::{CompletionPort, NotificationModes, Started};

/// How long to wait for a packet that should not exist. Long enough that a
/// queued packet would have arrived, short enough not to stall the suite.
const NO_PACKET_TIMEOUT_MS: u32 = 250;

fn skip_on_success() -> NotificationModes {
    NotificationModes {
        skip_completion_port_on_success: true,
        ..NotificationModes::default()
    }
}

/// A connected loopback pair. The server end must stay alive for the connection
/// to stay up, so it is returned rather than dropped.
fn connected_pair() -> (OwnedSocket, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let client = TcpStream::connect(addr).expect("connect");
    let (server, _peer) = listener.accept().expect("accept");
    (OwnedSocket::from(client), server)
}

#[test]
fn a_send_on_a_skip_socket_reports_whichever_arm_winsock_chose() {
    let (client, mut server) = connected_pair();
    let port = CompletionPort::new(0).expect("create port");
    let mut socket = port.associate_socket(client, 0).expect("associate socket");
    socket
        .set_notification_modes(skip_on_success())
        .expect("the base Winsock provider returns IFS handles");

    let data = b"skip-on-success socket send".to_vec();
    let expected = data.clone();

    let (payload, sent) = match socket.send(data).expect("submit send") {
        Started::Completed {
            payload,
            bytes_transferred,
        } => {
            // The whole point of the mode: reclaimed inline, so rundown is not
            // waiting on anything and no packet was queued.
            assert_eq!(port.outstanding(), 0);
            assert!(
                port.get(NO_PACKET_TIMEOUT_MS).expect("get").is_none(),
                "skip-on-success queued a packet for a synchronous success"
            );
            (payload, bytes_transferred)
        }
        Started::Pending(token) => {
            let completion = port.get(5_000).expect("get").expect("a completion");
            let (payload, result) = token.claim(&completion).expect("token matches");
            (payload, result.expect("send result"))
        }
    };

    assert_eq!(sent, expected.len());
    assert_eq!(payload, expected, "the send buffer comes back either way");

    let mut got = vec![0_u8; expected.len()];
    server.read_exact(&mut got).expect("peer read");
    assert_eq!(got, expected, "the bytes really went out on the wire");
    assert_eq!(port.outstanding(), 0);

    drop(socket);
}

#[test]
fn the_same_send_on_a_default_socket_is_always_pending() {
    // Without the mode, an immediate success still queues a packet, so the
    // adapter can only ever report `Pending` -- this arm is exact, not tolerant.
    let (client, mut server) = connected_pair();
    let port = CompletionPort::new(0).expect("create port");
    let socket = port.associate_socket(client, 0).expect("associate socket");

    let data = b"default socket send".to_vec();
    let expected = data.clone();

    let started = socket.send(data).expect("submit send");
    assert!(
        started.is_pending(),
        "a socket in the default mode always gets a completion packet"
    );

    let token = started.expect_pending("just asserted pending");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (payload, result) = token.claim(&completion).expect("token matches");
    assert_eq!(result.expect("send result"), expected.len());
    assert_eq!(payload, expected);

    let mut got = vec![0_u8; expected.len()];
    server.read_exact(&mut got).expect("peer read");
    assert_eq!(got, expected);
    assert_eq!(port.outstanding(), 0);

    drop(socket);
}

/// A receive with no data waiting cannot complete synchronously, so even in
/// skip mode it must go through the packet -- the mode changes what happens on
/// synchronous success, never what happens when a request genuinely blocks.
#[test]
fn a_receive_with_nothing_waiting_is_pending_even_in_skip_mode() {
    use std::io::Write;

    let (client, mut server) = connected_pair();
    let port = CompletionPort::new(0).expect("create port");
    let mut socket = port.associate_socket(client, 0).expect("associate socket");
    socket
        .set_notification_modes(skip_on_success())
        .expect("set skip-on-success");

    let started = socket.recv(vec![0_u8; 64]).expect("submit recv");
    assert!(
        started.is_pending(),
        "nothing has been sent yet, so the receive cannot have completed"
    );

    let token = started.expect_pending("just asserted pending");
    server.write_all(b"ping").expect("peer write");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (mut buffer, result) = token.claim(&completion).expect("token matches");
    let received = result.expect("recv result");
    buffer.truncate(received);
    assert_eq!(buffer, b"ping");
    assert_eq!(port.outstanding(), 0);

    drop(socket);
}

/// The byte count on the synchronous arm comes from `WSASend`'s own
/// out-parameter, which M12.2 wired up -- it was previously passed as null, so
/// a skip-mode socket would have had no count to report at all.
///
/// This is the one test that *requires* the synchronous arm, and deliberately
/// so: without it the whole file would still pass if the mode silently stopped
/// taking effect. Requiring it is defensible here in a way it is not for a file
/// read, where the I/O Manager's caching genuinely decides -- a small send into
/// a freshly connected loopback socket has room in the send buffer and is
/// copied inline. If this ever starts failing, the message is not "flaky test"
/// but "skip-on-success is no longer being applied to this socket."
#[test]
fn the_synchronous_arm_reports_the_full_byte_count() {
    let (client, mut server) = connected_pair();
    let port = CompletionPort::new(0).expect("create port");
    let mut socket = port.associate_socket(client, 0).expect("associate socket");
    socket
        .set_notification_modes(skip_on_success())
        .expect("set skip-on-success");

    let mut synchronous = 0_usize;
    for size in [1_usize, 16, 256, 1024] {
        let data = vec![b'x'; size];
        match socket.send(data).expect("submit send") {
            Started::Completed {
                bytes_transferred, ..
            } => {
                synchronous += 1;
                assert_eq!(
                    bytes_transferred, size,
                    "a synchronous send reports the count Winsock wrote, not zero"
                );
                assert_eq!(port.outstanding(), 0);
            }
            Started::Pending(token) => {
                let completion = port.get(5_000).expect("get").expect("a completion");
                let (_payload, result) = token.claim(&completion).expect("token matches");
                assert_eq!(result.expect("send result"), size);
            }
        }
        let mut got = vec![0_u8; size];
        server.read_exact(&mut got).expect("peer read");
        assert!(got.iter().all(|byte| *byte == b'x'));
    }

    assert!(
        synchronous > 0,
        "no send took the synchronous arm, so skip-on-success never applied and \
         the rest of this file proved nothing"
    );
    assert_eq!(port.outstanding(), 0);
    drop(socket);
}
