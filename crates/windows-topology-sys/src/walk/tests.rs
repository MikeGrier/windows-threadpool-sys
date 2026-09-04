// Copyright (c) 2026 Mike Grier
use super::*;

/// The records a buffer decodes to, discarding anomalies -- most tests are
/// about well-formed input and assert the anomaly list separately.
fn decode_records(base: *const u8, length: u32) -> Vec<Record> {
    // SAFETY: the caller passes a buffer of `length` initialized bytes.
    unsafe { decode(base, length) }.0
}

/// What the walk observed about the shape of a buffer.
fn decode_anomalies(base: *const u8, length: u32) -> Vec<crate::EnumerationAnomaly> {
    // SAFETY: the caller passes a buffer of `length` initialized bytes.
    unsafe { decode(base, length) }.1
}

/// Write `value` at `offset` in `buf`, ignoring alignment -- exactly what the
/// production walk must also tolerate, since real Windows buffers offer no
/// alignment guarantee for a mid-buffer record.
fn write_at<T>(buf: &mut [u8], offset: usize, value: T) {
    assert!(
        offset + size_of::<T>() <= buf.len(),
        "write_at out of bounds in test fixture"
    );
    // SAFETY: the bounds check above guarantees `offset + size_of::<T>()`
    // bytes are available in `buf`, and every `T` this test writes (Windows
    // topology structs and plain integers) has no invalid bit pattern.
    unsafe {
        buf.as_mut_ptr()
            .add(offset)
            .cast::<T>()
            .write_unaligned(value)
    };
}

fn group_affinity(group: u16, mask: usize) -> GROUP_AFFINITY {
    GROUP_AFFINITY {
        Mask: mask,
        Group: group,
        Reserved: [0; 3],
    }
}

/// Build a raw `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` record whose body is
/// a `PROCESSOR_RELATIONSHIP`, with `masks.len()` trailing `GROUP_AFFINITY`
/// entries -- deliberately more than the type's own declared `[GROUP_AFFINITY;
/// 1]` when `masks.len() > 1`, which is the case D-1 exists to handle.
fn processor_record(
    relationship: i32,
    flags: u8,
    efficiency_class: u8,
    masks: &[(u16, usize)],
) -> Vec<u8> {
    let array_offset = core::mem::offset_of!(PROCESSOR_RELATIONSHIP, GroupMask);
    let body_len = array_offset + masks.len() * size_of::<GROUP_AFFINITY>();
    let mut buf = vec![0_u8; UNION_OFFSET + body_len];
    write_at(&mut buf, RELATIONSHIP_OFFSET, relationship);
    write_at(&mut buf, SIZE_OFFSET, (UNION_OFFSET + body_len) as u32);
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(PROCESSOR_RELATIONSHIP, Flags),
        flags,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(PROCESSOR_RELATIONSHIP, EfficiencyClass),
        efficiency_class,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(PROCESSOR_RELATIONSHIP, GroupCount),
        masks.len() as u16,
    );
    let array_base = UNION_OFFSET + array_offset;
    for (i, &(group, mask)) in masks.iter().enumerate() {
        write_at(
            &mut buf,
            array_base + i * size_of::<GROUP_AFFINITY>(),
            group_affinity(group, mask),
        );
    }
    buf
}

fn numa_record(node_number: u32, masks: &[(u16, usize)]) -> Vec<u8> {
    let array_offset = core::mem::offset_of!(NUMA_NODE_RELATIONSHIP, Anonymous);
    let body_len = array_offset + masks.len() * size_of::<GROUP_AFFINITY>();
    let mut buf = vec![0_u8; UNION_OFFSET + body_len];
    write_at(&mut buf, RELATIONSHIP_OFFSET, RelationNumaNode);
    write_at(&mut buf, SIZE_OFFSET, (UNION_OFFSET + body_len) as u32);
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(NUMA_NODE_RELATIONSHIP, NodeNumber),
        node_number,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(NUMA_NODE_RELATIONSHIP, GroupCount),
        masks.len() as u16,
    );
    let array_base = UNION_OFFSET + array_offset;
    for (i, &(group, mask)) in masks.iter().enumerate() {
        write_at(
            &mut buf,
            array_base + i * size_of::<GROUP_AFFINITY>(),
            group_affinity(group, mask),
        );
    }
    buf
}

fn cache_record(
    level: u8,
    associativity: u8,
    line_size: u16,
    cache_size: u32,
    cache_type: i32,
    masks: &[(u16, usize)],
) -> Vec<u8> {
    let array_offset = core::mem::offset_of!(CACHE_RELATIONSHIP, Anonymous);
    let body_len = array_offset + masks.len() * size_of::<GROUP_AFFINITY>();
    let mut buf = vec![0_u8; UNION_OFFSET + body_len];
    write_at(&mut buf, RELATIONSHIP_OFFSET, RelationCache);
    write_at(&mut buf, SIZE_OFFSET, (UNION_OFFSET + body_len) as u32);
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(CACHE_RELATIONSHIP, Level),
        level,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(CACHE_RELATIONSHIP, Associativity),
        associativity,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(CACHE_RELATIONSHIP, LineSize),
        line_size,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(CACHE_RELATIONSHIP, CacheSize),
        cache_size,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(CACHE_RELATIONSHIP, Type),
        cache_type,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(CACHE_RELATIONSHIP, GroupCount),
        masks.len() as u16,
    );
    let array_base = UNION_OFFSET + array_offset;
    for (i, &(group, mask)) in masks.iter().enumerate() {
        write_at(
            &mut buf,
            array_base + i * size_of::<GROUP_AFFINITY>(),
            group_affinity(group, mask),
        );
    }
    buf
}

fn group_record(infos: &[(u8, u8, usize)]) -> Vec<u8> {
    let array_offset = core::mem::offset_of!(GROUP_RELATIONSHIP, GroupInfo);
    let body_len = array_offset + infos.len() * size_of::<PROCESSOR_GROUP_INFO>();
    let mut buf = vec![0_u8; UNION_OFFSET + body_len];
    write_at(&mut buf, RELATIONSHIP_OFFSET, RelationGroup);
    write_at(&mut buf, SIZE_OFFSET, (UNION_OFFSET + body_len) as u32);
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(GROUP_RELATIONSHIP, MaximumGroupCount),
        infos.len() as u16,
    );
    write_at(
        &mut buf,
        UNION_OFFSET + core::mem::offset_of!(GROUP_RELATIONSHIP, ActiveGroupCount),
        infos.len() as u16,
    );
    let array_base = UNION_OFFSET + array_offset;
    for (i, &(maximum, active, mask)) in infos.iter().enumerate() {
        let info = PROCESSOR_GROUP_INFO {
            MaximumProcessorCount: maximum,
            ActiveProcessorCount: active,
            Reserved: [0; 38],
            ActiveProcessorMask: mask,
        };
        write_at(
            &mut buf,
            array_base + i * size_of::<PROCESSOR_GROUP_INFO>(),
            info,
        );
    }
    buf
}

fn unknown_record(relationship: i32) -> Vec<u8> {
    let total_len = UNION_OFFSET + 8;
    let mut buf = vec![0_u8; total_len];
    write_at(&mut buf, RELATIONSHIP_OFFSET, relationship);
    write_at(&mut buf, SIZE_OFFSET, total_len as u32);
    buf
}

#[test]
fn an_empty_buffer_decodes_to_no_records() {
    // SAFETY: length 0 requires no initialized bytes, so a null/dangling
    // pointer is never dereferenced.
    let decoded = decode_records(std::ptr::null(), 0);
    assert!(decoded.is_empty());
}

#[test]
fn decodes_a_processor_core_with_a_single_group_mask() {
    let record = processor_record(RelationProcessorCore, 0x1, 3, &[(0, 0b101)]);
    // SAFETY: `record` is a well-formed single record whose `Size` equals its
    // own length.
    let decoded = decode_records(record.as_ptr(), record.len() as u32);
    assert_eq!(decoded.len(), 1);
    let Record::ProcessorCore(body) = &decoded[0] else {
        panic!("expected ProcessorCore")
    };
    assert_eq!(body.flags, 0x1);
    assert_eq!(body.efficiency_class, 3);
    assert_eq!(body.group_masks.len(), 1);
    assert_eq!(body.group_masks[0].group, 0);
    assert_eq!(body.group_masks[0].mask, 0b101);
}

#[test]
fn reads_past_the_types_declared_array_length_when_group_count_exceeds_one() {
    // PROCESSOR_RELATIONSHIP declares GroupMask as [GROUP_AFFINITY; 1], but a
    // system with more than 64 processors reports GroupCount > 1. This is the
    // exact case D-1 exists to handle correctly rather than silently
    // truncating or panicking.
    let record = processor_record(RelationProcessorCore, 0, 0, &[(0, 0b1), (5, 0b1010)]);
    // SAFETY: as above.
    let decoded = decode_records(record.as_ptr(), record.len() as u32);
    let Record::ProcessorCore(body) = &decoded[0] else {
        panic!("expected ProcessorCore")
    };
    assert_eq!(body.group_masks.len(), 2);
    assert_eq!(
        (body.group_masks[0].group, body.group_masks[0].mask),
        (0, 0b1)
    );
    assert_eq!(
        (body.group_masks[1].group, body.group_masks[1].mask),
        (5, 0b1010)
    );
}

#[test]
fn the_walk_advances_by_each_records_own_size_across_differing_record_sizes() {
    let mut buf = processor_record(RelationProcessorCore, 0, 0, &[(0, 1)]);
    let numa = numa_record(7, &[(0, 0b11), (1, 0b1)]);
    assert_ne!(
        buf.len(),
        numa.len(),
        "the test must actually exercise differing record sizes"
    );
    let first_len = buf.len();
    buf.extend_from_slice(&numa);
    // SAFETY: two well-formed, back-to-back records whose `Size` fields sum
    // to `buf.len()`.
    let decoded = decode_records(buf.as_ptr(), buf.len() as u32);
    assert_eq!(
        decoded.len(),
        2,
        "record boundary should be at {first_len}, not a fixed stride"
    );
    assert!(matches!(decoded[0], Record::ProcessorCore(_)));
    let Record::NumaNode(body) = &decoded[1] else {
        panic!("expected NumaNode")
    };
    assert_eq!(body.node_number, 7);
    assert_eq!(body.group_masks.len(), 2);
}

#[test]
fn decodes_a_cache_relationship_and_maps_its_type() {
    let record = cache_record(3, 0xFF, 64, 32 * 1024 * 1024, CacheUnified, &[(0, 0xFF)]);
    // SAFETY: a well-formed single record.
    let decoded = decode_records(record.as_ptr(), record.len() as u32);
    let Record::Cache(body) = &decoded[0] else {
        panic!("expected Cache")
    };
    assert_eq!(body.level, 3);
    assert_eq!(body.associativity, 0xFF);
    assert_eq!(body.line_size, 64);
    assert_eq!(body.cache_size, 32 * 1024 * 1024);
    assert_eq!(
        cache_kind_from_raw(body.cache_type),
        crate::CacheKind::Unified
    );
}

#[test]
fn cache_kind_from_raw_preserves_an_unrecognised_value() {
    assert_eq!(cache_kind_from_raw(42), crate::CacheKind::Other(42));
}

#[test]
fn a_cache_relationship_reporting_group_count_zero_still_reads_its_legacy_group_mask() {
    // Pre-Windows-20H2 systems always report GroupCount == 0 for
    // CACHE_RELATIONSHIP, yet the union at that same offset holds exactly
    // one legacy GROUP_AFFINITY -- decoding this as "zero groups" would
    // silently drop it.
    let mut record = cache_record(2, 8, 64, 1024 * 1024, CacheData, &[(3, 0b110)]);
    write_at(
        &mut record,
        UNION_OFFSET + core::mem::offset_of!(CACHE_RELATIONSHIP, GroupCount),
        0_u16,
    );
    // SAFETY: a well-formed single record; only its GroupCount field was
    // overwritten to simulate the legacy layout, its one trailing
    // GROUP_AFFINITY entry is left intact.
    let decoded = decode_records(record.as_ptr(), record.len() as u32);
    let Record::Cache(body) = &decoded[0] else {
        panic!("expected Cache")
    };
    assert_eq!(body.group_masks.len(), 1);
    assert_eq!(
        (body.group_masks[0].group, body.group_masks[0].mask),
        (3, 0b110)
    );
}

#[test]
fn a_numa_node_relationship_reporting_group_count_zero_still_reads_its_legacy_group_mask() {
    let mut record = numa_record(9, &[(2, 0b1010)]);
    write_at(
        &mut record,
        UNION_OFFSET + core::mem::offset_of!(NUMA_NODE_RELATIONSHIP, GroupCount),
        0_u16,
    );
    // SAFETY: as above, only GroupCount was overwritten.
    let decoded = decode_records(record.as_ptr(), record.len() as u32);
    let Record::NumaNode(body) = &decoded[0] else {
        panic!("expected NumaNode")
    };
    assert_eq!(body.node_number, 9);
    assert_eq!(body.group_masks.len(), 1);
    assert_eq!(
        (body.group_masks[0].group, body.group_masks[0].mask),
        (2, 0b1010)
    );
}

#[test]
fn decodes_a_group_relationship_with_multiple_groups() {
    let record = group_record(&[(64, 64, usize::MAX), (32, 16, 0xFFFF)]);
    // SAFETY: a well-formed single record.
    let decoded = decode_records(record.as_ptr(), record.len() as u32);
    let Record::Group(body) = &decoded[0] else {
        panic!("expected Group")
    };
    assert_eq!(body.group_info.len(), 2);
    assert_eq!(body.group_info[0].maximum_processor_count, 64);
    assert_eq!(body.group_info[0].active_processor_mask, usize::MAX);
    assert_eq!(body.group_info[1].active_processor_count, 16);
}

#[test]
fn an_unrecognised_relationship_is_carried_rather_than_dropped() {
    let record = unknown_record(999);
    // SAFETY: a well-formed single record with no variable-length body.
    let decoded = decode_records(record.as_ptr(), record.len() as u32);
    assert!(matches!(decoded[0], Record::Unknown(999)));
}

// The malformed-input cases this file never had. Per D-24 none of them may
// panic, none may read past the buffer, and each is reported rather than
// silently dropped.

#[test]
fn a_zero_size_record_is_reported_rather_than_panicking() {
    // This is the case that used to hit `assert!(size > 0)` and take the
    // caller's process with it.
    let mut record = processor_record(RelationProcessorCore, 0, 0, &[(0, 0b1)]);
    record[SIZE_OFFSET..SIZE_OFFSET + 4].copy_from_slice(&0_u32.to_le_bytes());

    let anomalies = decode_anomalies(record.as_ptr(), record.len() as u32);

    assert_eq!(
        anomalies,
        vec![crate::EnumerationAnomaly::undersized(
            Source::RelationshipWalk,
            0,
            0,
            UNION_OFFSET
        )]
    );
}

#[test]
fn a_record_overrunning_the_buffer_stops_the_walk_and_keeps_what_decoded() {
    let mut first = processor_record(RelationProcessorCore, 0, 0, &[(0, 0b1)]);
    let first_len = first.len();
    let mut second = processor_record(RelationProcessorCore, 0, 0, &[(0, 0b10)]);
    // The second record claims far more than the buffer holds.
    second[SIZE_OFFSET..SIZE_OFFSET + 4].copy_from_slice(&4096_u32.to_le_bytes());
    first.extend_from_slice(&second);

    let (records, anomalies) = (
        decode_records(first.as_ptr(), first.len() as u32),
        decode_anomalies(first.as_ptr(), first.len() as u32),
    );

    assert_eq!(records.len(), 1, "the first record still decodes");
    assert_eq!(anomalies.len(), 1);
    assert_eq!(anomalies[0].offset, first_len);
    assert!(matches!(
        anomalies[0].kind,
        crate::AnomalyKind::OverrunsBuffer { declared: 4096, .. }
    ));
}

#[test]
fn a_group_count_larger_than_the_record_reads_only_what_fits() {
    // The amplification: `GroupCount` is a `u16` multiplying a 16-byte stride,
    // so an unbounded read here would reach 1,048,560 bytes past the record.
    // The record's own `Size` is the bound, so only the entries inside it are
    // read and the overclaim is recorded.
    let mut record = processor_record(RelationProcessorCore, 0, 0, &[(0, 0b1)]);
    let count_at = UNION_OFFSET + core::mem::offset_of!(PROCESSOR_RELATIONSHIP, GroupCount);
    record[count_at..count_at + 2].copy_from_slice(&u16::MAX.to_le_bytes());

    let (records, anomalies) = (
        decode_records(record.as_ptr(), record.len() as u32),
        decode_anomalies(record.as_ptr(), record.len() as u32),
    );

    let Record::ProcessorCore(body) = &records[0] else {
        panic!("a processor core record");
    };
    assert_eq!(
        body.group_masks.len(),
        1,
        "only the one entry the record actually holds"
    );
    assert_eq!(anomalies.len(), 1);
    assert!(matches!(
        anomalies[0].kind,
        crate::AnomalyKind::TruncatedArray {
            declared: 65535,
            decoded: 1
        }
    ));
}

#[test]
fn a_record_too_short_for_its_body_yields_no_record_rather_than_a_neighbours_bytes() {
    // Declares a processor relationship but is only long enough for the header.
    let mut record = processor_record(RelationProcessorCore, 0, 0, &[(0, 0b1)]);
    record[SIZE_OFFSET..SIZE_OFFSET + 4].copy_from_slice(&(UNION_OFFSET as u32).to_le_bytes());
    record.truncate(UNION_OFFSET);

    let records = decode_records(record.as_ptr(), record.len() as u32);

    assert!(
        records.is_empty(),
        "no body fits, so no record is invented: {records:?}"
    );
}

#[test]
fn a_healthy_machine_reports_no_anomalies() {
    let (_, anomalies) = enumerate().expect("enumerating the running system");
    assert_eq!(anomalies, Vec::new());
}
