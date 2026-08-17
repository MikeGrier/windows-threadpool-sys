// Copyright (c) 2026 Mike Grier
//! Safe socket operation adapters, gated behind the `socket` feature.
//!
//! These wrappers own the I/O buffer and the `WSABUF` describing it, issue the
//! single native `WSARecv` / `WSASend` internally, and route completions through
//! the same [`CompletionPort`] as the handle backends. A socket is owned as a
//! [`OwnedSocket`] (its destructor is `closesocket`), so it gets its own
//! endpoint type rather than reusing the handle-based `AssociatedEndpoint`.
//!
//! The caller provides a connected, overlapped-capable socket -- every
//! `std::net` socket qualifies, and owning one keeps Winsock initialized -- so
//! the crate never calls `WSAStartup` itself.

use std::io;
use std::os::windows::io::{AsRawSocket, AsSocket, BorrowedSocket, OwnedSocket};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Networking::WinSock::{
    SOCKET, WSA_INVALID_EVENT, WSA_IO_PENDING, WSABUF, WSACloseEvent, WSACreateEvent, WSAEVENT,
    WSAGetOverlappedResult, WSARecv, WSASend,
};
use windows_sys::Win32::System::IO::{CancelIoEx, CreateIoCompletionPort, OVERLAPPED};

use crate::operation::payload_ptr_from_overlapped;
use crate::{Completion, CompletionPort, Issued, Operation, OperationId, Submitted};

impl CompletionPort {
    /// Associate an overlapped socket with this port under `key`.
    ///
    /// Completions for operations issued on the socket are delivered to this port
    /// and tagged with `key`. The socket must be overlapped-capable, which every
    /// `std::net` socket and any `WSASocket` created with `WSA_FLAG_OVERLAPPED`
    /// is; the association is permanent for the life of the socket.
    ///
    /// # Errors
    ///
    /// Returns any error from `CreateIoCompletionPort`.
    pub fn associate_socket(
        &self,
        socket: OwnedSocket,
        key: usize,
    ) -> io::Result<AssociatedSocket<'_>> {
        // SAFETY: associating a valid socket handle with a valid port; the
        // concurrency argument is ignored when an existing port is supplied.
        let result = unsafe {
            CreateIoCompletionPort(
                socket.as_raw_socket() as usize as HANDLE,
                self.raw(),
                key,
                0,
            )
        };
        if result.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(AssociatedSocket {
            port: self,
            socket,
            key,
        })
    }
}

/// An overlapped socket bound to exactly one [`CompletionPort`].
///
/// The endpoint owns its socket (closed with `closesocket` on drop) and borrows
/// the port it is associated with. It is intentionally not `Clone`.
#[derive(Debug)]
pub struct AssociatedSocket<'port> {
    port: &'port CompletionPort,
    socket: OwnedSocket,
    key: usize,
}

impl<'port> AssociatedSocket<'port> {
    /// Borrow the underlying socket.
    #[must_use]
    pub fn socket(&self) -> BorrowedSocket<'_> {
        self.socket.as_socket()
    }

    /// The completion key packets from this socket are tagged with.
    #[must_use]
    pub fn key(&self) -> usize {
        self.key
    }

    /// The completion port this socket is associated with.
    #[must_use]
    pub fn port(&self) -> &'port CompletionPort {
        self.port
    }

    fn raw_socket(&self) -> SOCKET {
        self.socket.as_raw_socket() as usize
    }

    /// Submit an overlapped receive of up to `len` bytes, returning a
    /// [`SocketIo`] token that recovers the buffer and byte count from the
    /// operation's completion.
    ///
    /// The socket must not be in a skip-on-success completion mode; this adapter
    /// always expects a completion packet.
    ///
    /// # Errors
    ///
    /// Returns any immediate failure from issuing the receive.
    #[track_caller]
    pub fn recv(&self, len: usize) -> io::Result<SocketIo> {
        let socket = self.raw_socket();
        let operation = Operation::new(recv_payload(len));
        // SAFETY: issues exactly one WSARecv into the payload's buffer via its
        // WSABUF and flags word, both reached through the pinned OVERLAPPED; they
        // live until the completion is claimed.
        let submitted = unsafe {
            self.port.submit_with(operation, |overlapped| {
                let payload = payload_ptr_from_overlapped::<SocketPayload>(overlapped);
                let ret = WSARecv(
                    socket,
                    std::ptr::addr_of!((*payload).wsabuf),
                    1,
                    std::ptr::null_mut(),
                    std::ptr::addr_of_mut!((*payload).flags),
                    overlapped,
                    None,
                );
                classify_socket(ret)
            })
        };
        finish_socket(submitted)
    }

    /// Submit an overlapped send of `data`, returning a [`SocketIo`] token that
    /// recovers the buffer and byte count from the operation's completion.
    ///
    /// The socket must not be in a skip-on-success completion mode.
    ///
    /// # Errors
    ///
    /// Returns any immediate failure from issuing the send.
    #[track_caller]
    pub fn send(&self, data: Vec<u8>) -> io::Result<SocketIo> {
        let socket = self.raw_socket();
        let operation = Operation::new(send_payload(data));
        // SAFETY: issues exactly one WSASend from the payload's buffer via its
        // WSABUF, reached through the pinned OVERLAPPED; it lives until the
        // completion is claimed.
        let submitted = unsafe {
            self.port.submit_with(operation, |overlapped| {
                let payload = payload_ptr_from_overlapped::<SocketPayload>(overlapped);
                let ret = WSASend(
                    socket,
                    std::ptr::addr_of!((*payload).wsabuf),
                    1,
                    std::ptr::null_mut(),
                    0,
                    overlapped,
                    None,
                );
                classify_socket(ret)
            })
        };
        finish_socket(submitted)
    }

    /// Request cancellation of a single outstanding operation on this socket.
    ///
    /// # Errors
    ///
    /// Returns any error from `CancelIoEx`.
    pub fn cancel(&self, id: OperationId) -> io::Result<()> {
        // SAFETY: cancelling by a valid socket handle and an OVERLAPPED identity.
        let ok = unsafe { CancelIoEx(self.raw_socket() as HANDLE, id.as_ptr()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Request cancellation of every outstanding operation on this socket.
    ///
    /// # Errors
    ///
    /// Returns any error from `CancelIoEx`.
    pub fn cancel_all(&self) -> io::Result<()> {
        // SAFETY: a null OVERLAPPED cancels all operations on the socket handle.
        let ok = unsafe { CancelIoEx(self.raw_socket() as HANDLE, std::ptr::null()) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// The pinned payload for an in-flight socket operation: the buffer, the
/// `WSABUF` pointing into it, and the receive `flags` word.
struct SocketPayload {
    buffer: Vec<u8>,
    wsabuf: WSABUF,
    flags: u32,
}

// SAFETY: `wsabuf.buf` points into `buffer`, which this payload owns; moving the
// payload moves the whole self-referential unit, and it exposes no aliasing
// access, so it is `Send` like the `Vec<u8>` it wraps.
unsafe impl Send for SocketPayload {}

fn recv_payload(len: usize) -> SocketPayload {
    let mut buffer = vec![0_u8; len];
    let wsabuf = WSABUF {
        len: clamp_u32(len),
        buf: buffer.as_mut_ptr(),
    };
    SocketPayload {
        buffer,
        wsabuf,
        flags: 0,
    }
}

fn send_payload(mut data: Vec<u8>) -> SocketPayload {
    let wsabuf = WSABUF {
        len: clamp_u32(data.len()),
        buf: data.as_mut_ptr(),
    };
    SocketPayload {
        buffer: data,
        wsabuf,
        flags: 0,
    }
}

/// Map a Winsock call's return value into the submission contract, expecting a
/// completion packet on success because the adapter never enables a skip mode.
fn classify_socket(ret: i32) -> io::Result<Issued> {
    if ret == 0 {
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(WSA_IO_PENDING) {
        Ok(Issued::Pending)
    } else {
        Err(error)
    }
}

/// Turn a socket submission outcome into a token or an immediate error.
fn finish_socket(submitted: Submitted<SocketPayload>) -> io::Result<SocketIo> {
    match submitted {
        Submitted::Pending(id) => Ok(SocketIo { id }),
        Submitted::Completed { .. } => Err(io::Error::other(
            "socket adapter observed a synchronous completion; the socket must not be in a \
             skip-on-success completion mode",
        )),
        Submitted::Failed { error, .. } => Err(error),
    }
}

/// Clamp a length to the `u32` byte-count field the Winsock calls take.
fn clamp_u32(len: usize) -> u32 {
    u32::try_from(len).unwrap_or(u32::MAX)
}

/// A pending socket operation submitted through [`AssociatedSocket::recv`] or
/// [`AssociatedSocket::send`].
///
/// The token carries the operation's identity and its `Vec<u8>` payload type, so
/// [`SocketIo::claim`] recovers the buffer and byte count safely once the
/// matching completion is dequeued.
#[derive(Debug)]
pub struct SocketIo {
    id: OperationId,
}

impl SocketIo {
    /// The identity of the in-flight operation, for cancellation or matching.
    #[must_use]
    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Claim this operation's result from `completion`.
    ///
    /// On a match returns `Ok((buffer, result))`: `buffer` is the payload -- the
    /// bytes received (valid up to the byte count), or the data sent -- and
    /// `result` is the byte count or the operation's error. Returns `Err(self)`
    /// when `completion` belongs to a different operation.
    pub fn claim(self, completion: &Completion) -> Result<(Vec<u8>, io::Result<usize>), Self> {
        if completion.overlapped_ptr() != self.id.as_ptr() {
            return Err(self);
        }
        // SAFETY: the identity match proves this completion is the
        // Operation<SocketPayload> this token submitted; claim it exactly once.
        let operation = unsafe { completion.claim::<SocketPayload>() };
        let buffer = operation.into_payload().buffer;
        let result = match completion.error() {
            Some(error) => Err(io::Error::from_raw_os_error(
                error.raw_os_error().unwrap_or_default(),
            )),
            None => Ok(completion.bytes_transferred() as usize),
        };
        Ok((buffer, result))
    }
}

/// A connected overlapped socket that completes operations synchronously, one at
/// a time, via a Winsock completion event.
///
/// This is the socket counterpart of the handle blocking backend. It cannot use
/// `GetOverlappedResult` on the socket handle, so each call creates a
/// `WSACreateEvent`, issues the operation with that event in `OVERLAPPED.hEvent`,
/// and blocks on `WSAGetOverlappedResult`.
#[derive(Debug)]
pub struct BlockingSocket {
    socket: OwnedSocket,
}

impl BlockingSocket {
    /// Take ownership of a connected overlapped socket for synchronous
    /// completion.
    #[must_use]
    pub fn new(socket: OwnedSocket) -> Self {
        Self { socket }
    }

    /// Borrow the underlying socket.
    #[must_use]
    pub fn socket(&self) -> BorrowedSocket<'_> {
        self.socket.as_socket()
    }

    fn raw_socket(&self) -> SOCKET {
        self.socket.as_raw_socket() as usize
    }

    /// Receive up to `len` bytes, blocking until the receive completes.
    ///
    /// Returns the buffer truncated to the bytes received and that count.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the receive.
    pub fn recv(&self, len: usize) -> io::Result<(Vec<u8>, usize)> {
        let mut buffer = vec![0_u8; len];
        let wsabuf = WSABUF {
            len: clamp_u32(len),
            buf: buffer.as_mut_ptr(),
        };
        // SAFETY: issues exactly one WSARecv into `buffer` via `wsabuf`, both of
        // which stay valid for the whole blocking call.
        let received = unsafe {
            self.run(|socket, overlapped| {
                let mut flags = 0_u32;
                WSARecv(
                    socket,
                    &wsabuf,
                    1,
                    std::ptr::null_mut(),
                    &mut flags,
                    overlapped,
                    None,
                )
            })
        }?;
        buffer.truncate(received);
        Ok((buffer, received))
    }

    /// Send `data`, blocking until the send completes, and return the bytes sent.
    ///
    /// # Errors
    ///
    /// Returns any error from issuing or completing the send.
    pub fn send(&self, data: &[u8]) -> io::Result<usize> {
        let wsabuf = WSABUF {
            len: clamp_u32(data.len()),
            buf: data.as_ptr().cast_mut(),
        };
        // SAFETY: issues exactly one WSASend from `data` via `wsabuf`, both of
        // which stay valid for the whole blocking call; WSASend does not write
        // through the buffer pointer.
        unsafe {
            self.run(|socket, overlapped| {
                WSASend(
                    socket,
                    &wsabuf,
                    1,
                    std::ptr::null_mut(),
                    0,
                    overlapped,
                    None,
                )
            })
        }
    }

    /// Issue one overlapped socket operation with a completion event and block on
    /// `WSAGetOverlappedResult` until it finishes, returning the bytes transferred.
    ///
    /// # Safety
    ///
    /// `issue` must start exactly one overlapped operation using the provided
    /// socket and `OVERLAPPED`, with any buffers valid for the whole call.
    unsafe fn run<F>(&self, issue: F) -> io::Result<usize>
    where
        F: FnOnce(SOCKET, *mut OVERLAPPED) -> i32,
    {
        let socket = self.raw_socket();
        // SAFETY: creates a manual-reset Winsock event; the null handle
        // `WSA_INVALID_EVENT` signals failure.
        let event: WSAEVENT = unsafe { WSACreateEvent() };
        if event == WSA_INVALID_EVENT {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `OVERLAPPED` is plain data; a zeroed value with the event in
        // `hEvent` is the documented way to wait for a single operation.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event as HANDLE;

        let ret = issue(socket, &mut overlapped);
        if ret != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(WSA_IO_PENDING) {
                // SAFETY: `event` was created above and is closed exactly once.
                unsafe { WSACloseEvent(event) };
                return Err(error);
            }
        }

        let mut transferred = 0_u32;
        let mut flags = 0_u32;
        // SAFETY: waits on `overlapped`'s event for this one operation to finish.
        let ok = unsafe {
            WSAGetOverlappedResult(
                socket,
                &overlapped,
                &mut transferred,
                i32::from(true),
                &mut flags,
            )
        };
        let result = if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(transferred as usize)
        };
        // SAFETY: `event` was created above and is closed exactly once here.
        unsafe { WSACloseEvent(event) };
        result
    }
}

#[cfg(test)]
mod tests;
