//! Owned overlapped-capable endpoints and controlled association provenance.
//!
//! An endpoint is a Windows handle that has been established for overlapped I/O
//! but not yet associated with a completion backend. Association is a later,
//! consuming transition; this module models only ownership and the provenance
//! that must hold before an endpoint may be trusted for safe completion routing.

use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};

/// An overlapped-capable endpoint that has not yet been associated with a
/// completion backend.
///
/// The endpoint owns its handle and closes it on drop. It is intentionally not
/// `Clone`: a second owner could route completions through a duplicate handle
/// and break the single-association invariant that the completion backends rely
/// on.
#[derive(Debug)]
pub struct UnassociatedEndpoint {
    handle: OwnedHandle,
}

impl UnassociatedEndpoint {
    /// Wrap an owned handle whose overlapped provenance the caller vouches for.
    ///
    /// This is the narrow unsafe seam that exists until the crate offers safe
    /// endpoint creators. Callers use it to assert the invariants that a safe
    /// constructor would otherwise establish.
    ///
    /// # Safety
    ///
    /// The caller guarantees that:
    ///
    /// - the handle was opened for overlapped I/O (for example a file opened
    ///   with `FILE_FLAG_OVERLAPPED`, or an overlapped-capable socket handle);
    /// - the handle is not already associated with any completion port;
    /// - no duplicate of the handle exists that could generate competing
    ///   completions for the same operations; and
    /// - ownership is transferred exclusively into the returned endpoint.
    #[must_use]
    pub unsafe fn assume_overlapped(handle: OwnedHandle) -> Self {
        Self { handle }
    }

    /// Borrow the underlying handle for the duration of a native call.
    ///
    /// The borrow cannot outlive the endpoint and does not confer ownership, so
    /// it cannot be used to establish a competing completion association.
    #[must_use]
    pub fn handle(&self) -> BorrowedHandle<'_> {
        self.handle.as_handle()
    }

    /// Consume the endpoint and recover the owned handle.
    ///
    /// This abandons the overlapped-endpoint invariants; the recovered handle is
    /// an ordinary [`OwnedHandle`] again.
    #[must_use]
    pub fn into_handle(self) -> OwnedHandle {
        self.handle
    }
}

#[cfg(test)]
mod tests {
    use super::UnassociatedEndpoint;
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, OwnedHandle};

    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

    #[test]
    fn borrows_and_reclaims_the_same_handle() {
        let path = std::env::temp_dir().join(format!(
            "windows-overlapped-io-sys-{}.tmp",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .custom_flags(FILE_FLAG_OVERLAPPED)
            .open(&path)
            .expect("create overlapped temp file");
        let owned = OwnedHandle::from(file);
        let expected = owned.as_raw_handle();

        // SAFETY: the file was just created with FILE_FLAG_OVERLAPPED, is not
        // associated with any completion port, has no duplicates, and its
        // ownership moves into the endpoint.
        let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
        assert_eq!(endpoint.handle().as_raw_handle(), expected);

        let recovered = endpoint.into_handle();
        assert_eq!(recovered.as_raw_handle(), expected);
        drop(recovered);

        let _ = std::fs::remove_file(&path);
    }
}
