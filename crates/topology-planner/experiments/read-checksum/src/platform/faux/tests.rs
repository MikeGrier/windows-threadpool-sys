// Copyright (c) Mike Grier.
use super::*;
use crate::experiment::run_with_platform;
use crate::placement::{Relationship, gather_placements};
use crate::{Arrangement, Config, InputKind};
use windows_topology_sys::{Domain, DomainKind, Observation, Processor};

fn topology(seed: u16) -> MachineMemoryTopology {
    let mut topology = MachineMemoryTopology::default();
    for domain in 0..2 {
        let ids: Vec<_> = (0..2)
            .map(|number| ProcessorId {
                group: 60000 + seed * 2 + domain,
                number,
            })
            .collect();
        for &id in &ids {
            topology.processors.push(Processor {
                id,
                online: true,
                capacity: 0,
            });
            topology.domains.push(Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                processors: [(id.group, id.number)].into_iter().collect(),
                observations: Vec::new(),
            });
        }
        topology.domains.push(Domain {
            kind: DomainKind::Memory {
                memory_bytes: Observed::NotObserved,
            },
            processors: ids.iter().map(|id| (id.group, id.number)).collect(),
            observations: vec![Observation::new(
                Source::RelationshipWalk,
                73 + u32::from(seed) * 100 + u32::from(domain) * 17,
            )],
        });
    }
    topology
}

fn config(pair: Option<[ProcessorId; 2]>, node: Option<u32>, seed: usize) -> Config {
    Config {
        file: "must-not-open".into(),
        input: InputKind::Generated,
        generated_bytes: Some((128 * (5 + seed) + 17) as u64),
        payload_node: node,
        block_bytes: 128,
        depth: 2,
        buffer_count: Some(8),
        batch_size: 3,
        queue_capacity: 1,
        checksum_passes: 2,
        repetitions: 1,
        timeout_ms: 1000,
        processors: pair,
    }
}

fn assert_released(platform: &FauxPlatform) {
    assert_eq!(platform.active_allocations(), 0);
    let events = platform.events();
    let allocated: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let Event::Allocate { id, .. } = event {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    let mut released: Vec<_> = events
        .iter()
        .filter_map(|event| {
            if let Event::Release(id) = event {
                Some(*id)
            } else {
                None
            }
        })
        .collect();
    released.sort_unstable();
    assert_eq!(allocated, released);
}

#[test]
fn gathering_to_real_schedulers_uses_one_faux_environment_in_both_directions() {
    for seed in 0..10 {
        let platform = FauxPlatform::new(topology(seed), Fault::None);
        let plan = gather_placements(&platform).unwrap();
        assert!(matches!(platform.events().first(), Some(Event::Discover)));
        let pair = plan
            .pairs
            .iter()
            .find(|pair| pair.memory_relationship == Relationship::Separate)
            .unwrap();
        for node in pair.memory_nodes {
            let capture = run_with_platform(
                config(Some(pair.processors), node, seed as usize),
                &platform,
            )
            .unwrap();
            assert_eq!(capture.topology, platform.topology);
            assert_eq!(
                capture.evidence_class,
                "faux_numa_behavior_only_not_hardware_timing"
            );
            for trial in capture.trials {
                assert_eq!(trial.completed_blocks, capture.blocks);
                assert_eq!(trial.worker_cpu_ns, 0);
                assert_eq!(trial.resources.payload_pool_bytes, 1024);
                for (index, worker) in trial.workers.iter().enumerate() {
                    let expected = pair.processors[if trial.reversed { 1 - index } else { index }];
                    assert_eq!(worker.requested_processor, expected);
                    assert_eq!(worker.processor_at_start, expected);
                    assert_eq!(worker.processor_at_end, expected);
                    assert_eq!(worker.numa_backed_buffers, 0);
                    if worker.buffer_capacity != 0 {
                        for pages in [
                            worker.pages_before.as_ref().unwrap(),
                            worker.pages_after.as_ref().unwrap(),
                        ] {
                            assert_eq!(
                                pages.pages_by_node.keys().copied().collect::<Vec<_>>(),
                                vec![node.unwrap() as usize]
                            );
                            assert_eq!(pages.unresident_pages, 0);
                        }
                    }
                }
                if trial.arrangement == Arrangement::Pipeline {
                    assert_eq!(trial.workers[1].checksummed_blocks, capture.blocks);
                }
            }
            assert_released(&platform);
        }
        let events = platform.events();
        for (source, target) in [
            (pair.processors[0], pair.processors[1]),
            (pair.processors[1], pair.processors[0]),
        ] {
            assert!(events.iter().any(|event| {
                let Event::Allocate { id, owner, .. } = event else { return false };
                *owner == source && events.iter().any(|event| matches!(event, Event::Process { id: processed, worker } if processed == id && *worker == target))
            }));
        }
    }
}

#[test]
fn changing_gathered_node_membership_changes_selection_and_default_allocation() {
    let mut machine = topology(0);
    let first = FauxPlatform::new(machine.clone(), Fault::None);
    let capture = run_with_platform(config(None, None, 0), &first).unwrap();
    assert_eq!(
        capture.processors,
        [machine.processors[0].id, machine.processors[1].id]
    );
    machine.domains.retain(|domain| {
        !matches!(domain.kind, DomainKind::Memory { .. })
            || !domain
                .processors
                .contains(machine.processors[0].id.group, 0)
    });
    let second = FauxPlatform::new(machine.clone(), Fault::None);
    let capture = run_with_platform(config(None, None, 0), &second).unwrap();
    assert_eq!(
        capture.processors,
        [machine.processors[2].id, machine.processors[3].id]
    );
    assert!(
        second
            .events()
            .iter()
            .filter_map(|event| {
                if let Event::Allocate {
                    requested, node, ..
                } = event
                {
                    Some((*requested, *node))
                } else {
                    None
                }
            })
            .all(|(requested, node)| requested.is_none() && node == Some(90))
    );
    assert_released(&first);
    assert_released(&second);
}

#[test]
fn refusals_and_mid_processing_failure_release_every_payload() {
    let machine = topology(0);
    let pair = [machine.processors[0].id, machine.processors[2].id];
    for (fault, expected) in [
        (Fault::Discover, "discovery refusal"),
        (Fault::Pin(pair[0]), "binding refusal"),
        (Fault::Pin(pair[1]), "binding refusal"),
        (Fault::Allocation(3), "allocation refusal"),
        (Fault::Residency(1), "residency query refusal"),
        (Fault::Residency(2), "residency query refusal"),
        (Fault::Processing(2), "processing refusal"),
        (Fault::CpuSample(1), "CPU sample refusal"),
        (Fault::CpuSample(2), "CPU sample refusal"),
        (Fault::BindingMismatch(pair[1]), "did not retain"),
        (Fault::Allocation(11), "allocation refusal"),
        (Fault::Residency(3), "residency query refusal"),
        (Fault::Processing(8), "processing refusal"),
    ] {
        let platform = FauxPlatform::new(machine.clone(), fault);
        let error = run_with_platform(config(Some(pair), Some(73), 0), &platform).unwrap_err();
        assert!(error.to_string().contains(expected), "{fault:?}: {error}");
        assert_released(&platform);
    }
}

#[test]
fn unknown_and_mismatched_residency_are_not_fabricated_as_requested() {
    for outcome in [PageOutcome::Unknown, PageOutcome::Mismatch(90)] {
        let mut platform = FauxPlatform::new(topology(0), Fault::None);
        platform.pages = outcome;
        let capture = run_with_platform(config(None, Some(73), 0), &platform).unwrap();
        for worker in capture
            .trials
            .iter()
            .flat_map(|trial| &trial.workers)
            .filter(|worker| worker.buffer_capacity != 0)
        {
            for pages in [
                worker.pages_before.as_ref().unwrap(),
                worker.pages_after.as_ref().unwrap(),
            ] {
                match outcome {
                    PageOutcome::Unknown => {
                        assert!(pages.pages_by_node.is_empty());
                        assert!(pages.unresident_pages > 0);
                    }
                    PageOutcome::Mismatch(node) => {
                        assert!(pages.pages_by_node.contains_key(&(node as usize)));
                        assert!(!pages.pages_by_node.contains_key(&73));
                    }
                    PageOutcome::Placed => unreachable!(),
                }
            }
        }
        assert_released(&platform);
    }
}

#[test]
fn invalid_requests_never_reach_fake_or_live_realization() {
    let platform = FauxPlatform::new(topology(0), Fault::None);
    let mut file = config(None, None, 0);
    file.input = InputKind::BufferedFile;
    assert!(
        run_with_platform(file, &platform)
            .unwrap_err()
            .to_string()
            .contains("no live file/IOCP")
    );
    assert!(platform.events().is_empty());
    assert!(run_with_platform(config(None, Some(0), 0), &platform).is_err());
    assert!(
        !platform
            .events()
            .iter()
            .any(|event| matches!(event, Event::Pin(_) | Event::Allocate { .. }))
    );
    let invalid = ProcessorId {
        group: 59999,
        number: 1,
    };
    assert!(run_with_platform(config(Some([invalid, invalid]), None, 0), &platform).is_err());
    assert_released(&platform);
}

#[test]
fn faux_residency_rejects_live_and_foreign_payloads() {
    let first = FauxPlatform::new(topology(0), Fault::None);
    let second = FauxPlatform::new(topology(0), Fault::None);
    first.pin(first.topology.processors[0].id).unwrap();
    second.pin(second.topology.processors[0].id).unwrap();
    assert_eq!(
        first.allocate(128, Some(0)).err().unwrap().kind(),
        io::ErrorKind::InvalidInput
    );
    let foreign = first.allocate(128, Some(73)).unwrap();
    assert!(
        second
            .residency(std::slice::from_ref(&foreign))
            .unwrap_err()
            .to_string()
            .contains("different faux environment")
    );
    assert!(
        second
            .residency(&[Payload::Heap(vec![0; 128])])
            .unwrap_err()
            .to_string()
            .contains("live payload")
    );
    drop(foreign);
    assert_released(&first);
    assert_released(&second);
}
