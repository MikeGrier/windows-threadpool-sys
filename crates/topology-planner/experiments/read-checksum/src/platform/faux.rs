// Copyright (c) Mike Grier.
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use windows_topology_sys::{MachineMemoryTopology, Observed, ProcessorId, Source};

use super::{Platform, Residency, memory_nodes};
use crate::payload::Payload;
use crate::{failed, invalid};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum Fault {
    #[default]
    None,
    Discover,
    Pin(ProcessorId),
    Allocation(usize),
    Residency(usize),
    Processing(usize),
    CpuSample(usize),
    BindingMismatch(ProcessorId),
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum PageOutcome {
    #[default]
    Placed,
    Unknown,
    Mismatch(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Discover,
    Pin(ProcessorId),
    Allocate {
        id: usize,
        owner: ProcessorId,
        requested: Option<u32>,
        node: Option<u32>,
        bytes: usize,
    },
    Residency {
        owner: ProcessorId,
        ids: Vec<usize>,
    },
    Process {
        id: usize,
        worker: ProcessorId,
    },
    Release(usize),
}

#[derive(Default)]
struct State {
    events: Vec<Event>,
    bindings: HashMap<ThreadId, ProcessorId>,
    cpu_samples: HashMap<ThreadId, usize>,
    allocations: BTreeMap<usize, Option<u32>>,
    next_id: usize,
    residency_calls: usize,
    processing_calls: usize,
    fault: Fault,
}

pub(crate) struct FauxPlatform {
    pub topology: MachineMemoryTopology,
    state: Arc<Mutex<State>>,
    pub pages: PageOutcome,
}

impl FauxPlatform {
    pub fn new(topology: MachineMemoryTopology, fault: Fault) -> Self {
        Self {
            topology,
            state: Arc::new(Mutex::new(State {
                fault,
                ..State::default()
            })),
            pages: PageOutcome::Placed,
        }
    }

    pub fn events(&self) -> Vec<Event> {
        self.state.lock().unwrap().events.clone()
    }
    pub fn active_allocations(&self) -> usize {
        self.state.lock().unwrap().allocations.len()
    }
}

impl Platform for FauxPlatform {
    fn synthetic(&self) -> bool {
        true
    }

    fn discover(&self) -> io::Result<MachineMemoryTopology> {
        let mut state = self.state.lock().unwrap();
        state.events.push(Event::Discover);
        if matches!(state.fault, Fault::Discover) {
            return Err(failed("faux discovery refusal"));
        }
        Ok(self.topology.clone())
    }

    fn pin(&self, processor: ProcessorId) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.events.push(Event::Pin(processor));
        if !self
            .topology
            .processors
            .iter()
            .any(|value| value.id == processor && value.online)
            || matches!(state.fault, Fault::Pin(target) if target == processor)
        {
            return Err(failed("faux binding refusal"));
        }
        state.bindings.insert(thread::current().id(), processor);
        Ok(())
    }

    fn current_processor(&self) -> ProcessorId {
        let state = self.state.lock().unwrap();
        match state.fault {
            Fault::BindingMismatch(observed) => observed,
            _ => state.bindings[&thread::current().id()],
        }
    }

    fn cpu_ns(&self) -> io::Result<u64> {
        let mut state = self.state.lock().unwrap();
        let fault = state.fault;
        let calls = state.cpu_samples.entry(thread::current().id()).or_default();
        *calls += 1;
        if matches!(fault, Fault::CpuSample(call) if call == *calls) {
            return Err(failed("faux CPU sample refusal"));
        }
        Ok(0)
    }

    fn allocate(&self, bytes: usize, requested: Option<u32>) -> io::Result<Payload> {
        let mut state = self.state.lock().unwrap();
        let owner = state.bindings[&thread::current().id()];
        if requested.is_some_and(|node| !memory_nodes(&self.topology).contains(&node)) {
            return Err(invalid("faux allocation requested an unknown node"));
        }
        let node = requested.or_else(|| match self.topology.memory_domain_of(owner) {
            Observed::Known(domain) => domain.label_from(Source::RelationshipWalk),
            _ => None,
        });
        state.next_id += 1;
        let id = state.next_id;
        if matches!(state.fault, Fault::Allocation(call) if call == id) {
            return Err(failed("faux allocation refusal"));
        }
        let buffer = vec![0; bytes];
        state.allocations.insert(id, node);
        state.events.push(Event::Allocate {
            id,
            owner,
            requested,
            node,
            bytes,
        });
        Ok(Payload::Faux(FauxPayload {
            bytes: buffer,
            id,
            state: Arc::clone(&self.state),
        }))
    }

    fn residency(&self, buffers: &[Payload]) -> io::Result<Residency> {
        let mut state = self.state.lock().unwrap();
        let owner = state.bindings[&thread::current().id()];
        state.residency_calls += 1;
        if matches!(state.fault, Fault::Residency(call) if call == state.residency_calls) {
            return Err(failed("faux residency query refusal"));
        }
        let mut result = Residency {
            pages_by_node: BTreeMap::new(),
            unresident_pages: 0,
            node_ids_truncated: false,
        };
        let mut ids = Vec::new();
        for payload in buffers {
            let Payload::Faux(buffer) = payload else {
                return Err(failed("live payload reached faux residency"));
            };
            if !Arc::ptr_eq(&self.state, &buffer.state) {
                return Err(failed("payload belongs to a different faux environment"));
            }
            ids.push(buffer.id);
            let node = match self.pages {
                PageOutcome::Placed => state.allocations[&buffer.id],
                PageOutcome::Unknown => None,
                PageOutcome::Mismatch(node) => Some(node),
            };
            let pages = buffer.bytes.len().div_ceil(4096);
            match node {
                Some(node) => *result.pages_by_node.entry(node as usize).or_default() += pages,
                None => result.unresident_pages += pages,
            }
        }
        state.events.push(Event::Residency { owner, ids });
        Ok(result)
    }
}

pub(crate) struct FauxPayload {
    pub bytes: Vec<u8>,
    id: usize,
    state: Arc<Mutex<State>>,
}

impl FauxPayload {
    pub fn record_processing(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.processing_calls += 1;
        if matches!(state.fault, Fault::Processing(call) if call == state.processing_calls) {
            return Err(failed("faux processing refusal"));
        }
        let worker = state.bindings[&thread::current().id()];
        state.events.push(Event::Process {
            id: self.id,
            worker,
        });
        Ok(())
    }
}

impl Drop for FauxPayload {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        assert!(state.allocations.remove(&self.id).is_some());
        state.events.push(Event::Release(self.id));
    }
}

#[cfg(test)]
mod tests;
