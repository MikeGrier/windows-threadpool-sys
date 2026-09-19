// Copyright (c) Mike Grier.
use super::*;

fn config() -> Config {
    Config {
        file: "unused".into(),
        block_bytes: 1024,
        depth: 8,
        buffer_count: None,
        batch_size: 1,
        queue_capacity: 4,
        checksum_passes: 1,
        repetitions: 6,
        timeout_ms: 1000,
        processors: None,
    }
}

#[test]
fn ten_normal_shapes_and_short_tails() {
    for (size, depth, queue, blocks, tail) in [
        (1, 2, 1, 2, 0),
        (2, 2, 2, 3, 1),
        (7, 4, 1, 10, 3),
        (16, 4, 2, 9, 0),
        (31, 8, 4, 8, 17),
        (64, 8, 8, 2, 1),
        (128, 16, 1, 50, 127),
        (1024, 32, 16, 16, 0),
        (4096, 64, 32, 3, 57),
        (65536, 128, 64, 20, 0),
    ] {
        let mut c = config();
        c.block_bytes = size;
        c.depth = depth;
        c.queue_capacity = queue;
        assert_eq!(
            c.validate((size * blocks + tail) as u64).unwrap(),
            blocks + usize::from(tail != 0)
        );
    }
}

#[test]
fn invalid_depths_queues_and_sizes() {
    for depth in [0, 1, 3, 6, MAX_BUFFERS * 2] {
        let mut c = config();
        c.depth = depth;
        assert!(c.validate(4096).is_err());
    }
    for capacity in [0, 3, 16] {
        let mut c = config();
        c.queue_capacity = capacity;
        assert!(c.validate(4096).is_err());
    }
    for bytes in [0, 1, 1024, u64::MAX] {
        assert!(config().validate(bytes).is_err());
    }
    for bytes in [0, usize::MAX, MAX_POOL_BYTES] {
        let mut c = config();
        c.block_bytes = bytes;
        assert!(c.validate(4096).is_err());
    }
}

#[test]
fn rejects_invalid_work_limits() {
    for passes in [0, MAX_PASSES + 1] {
        let mut c = config();
        c.checksum_passes = passes;
        assert!(c.validate(4096).is_err());
    }
    for repetitions in [0, MAX_REPETITIONS + 1] {
        let mut c = config();
        c.repetitions = repetitions;
        assert!(c.validate(4096).is_err());
    }
    for timeout in [0, MAX_TIMEOUT_MS + 1] {
        let mut c = config();
        c.timeout_ms = timeout;
        assert!(c.validate(4096).is_err());
    }
}

#[test]
fn known_checksum_vectors_and_rounds() {
    assert_eq!(checksum(b"", 1), HASH_OFFSET);
    assert_eq!(checksum(b"a", 1), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(checksum(b"foobar", 1), 0x8594_4171_f739_67e8);
    assert_eq!(checksum(b"abc", 2), checksum(b"abcabc", 1));
    assert_ne!(checksum(b"abc", 1), checksum(b"abd", 1));
}

#[test]
fn independent_read_pool_and_batch_limits() {
    for depth in [2, 4, 8, 16, 32] {
        for batch_size in [1, 3, 16, MAX_BUFFERS] {
            let mut candidate = config();
            candidate.depth = depth;
            candidate.buffer_count = Some(32);
            candidate.queue_capacity = 32;
            candidate.batch_size = batch_size;
            assert!(candidate.validate(4096).is_ok());
            assert_eq!(candidate.buffers(), 32);
        }
    }
    for buffers in [0, 1, 4, 9, MAX_BUFFERS + 1, usize::MAX] {
        let mut candidate = config();
        candidate.buffer_count = Some(buffers);
        assert!(candidate.validate(4096).is_err());
    }
    for batch_size in [0, MAX_BUFFERS + 1, usize::MAX] {
        let mut candidate = config();
        candidate.batch_size = batch_size;
        assert!(candidate.validate(4096).is_err());
    }
    let mut candidate = config();
    candidate.block_bytes = MAX_POOL_BYTES / 8;
    candidate.buffer_count = Some(16);
    assert!(candidate.validate(MAX_FILE_BYTES).is_err());
}

#[test]
fn legacy_configs_default_to_depth_buffers_and_single_jobs() {
    let mut value = serde_json::to_value(config()).unwrap();
    value.as_object_mut().unwrap().remove("buffer_count");
    value.as_object_mut().unwrap().remove("batch_size");
    let decoded: Config = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.buffers(), decoded.depth);
    assert_eq!(decoded.batch_size, 1);
    assert!(decoded.validate(4096).is_ok());
}

#[test]
fn fixture_is_deterministic_across_chunk_boundaries() {
    for length in [0, 1, 2, 31, 255, 256, 1024, 8191, 8192, 8193] {
        let mut bytes = Vec::new();
        write_fixture(&mut bytes, length).unwrap();
        assert_eq!(bytes.len() as u64, length);
        for (index, &byte) in bytes.iter().enumerate() {
            let index = index as u64;
            assert_eq!(
                byte,
                (index.wrapping_mul(31) ^ (index >> 8) ^ (index >> 17)) as u8
            );
        }
    }
}

#[test]
fn fixture_propagates_writer_failure() {
    struct Reject;
    impl Write for Reject {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(failed("injected write"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(failed("injected flush"))
        }
    }
    assert!(write_fixture(&mut Reject, 1).is_err());
    assert!(write_fixture(&mut Reject, 0).is_err());
}

#[test]
fn order_balances_positions_and_directions() {
    let orders: Vec<_> = (0..6).map(trial_order).collect();
    for arrangement in [
        Arrangement::Direct,
        Arrangement::Pipeline,
        Arrangement::Independent,
    ] {
        for position in 0..3 {
            assert_eq!(
                orders
                    .iter()
                    .filter(|order| order[position] == arrangement)
                    .count(),
                2
            );
        }
    }
    for (i, order) in orders.iter().enumerate() {
        assert!(!orders[..i].contains(order));
        assert_eq!(*order, trial_order(i + 6));
    }
}

#[test]
fn result_guard_accepts_reordering_but_rejects_corruption_and_census_errors() {
    let make = || {
        vec![
            BlockResult {
                id: 1,
                hash: 22,
                latency_ns: 0,
                queue_ns: 0,
                read_ns: 0,
                compute_ns: 0,
            },
            BlockResult {
                id: 0,
                hash: 11,
                latency_ns: 0,
                queue_ns: 0,
                read_ns: 0,
                compute_ns: 0,
            },
        ]
    };
    assert!(verify(&mut make(), &[11, 22]).is_ok());
    assert!(verify(&mut make(), &[11]).is_err());
    assert!(verify(&mut make(), &[11, 22, 33]).is_err());
    assert!(verify(&mut make(), &[11, 23]).is_err());
    let mut duplicate = make();
    duplicate[0].id = 0;
    assert!(verify(&mut duplicate, &[11, 22]).is_err());
    let mut out_of_range = make();
    out_of_range[0].id = usize::MAX;
    assert!(verify(&mut out_of_range, &[11, 22]).is_err());
}

#[test]
fn comparisons_balance_positions_and_predecessors_for_ten_cycles() {
    let treatments = comparison_order(0);
    for cycle in 0..10 {
        let orders: Vec<_> = (0..6)
            .map(|row| comparison_order(cycle * 6 + row))
            .collect();
        for treatment in treatments {
            for order in &orders {
                assert_eq!(order.iter().filter(|&&case| case == treatment).count(), 1);
            }
            for position in 0..6 {
                assert_eq!(
                    orders
                        .iter()
                        .filter(|order| order[position] == treatment)
                        .count(),
                    1
                );
            }
            for successor in treatments {
                let adjacent = orders
                    .iter()
                    .flat_map(|order| order.windows(2))
                    .filter(|pair| pair[0] == treatment && pair[1] == successor)
                    .count();
                assert_eq!(adjacent, usize::from(treatment != successor));
            }
        }
    }
    assert_eq!(
        comparison_order(usize::MAX),
        comparison_order(usize::MAX % 6)
    );
}

#[test]
fn comparisons_keep_both_orientations_of_each_arrangement() {
    let pair = [
        ProcessorId {
            group: 1,
            number: 3,
        },
        ProcessorId {
            group: 2,
            number: 7,
        },
    ];
    for arrangement in trial_order(0) {
        for reversed in [false, true] {
            let comparison = Comparison {
                arrangement,
                reversed,
            };
            assert!(comparison_order(0).contains(&comparison));
            assert_eq!(
                comparison.processors(pair),
                if reversed { [pair[1], pair[0]] } else { pair }
            );
        }
    }
}

#[test]
fn percentiles_use_nearest_rank() {
    let empty = Distribution::from_values(Vec::new());
    assert_eq!(
        (empty.count, empty.p50_ns, empty.p99_ns, empty.max_ns),
        (0, 0, 0, 0)
    );
    let d = Distribution::from_values((1..=100).rev().collect());
    assert_eq!((d.count, d.p50_ns, d.p99_ns, d.max_ns), (100, 50, 99, 100));
    let single = Distribution::from_values(vec![17]);
    assert_eq!((single.p50_ns, single.p99_ns, single.max_ns), (17, 17, 17));
}

#[test]
fn working_set_decoder_requires_valid_and_masks_other_fields() {
    for node in 0..64 {
        let flags = (node << 16) | 1;
        assert_eq!(platform::resident_node(flags), Some(node));
        assert_eq!(platform::resident_node(flags & !1), None);
        assert_eq!(
            platform::resident_node(flags | (usize::MAX << 22)),
            Some(node)
        );
    }
}

#[test]
fn config_rejects_unknown_fields() {
    let mut value = serde_json::to_value(config()).unwrap();
    value["typo_depth"] = serde_json::json!(8);
    assert!(serde_json::from_value::<Config>(value).is_err());
}
