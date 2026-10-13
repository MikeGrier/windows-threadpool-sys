// Copyright (c) 2026 Mike Grier
// Copied from windows-ioring-sys src/batch.rs at b365a2b1.
//! The part of a registered buffer an operation reads or writes.

/// Which bytes of one registered buffer an operation reads or writes: bundled into one value so a
/// registered read or write stays under a sane argument count.
///
/// A registration is a set of buffers a layer holds for many operations and names by index, as
/// `io_uring`'s fixed buffers and an `IoRing`'s registered buffers both do; this says which of them,
/// and which part.
#[derive(Clone, Copy, Debug)]
pub struct RegisteredSpan {
    /// Which buffer within the registration, as the registering layer numbers them.
    pub buffer_index: u32,
    /// The starting byte offset within that buffer.
    pub offset: u32,
    /// How many bytes to read or write.
    pub len: u32,
}

#[cfg(test)]
mod tests;
