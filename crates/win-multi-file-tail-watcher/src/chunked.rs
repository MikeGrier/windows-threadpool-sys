// Copyright (c) 2026 Mike Grier
//! An append-only sequence held in fixed-size chunks, for the poll's per-poll copies.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the snapshot and probe passes (TW-2.5, TW-2.6) use it"
    )
)]

use std::mem::size_of;

#[cfg(test)]
mod tests;

/// The most a single chunk allocates (TW-D-7).
const CHUNK_BYTES: usize = 64 * 1024;

/// A sequence of `T` stored in chunks of at most [`CHUNK_BYTES`], so that growing it never asks
/// the allocator for more than that and never moves an item already stored.
///
/// [`clear`](Self::clear) and [`truncate`](Self::truncate) keep the chunks they empty, so a buffer
/// that has been filled once is refilled without allocating. How many to keep after an unusually
/// large fill is not decided.
#[derive(Debug)]
pub(crate) struct Chunked<T> {
    // Every chunk is allocated at `PER_CHUNK` and never grows. Chunks before the one holding the
    // last item are full; those after it are empty, and kept.
    chunks: Vec<Vec<T>>,
    len: usize,
}

impl<T> Chunked<T> {
    /// Items per chunk. An item of zero size, or larger than a chunk, does not compile.
    const PER_CHUNK: usize = {
        assert!(
            size_of::<T>() > 0 && size_of::<T>() <= CHUNK_BYTES,
            "an item must be larger than nothing and no larger than a chunk"
        );
        CHUNK_BYTES / size_of::<T>()
    };

    /// An empty buffer. Nothing is allocated until the first push.
    pub(crate) const fn new() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }

    /// How many items are held.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing is held.
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append `item`.
    pub(crate) fn push(&mut self, item: T) {
        let chunk = self.len / Self::PER_CHUNK;
        if chunk == self.chunks.len() {
            self.chunks.push(Vec::with_capacity(Self::PER_CHUNK));
        }
        self.chunks[chunk].push(item);
        self.len += 1;
    }

    /// The item at `index`, or `None` past the end.
    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        Some(&self.chunks[index / Self::PER_CHUNK][index % Self::PER_CHUNK])
    }

    /// The items in the order they were pushed.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flatten()
    }

    /// Drop every item and keep the chunks.
    pub(crate) fn clear(&mut self) {
        self.truncate(0);
    }

    /// Drop the items from `new_len` on, and keep the chunks. Does nothing when `new_len` is not
    /// less than the length.
    pub(crate) fn truncate(&mut self, new_len: usize) {
        if new_len >= self.len {
            return;
        }
        let first = new_len / Self::PER_CHUNK;
        for (index, chunk) in self.chunks.iter_mut().enumerate().skip(first) {
            chunk.truncate(if index == first {
                new_len % Self::PER_CHUNK
            } else {
                0
            });
        }
        self.len = new_len;
    }

    /// Keep the items `keep` accepts, in their order, and drop the rest, without allocating.
    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        let mut write = 0;
        for read in 0..self.len {
            if keep(&self.chunks[read / Self::PER_CHUNK][read % Self::PER_CHUNK]) {
                if write != read {
                    self.swap_items(write, read);
                }
                write += 1;
            }
        }
        self.truncate(write);
    }

    // `low <= high`, so when they are in different chunks `low`'s is the earlier.
    fn swap_items(&mut self, low: usize, high: usize) {
        let (low_chunk, low_at) = (low / Self::PER_CHUNK, low % Self::PER_CHUNK);
        let (high_chunk, high_at) = (high / Self::PER_CHUNK, high % Self::PER_CHUNK);
        if low_chunk == high_chunk {
            self.chunks[low_chunk].swap(low_at, high_at);
        } else {
            let (head, tail) = self.chunks.split_at_mut(high_chunk);
            std::mem::swap(&mut head[low_chunk][low_at], &mut tail[0][high_at]);
        }
    }

    #[cfg(test)]
    fn chunk_allocation_bytes(&self) -> Vec<usize> {
        self.chunks
            .iter()
            .map(|chunk| chunk.capacity() * size_of::<T>())
            .collect()
    }

    #[cfg(test)]
    fn chunk_addresses(&self) -> Vec<usize> {
        self.chunks
            .iter()
            .map(|chunk| chunk.as_ptr() as usize)
            .collect()
    }
}
