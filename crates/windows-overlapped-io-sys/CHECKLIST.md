# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions
are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

Carried forward, recorded here rather than in a design note so it is not orphaned: there is no socket-side
notification-mode setter, because sockets have no unassociated endpoint type to carry the provenance
attribute and Win32 restricts skip-on-success on a socket to Layered Service Providers returning IFS
handles. `socket::classify_socket` is correct only while that stays true, and says so. Adding the setter
means adding the capability probe and updating `classify_socket` in the same change.

## M11: caller-supplied owned buffers, so no adapter forces a copy

The adapters already transfer buffer ownership to the operation and hand it back on completion -- the
"protracted borrow" that completion-based I/O requires, since the kernel touches the memory after the
submitting call returns. What they get wrong is hardcoding **which** owned buffer: `Vec<u8>`. A caller
holding a `Box<[u8]>`, an `Arc<[u8]>`, a `bytes::Bytes`, an alignment-constrained buffer, or one from a
pool must convert, and every one of those conversions is the data copy this crate exists to avoid.

The target: **no adapter copies a caller's bytes by default**, and a caller can hand over whatever owned
buffer it already has, including a shared one. A naive caller passing a slice still pays a copy, which is
fine and expected -- but it has to be *visible at the call site*, never something the adapter does behind
a performance-minded caller's back.

- [x] **M11.1** -- Add `IoBuf` (readable) and `IoBufMut` (writable) to a new `buf` module, re-exported
  from the crate root. Both are `unsafe` traits, because the whole contract is a promise the compiler
  cannot check: the address must be **stable** for the value's life, so a type whose accessor returns a
  fresh address each call (or reallocates) is what makes the operation write into freed memory. Require
  `Send + 'static`, matching what the leaked operation storage already needs. Provide impls for `Vec<u8>`,
  `Box<[u8]>`, and `PageBuffers` (read and write), and for `Arc<[u8]>` and `&'static [u8]` (read only --
  neither can hand out `&mut`, which is exactly why the split exists rather than one trait). Unit tests
  including a stability check that the pointer does not move across a move of the value.

  Read buffers are required to be **fully initialized** for `bytes_len()` bytes rather than tracking an
  initialized prefix through `MaybeUninit`. A caller-supplied pooled buffer is initialized once and reused
  for the life of the pool, so the cost is per-pool, not per-operation, and that buys an API with no
  `set_init`-style obligation to get wrong. Record the trade in DESIGN-NOTES (M11.6).

- [x] **M11.2** -- Make the file adapters generic over the buffer: `AssociatedEndpoint::read<B: IoBufMut>`
  takes the buffer to read into instead of a length it allocates, `write<B: IoBuf>` takes any readable
  owned buffer, and `FileIo<B>` / the `Started` payload carry `B` so `claim` returns the caller's own
  buffer back. Allocating a `Vec` becomes the caller's visible `vec![0; n]` rather than something the
  adapter does for them.

- [ ] **M11.3** -- Make the socket adapters generic the same way: `recv<B: IoBufMut>`, `send<B: IoBuf>`,
  `SocketPayload<B>`, `SocketIo<B>`. The `WSABUF` is built from the buffer's stable pointer and length
  rather than from a `Vec`'s.

- [ ] **M11.4** -- Make `device::ioctl` generic over both of its buffers (`I: IoBuf` for input, `O:
  IoBufMut` for output), replacing the `output_len` parameter that currently makes the adapter allocate.
  `DeviceIoControlIo<O>` returns the caller's output buffer. The blocking form follows.

- [ ] **M11.5** -- Sweep for remaining forced copies now that the traits exist, and fix or record each:
  the blocking adapters (which take slices legitimately, since they block for the whole operation), the
  scatter/gather path (already owns `PageBuffers`; confirm no conversion sneaks in), and any `to_vec` /
  `clone` left in the adapters or their tests.

- [ ] **M11.6** -- Record in [DESIGN-NOTES.md](DESIGN-NOTES.md): why completion-based I/O forces owned
  buffers rather than slices (the token has no `Drop`, and even one could be defeated by `mem::forget`, so
  no borrow can be made to span the operation); why the blocking adapters may still take slices; why the
  trait is `unsafe` and what the stable-address contract means; why read buffers are fully initialized
  instead of init-tracked; and why the split into `IoBuf`/`IoBufMut` exists (so a shared `Arc<[u8]>` can be
  written from but never read into). Update the README's adapter section.

