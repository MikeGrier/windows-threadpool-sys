// Copyright (c) 2026 Mike Grier
//! The NUMA node a volume resides on: `FSCTL_QUERY_VOLUME_NUMA_INFO`.

use std::io;
use std::mem::size_of;
use std::os::windows::io::AsHandle;

use windows_sys::Win32::System::Ioctl::FSCTL_QUERY_VOLUME_NUMA_INFO;

use crate::device_control::device_control;

#[cfg(test)]
mod tests;

/// The size of `FSCTL_QUERY_VOLUME_NUMA_INFO_OUTPUT`, which is a single `ULONG`, `NumaNode`.
///
/// The structure is declared in `ntifs.h` and, in `windows-sys`, only under its driver-kit
/// namespace, so this crate states its one field here instead of taking that feature for a `u32`.
/// The documentation says an output buffer that is too small fails with
/// `ERROR_INSUFFICIENT_BUFFER`, so a structure that grew would be reported and not truncated.
const OUTPUT_SIZE: usize = size_of::<u32>();

/// The NUMA node the volume that `handle` is on resides on, as the device wrote it.
///
/// `handle` names a file or a directory on the volume, or the volume itself, and needs no access to
/// it beyond what opening it took. The documented meaning is the node of the *volume that contains*
/// the file or directory: it says nothing of where a file's extents are, and a volume that spans
/// several disks still gets one answer, which is not evidence about each of them.
///
/// The value is carried unaltered. The documentation does not say what a volume that advertises no
/// proximity domain answers, so nothing here decides that a particular value, `u32::MAX` included,
/// means "no node"; that is for the caller.
///
/// # Errors
///
/// The device's own error, with its code exactly: a handle that is not on a volume that answers,
/// such as the `NUL` device, is refused, and so is a volume that advertises no proximity domain,
/// whatever code it answers with. [`io::ErrorKind::InvalidData`] for a call that succeeded and says
/// it wrote other than the one `ULONG` the structure is, which is not read: a value taken from a
/// buffer the device did not fill would read as a node.
pub fn volume_numa_node(handle: &impl AsHandle) -> io::Result<u32> {
    let mut output = [0_u8; OUTPUT_SIZE];
    let written = device_control(handle, FSCTL_QUERY_VOLUME_NUMA_INFO, &[], &mut output)?;
    decode(written, output)
}

/// Reads the structure out of what the call wrote, if it wrote all of it and only it.
fn decode(written: usize, output: [u8; OUTPUT_SIZE]) -> io::Result<u32> {
    if written != OUTPUT_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "FSCTL_QUERY_VOLUME_NUMA_INFO wrote {written} bytes, not the {OUTPUT_SIZE} its \
                 output structure is"
            ),
        ));
    }
    Ok(u32::from_ne_bytes(output))
}
