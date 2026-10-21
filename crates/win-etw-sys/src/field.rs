// Copyright (c) 2026 Mike Grier
//! `EVENT_DATA_DESCRIPTOR`: the blocks of data an event is made of, built from typed fields.
//!
//! Read 2026-10-20 from the structure's page (`ms.date` 2018-12-05, `updated_at` 2024-02-22) and from `EventWrite`'s.
//! `EventWrite` takes an array of descriptors, each "one block of memory to be included in the event", and the
//! blocks "will be concatenated in order with no padding or alignment to form the event content". So an event's
//! layout is the order and the bytes of its fields, and this crate adds none of its own.

use std::marker::PhantomData;

use windows_sys::Win32::System::Diagnostics::Etw::{
    EVENT_DATA_DESCRIPTOR, EVENT_DATA_DESCRIPTOR_0,
};

use crate::guid::Guid;
use crate::registration::WriteError;

/// The unit that ends a UTF-16 string a decoder reads to a terminator.
static NUL_UNIT: u16 = 0;
/// The byte that ends an ANSI or UTF-8 string a decoder reads to a terminator.
static NUL_BYTE: u8 = 0;

/// One field of an event's data: a value, written to the event as the bytes it has in memory.
///
/// The fields of an event are written in order and with no padding, so what a decoder reads them back as is what its
/// template says (a manifest's, for a manifest-based event) and not something this type knows: an `I32` is four bytes
/// whichever name the template gives it. Scalars are in the machine's own byte order, which is what a decoder
/// expects.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Field<'a> {
    /// One byte.
    U8(u8),
    /// One byte, signed.
    I8(i8),
    /// Two bytes.
    U16(u16),
    /// Two bytes, signed.
    I16(i16),
    /// Four bytes.
    U32(u32),
    /// Four bytes, signed.
    I32(i32),
    /// Eight bytes.
    U64(u64),
    /// Eight bytes, signed.
    I64(i64),
    /// Four bytes, an IEEE 754 single.
    F32(f32),
    /// Eight bytes, an IEEE 754 double.
    F64(f64),
    /// As many bytes as a pointer has in this process: four or eight.
    Pointer(usize),
    /// Sixteen bytes, laid out as the `GUID` structure.
    Guid(Guid),
    /// These bytes, and nothing else: no length, and no terminator. A template that carries a length does so in a
    /// field of its own, before this one.
    Bytes(&'a [u8]),
    /// These UTF-16 units, and nothing else: no length, and no terminator.
    Utf16(&'a [u16]),
    /// These UTF-16 units and then a zero unit, as a decoder reads a string of a template's `UnicodeString` type.
    ///
    /// A zero unit already in the slice ends the string for such a decoder at that unit. That is the decoder's rule
    /// and not the call's, so the units are written as they are.
    Utf16Terminated(&'a [u16]),
    /// These bytes and then a zero byte, as a decoder reads a string of a template's `AnsiString` type.
    ///
    /// As [`Utf16Terminated`](Self::Utf16Terminated): a zero byte already in the slice ends the string for the
    /// decoder.
    BytesTerminated(&'a [u8]),
}

impl Field<'_> {
    /// A four-byte boolean, which is what a template's `Boolean` type is: 1 for true and 0 for false.
    #[must_use]
    pub const fn boolean(value: bool) -> Self {
        Self::U32(value as u32)
    }

    /// The blocks of memory this field is: one, or two for a terminated string, whose second is the terminator.
    fn blocks(&self) -> [Option<Block>; 2] {
        fn of<T>(value: &T) -> Block {
            Block {
                ptr: std::ptr::from_ref(value).cast(),
                len: size_of::<T>(),
            }
        }
        fn slice<T>(values: &[T]) -> Block {
            Block {
                ptr: values.as_ptr().cast(),
                len: size_of_val(values),
            }
        }
        let one = |block| [Some(block), None];
        match self {
            Self::U8(v) => one(of(v)),
            Self::I8(v) => one(of(v)),
            Self::U16(v) => one(of(v)),
            Self::I16(v) => one(of(v)),
            Self::U32(v) => one(of(v)),
            Self::I32(v) => one(of(v)),
            Self::U64(v) => one(of(v)),
            Self::I64(v) => one(of(v)),
            Self::F32(v) => one(of(v)),
            Self::F64(v) => one(of(v)),
            Self::Pointer(v) => one(of(v)),
            Self::Guid(v) => one(of(v.raw())),
            Self::Bytes(v) => one(slice(v)),
            Self::Utf16(v) => one(slice(v)),
            Self::Utf16Terminated(v) => [Some(slice(v)), Some(of(&NUL_UNIT))],
            Self::BytesTerminated(v) => [Some(slice(v)), Some(of(&NUL_BYTE))],
        }
    }
}

/// A block of memory, by address and length.
#[derive(Clone, Copy)]
struct Block {
    ptr: *const u8,
    len: usize,
}

/// The descriptors for a list of fields, valid for as long as the fields are borrowed.
pub(crate) struct Built<'a> {
    descriptors: Vec<EVENT_DATA_DESCRIPTOR>,
    /// What the descriptors point into: the fields, and the statics for terminators.
    fields: PhantomData<&'a ()>,
}

impl Built<'_> {
    /// The array and its count, as `EventWrite` takes them.
    pub(crate) fn as_call(&self) -> (u32, *const EVENT_DATA_DESCRIPTOR) {
        // The count was checked to fit when this was built.
        let count = u32::try_from(self.descriptors.len()).expect("checked in `build`");
        // The page for `EventWrite`: "UserData may be NULL if UserDataCount is zero". An empty vector's pointer is not
        // null but dangling, and the call refused an event given it with `ERROR_INVALID_PARAMETER`, so no fields are
        // passed as the null the page names.
        let blocks = if self.descriptors.is_empty() {
            std::ptr::null()
        } else {
            self.descriptors.as_ptr()
        };
        (count, blocks)
    }

    /// The descriptors, for a test to read the blocks they describe.
    #[cfg(test)]
    pub(crate) fn descriptors(&self) -> &[EVENT_DATA_DESCRIPTOR] {
        &self.descriptors
    }
}

/// The descriptors for `fields`, or the first thing that cannot be passed as the call takes it.
pub(crate) fn build<'a>(fields: &'a [Field<'a>]) -> Result<Built<'a>, WriteError> {
    let mut descriptors = Vec::with_capacity(fields.len());
    for (index, field) in fields.iter().enumerate() {
        for block in field.blocks().into_iter().flatten() {
            descriptors.push(descriptor(block, index)?);
        }
    }
    if u32::try_from(descriptors.len()).is_err() {
        return Err(WriteError::TooManyBlocks {
            count: descriptors.len(),
        });
    }
    Ok(Built {
        descriptors,
        fields: PhantomData,
    })
}

/// The descriptor of one block, which `EventDataDescCreate` would make: the address as a 64-bit integer, "in both
/// 32-bit and 64-bit architectures", and the size in bytes, as a `ULONG`.
fn descriptor(block: Block, field: usize) -> Result<EVENT_DATA_DESCRIPTOR, WriteError> {
    let size = size_of_block(block.len).ok_or(WriteError::BlockTooLong {
        field,
        len: block.len,
    })?;
    Ok(EVENT_DATA_DESCRIPTOR {
        Ptr: block.ptr as usize as u64,
        Size: size,
        // The type is zero, "Normal event data", and the rest is reserved and zero.
        Anonymous: EVENT_DATA_DESCRIPTOR_0 { Reserved: 0 },
    })
}

/// A block's length as the descriptor's `ULONG` size, or `None` if it does not fit.
pub(crate) fn size_of_block(len: usize) -> Option<u32> {
    u32::try_from(len).ok()
}

#[cfg(test)]
mod tests;
