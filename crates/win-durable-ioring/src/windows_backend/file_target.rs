// Copyright (c) 2026 Mike Grier
//! [`Win32FileTarget`]: a target backed by a Win32 file (DI-D-48, DI-D-50).

use win_shared_os_owned_handle::SharedHandle;

use durable_ioring::FlushDomain;
use durable_ioring::{Capabilities, Geometry, IoTarget, WarrantyClass};

/// A target backed by a Win32 file, held as a `SharedHandle` -- an owning form, never a raw handle
/// (DI-D-4). It states what it is built with: no flush domains,
/// [`WarrantyClass::BestEffort`], [`Geometry::UNALIGNED`] and [`Capabilities::NONE`] unless set.
///
/// The warranty class is taken on the consumer's declaration, and `BestEffort` is the default
/// because a file opened for cached I/O can claim no more: its system write-back can fail without
/// telling its writer. One opened unbuffered can claim `SharedDevice`. Whether dioring should read
/// the class from the handle rather than take it on declaration is `DI-10.1`'s.
#[derive(Clone, Debug)]
pub struct Win32FileTarget {
    handle: SharedHandle,
    domains: Vec<FlushDomain>,
    warranty: WarrantyClass,
    geometry: Geometry,
    capabilities: Capabilities,
}

impl Win32FileTarget {
    /// The file `handle` names.
    #[must_use]
    pub fn new(handle: SharedHandle) -> Self {
        Self {
            handle,
            domains: Vec::new(),
            warranty: WarrantyClass::BestEffort,
            geometry: Geometry::UNALIGNED,
            capabilities: Capabilities::NONE,
        }
    }

    /// Declare every flush domain its writes depend on.
    #[must_use]
    pub fn with_domains(mut self, domains: impl IntoIterator<Item = FlushDomain>) -> Self {
        self.domains = domains.into_iter().collect();
        self
    }

    /// State its warranty class.
    #[must_use]
    pub fn with_warranty(mut self, warranty: WarrantyClass) -> Self {
        self.warranty = warranty;
        self
    }

    /// State the alignment its operations need, which the backend enforces: an unbuffered file's
    /// is its device's sector size.
    #[must_use]
    pub fn with_geometry(mut self, geometry: Geometry) -> Self {
        self.geometry = geometry;
        self
    }

    /// State its capabilities: write-through, for a file that honours it.
    #[must_use]
    pub fn with_capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// The file's handle.
    #[must_use]
    pub fn handle(&self) -> &SharedHandle {
        &self.handle
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
