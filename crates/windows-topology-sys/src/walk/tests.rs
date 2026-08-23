// Copyright (c) 2026 Mike Grier
use super::*;

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
    let decoded = unsafe { decode(std::ptr::null(), 0) };
    assert!(decoded.is_empty());
}

#[test]
fn decodes_a_processor_core_with_a_single_group_mask() {
    let record = processor_record(RelationProcessorCore, 0x1, 3, &[(0, 0b101)]);
    // SAFETY: `record` is a well-formed single record whose `Size` equals its
    // own length.
    let decoded = unsafe { decode(record.as_ptr(), record.len() as u32) };
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
    let decoded = unsafe { decode(record.as_ptr(), record.len() as u32) };
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
    let decoded = unsafe { decode(buf.as_ptr(), buf.len() as u32) };
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
    let decoded = unsafe { decode(record.as_ptr(), record.len() as u32) };
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
fn decodes_a_group_relationship_with_multiple_groups() {
    let record = group_record(&[(64, 64, usize::MAX), (32, 16, 0xFFFF)]);
    // SAFETY: a well-formed single record.
    let decoded = unsafe { decode(record.as_ptr(), record.len() as u32) };
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
    let decoded = unsafe { decode(record.as_ptr(), record.len() as u32) };
    assert!(matches!(decoded[0], Record::Unknown(999)));
}
