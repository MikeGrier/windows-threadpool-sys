// Copyright (c) 2026 Mike Grier
use super::*;

/// Build a byte buffer of `CpuSetInformation` records the way Windows lays them
/// out, so `decode` is exercised against real record geometry rather than
/// against a `Vec<CpuSet>` the test built directly.
fn encode(records: &[SYSTEM_CPU_SET_INFORMATION]) -> Vec<u64> {
    let size = size_of::<SYSTEM_CPU_SET_INFORMATION>();
    let mut storage = vec![0_u64; size_of_val(records).div_ceil(8).max(1)];
    let base = storage.as_mut_ptr().cast::<u8>();
    for (index, record) in records.iter().enumerate() {
        // SAFETY: `storage` was sized to hold every record end to end.
        unsafe {
            base.add(index * size)
                .cast::<SYSTEM_CPU_SET_INFORMATION>()
                .write_unaligned(*record);
        }
    }
    storage
}

fn record(
    id: u32,
    group: u16,
    number: u8,
    llc: u8,
    class: u8,
    flags: u8,
) -> SYSTEM_CPU_SET_INFORMATION {
    let mut raw = SYSTEM_CPU_SET_INFORMATION {
        Size: size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32,
        Type: CpuSetInformation,
        ..Default::default()
    };
    // Writing a union field is safe; only reading one is not. The storage is
    // already zeroed, and only the `CpuSet` arm is ever read back.
    raw.Anonymous.CpuSet.Id = id;
    raw.Anonymous.CpuSet.Group = group;
    raw.Anonymous.CpuSet.LogicalProcessorIndex = number;
    raw.Anonymous.CpuSet.CoreIndex = number / 2;
    raw.Anonymous.CpuSet.LastLevelCacheIndex = llc;
    raw.Anonymous.CpuSet.NumaNodeIndex = 0;
    raw.Anonymous.CpuSet.EfficiencyClass = class;
    raw.Anonymous.CpuSet.Anonymous1.AllFlags = flags;
    raw.Anonymous.CpuSet.AllocationTag = u64::from(id) << 32;
    raw
}

fn decode_all(storage: &[u64], length: u32) -> Vec<CpuSet> {
    // SAFETY: `storage` holds `length` initialized bytes of consecutive records.
    unsafe { decode(storage.as_ptr().cast::<u8>(), length) }
}

#[test]
fn every_field_survives_the_walk() {
    let size = size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32;
    let storage = encode(&[record(256, 1, 5, 3, 2, flags::PARKED | flags::REAL_TIME)]);
    let decoded = decode_all(&storage, size);

    assert_eq!(decoded.len(), 1);
    let r = decoded[0];
    assert_eq!(r.id, 256);
    assert_eq!(r.group, 1);
    assert_eq!(r.logical_processor_index, 5);
    assert_eq!(r.core_index, 2);
    assert_eq!(r.last_level_cache_index, 3);
    assert_eq!(r.numa_node_index, 0);
    assert_eq!(r.efficiency_class, 2);
    assert_eq!(r.allocation_tag, 256_u64 << 32);
}

#[test]
fn each_flag_is_read_from_its_own_bit() {
    // Written as one test per bit rather than one combined value, because a
    // wrong shift is invisible when several bits are set at once.
    let size = size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32;
    for (bit, name) in [
        (flags::PARKED, "parked"),
        (flags::ALLOCATED, "allocated"),
        (
            flags::ALLOCATED_TO_TARGET_PROCESS,
            "allocated_to_target_process",
        ),
        (flags::REAL_TIME, "real_time"),
    ] {
        let storage = encode(&[record(0, 0, 0, 0, 0, bit)]);
        let r = decode_all(&storage, size)[0];
        let observed = [
            ("parked", r.parked),
            ("allocated", r.allocated),
            ("allocated_to_target_process", r.allocated_to_target_process),
            ("real_time", r.real_time),
        ];
        for (which, value) in observed {
            assert_eq!(
                value,
                which == name,
                "with only {name} set, {which} read as {value}"
            );
        }
    }
}

#[test]
fn a_zero_size_record_stops_the_walk_rather_than_looping() {
    // Windows does not emit one. Trusting that is how a corrupt buffer becomes
    // a hang instead of a stop, so the guard is asserted rather than assumed.
    let mut raw = record(1, 0, 0, 0, 0, 0);
    raw.Size = 0;
    let storage = encode(&[raw]);
    let decoded = decode_all(&storage, size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32);
    assert!(decoded.is_empty(), "a zero-size record must end the walk");
}

#[test]
fn a_record_claiming_more_than_the_buffer_holds_is_refused() {
    let mut raw = record(1, 0, 0, 0, 0, 0);
    raw.Size = size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32 * 4;
    let storage = encode(&[raw]);
    let decoded = decode_all(&storage, size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32);
    assert!(
        decoded.is_empty(),
        "a record overrunning the reported length must not be read"
    );
}

#[test]
fn an_unrecognised_record_type_is_skipped_without_stopping() {
    // The walk advances by `Size` whatever the type is, so a future record kind
    // must not truncate the enumeration at the first one Windows adds.
    let size = size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32;
    let mut unknown = record(1, 0, 0, 0, 0, 0);
    unknown.Type = CpuSetInformation + 1;
    let storage = encode(&[unknown, record(2, 0, 1, 0, 0, 0)]);

    let decoded = decode_all(&storage, size * 2);
    assert_eq!(decoded.len(), 1, "the unknown record is skipped, not fatal");
    assert_eq!(decoded[0].id, 2, "and the record after it is still read");
}

#[test]
fn several_records_are_walked_in_order() {
    let size = size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32;
    let storage = encode(&[
        record(10, 0, 0, 0, 0, 0),
        record(11, 0, 1, 0, 0, 0),
        record(12, 0, 2, 1, 0, 0),
    ]);
    let decoded = decode_all(&storage, size * 3);

    assert_eq!(
        decoded.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![10, 11, 12]
    );
    assert_eq!(decoded[2].last_level_cache_index, 1);
}

#[test]
fn a_truncated_trailing_record_is_dropped_rather_than_read() {
    // `actual_length` is what the API wrote, and a record straddling its end is
    // not a record. Reported as fewer records, never as a partial one.
    let size = size_of::<SYSTEM_CPU_SET_INFORMATION>() as u32;
    let storage = encode(&[record(1, 0, 0, 0, 0, 0), record(2, 0, 1, 0, 0, 0)]);
    let decoded = decode_all(&storage, size + size / 2);

    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].id, 1);
}

#[test]
fn enumerating_the_running_system_agrees_with_itself() {
    // The only test that touches the real API. It cannot assert a machine's
    // shape, so it asserts internal consistency instead: ids are unique, and
    // every record names a group and processor number that could exist.
    let records = enumerate().expect("enumerating cpu sets on a live system");
    assert!(
        !records.is_empty(),
        "a running Windows system reports at least one cpu set"
    );

    let mut ids: Vec<u32> = records.iter().map(|r| r.id).collect();
    ids.sort_unstable();
    let unique = {
        let mut copy = ids.clone();
        copy.dedup();
        copy.len()
    };
    assert_eq!(unique, ids.len(), "cpu set ids must be unique");

    for r in &records {
        assert!(
            usize::from(r.logical_processor_index) < usize::BITS as usize,
            "processor {} exceeds a group's maximum",
            r.logical_processor_index
        );
    }
}

#[test]
fn windows_llc_grouping_is_not_the_derived_partitioning_cache() {
    // Measured on the x64 development host, and kept because the two numbers
    // differ for a reason a merge would destroy. CPU Sets reports **one**
    // distinct `LastLevelCacheIndex` across all sixteen processors -- the L3
    // that spans the machine -- while `outermost_partitioning_cache` reports
    // eight partitions at L2. Both are right: Windows names the *last* level,
    // and the derivation names the outermost level that *divides*.
    //
    // A reconciliation that treated `LastLevelCacheIndex` as "the cache domain"
    // would therefore collapse eight groups into one on this machine, which is
    // why SH-16.13 is a decision rather than a cleanup.
    //
    // Asserted as a *relationship* rather than as the host's numbers, so this
    // does not fail on a machine with a different shape: wherever both are
    // known, Windows's LLC grouping is never finer than the derived one, since
    // the last level is at or outside whatever level first divides the machine.
    let records = enumerate().expect("cpu sets");
    let topo = crate::Topology::discover().expect("discover");

    let mut llc: Vec<u8> = records.iter().map(|r| r.last_level_cache_index).collect();
    llc.sort_unstable();
    llc.dedup();

    let derived = topo
        .outermost_partitioning_cache()
        .map_or(1, |(_, partitions)| partitions.len());

    assert!(
        llc.len() <= derived,
        "Windows reports {} last-level-cache groups against {derived} derived partitions; the \
         last level cannot divide the machine more finely than the outermost dividing level",
        llc.len()
    );
}
