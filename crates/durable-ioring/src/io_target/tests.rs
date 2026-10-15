// Copyright (c) 2026 Mike Grier
//! A target's geometry and description.

use super::{Capabilities, Geometry, GeometryError, IoTarget, IoTargetDescription, WarrantyClass};
use crate::memory::MemoryTarget;
use crate::types::FlushDomain;

#[test]
fn every_power_of_two_is_an_alignment() {
    for shift in 0..32 {
        let alignment = 1u32 << shift;
        let geometry = Geometry::uniform(alignment).expect("a power of two");
        assert_eq!(geometry.offset_alignment(), alignment);
        assert_eq!(geometry.length_alignment(), alignment);
        assert_eq!(geometry.buffer_alignment(), alignment);
    }
}

#[test]
fn zero_and_non_powers_of_two_are_refused() {
    for alignment in [0, 3, 6, 12, 513, 4097, u32::MAX] {
        assert_eq!(
            Geometry::uniform(alignment),
            Err(GeometryError { alignment }),
            "{alignment}"
        );
    }
}

#[test]
fn new_names_the_first_bad_alignment_in_order() {
    assert_eq!(Geometry::new(3, 5, 7), Err(GeometryError { alignment: 3 }));
    assert_eq!(Geometry::new(4, 5, 7), Err(GeometryError { alignment: 5 }));
    assert_eq!(Geometry::new(4, 8, 7), Err(GeometryError { alignment: 7 }));
}

#[test]
fn each_alignment_is_kept_apart() {
    let geometry = Geometry::new(512, 4096, 8).expect("powers of two");
    assert_eq!(geometry.offset_alignment(), 512);
    assert_eq!(geometry.length_alignment(), 4096);
    assert_eq!(geometry.buffer_alignment(), 8);
}

#[test]
fn unaligned_admits_everything() {
    let geometry = Geometry::UNALIGNED;
    assert_eq!(Geometry::default(), geometry);
    for (offset, length) in [(0, 0), (1, 1), (3, 7), (u64::MAX, u64::MAX)] {
        assert!(geometry.admits(offset, length));
    }
    assert!(geometry.admits_buffer(1));
    assert!(geometry.admits_buffer(usize::MAX));
}

#[test]
fn admits_checks_offset_and_length_each_against_its_own_alignment() {
    let geometry = Geometry::new(512, 4096, 1).expect("powers of two");
    assert!(geometry.admits(0, 0));
    assert!(geometry.admits(512, 4096));
    assert!(geometry.admits(1024, 8192));
    assert!(!geometry.admits(256, 4096), "an unaligned offset");
    assert!(
        !geometry.admits(512, 512),
        "a length aligned only to the offset's alignment"
    );
    assert!(!geometry.admits(4096, 4095), "an unaligned length");
}

#[test]
fn admits_buffer_checks_the_address() {
    let geometry = Geometry::new(1, 1, 64).expect("powers of two");
    assert!(geometry.admits_buffer(0));
    assert!(geometry.admits_buffer(128));
    assert!(!geometry.admits_buffer(32));
    assert!(!geometry.admits_buffer(65));
}

#[test]
fn a_geometry_error_names_its_alignment() {
    assert_eq!(
        GeometryError { alignment: 6 }.to_string(),
        "alignment 6 is not a power of two"
    );
}

#[test]
fn capabilities_start_empty_and_gain_write_through() {
    assert!(!Capabilities::NONE.write_through());
    assert_eq!(Capabilities::default(), Capabilities::NONE);
    assert!(Capabilities::NONE.with_write_through().write_through());
}

#[test]
fn a_description_is_everything_the_target_states() {
    let domains = [
        FlushDomain::new(b"disk-0".as_slice()),
        FlushDomain::new(b"disk-1".as_slice()),
    ];
    let geometry = Geometry::uniform(4096).expect("a power of two");
    let target = MemoryTarget::new()
        .with_domains(domains.clone())
        .with_warranty(WarrantyClass::SharedDevice)
        .with_geometry(geometry)
        .with_capabilities(Capabilities::NONE.with_write_through());
    assert_eq!(
        target.describe(),
        IoTargetDescription {
            flush_domains: domains.to_vec(),
            warranty: WarrantyClass::SharedDevice,
            geometry,
            capabilities: Capabilities::NONE.with_write_through(),
        }
    );
}

#[test]
fn a_target_with_no_domains_describes_none() {
    let description = MemoryTarget::new().describe();
    assert!(description.flush_domains.is_empty());
    assert_eq!(description.warranty, WarrantyClass::Complete);
    assert_eq!(description.geometry, Geometry::UNALIGNED);
    assert_eq!(description.capabilities, Capabilities::NONE);
}
