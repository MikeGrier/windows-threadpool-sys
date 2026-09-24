// Copyright (c) 2026 Mike Grier
//! The record format and the append path (M13.2).
//!
//! # The record format
//!
//! Every record on disk is a fixed header followed by its payload:
//!
//! ```text
//! offset  size  field
//!      0     4  magic          identifies a record start
//!      4     8  sequence       monotonic, assigned at append
//!     12     8  epoch          the epoch this record joined
//!     20     4  payload_len    bytes of payload that follow the header
//!     24     4  checksum       over the sequence, epoch, length, and payload
//!     28     n  payload
//! ```
//!
//! The four fields, and why each is there rather than being nice to have:
//!
//! - **a length**, because replay reads a stream of bytes with no framing of
//!   its own and has to know where this record ends and the next begins;
//! - **a sequence number**, because [`crate::contract`] guarantees no ordering
//!   *within* an epoch, so the on-disk order is not the logical order and
//!   replay has to reconstruct it from something;
//! - **an epoch**, because the contract defines durability per epoch, and a
//!   replay after a crash runs in a *new process* with no memory of which
//!   record joined which epoch. Keeping it only in RAM would make the contract
//!   verifiable in this demonstration and unverifiable in the situation it
//!   exists for;
//! - **a checksum**, because the contract promises records after the last
//!   committed epoch may be **torn**, and a reader that cannot tell a torn
//!   record from a whole one cannot honour the part of the contract that says
//!   it must tolerate them.
//!
//! The checksum covers the header fields as well as the payload, so a header
//! that is itself torn -- a plausible length with a stale payload behind it --
//! is caught rather than trusted. The magic gives replay a cheap way to reject
//! a region that was never written at all, which on a freshly-extended file is
//! zeroes rather than garbage.
//!
//! # Why a registered arena rather than an owned `Vec` per record
//!
//! `Batch::write` would take an owned buffer per append and hand it back on
//! completion, which is simpler. This sample deliberately does not use it: a
//! real consumer of a log has a buffer arena it manages itself -- sized once,
//! reused, often placed deliberately -- and registering it once means the
//! kernel resolves an index instead of an address on every push. Using the
//! registered form is the whole reason this sample is worth reading, so it
//! uses [`Batch::write_registered_raw`] over
//! [`RegisteredBuffers`](windows_ioring_sys::RegisteredBuffers) throughout.
//!
//! That choice is also what surfaced the gap M13.2 had to fix in the crate
//! itself: a registered arena that a caller fills needs mutable access to its
//! own buffers, which `RegisteredBuffers` did not offer until `get_mut`.

use std::io;

use crate::commit::Epoch;

/// Marks the start of a record. A region that was never written reads as
/// zeroes on a freshly-extended file, so any value with a bit set will do;
/// this one is recognisable in a hex dump.
const MAGIC: u32 = 0x676F_4C45; // "ELog" little-endian-ish, chosen to be visible.

/// Byte offsets and widths of the header fields. Named rather than written
/// inline at each use, per the repository's "no manifest numeric constants"
/// rule: a wrong offset here is a silent format change, not a compile error.
mod field {
    pub const MAGIC: std::ops::Range<usize> = 0..4;
    pub const SEQUENCE: std::ops::Range<usize> = 4..12;
    pub const EPOCH: std::ops::Range<usize> = 12..20;
    pub const PAYLOAD_LEN: std::ops::Range<usize> = 20..24;
    pub const CHECKSUM: std::ops::Range<usize> = 24..28;
}

/// Total header size. The payload begins here.
pub const HEADER_LEN: usize = field::CHECKSUM.end;

/// The largest physical sector size this sample is prepared for.
///
/// `NO_BUFFERING` (M25.3) requires the file offset *and* the transfer length
/// to be multiples of the volume's physical sector size. 4096 is the largest
/// in common use, and anything that is a multiple of it is a multiple of 512
/// as well, so a stride satisfying this satisfies every volume the sample can
/// plausibly run on.
const LARGEST_SECTOR_LEN: usize = 4096;

/// Bytes on disk per record, whatever the record's own length (M25.1).
///
/// Records are variable-length but land one per fixed-size block, so record
/// *n* occupies `[n * RECORD_STRIDE, n * RECORD_STRIDE + total_len)` and the
/// remainder of its block is zero. Two reasons, and the first is what forced
/// it:
///
/// 1. **`NO_BUFFERING` constrains transfer lengths, not just offsets.** A
///    packed layout could satisfy the offset rule only by accident and cannot
///    satisfy the length rule at all, since record lengths are whatever a
///    payload makes them.
/// 2. **It is what a real write-ahead log does**, for a related reason: a
///    device's power-fail atomic unit is a sector, so a record sharing a
///    sector with its neighbour can be torn by that neighbour's write.
///
/// # This lives here, with the format, and not with either writer
///
/// The stride is a property of the on-disk format, so both writers
/// ([`crate::append::Appender`] and the measurement harness's `Lane`) and the
/// reader ([`crate::replay`]) bind to this one definition. They previously
/// carried a packed layout each, with the *same* justifying comment written
/// out twice -- which is exactly the shape that lets two copies of one rule
/// drift apart while each looks locally correct.
///
/// # The cost, reported rather than described
///
/// This is write amplification, and for records as small as this sample's it
/// is large. The sample measures it and prints it -- `layout: N bytes of
/// records in M bytes of file` -- rather than stating a ratio here that would
/// be a hand-maintained copy of a number the program already computes. What
/// is acceptable depends entirely on a caller's record size. A production
/// design amortises it by packing many records into one block and flushing the
/// block once, which is a different sample than this one. What is bought is
/// sector atomicity, which is what a log actually needs.
pub const RECORD_STRIDE: usize = 4096;

// A stride that is not a whole number of sectors cannot be the length of a
// `NO_BUFFERING` write, so `M25.3` would fail at runtime with
// `ERROR_INVALID_PARAMETER` on a volume whose sectors are this size. A `const`
// assertion refuses it at build time instead, which is the stronger rung: it
// cannot be skipped, and it fails for whoever changes the stride rather than
// for whoever next runs the sample on 4K-native storage.
const _: () = assert!(
    RECORD_STRIDE.is_multiple_of(LARGEST_SECTOR_LEN),
    "RECORD_STRIDE must be a whole number of sectors, or NO_BUFFERING writes are refused"
);

// A record has to fit in its own block, header included, or the format cannot
// represent even an empty payload.
const _: () = assert!(
    HEADER_LEN < RECORD_STRIDE,
    "RECORD_STRIDE must leave room for a record header"
);

/// A record's monotonic identity, assigned when the append is accepted.
///
/// Orders records *logically*. The contract is explicit that it says nothing
/// about the order their bytes reach the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Sequence(pub u64);

/// A checksum over a record's sequence, epoch, payload length, and payload.
///
/// FNV-1a: not cryptographic and not trying to be. Its job is to detect a
/// *torn* record -- a partial write across a power failure -- which is a
/// corruption with structure rather than an adversary. A real consumer should
/// prefer a CRC with hardware support; this is chosen so the sample carries no
/// dependency and the reader can see the whole thing.
fn checksum(sequence: Sequence, epoch: Epoch, payload: &[u8]) -> u32 {
    const OFFSET_BASIS: u32 = 0x811C_9DC5;
    const PRIME: u32 = 0x0100_0193;

    let mut hash = OFFSET_BASIS;
    let mut eat = |byte: u8| {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(PRIME);
    };
    for byte in sequence.0.to_le_bytes() {
        eat(byte);
    }
    for byte in epoch.0.to_le_bytes() {
        eat(byte);
    }
    for byte in (payload.len() as u32).to_le_bytes() {
        eat(byte);
    }
    for &byte in payload {
        eat(byte);
    }
    hash
}

/// Write one record into `slot`, returning how many bytes it occupies.
///
/// `slot` is a borrowed view of a registered buffer, which is why this takes a
/// slice rather than allocating: the arena's memory is the destination, and
/// this function's whole job is to lay a record out inside it.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] if the record does not fit in `slot`. A log
/// that silently truncated a record would produce a checksum failure at replay
/// and look like corruption, so this refuses at append time instead.
pub fn encode(
    slot: &mut [u8],
    sequence: Sequence,
    epoch: Epoch,
    payload: &[u8],
) -> io::Result<usize> {
    let payload_len = u32::try_from(payload.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "record payload exceeds u32::MAX bytes",
        )
    })?;
    let total = HEADER_LEN
        .checked_add(payload.len())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "record length overflows"))?;
    if total > slot.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "record of {total} bytes does not fit the {} available in this buffer",
                slot.len()
            ),
        ));
    }

    slot[field::MAGIC].copy_from_slice(&MAGIC.to_le_bytes());
    slot[field::SEQUENCE].copy_from_slice(&sequence.0.to_le_bytes());
    slot[field::EPOCH].copy_from_slice(&epoch.0.to_le_bytes());
    slot[field::PAYLOAD_LEN].copy_from_slice(&payload_len.to_le_bytes());
    slot[field::CHECKSUM].copy_from_slice(&checksum(sequence, epoch, payload).to_le_bytes());
    slot[HEADER_LEN..total].copy_from_slice(payload);
    Ok(total)
}

/// Compose a record into `slot` as a whole block, returning the record's own
/// extent (M25.1).
///
/// [`encode`] lays the record out; this additionally zeroes the rest of its
/// stride, which is what makes the block safe to write whole. Slots are reused
/// for a log's entire life -- a fresh buffer arrives zeroed, but only once --
/// so without this the bytes past a short record are whatever the previous,
/// longer record left there, and the write puts them on disk.
///
/// # Why both writers call this rather than each doing the two steps
///
/// The sample has two writers over this format: [`crate::append::Appender`]
/// and the measurement harness's `Lane`. They previously carried a packed
/// layout each, with the same justifying comment written out twice, and
/// converting them to the stride converted that duplication into a rule
/// stated at two sites. Measured before this function existed: reverting the
/// harness lane to a packed layout was **caught by nothing** -- the end-to-end
/// test covers `Appender`, and only running the sample exercises `Lane`.
/// Giving the composition one site makes half that drift unrepresentable
/// rather than merely tested for.
pub fn encode_block(
    slot: &mut [u8],
    sequence: Sequence,
    epoch: Epoch,
    payload: &[u8],
) -> io::Result<usize> {
    let total = encode(slot, sequence, epoch, payload)?;
    slot[total..RECORD_STRIDE].fill(0);
    Ok(total)
}

/// A record recovered from the log.
#[derive(Debug)]
pub struct Decoded<'a> {
    pub sequence: Sequence,
    pub epoch: Epoch,
    pub payload: &'a [u8],
}

impl Decoded<'_> {
    /// How many bytes this record occupies, header included.
    ///
    /// Derived rather than stored. It was a field until M25.2, when replay
    /// stopped reading it -- a strided reader advances by
    /// [`RECORD_STRIDE`], not by the record's own length, so the only
    /// remaining consumer was a test. Keeping it as a field would have kept a
    /// second copy of a fact `payload` already carries, which is the shape
    /// that lets two statements of one rule drift apart.
    ///
    /// Note this is the record's **extent**, not its footprint: the rest of
    /// its block is zero padding, so `extent() <= RECORD_STRIDE`.
    pub fn extent(&self) -> usize {
        HEADER_LEN + self.payload.len()
    }
}

/// Why a region of the log did not yield a record.
///
/// Replay has to distinguish these: the contract says records after the last
/// committed epoch may be absent *or* torn, and a reader that treated the two
/// alike could not report which one it found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Torn {
    /// No record was ever written here -- no magic, and on a freshly-extended
    /// file that means zeroes.
    NeverWritten,
    /// A record starts here but the bytes run out before it ends.
    Truncated,
    /// A whole record is present but its checksum does not match, so some part
    /// of it did not survive.
    ChecksumMismatch,
}

/// Read the record at the front of `bytes`, if there is a whole valid one.
///
/// # Errors
///
/// Returns the [`Torn`] reason instead of a record. All three are legal
/// outcomes past the last committed epoch, which is precisely what the
/// contract's tail clause requires a reader to tolerate.
pub fn decode(bytes: &[u8]) -> Result<Decoded<'_>, Torn> {
    if bytes.len() < HEADER_LEN {
        return Err(Torn::Truncated);
    }
    let magic = u32::from_le_bytes(
        bytes[field::MAGIC]
            .try_into()
            .expect("MAGIC is four bytes wide"),
    );
    if magic != MAGIC {
        return Err(Torn::NeverWritten);
    }
    let sequence = Sequence(u64::from_le_bytes(
        bytes[field::SEQUENCE]
            .try_into()
            .expect("SEQUENCE is eight bytes wide"),
    ));
    let epoch = Epoch(u64::from_le_bytes(
        bytes[field::EPOCH]
            .try_into()
            .expect("EPOCH is eight bytes wide"),
    ));
    let payload_len = u32::from_le_bytes(
        bytes[field::PAYLOAD_LEN]
            .try_into()
            .expect("PAYLOAD_LEN is four bytes wide"),
    ) as usize;
    let stored = u32::from_le_bytes(
        bytes[field::CHECKSUM]
            .try_into()
            .expect("CHECKSUM is four bytes wide"),
    );

    let total_len = match HEADER_LEN.checked_add(payload_len) {
        Some(total) if total <= bytes.len() => total,
        // Either the length overflowed or it points past what was read. Both
        // mean this record did not land whole; a torn header can carry a
        // plausible-looking length, which is why the checksum covers it.
        _ => return Err(Torn::Truncated),
    };
    let payload = &bytes[HEADER_LEN..total_len];
    if checksum(sequence, epoch, payload) != stored {
        return Err(Torn::ChecksumMismatch);
    }
    Ok(Decoded {
        sequence,
        epoch,
        payload,
    })
}
