// Copyright (c) 2026 Mike Grier
#![doc = include_str!("../README.md")]
#![cfg(windows)]
#![deny(missing_docs)]
#![deny(unsafe_op_in_unsafe_fn)]

mod device_control;
mod extents;
mod geometry;
mod lock;
mod storage_dependency;
mod storage_numa;
mod storage_property;
mod storage_query;
mod storage_unique_id;
#[cfg(test)]
mod test_support;
mod volume_numa;

pub use extents::{DiskExtent, volume_disk_extents};
pub use geometry::bytes_per_sector;
pub use lock::{lock_volume, unlock_volume};
pub use storage_dependency::{
    DependentDiskFlags, StorageDependency, StorageDependencyFlags, StorageDependencyInfo,
    StorageDependencyVersion, VirtualStorageType, storage_dependency_information,
    storage_dependency_information_with_capacity,
};
pub use storage_numa::{StorageDeviceNuma, storage_device_numa};
pub use storage_query::{StoragePropertyId, StorageQueryType, query_storage_property};
pub use storage_unique_id::{StorageDeviceUniqueId, storage_device_unique_id};
pub use volume_numa::volume_numa_node;
