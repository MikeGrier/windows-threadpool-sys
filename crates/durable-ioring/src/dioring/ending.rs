// Copyright (c) 2026 Mike Grier
//! Ending (DI-3.2.7): removing a file, and ending the instance (DI-D-25, DI-D-45).
//!
//! **Removing a file** is refused while anything still holds it, and the refusal reports every
//! hold at once: operations the ring holds and operations held for a gate, both counted here, and
//! uncovered writes and the failures suspecting writes to it, which the core answers. A file given
//! at construction stays open after removal, because the ring's registration holds it.
//!
//! **Ending the instance** -- `close()` or drop -- is one path. It closes the core and takes the
//! held operations in one locked step, so nothing recorded afterwards is reported and no callback
//! reaches the ring again; drops the consumer's provider, so a completion it keeps in itself is
//! answered as abandoned rather than holding up the wait; waits, with the lock released, until
//! every provider completion has been answered or dropped; and leaves the ring to the field drops
//! that follow, whose rundown waits for every kernel operation. An internal inconsistency is not
//! looked for here: it is panicked on where it is found (DI-D-46).

use std::collections::BTreeMap;

use win_shared_os_owned_handle::SharedHandle;
use windows_ioring_sys::IoBufMut;

use super::relay::{Core, HeldPush, OpData, Shut};
use super::{Dioring, FileSlot, TimeBase};
use crate::contract::EpochId;
use crate::ids::DioringIds;
use crate::types::{Epoch, HeldOperation, IoTargetBusy, IoTargetKey, RemoveTargetError};

type V<E> = DioringIds<E>;

impl<B, E: EpochId + 'static, C, R: IoBufMut, K> Dioring<B, E, C, R, K> {
    /// End the instance, reading `snapshot` from the core in the step that closes it. Returns what
    /// that step took -- the held operations, in push order, and the snapshot -- or `None` if the
    /// instance has already ended. The ring itself runs down when the instance's fields are
    /// dropped, after this returns.
    pub(crate) fn end<S>(
        &mut self,
        snapshot: impl FnOnce(&Core<E, B, C, K>) -> S,
    ) -> Option<Shut<E, B, C, S>> {
        let shut = self.relay.shut(snapshot)?;
        // Before the wait: a completion the provider keeps in itself is answered, as abandoned, by
        // the provider's drop, which would otherwise wait for the wait it holds up.
        self.provider = None;
        self.relay.wait_for_answers();
        Some(shut)
    }
}

impl<B, E, C, R, K> Dioring<B, E, C, R, K>
where
    B: Send + 'static,
    E: EpochId + Send + Sync + 'static,
    C: Send + 'static,
    R: IoBufMut,
    K: TimeBase,
{
    /// Remove `key`'s file, handing back the consumer's handle, unless something still holds it.
    pub(crate) fn remove(
        &mut self,
        key: IoTargetKey,
    ) -> Result<SharedHandle, RemoveTargetError<V<E>>> {
        if !self.files.contains_key(&key) {
            return Err(RemoveTargetError::UnknownKey(key));
        }
        let (in_flight, held_for_gate, lowest, holds) = {
            let core = self.relay.lock();
            let in_flight = core.in_flight.get(&key).copied().unwrap_or(0);
            let mut held_for_gate = 0;
            let mut lowest: BTreeMap<u64, Epoch<V<E>>> = BTreeMap::new();
            for held in core.held.values().filter(|held| held.op.file == key) {
                held_for_gate += 1;
                lowest
                    .entry(held.gate.lineage.seq)
                    .and_modify(|low| {
                        if held.gate.id < low.id {
                            *low = held.gate;
                        }
                    })
                    .or_insert(held.gate);
            }
            (
                in_flight,
                held_for_gate,
                lowest,
                core.durability.file_holds(key),
            )
        };
        if in_flight == 0 && held_for_gate == 0 && holds.is_empty() {
            let record = self.files.remove(&key).expect("present, checked above");
            return Ok(match record.target {
                FileSlot::Registered { file, .. } => file,
                FileSlot::Shared(handle) => handle,
            });
        }
        Err(RemoveTargetError::Busy(IoTargetBusy {
            target: key,
            in_flight,
            held_for_gate,
            lowest_gates: lowest.into_values().collect(),
            uncovered: holds.uncovered,
            uncovered_through: holds.uncovered_through,
            failures: holds.failures,
        }))
    }
}

/// A held operation, as `close()` hands it back.
pub(crate) fn held_operation<E: EpochId + 'static, B, C>(
    held: HeldPush<E, B, C>,
) -> HeldOperation<V<E>, B, C> {
    let HeldPush { op, gate, .. } = held;
    HeldOperation {
        id: op.id,
        kind: op.kind,
        gate,
        buffer: match op.data {
            OpData::Owned(buffer, _) => Some(buffer),
            OpData::Span(_) => None,
        },
        context: op.context,
    }
}
