// Copyright (c) 2026 Mike Grier
//! [`Win32FileTarget`]: a target backed by a Win32 file, a raw volume or a whole disk (DI-D-48,
//! DI-D-50, DI-D-60, DI-D-61, DI-D-62).

use win_shared_os_owned_handle::SharedHandle;

use super::opened_disk::OpenedDisk;
use super::opened_file::{Buffering, OpenedFile};
use super::opened_volume::OpenedVolume;
use durable_ioring::FlushDomain;
use durable_ioring::{Access, Capabilities, Geometry, HasData, IoTarget, WarrantyClass};

/// A target backed by a Win32 file, a raw volume or a whole disk this crate opened, held as a
/// `SharedHandle` -- an owning form, never a raw handle (DI-D-4).
///
/// It is made from an [`OpenedFile`], an [`OpenedVolume`] or an [`OpenedDisk`] and from nothing else, and takes its
/// warranty class, its alignment and its capabilities from how that was opened, not from a
/// declaration (DI-D-60): one opened cached is [`WarrantyClass::BestEffort`], and one opened
/// [`Unbuffered`](super::Unbuffered) is [`WarrantyClass::SharedDevice`], needs a sector's alignment,
/// and offers per-write write-through. [`WarrantyClass::Complete`] is never one's: it needs
/// knowledge of what else writes the device, which a volume's lock does not give (DI-D-61).
///
/// A target needs a data right, so it is made only from a handle whose data access is [`HasData`]: one
/// opened with [`NoData`](super::NoData) is refused when the program is compiled, not when the
/// first operation fails.
///
/// ```compile_fail,E0277
/// use win_durable_ioring::Wtf16String;
/// use win_durable_ioring::windows_backend::{Cached, FileOptions, OpenedFile, Win32FileTarget};
///
/// let name = Wtf16String::from_os_str("a-file".as_ref());
/// let no_data = OpenedFile::<Cached, _>::open(name, FileOptions::new()).unwrap();
/// let _ = Win32FileTarget::new(no_data);
/// ```
///
/// A volume's flush domains are found from its disks. A file's are what it cannot know, so they are
/// declared with [`with_domains`](Self::with_domains); declaring them on a volume adds to the ones
/// found, and never removes one.
#[derive(Clone, Debug)]
pub struct Win32FileTarget {
    handle: SharedHandle,
    /// What the backing found for itself: a volume's disks.
    found: Vec<FlushDomain>,
    /// What the consumer declared.
    declared: Vec<FlushDomain>,
    /// Both, each once: what the target reports.
    domains: Vec<FlushDomain>,
    warranty: WarrantyClass,
    geometry: Geometry,
    capabilities: Capabilities,
    /// What the handle was opened for, stated to the core (DI-D-66).
    access: Access,
}

impl Win32FileTarget {
    /// The target `file` makes, claiming what its open mode allows and no more.
    #[must_use]
    pub fn new<B: Buffering, A: HasData>(file: OpenedFile<B, A>) -> Self {
        Self {
            geometry: file.geometry(),
            handle: file.handle().clone(),
            found: Vec::new(),
            declared: Vec::new(),
            domains: Vec::new(),
            warranty: B::WARRANTY,
            capabilities: B::CAPABILITIES,
            access: A::ACCESS,
        }
    }

    /// The target `volume` makes: its sector's alignment, the flush domains of the disks it sits
    /// on, and what its open mode allows and no more.
    #[must_use]
    pub fn from_volume<B: Buffering, A: HasData>(volume: OpenedVolume<B, A>) -> Self {
        let found = volume.flush_domains();
        Self {
            geometry: volume.geometry(),
            handle: volume.handle().clone(),
            domains: found.clone(),
            found,
            declared: Vec::new(),
            warranty: B::WARRANTY,
            capabilities: B::CAPABILITIES,
            access: A::ACCESS,
        }
    }

    /// The target `disk` makes: its sector's alignment, the disk itself as its flush domain, and
    /// what its open mode allows and no more.
    #[must_use]
    pub fn from_disk<B: Buffering, A: HasData>(disk: OpenedDisk<B, A>) -> Self {
        let found = vec![disk.flush_domain()];
        Self {
            geometry: disk.geometry(),
            handle: disk.handle().clone(),
            domains: found.clone(),
            found,
            declared: Vec::new(),
            warranty: B::WARRANTY,
            capabilities: B::CAPABILITIES,
            access: A::ACCESS,
        }
    }

    /// Declare every flush domain its writes depend on, beyond any the backing found for itself.
    /// Declaring again replaces what was declared before, and never what the backing found.
    #[must_use]
    pub fn with_domains(mut self, domains: impl IntoIterator<Item = FlushDomain>) -> Self {
        self.declared = domains.into_iter().collect();
        self.domains = self.found.clone();
        for domain in &self.declared {
            if !self.domains.contains(domain) {
                self.domains.push(domain.clone());
            }
        }
        self
    }

    /// The file's handle.
    #[must_use]
    pub fn handle(&self) -> &SharedHandle {
        &self.handle
    }

    /// State an access the handle was not opened with, to test what happens when a target
    /// overstates it: the core trusts what a target states, so the kernel's own refusal is what is
    /// left to meet.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn claiming(mut self, access: Access) -> Self {
        self.access = access;
        self
    }

    /// State a class a file's open mode cannot give, to test what the backend does with each.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn declared(
        mut self,
        warranty: WarrantyClass,
        geometry: Geometry,
        capabilities: Capabilities,
    ) -> Self {
        self.warranty = warranty;
        self.geometry = geometry;
        self.capabilities = capabilities;
        self
    }
}

impl IoTarget for Win32FileTarget {
    fn flush_domains(&self) -> &[FlushDomain] {
        &self.domains
    }

    fn warranty(&self) -> WarrantyClass {
        self.warranty
    }

    fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    fn access(&self) -> Access {
        self.access
    }
}
