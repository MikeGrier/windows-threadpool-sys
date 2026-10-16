// Copyright (c) 2026 Mike Grier
//! An append-only sequence held in fixed-size chunks, for the poll's per-poll copies.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the poll engine (TW-3.1) uses it")
)]

use std::mem::size_of;

#[cfg(test)]
mod tests;

/// The most a single chunk allocates (TW-D-7).
const CHUNK_BYTES: usize = 64 * 1024;

/// Chunks per group of the directory, so that no allocation of it is more than [`CHUNK_BYTES`].
const FAN: usize = CHUNK_BYTES / size_of::<Vec<u8>>();

/// The chunks, in two levels: groups of at most `fan` chunks, and at most `fan` groups. Both levels
/// grow in steps that stop at `fan`, so neither allocation is ever more than [`CHUNK_BYTES`]; a
/// plain `Vec` of chunks doubles past it.
#[derive(Debug)]
struct Directory<T> {
    groups: Vec<Vec<Vec<T>>>,
    fan: usize,
    count: usize,
}

impl<T> Directory<T> {
    const fn new(fan: usize) -> Self {
        Self {
            groups: Vec::new(),
            fan,
            count: 0,
        }
    }

    fn len(&self) -> usize {
        self.count
    }

    /// Add a chunk. Panics when all `fan` groups are full.
    fn push(&mut self, chunk: Vec<T>) {
        let at = self.count / self.fan;
        if at == self.groups.len() {
            assert!(
                at < self.fan,
                "a chunked buffer holds at most {} chunks",
                self.fan * self.fan
            );
            grow(&mut self.groups, self.fan);
            self.groups.push(Vec::new());
        }
        let group = &mut self.groups[at];
        grow(group, self.fan);
        group.push(chunk);
        self.count += 1;
    }

    fn iter(&self) -> impl Iterator<Item = &Vec<T>> {
        self.groups.iter().flatten()
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = &mut Vec<T>> {
        self.groups.iter_mut().flatten()
    }

    // `low < high`.
    fn pair_mut(&mut self, low: usize, high: usize) -> (&mut Vec<T>, &mut Vec<T>) {
        let (low_group, low_at) = (low / self.fan, low % self.fan);
        let (high_group, high_at) = (high / self.fan, high % self.fan);
        if low_group == high_group {
            let (head, tail) = self.groups[low_group].split_at_mut(high_at);
            (&mut head[low_at], &mut tail[0])
        } else {
            let (head, tail) = self.groups.split_at_mut(high_group);
            (&mut head[low_group][low_at], &mut tail[0][high_at])
        }
    }
}

impl<T> std::ops::Index<usize> for Directory<T> {
    type Output = Vec<T>;

    fn index(&self, index: usize) -> &Vec<T> {
        &self.groups[index / self.fan][index % self.fan]
    }
}

impl<T> std::ops::IndexMut<usize> for Directory<T> {
    fn index_mut(&mut self, index: usize) -> &mut Vec<T> {
        &mut self.groups[index / self.fan][index % self.fan]
    }
}

// Room for one more, by doubling but never past `limit`; the caller has seen to it that `items` is
// not already at `limit`.
fn grow<E>(items: &mut Vec<E>, limit: usize) {
    if items.len() == items.capacity() {
        let want = (items.capacity() * 2).max(4).min(limit);
        items.reserve_exact(want - items.len());
    }
}

/// A sequence of `T` stored in chunks of at most [`CHUNK_BYTES`], so that growing it never asks
/// the allocator for more than that and never moves an item already stored.
///
/// [`clear`](Self::clear) and [`truncate`](Self::truncate) keep the chunks they empty, so a buffer
/// that has been filled once is refilled without allocating. How many to keep after an unusually
/// large fill is not decided (TW-2+.2).
#[derive(Debug)]
pub(crate) struct Chunked<T> {
    // Every chunk is allocated at `PER_CHUNK` and never grows. Chunks before the one holding the
    // last item are full; those after it are empty, and kept.
    chunks: Directory<T>,
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
            chunks: Directory::new(FAN),
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

    /// Hand every item to `each`, oldest first, dropping each as `each` returns, and keep the
    /// chunks. The buffer is empty afterwards. If `each` panics the buffer is inconsistent, and
    /// [`clear`](Self::clear) makes it usable again.
    pub(crate) fn drain_each(&mut self, mut each: impl FnMut(T)) {
        for chunk in self.chunks.iter_mut() {
            for item in chunk.drain(..) {
                each(item);
            }
        }
        self.len = 0;
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
            let (low, high) = self.chunks.pair_mut(low_chunk, high_chunk);
            std::mem::swap(&mut low[low_at], &mut high[high_at]);
        }
    }

    #[cfg(test)]
    fn with_fan(fan: usize) -> Self {
        Self {
            chunks: Directory::new(fan),
            len: 0,
        }
    }

    // Every allocation the directory makes, as bytes, beside what `chunk_allocation_bytes` gives.
    #[cfg(test)]
    fn directory_allocation_bytes(&self) -> Vec<usize> {
        let groups = &self.chunks.groups;
        std::iter::once(groups.capacity() * size_of::<Vec<Vec<T>>>())
            .chain(
                groups
                    .iter()
                    .map(|group| group.capacity() * size_of::<Vec<T>>()),
            )
            .collect()
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
