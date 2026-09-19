// Copyright (c) Mike Grier.
use std::io;
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

use windows_overlapped_io_sys::{IoBuf, IoBufMut};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocExNuma, VirtualFree,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub(crate) enum Payload {
    Heap(Vec<u8>),
    Numa(NumaPayload),
    #[cfg(test)]
    Faux(crate::platform::faux::FauxPayload),
}

pub(crate) struct NumaPayload {
    pointer: NonNull<u8>,
    length: usize,
    capacity: usize,
}

/// The mapping is uniquely owned and has no thread-affine release requirement.
unsafe impl Send for NumaPayload {}

impl Payload {
    #[cfg(test)]
    pub fn record_processing(&self) -> io::Result<()> {
        if let Self::Faux(buffer) = self {
            buffer.record_processing()?;
        }
        Ok(())
    }
    pub fn is_numa(&self) -> bool {
        matches!(self, Self::Numa(_))
    }

    pub fn new(bytes: usize, node: Option<u32>) -> io::Result<Self> {
        if bytes == 0 || bytes > isize::MAX as usize {
            return Err(crate::invalid("payload size must be nonzero and fit isize"));
        }
        let Some(node) = node else {
            return Ok(Self::Heap(vec![0; bytes]));
        };
        let pointer = unsafe {
            VirtualAllocExNuma(
                GetCurrentProcess(),
                std::ptr::null(),
                bytes,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
                node,
            )
        };
        let pointer = NonNull::new(pointer.cast::<u8>()).ok_or_else(io::Error::last_os_error)?;
        let mut payload = Self::Numa(NumaPayload {
            pointer,
            length: bytes,
            capacity: bytes,
        });
        payload.fill(0);
        Ok(payload)
    }

    pub fn truncate(&mut self, bytes: usize) {
        match self {
            Self::Heap(buffer) => buffer.truncate(bytes),
            Self::Numa(buffer) => buffer.length = buffer.length.min(bytes),
            #[cfg(test)]
            Self::Faux(buffer) => buffer.bytes.truncate(bytes),
        }
    }

    pub fn resize(&mut self, bytes: usize, value: u8) {
        match self {
            Self::Heap(buffer) => buffer.resize(bytes, value),
            #[cfg(test)]
            Self::Faux(buffer) => buffer.bytes.resize(bytes, value),
            Self::Numa(buffer) => {
                assert!(bytes <= buffer.capacity);
                let old_length = buffer.length;
                buffer.length = bytes;
                if bytes > old_length {
                    self[old_length..].fill(value);
                }
            }
        }
    }
}

impl Deref for Payload {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Heap(buffer) => buffer,
            #[cfg(test)]
            Self::Faux(buffer) => &buffer.bytes,
            Self::Numa(buffer) => unsafe {
                std::slice::from_raw_parts(buffer.pointer.as_ptr(), buffer.length)
            },
        }
    }
}

impl DerefMut for Payload {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Heap(buffer) => buffer,
            #[cfg(test)]
            Self::Faux(buffer) => &mut buffer.bytes,
            Self::Numa(buffer) => unsafe {
                std::slice::from_raw_parts_mut(buffer.pointer.as_ptr(), buffer.length)
            },
        }
    }
}

impl Drop for NumaPayload {
    fn drop(&mut self) {
        unsafe {
            VirtualFree(self.pointer.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}

/// Moving the owner does not move either allocation. Every exposed byte is initialized;
/// length changes require ownership after the I/O token has returned the payload.
unsafe impl IoBuf for Payload {
    fn stable_ptr(&self) -> *const u8 {
        self.as_ptr()
    }
    fn bytes_len(&self) -> usize {
        self.len()
    }
}

/// Exclusive ownership protects the initialized allocation until completion or rundown.
unsafe impl IoBufMut for Payload {
    fn stable_mut_ptr(&mut self) -> *mut u8 {
        self.as_mut_ptr()
    }
}
