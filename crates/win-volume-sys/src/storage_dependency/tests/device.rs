// Copyright (c) 2026 Mike Grier
//! The size protocol and what the call's status becomes, against a fake device that checks what it
//! is asked.

use windows_sys::Win32::Foundation::{
    ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_NOT_ENOUGH_MEMORY,
};

use super::*;

// --- The size protocol, against a fake device -----------------------------------------------

/// A device that holds `entries`, checks what it is asked, and answers as the documentation says:
/// a buffer too small is `status` with the size needed written, and otherwise the reply.
struct Device {
    version: i32,
    entries: Vec<Built>,
    status: u32,
    sizes: Vec<usize>,
}

impl Device {
    fn new(version: i32, entries: Vec<Built>, status: u32) -> Self {
        Self {
            version,
            entries,
            status,
            sizes: Vec::new(),
        }
    }

    fn ask(&mut self, buffer: &mut [u8], used: *mut u32) -> u32 {
        assert_eq!(buffer.as_ptr().addr() % ALIGNMENT, 0, "an aligned buffer");
        assert!(buffer.len() >= MIN_BYTES, "never a buffer the call refuses");
        let asked = i32::from_ne_bytes(buffer[..4].try_into().expect("four bytes"));
        assert_eq!(asked, self.version, "the version is set before the call");
        self.sizes.push(buffer.len());

        let needed = needed(self.version, &self.entries);
        // SAFETY: `used` is the `u32` the entry lent for this call.
        unsafe { *used = u32::try_from(needed).expect("small") };
        if buffer.len() < needed {
            return self.status;
        }
        fill(buffer, self.version, &self.entries);
        0
    }
}

#[test]
fn a_reply_that_fits_the_first_buffer_is_asked_for_once() {
    let mut device = Device::new(2, vec![distinct(20)], ERROR_INSUFFICIENT_BUFFER);

    let info = dependencies_via(FIRST_BYTES, StorageDependencyVersion::V2, |buffer, used| {
        device.ask(buffer, used)
    })
    .expect("a reply that fits");

    assert_eq!(device.sizes, [FIRST_BYTES]);
    assert_eq!(info.entries().len(), 1);
}

#[test]
fn a_reply_that_does_not_fit_is_asked_for_again_at_the_size_the_call_gave() {
    for status in [ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA] {
        let entries: Vec<Built> = (1..=3).map(distinct).collect();
        let needed = needed(2, &entries);
        let mut device = Device::new(2, entries, status);

        let info = dependencies_via(MIN_BYTES, StorageDependencyVersion::V2, |buffer, used| {
            device.ask(buffer, used)
        })
        .unwrap_or_else(|error| panic!("status {status}: {error}"));

        assert_eq!(device.sizes, [MIN_BYTES, needed], "status {status}");
        assert_eq!(info.entries().len(), 3);
        assert_eq!(
            info.entries()[2].dependent_volume_relative_path(),
            name(r"images\disk.vhdx").as_ref()
        );
    }
}

#[test]
fn a_first_size_below_the_structure_is_raised_for_every_size_under_it() {
    for first in [0, 1, 7, 8, MIN_BYTES - 1] {
        let mut device = Device::new(1, vec![], ERROR_INSUFFICIENT_BUFFER);

        dependencies_via(first, StorageDependencyVersion::V1, |buffer, used| {
            device.ask(buffer, used)
        })
        .unwrap_or_else(|error| panic!("a first size of {first}: {error}"));

        assert_eq!(device.sizes, [MIN_BYTES], "a first size of {first}");
    }
}

#[test]
fn a_call_that_names_a_size_that_is_not_larger_is_reported_and_not_repeated() {
    // Asking again at the same size would be asking the same question: the call has not said
    // what it needs, so the reply cannot be trusted to be coming.
    for named in [0_u32, 1, u32::try_from(MIN_BYTES).expect("small")] {
        let mut calls = 0;
        let result = dependencies_via(MIN_BYTES, StorageDependencyVersion::V2, |_, used| {
            calls += 1;
            // SAFETY: `used` is the `u32` the entry lent for this call.
            unsafe { *used = named };
            ERROR_INSUFFICIENT_BUFFER
        });

        assert!(
            matches!(&result, Err(error) if error.kind() == io::ErrorKind::InvalidData),
            "named {named}: {result:?}"
        );
        assert_eq!(calls, 1, "named {named}");
    }
}

#[test]
fn a_size_that_keeps_growing_is_given_up_on_after_the_attempts_allowed() {
    let mut sizes = Vec::new();
    let result = dependencies_via(MIN_BYTES, StorageDependencyVersion::V2, |buffer, used| {
        sizes.push(buffer.len());
        // SAFETY: `used` is the `u32` the entry lent for this call.
        unsafe { *used = u32::try_from(buffer.len() + 8).expect("small") };
        ERROR_INSUFFICIENT_BUFFER
    });

    let error = result.expect_err("a size that never settles");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("kept changing"), "{error}");
    assert_eq!(sizes.len(), MAX_ATTEMPTS);
    assert!(sizes.windows(2).all(|pair| pair[1] == pair[0] + 8));
}

#[test]
fn any_other_status_is_carried_unaltered_and_is_not_asked_again() {
    for status in [
        NOT_A_VIRTUAL_DISK,
        ERROR_INVALID_LEVEL,
        ERROR_INVALID_PARAMETER,
        5,
        u32::MAX,
    ] {
        let mut calls = 0;
        let result = dependencies_via(FIRST_BYTES, StorageDependencyVersion::V2, |_, _| {
            calls += 1;
            status
        });

        let error = result.expect_err("a refusal");
        assert_eq!(win32_code(&error), status);
        assert_eq!(calls, 1, "status {status}");
    }
}

#[test]
fn a_buffer_that_cannot_be_allocated_is_out_of_memory_and_the_call_is_not_made() {
    let mut calls = 0;
    let result = dependencies_via(usize::MAX, StorageDependencyVersion::V2, |_, _| {
        calls += 1;
        0
    });

    let error = result.expect_err("no buffer of that size");
    assert_eq!(win32_code(&error), ERROR_NOT_ENOUGH_MEMORY);
    assert_eq!(calls, 0);
}

#[test]
fn the_version_asked_for_is_in_the_buffer_the_call_is_given() {
    for raw in [1, 2, 0, 7, -3] {
        let mut seen = None;
        let _ = dependencies_via(
            FIRST_BYTES,
            StorageDependencyVersion::from_raw(raw),
            |buffer, _| {
                seen = Some(i32::from_ne_bytes(
                    buffer[..4].try_into().expect("four bytes"),
                ));
                5
            },
        );
        assert_eq!(seen, Some(raw));
    }
}

#[test]
fn a_new_buffer_after_a_regrow_has_the_version_set_again() {
    let mut device = Device::new(
        1,
        vec![distinct(21), distinct(22), distinct(23)],
        ERROR_MORE_DATA,
    );

    // The fake checks the version on both attempts; a buffer reused or re-zeroed without it
    // would fail there.
    let info = dependencies_via(MIN_BYTES, StorageDependencyVersion::V1, |buffer, used| {
        device.ask(buffer, used)
    })
    .expect("a regrown reply");

    assert_eq!(device.sizes.len(), 2);
    assert_eq!(info.entries().len(), 3);
}
