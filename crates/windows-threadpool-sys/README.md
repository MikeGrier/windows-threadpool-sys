# windows-threadpool-sys

Memory-safe Rust access to the Windows thread pool APIs.

This crate is in its initial development stage. Its public API will wrap the
Windows thread pool primitives while making callback and resource lifetimes
explicit in Rust.