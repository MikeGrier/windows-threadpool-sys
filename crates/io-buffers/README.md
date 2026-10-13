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

## What the crate is for

- **The contract:** the two traits, the span type that names part of a registered buffer, and
  implementations for the standard types that meet it.
- **One concrete buffer:** an owned, aligned allocation. A target that does unbuffered I/O needs
  buffer addresses aligned to its sector size, which a `Vec<u8>` cannot promise, and a consumer
  should not have to write `unsafe` to get one.

## What it is not

**A place for a platform's memory.** A buffer placed on a NUMA node, backed by huge pages, or
registered with a kernel is a Windows or Linux facility and lives in that platform's crate, which
implements the traits here. This crate has no opinion about where memory comes from, only about
what its owner promises.

The reasoning behind each choice is in this crate's `DESIGN-NOTES.md`.
