# windows-threadpool-sys

Memory-safe Rust access to the Windows thread pool APIs.

This crate is in its initial development stage. Its public API will wrap the
Windows thread pool primitives while making callback and resource lifetimes
explicit in Rust.

## What exists today

- `callback_env`: SDK-equivalent `TP_CALLBACK_ENVIRON_V3` initialization and
  mutation, which `windows-sys` cannot emit because the SDK defines them as
  header-only inline helpers.
- `work`: owned `TP_WORK` objects whose `Drop` drains in-flight callbacks before
  releasing the callback context.
- `io`: the `TP_IO` completion backend over the overlapped submission seam owned
  by [`windows-overlapped-io-sys`](../windows-overlapped-io-sys), with balanced
  `StartThreadpoolIo` / `CancelThreadpoolIo` accounting and callback-driven
  reclamation of operation storage.
