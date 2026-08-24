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
    SO_PROTOCOL_INFOW, SOCKET, SOCKET_ERROR, SOL_SOCKET, WSA_INVALID_EVENT, WSA_IO_PENDING, WSABUF,
    WSACloseEvent, WSACreateEvent, WSAEVENT, WSAGetOverlappedResult, WSAPROTOCOL_INFOW, WSARecv,
    WSASend, XP1_IFS_HANDLES, getsockopt,
};
use windows_sys::Win32::System::IO::{CancelIoEx, CreateIoCompletionPort, OVERLAPPED};

use crate::endpoint::notification_flags;
use crate::operation::{payload_ptr_from_overlapped, sync_bytes_ptr_from_overlapped};
use crate::{
    Completion, CompletionPort, IoBuf, IoBufMut, Issued, NotificationModes, Operation, OperationId,
    Started, Submitted,
};

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
            modes: NotificationModes::default(),
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
    /// What [`AssociatedSocket::set_notification_modes`] has established.
    ///
    /// Read at every submission, because it decides whether a synchronous
    /// success will be followed by a completion packet.
    modes: NotificationModes,
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

    /// The completion-notification modes established on this socket.
    #[must_use]
    pub fn notification_modes(&self) -> NotificationModes {
        self.modes
    }

    fn raw_socket(&self) -> SOCKET {
        self.socket.as_raw_socket() as usize
    }

    /// Set this socket's completion-notification modes, after checking that its
    /// provider actually supports them.
    ///
    /// The handle side declares its modes *before* association, on
    /// [`UnassociatedEndpoint::set_notification_modes`](crate::UnassociatedEndpoint::set_notification_modes),
    /// because there the mode is part of an endpoint's provenance. A socket has
    /// no unassociated stage to hang that on, so it declares here instead.
    /// Setting after association is still safe: the flag only takes effect at
    /// I/O time, and `recv`/`send` take `&self`, so a caller sets the mode once
    /// against `&mut self` and then submits freely.
    ///
    /// Passing every field `false` is a no-op call, not a reset. **A mode cannot
    /// be removed once set** -- a Win32 property of the handle, not a limitation
    /// of this wrapper -- so a second call can only ever add modes.
    ///
    /// # The capability probe
    ///
    /// Win32 restricts [`NotificationModes::skip_completion_port_on_success`] on
    /// a socket to Layered Service Providers that return IFS handles, and a
    /// socket wrongly put in that mode reports [`Started::Pending`] for an
    /// operation whose packet was suppressed -- leaving it outstanding forever
    /// and wedging [`CompletionPort::run_down`]. So this asks first, reading
    /// *this* socket's own `WSAPROTOCOL_INFOW` via `SO_PROTOCOL_INFOW` and
    /// requiring `XP1_IFS_HANDLES`. That is narrower and more accurate than the
    /// `WSAEnumProtocols` sweep the flag's documentation suggests: it asks about
    /// the provider that actually created this socket, not about every LSP
    /// installed on the machine.
    ///
    /// `skip_set_event_on_handle` carries no such restriction and is not probed.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::Unsupported`] if skip-on-success was requested
    /// and this socket's provider does not return IFS handles, or any error from
    /// `getsockopt` or `SetFileCompletionNotificationModes`.
    pub fn set_notification_modes(&mut self, modes: NotificationModes) -> io::Result<()> {
        if modes.skip_completion_port_on_success {
            require_ifs_handles(self.provider_service_flags()?)?;
        }

        let mut flags = 0_u8;
        if modes.skip_completion_port_on_success {
            flags |= notification_flags::SKIP_COMPLETION_PORT_ON_SUCCESS;
        }
        if modes.skip_set_event_on_handle {
            flags |= notification_flags::SKIP_SET_EVENT_ON_HANDLE;
        }
        // SAFETY: a live socket this endpoint owns -- a socket handle is a
        // kernel handle, which is why this file-named call accepts one -- and a
        // flags byte built only from the two documented bits. The call sets a
        // handle attribute and starts no I/O.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::SetFileCompletionNotificationModes(
                self.raw_socket() as HANDLE,
                flags,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // Accumulated, never replaced: Win32 cannot clear a mode, so what this
        // socket records has to be the union of everything ever set on it.
        self.modes.skip_completion_port_on_success |= modes.skip_completion_port_on_success;
        self.modes.skip_set_event_on_handle |= modes.skip_set_event_on_handle;
        Ok(())
    }

    /// The `dwServiceFlags1` word of the provider that created this socket.
    fn provider_service_flags(&self) -> io::Result<u32> {
        let mut info = std::mem::MaybeUninit::<WSAPROTOCOL_INFOW>::uninit();
        let mut len = i32::try_from(size_of::<WSAPROTOCOL_INFOW>())
            .expect("WSAPROTOCOL_INFOW is far smaller than i32::MAX");
        // SAFETY: a live socket, a documented option pair, and an output buffer
        // exactly `len` bytes long that Winsock fills before returning success.
        let ret = unsafe {
            getsockopt(
                self.raw_socket(),
                SOL_SOCKET,
                SO_PROTOCOL_INFOW,
                info.as_mut_ptr().cast(),
                &raw mut len,
            )
        };
        if ret == SOCKET_ERROR {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `getsockopt` reported success, so it wrote the whole struct.
        Ok(unsafe { info.assume_init() }.dwServiceFlags1)
    }

    /// Submit an overlapped receive into `buffer`.
    ///
    /// The buffer is any owned [`IoBufMut`] -- handed over for the operation's
    /// life and returned when it completes, with nothing copied and nothing
    /// allocated here.
    ///
    /// Returns [`Started::Pending`] with a [`SocketIo`] token that recovers the
    /// buffer and byte count from the operation's completion, or -- only on a
    /// socket in a skip-on-success completion mode, where a synchronous success
    /// queues no packet -- [`Started::Completed`] with the buffer already in
    /// hand.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the buffer is longer than
    /// `u32::MAX`, which `WSABUF`'s byte count cannot express, or any immediate
    /// failure from issuing the receive.
    #[track_caller]
    pub fn recv<B: IoBufMut>(&self, buffer: B) -> io::Result<Started<SocketIo<B>, B>> {
        let socket = self.raw_socket();
        let skip = self.modes.skip_completion_port_on_success;
        let operation = Operation::new(recv_payload(buffer)?);
        // SAFETY: issues exactly one WSARecv into the payload's buffer via its
        // WSABUF and flags word, both reached through the pinned OVERLAPPED; they
        // and the byte-count cell live until the completion is claimed.
        let submitted = unsafe {
            self.port.submit_with(operation, |overlapped| {
                let payload = payload_ptr_from_overlapped::<SocketPayload<B>>(overlapped);
                let bytes = sync_bytes_ptr_from_overlapped(overlapped);
                let ret = WSARecv(
                    socket,
                    std::ptr::addr_of!((*payload).wsabuf),
                    1,
                    bytes,
                    std::ptr::addr_of_mut!((*payload).flags),
                    overlapped,
                    None,
                );
                classify_socket(ret, skip, bytes)
            })
        };
        finish_socket(submitted)
    }

    /// Submit an overlapped send of `buffer`.
    ///
    /// The buffer is any owned [`IoBuf`] -- including a shared `Arc<[u8]>` or a
    /// `&'static [u8]` -- handed over for the operation's life and returned when
    /// it completes. Nothing is copied.
    ///
    /// Returns [`Started::Pending`] with a [`SocketIo`] token, or
    /// [`Started::Completed`] with the buffer already in hand when the socket is
    /// in a skip-on-success completion mode and the send completed
    /// synchronously.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if the buffer is longer than
    /// `u32::MAX`, which `WSABUF`'s byte count cannot express, or any immediate
    /// failure from issuing the send.
    #[track_caller]
    pub fn send<B: IoBuf>(&self, buffer: B) -> io::Result<Started<SocketIo<B>, B>> {
        let socket = self.raw_socket();
        let skip = self.modes.skip_completion_port_on_success;
        let operation = Operation::new(send_payload(buffer)?);
        // SAFETY: issues exactly one WSASend from the payload's buffer via its
        // WSABUF, reached through the pinned OVERLAPPED; it and the byte-count
        // cell live until the completion is claimed.
        let submitted = unsafe {
            self.port.submit_with(operation, |overlapped| {
                let payload = payload_ptr_from_overlapped::<SocketPayload<B>>(overlapped);
                let bytes = sync_bytes_ptr_from_overlapped(overlapped);
                let ret = WSASend(
                    socket,
                    std::ptr::addr_of!((*payload).wsabuf),
                    1,
                    bytes,
                    0,
                    overlapped,
                    None,
                );
                classify_socket(ret, skip, bytes)
            })
        };
        finish_socket(submitted)
    }

    /// Request cancellation of a single outstanding operation on this socket.
    ///
    /// The identity is validated against the port's live operations, and the
    /// native cancellation happens under the same guard, so an identity retained
    /// past its operation's completion cannot reach a later operation that was
    /// given the same storage address.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::NotFound`] if `id` no longer names a live
    /// operation, or any error from `CancelIoEx`.
    pub fn cancel(&self, id: OperationId) -> io::Result<()> {
        // Socket cancellation goes through the same registry as file
        // cancellation; routing around it would leave the identity guarantee
        // holding for one endpoint kind and not the other.
        self.port.live_operations().cancel_if_live(id, || {
            // SAFETY: cancelling by a valid socket handle and an OVERLAPPED
            // identity the registry has confirmed still names a live operation,
            // and which cannot be reissued while the guard is held.
            let ok = unsafe { CancelIoEx(self.raw_socket() as HANDLE, id.as_ptr()) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })
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
struct SocketPayload<B> {
    buffer: B,
    wsabuf: WSABUF,
    flags: u32,
}

// SAFETY: `wsabuf.buf` points into `buffer`, which this payload owns; moving the
// payload moves the whole self-referential unit -- sound because `IoBuf` promises
// the buffer's address does not move with it -- and it exposes no aliasing
// access, so it is `Send` whenever the buffer is.
unsafe impl<B: Send> Send for SocketPayload<B> {}

fn recv_payload<B: IoBufMut>(mut buffer: B) -> io::Result<SocketPayload<B>> {
    let wsalen = checked_len(buffer.bytes_len(), "receive buffer")?;
    let wsabuf = WSABUF {
        len: wsalen,
        buf: buffer.stable_mut_ptr(),
    };
    Ok(SocketPayload {
        buffer,
        wsabuf,
        flags: 0,
    })
}

fn send_payload<B: IoBuf>(buffer: B) -> io::Result<SocketPayload<B>> {
    let wsabuf = WSABUF {
        len: checked_len(buffer.bytes_len(), "send buffer")?,
        // `WSABUF` is one type for both directions, so its `buf` is `*mut` even
        // for a send. `WSASend` only reads through it, which is what makes this
        // sound for a shared buffer whose pointer carries no write provenance.
        buf: buffer.stable_ptr().cast_mut(),
    };
    Ok(SocketPayload {
        buffer,
        wsabuf,
        flags: 0,
    })
}

/// Decide whether a provider's `dwServiceFlags1` permits skip-on-success.
///
/// Split from the `getsockopt` that reads the word so the refusal can be tested
/// directly: every base Winsock provider on a stock Windows returns IFS handles,
/// so the failing arm is otherwise unreachable without installing a Layered
/// Service Provider.
///
/// # Errors
///
/// Returns [`io::ErrorKind::Unsupported`] -- deliberately not a Win32 error,
/// because nothing failed: the question was asked and answered.
fn require_ifs_handles(service_flags1: u32) -> io::Result<()> {
    if service_flags1 & XP1_IFS_HANDLES == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this socket's provider does not return IFS handles, so Win32 does not support \
             FILE_SKIP_COMPLETION_PORT_ON_SUCCESS on it",
        ));
    }
    Ok(())
}

/// Map a Winsock call's return value into the submission contract.
///
/// [`Issued`] records whether a **completion packet will arrive**, not whether
/// the call finished synchronously. For an IOCP-bound overlapped socket those
/// are different facts: a packet is queued for every completed request,
/// *including* one that succeeded immediately without `WSA_IO_PENDING`.
///
/// The single exception is `skip_on_success`, which is why this needs to know
/// it: on a socket put into `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode by
/// [`AssociatedSocket::set_notification_modes`] no packet is queued for an
/// immediate success, so that -- and only that -- is an [`Issued::Completed`].
/// Getting this backwards in either direction is a bug with teeth: claiming
/// `Completed` when a packet is coming frees the operation under a live
/// `OVERLAPPED`, and claiming `Pending` when none is coming leaves the operation
/// outstanding forever and wedges [`CompletionPort::run_down`].
///
/// # Safety
///
/// `sync_bytes` must be the byte-count cell of the operation being submitted,
/// which is live for the whole call.
unsafe fn classify_socket(
    ret: i32,
    skip_on_success: bool,
    sync_bytes: *mut u32,
) -> io::Result<Issued> {
    if ret == 0 {
        if skip_on_success {
            // SAFETY: the call reported immediate success, so Winsock has
            // already written the count and will not write it again.
            let bytes_transferred = unsafe { *sync_bytes };
            return Ok(Issued::Completed { bytes_transferred });
        }
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(WSA_IO_PENDING) {
        Ok(Issued::Pending)
    } else {
        Err(error)
    }
}

/// Turn a socket submission outcome into the adapter's two-state outcome.
fn finish_socket<B: IoBuf>(
    submitted: Submitted<SocketPayload<B>>,
) -> io::Result<Started<SocketIo<B>, B>> {
    match submitted {
        Submitted::Pending(id) => Ok(Started::Pending(SocketIo {
            id,
            buffer: std::marker::PhantomData,
        })),
        Submitted::Completed {
            operation,
            bytes_transferred,
        } => Ok(Started::Completed {
            payload: operation.into_payload().buffer,
            bytes_transferred: bytes_transferred as usize,
        }),
        Submitted::Failed { error, .. } => Err(error),
    }
}

/// Convert a buffer length to the `u32` byte count `WSABUF` carries.
///
/// Rejects rather than caps, for the same reason as the file and device
/// helpers: capping would transfer a prefix of the caller's buffer and then
/// report success for an operation that did something other than what was asked.
fn checked_len(len: usize, which: &str) -> io::Result<u32> {
    u32::try_from(len).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a {which} is limited to u32::MAX bytes; {len} does not fit"),
        )
    })
}

/// A pending socket operation submitted through [`AssociatedSocket::recv`] or
/// [`AssociatedSocket::send`].
///
/// The token carries the operation's identity and remembers the buffer type it
/// was submitted with, so [`SocketIo::claim`] hands back the caller's own buffer
/// -- the same value, not a copy -- once the matching completion is dequeued.
#[derive(Debug)]
pub struct SocketIo<B> {
    id: OperationId,
    /// The buffer itself is in the pinned operation, not here; this only keeps
    /// the token's type tied to it so `claim` cannot be handed the wrong one.
    buffer: std::marker::PhantomData<fn() -> B>,
}

impl<B: IoBuf> SocketIo<B> {
    /// The identity of the in-flight operation, for cancellation or matching.
    #[must_use]
    pub fn id(&self) -> OperationId {
        self.id
    }

    /// Claim this operation's result from `completion`.
    ///
    /// On a match returns `Ok((buffer, result))`: `buffer` is the one the caller
    /// handed over -- the bytes received (valid up to the byte count), or the
    /// data sent -- and `result` is the byte count or the operation's error.
    /// Returns `Err(self)` when `completion` belongs to a different operation.
    pub fn claim(self, completion: &Completion) -> Result<(B, io::Result<usize>), Self> {
        if completion.id() != Some(self.id) {
            return Err(self);
        }
        // SAFETY: the full identity -- address *and* generation -- matches, which
        // an address alone would not: a recycled address can belong to a later
        // operation of a different payload type. The match therefore proves this
        // completion is the Operation<SocketPayload<B>> this token submitted, and
        // the token's own type parameter names that B; claim it exactly once.
        let operation = unsafe { completion.claim::<SocketPayload<B>>() };
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

    /// Receive into `buffer`, blocking until the receive completes, and return
    /// the number of bytes received.
    ///
    /// Takes a plain `&mut [u8]` and allocates nothing, matching
    /// [`BlockingSocket::send`]: this call does not return until the operation
    /// is over, so an ordinary borrow provably covers it.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `buffer` is longer than
    /// `u32::MAX`, which `WSABUF`'s byte count cannot express, or any error from
    /// issuing or completing the receive.
    pub fn recv(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let wsalen = checked_len(buffer.len(), "receive buffer")?;
        let wsabuf = WSABUF {
            len: wsalen,
            buf: buffer.as_mut_ptr(),
        };
        // SAFETY: issues exactly one WSARecv into `buffer` via `wsabuf`, both of
        // which stay valid for the whole blocking call.
        unsafe {
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
        }
    }

    /// Send `data`, blocking until the send completes, and return the bytes sent.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidInput`] if `data` is longer than
    /// `u32::MAX`, which `WSABUF`'s byte count cannot express, or any error from
    /// issuing or completing the send.
    pub fn send(&self, data: &[u8]) -> io::Result<usize> {
        let wsabuf = WSABUF {
            len: checked_len(data.len(), "send buffer")?,
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
