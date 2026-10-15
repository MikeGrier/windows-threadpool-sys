// Copyright (c) 2026 Mike Grier
//! [`Win32FileTarget`]: a target backed by a Win32 file (DI-D-48, DI-D-50, DI-D-60).

use win_shared_os_owned_handle::SharedHandle;

use super::opened_file::{Buffering, OpenedFile};
use durable_ioring::FlushDomain;
use durable_ioring::{Capabilities, Geometry, IoTarget, WarrantyClass};

/// A target backed by a Win32 file this crate opened, held as a `SharedHandle` -- an owning form,
/// never a raw handle (DI-D-4).
///
/// It is made from an [`OpenedFile`] and from nothing else, and takes its warranty class, its
/// alignment and its capabilities from how that file was opened, not from a declaration
/// (DI-D-60): a file opened cached is [`WarrantyClass::BestEffort`] and needs no alignment, and one
/// opened [`Unbuffered`](super::Unbuffered) is [`WarrantyClass::SharedDevice`], needs a sector's
/// alignment, and offers per-write write-through. [`WarrantyClass::Complete`] is never a file's: it
/// needs knowledge of what else shares the device, which no open mode gives. What a target cannot
/// know, its flush domains, is declared with [`with_domains`](Self::with_domains).
#[derive(Clone, Debug)]
pub struct Win32FileTarget {
    handle: SharedHandle,
    domains: Vec<FlushDomain>,
    warranty: WarrantyClass,
    geometry: Geometry,
    capabilities: Capabilities,
}

impl Win32FileTarget {
    /// The target `file` makes, claiming what its open mode allows and no more.
    #[must_use]
    pub fn new<B: Buffering>(file: OpenedFile<B>) -> Self {
        Self {
            geometry: file.geometry(),
            handle: file.handle().clone(),
            domains: Vec::new(),
            warranty: B::WARRANTY,
            capabilities: B::CAPABILITIES,
        }
    }

    /// Declare every flush domain its writes depend on.
    #[must_use]
    pub fn with_domains(mut self, domains: impl IntoIterator<Item = FlushDomain>) -> Self {
        self.domains = domains.into_iter().collect();
        self
    }

    /// The file's handle.
    #[must_use]
    pub fn handle(&self) -> &SharedHandle {
        &self.handle
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
}
