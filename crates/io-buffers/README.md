# io-buffers

Buffers for asynchronous I/O: the contract a buffer must meet when the kernel is going to use its
memory after the call that handed it over has returned, and one concrete buffer that meets it with an
alignment the caller chooses.

The crate is portable. It names no operating system, has no dependencies, and is built and tested on
Windows, Linux and macOS.

## The problem

A completion-based I/O API -- a Windows `IoRing`, Linux `io_uring`, overlapped I/O -- takes an
address and a length when an operation is submitted and uses them later, on the kernel's schedule. A
buffer given to one therefore has to stay where it is, and stay alive, until the operation completes.
Rust cannot check that from a `&[u8]`, whose lifetime ends when the call returns.

The traits here, `IoBuf` and `IoBufMut`, are how an owner promises it. They are `unsafe` to
implement, because the promise is the whole content of the trait. A ring crate takes ownership of a
buffer for the operation's duration and hands it back at the completion, so no buffer the kernel may
still be using is reachable from safe code.

A buffer of your own implements the traits the same way the standard types do:

```rust
use io_buffers::{IoBuf, IoBufMut};

/// A pooled buffer. A boxed slice keeps its bytes where they are when the value moves.
struct Pooled(Box<[u8]>);

// SAFETY: the bytes live in an owned boxed slice, so the address is stable across moves and the
// length is fixed, and nothing else holds them.
unsafe impl IoBuf for Pooled {
    fn stable_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }

    fn bytes_len(&self) -> usize {
        self.0.len()
    }
}

// SAFETY: unique ownership, and the same allocation `stable_ptr` reports.
unsafe impl IoBufMut for Pooled {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.0.as_mut_ptr()
    }
}

let mut buffer = Pooled(vec![0_u8; 4096].into_boxed_slice());
let address = buffer.stable_ptr();
assert_eq!(buffer.bytes_len(), 4096);
assert_eq!(buffer.stable_mut_ptr().cast_const(), address);

// Moving the value does not move the bytes, which is the whole promise.
let moved = buffer;
assert_eq!(moved.stable_ptr(), address);
```

## What the crate is for

- **The contract:** the two traits, the span type that names part of a registered buffer, and
  implementations for the standard types that meet it.
- **One concrete buffer, `AlignedBuf`:** an owned, zeroed allocation at an alignment the caller
  chooses. A target that does unbuffered I/O needs buffer addresses aligned to its sector size, which
  a `Vec<u8>` cannot promise, and a consumer should not have to write `unsafe` to get one. Building
  one is fallible -- a bad alignment or a request the machine cannot meet is an error, not a panic or
  an abort -- because the alignment is usually read from a device.

```rust
use io_buffers::{AlignedBuf, AlignedBufError, IoBuf};

let sector = 4096; // as a device reported it
let buffer = AlignedBuf::zeroed(8 * sector, sector)?;
assert_eq!(buffer.stable_ptr() as usize % sector, 0);

// A device that reports nonsense is an error to handle.
assert_eq!(
    AlignedBuf::zeroed(512, 3).unwrap_err(),
    AlignedBufError::InvalidAlignment { align: 3 },
);
# Ok::<(), AlignedBufError>(())
```

## What it is not

**A place for a platform's memory.** A buffer placed on a NUMA node, backed by huge pages, or
registered with a kernel is a Windows or Linux facility and lives in that platform's crate, which
implements the traits here. This crate has no opinion about where memory comes from, only about
what its owner promises.

The reasoning behind each choice is in this crate's `DESIGN-NOTES.md`.
