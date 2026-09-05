// Copyright (c) 2026 Mike Grier
use super::*;

/// A record chain with `Size` at offset 0, like `SYSTEM_CPU_SET_INFORMATION`.
const SIZE_AT_0: usize = 0;
const MIN: usize = 8;

/// Build a buffer of records, each `size` bytes, `Size` written at offset 0.
fn chain(sizes: &[u32]) -> Vec<u64> {
    let total: u32 = sizes.iter().sum();
    let mut storage = vec![0_u64; (total as usize).div_ceil(8).max(1)];
    let base = storage.as_mut_ptr().cast::<u8>();
    let mut offset = 0usize;
    for &size in sizes {
        // SAFETY: `storage` was sized to hold every record end to end.
        unsafe { base.add(offset).cast::<u32>().write_unaligned(size) };
        offset += size as usize;
    }
    storage
}

fn walk(storage: &[u64], length: u32) -> RecordWalk {
    // SAFETY: `storage` holds `length` initialized bytes.
    unsafe {
        RecordWalk::new(
            storage.as_ptr().cast(),
            length,
            SIZE_AT_0,
            MIN,
            Source::CpuSets,
        )
    }
}

#[test]
fn an_empty_buffer_yields_nothing_and_no_anomaly() {
    let storage = chain(&[]);
    let mut w = walk(&storage, 0);
    assert!(w.next().is_none());
    assert_eq!(w.anomaly(), None);
}

#[test]
fn well_formed_records_are_walked_in_order_with_no_anomaly() {
    let storage = chain(&[8, 16, 8]);
    let mut w = walk(&storage, 32);
    let sizes: Vec<_> = (&mut w).map(|r| r.size).collect();
    assert_eq!(sizes, vec![8, 16, 8]);
    assert_eq!(w.anomaly(), None);
}

#[test]
fn a_zero_size_record_is_reported_rather_than_looping_or_panicking() {
    // The case that used to `assert!` in `walk.rs`.
    let mut storage = chain(&[8]);
    let base = storage.as_mut_ptr().cast::<u8>();
    // SAFETY: writing the first record's `Size` field, inside the buffer.
    unsafe { base.cast::<u32>().write_unaligned(0) };
    let mut w = walk(&storage, 8);

    assert!(w.next().is_none(), "a zero-size record yields nothing");
    assert_eq!(
        w.anomaly(),
        Some(EnumerationAnomaly::undersized(Source::CpuSets, 0, 0, MIN)),
    );
}

#[test]
fn a_record_overrunning_the_buffer_is_reported_and_earlier_ones_survive() {
    let mut storage = chain(&[8, 8]);
    let base = storage.as_mut_ptr().cast::<u8>();
    // Second record claims more than the buffer holds.
    // SAFETY: offset 8 is the second record's `Size`, inside the buffer.
    unsafe { base.add(8).cast::<u32>().write_unaligned(4096) };
    let mut w = walk(&storage, 16);

    assert_eq!((&mut w).count(), 1, "the first record still decodes");
    assert_eq!(
        w.anomaly(),
        Some(EnumerationAnomaly::overruns(Source::CpuSets, 8, 4096, 8)),
    );
}

#[test]
fn leftover_bytes_too_short_for_a_header_are_reported() {
    let storage = chain(&[8]);
    let mut w = walk(&storage, 10);
    assert_eq!((&mut w).count(), 1);
    assert_eq!(
        w.anomaly(),
        Some(EnumerationAnomaly::trailing_bytes(Source::CpuSets, 8, 2)),
    );
}

#[test]
fn a_read_that_would_leave_the_record_yields_none() {
    let storage = chain(&[8]);
    let mut w = walk(&storage, 8);
    let record = w.next().expect("one record");

    // SAFETY: the record addresses its own 8 initialized bytes.
    unsafe {
        assert!(record.read::<u32>(0).is_some(), "inside");
        assert!(record.read::<u32>(4).is_some(), "flush against the end");
        assert!(record.read::<u32>(5).is_none(), "one byte over");
        assert!(record.read::<u64>(4).is_none(), "spans the end");
        assert!(record.read::<u32>(usize::MAX).is_none(), "cannot overflow");
    }
}

#[test]
fn a_trailing_array_cannot_be_read_past_its_own_record() {
    // The `GroupCount` amplification, in miniature: the count claims far more
    // entries than the record can hold, and the walk hands back only what fit.
    let storage = chain(&[16]);
    let mut w = walk(&storage, 16);
    let record = w.next().expect("one record");

    // SAFETY: the record addresses its own 16 initialized bytes.
    let (entries, complete) = unsafe { record.read_array::<u32>(8, usize::from(u16::MAX)) };

    assert_eq!(entries.len(), 2, "only the two that fit after offset 8");
    assert!(!complete, "and the walk says the count claimed more");
}
