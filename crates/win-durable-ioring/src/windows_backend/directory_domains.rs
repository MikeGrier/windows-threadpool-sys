// Copyright (c) 2026 Mike Grier
//! The flush domains a file created in a directory will have, predicted before it is created, and a
//! file's own checked against the prediction (DI-D-118).
//!
//! A consumer presents storage roots as directories and creates files in them by name, and may want
//! to know where those files will land before creating them -- a log apart from data, say. The
//! prediction opens the directory through `windows-namespace-request-sys`' `OpenFile`, with no
//! access and `FILE_FLAG_BACKUP_SEMANTICS`, which is what opening a directory needs, and runs the
//! same derivation a file's open runs ([`file_domains`](super::file_domains), DI-D-117) on that
//! handle, so the prediction and the file's domains are one rule and the same bytes.
//!
//! **A prediction is never a declaration.** A file's domains are always what its own open derived.
//! The directory's path is resolved again at each create, so a junction swapped between the two
//! sends the file elsewhere; [`DirectoryDomains::confirm`] is how a consumer learns that happened.

use std::fmt;
use std::io;

use durable_ioring::{DataAccess, FlushDomain};
use win_shared_os_owned_handle::SharedHandle;
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, OPEN_EXISTING};
use wtf_string::Wtf16String;

use super::file_domains::{self, StorageStack};
use super::opened_file::open_request::{self, Parameters};
use super::opened_file::{Buffering, OpenedFile};
use super::share_mode::ShareMode;

#[cfg(test)]
mod tests;

/// What the prediction asks of `CreateFileW` for the directory: no access, which is enough to read
/// its final path, shared with everyone, and the flag a directory is opened with.
const PROBE: Parameters = Parameters {
    access: 0,
    disposition: OPEN_EXISTING,
    flags: FILE_FLAG_BACKUP_SEMANTICS,
    share: ShareMode::ALL.bits(),
};

/// The flush domains predicted for files created in a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryDomains {
    directory: Wtf16String,
    domains: Vec<FlushDomain>,
}

impl DirectoryDomains {
    /// Predict the flush domains a file created in `directory` will have: the disks of the
    /// directory's volume, mount points included, found as a file's are (DI-D-117).
    ///
    /// **This blocks for as long as opening the directory does**, which on a network path can be a
    /// long time; a caller that cannot block calls it on a thread that can.
    ///
    /// # Errors
    ///
    /// The open's own error, such as `ERROR_FILE_NOT_FOUND`; and, where a file's open would find no
    /// domains -- a directory on a share, or a volume that could not be read -- the reason, since a
    /// prediction that could not be made is worth saying and not worth guessing.
    pub fn predict(directory: Wtf16String) -> io::Result<Self> {
        let request = open_request::request(directory.clone(), PROBE)?;
        let handle = SharedHandle::from(open_request::perform(&request)?);
        Self::from_handle(&file_domains::Windows, directory, &handle)
    }

    /// The prediction for the directory open as `handle`, from the storage `stack`.
    pub(super) fn from_handle(
        stack: &impl StorageStack,
        directory: Wtf16String,
        handle: &SharedHandle,
    ) -> io::Result<Self> {
        let domains = file_domains::derive(stack, handle)?;
        Ok(Self { directory, domains })
    }

    /// The directory, as it was given.
    #[must_use]
    pub fn directory(&self) -> &Wtf16String {
        &self.directory
    }

    /// The domains predicted, in the order a file's are found.
    #[must_use]
    pub fn domains(&self) -> &[FlushDomain] {
        &self.domains
    }

    /// Check `file`'s own domains, found when it was opened, against the prediction.
    ///
    /// # Errors
    ///
    /// [`DomainMismatch`] when they differ: the file landed somewhere the directory was not when it
    /// was predicted -- a junction swapped, a volume remounted -- or its open found none, as a file on
    /// a share does. The file's domains are what it declares either way.
    pub fn confirm<B: Buffering, A: DataAccess>(
        &self,
        file: &OpenedFile<B, A>,
    ) -> Result<(), DomainMismatch> {
        if file.flush_domains() == self.domains.as_slice() {
            Ok(())
        } else {
            Err(DomainMismatch {
                predicted: self.domains.clone(),
                found: file.flush_domains().to_vec(),
            })
        }
    }
}

/// A file's flush domains were not the ones predicted for its directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainMismatch {
    /// What the directory predicted.
    pub predicted: Vec<FlushDomain>,
    /// What the file's open found; empty when it found none.
    pub found: Vec<FlushDomain>,
}

impl fmt::Display for DomainMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the file's flush domains are not the ones predicted for its directory: predicted {}, found {}",
            self.predicted.len(),
            self.found.len()
        )
    }
}

impl std::error::Error for DomainMismatch {}
