// Copyright (c) 2026 Mike Grier
//! **SPIKE for `M23.3`, not yet a decision.** `Pending<T, X>` -- the identity
//! map every ring consumer writes, with the conservation oracle wired in rather
//! than driven alongside it.
//!
//! # What this is validating
//!
//! A census found twelve sites in this crate keeping a map from `UserData` to
//! an unclaimed [`Token`], and every one that also uses [`RingContract`] drives
//! the two **in parallel, by hand**:
//!
//! ```text
//! self.contract.observe_push(token.id());
//! self.in_flight.insert(token.id(), InFlight { token, slot });
//! ```
//!
//! That is the same event recorded twice, which the repository's CONTRACT
//! INTEGRITY rule calls a restatement: it can drift in both directions, and the
//! oracle's value depends on being driven correctly by the very code it checks.
//! This type exists to find out whether one call site can keep both true.
//!
//! # The generic parameter is the finding, not a convenience
//!
//! The census also falsified the shape the checklist assumed. Only a third of
//! the sites keep a bare `Token<T>`; the rest carry per-operation sidecar data
//! -- a slot index, an expected length, a phase, a sequence number. `X` is that
//! sidecar, defaulted to `()` so the bare sites read unchanged.
//!
//! # What this cannot do, which is also a finding
//!
//! **It cannot force the discipline.** Rust has no linear types, so nothing
//! makes a caller invoke [`Pending::finish`]. What it can do is make the
//! violation *loud* at the moment it happens rather than silent forever -- see
//! this type's `Drop`.
//!
//! **It does not make ordering hazards unrepresentable, only detectable.**
//! Measured rather than assumed: reinstating `M22.2`'s defect in the converted
//! consumer -- checking a write's result *before* claiming, so a failed write
//! returns early with the token still held -- still compiles and still passes
//! every test, because no test produces a failed write. The difference is that
//! the token remains in the map, so teardown reports it. A detected leak, not a
//! prevented one.
//!
//! **Owning the oracle creates a decoy hazard.** [`Pending::checked`] mints its
//! own [`RingContract`], so a consumer that already had one keeps a field that
//! is never written to again. Converting `epoch_log`'s appender did exactly
//! that, and the result compiled, ran, and made its teardown
//! `assert_quiescent()` pass **vacuously**. Sabotage confirms nothing in the
//! suite catches it: replacing that accessor with an oracle that has observed
//! nothing leaves every test green. It was found by reading. Any consumer
//! converted to a checked map must route its existing accessor through
//! [`Pending::contract`], and that obligation is invisible to the compiler.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use crate::contract::{RingContract, Violation};
use crate::{Completion, Token};

/// A map from `UserData` to an unclaimed [`Token`], optionally checked.
///
/// `X` is per-operation data the caller wants back when the completion
/// arrives: a registered-slot index, an expected length, a phase. It defaults
/// to `()`.
pub struct Pending<T: Send + 'static, X = ()> {
    entries: HashMap<usize, (Token<T>, X)>,
    contract: Option<RingContract>,
}

impl<T: Send + 'static, X> Pending<T, X> {
    /// An unchecked map: bookkeeping only, no oracle.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            contract: None,
        }
    }

    /// A map that drives its own [`RingContract`].
    ///
    /// The oracle is *owned* rather than borrowed, which is what makes drift
    /// impossible: there is no way to record a push in one and not the other.
    /// The cost is that a caller running several maps against one shared
    /// contract cannot use this -- see the module docs.
    #[must_use]
    pub fn checked() -> Self {
        Self {
            entries: HashMap::new(),
            contract: Some(RingContract::new()),
        }
    }

    /// Record a pushed operation and take ownership of its token.
    ///
    /// # Panics
    ///
    /// If `token`'s identity is already pending. That is not a caller mistake
    /// this type should paper over: `UserData` is minted unique per operation,
    /// so a collision means either the ring's accounting is wrong or a token
    /// was pushed twice, and both are worse than the panic.
    pub fn push(&mut self, token: Token<T>, extra: X) {
        let user_data = token.id();
        match self.entries.entry(user_data) {
            Entry::Occupied(_) => {
                panic!("user_data {user_data:#x} is already pending; identities are minted unique")
            }
            Entry::Vacant(slot) => {
                slot.insert((token, extra));
            }
        }
        if let Some(contract) = &mut self.contract {
            contract.observe_push(user_data);
        }
    }

    /// Claim the token matching `completion`, returning its payload and sidecar.
    ///
    /// `None` when this map never held that identity, which is not an error:
    /// a consumer draining a ring it shares with tokenless operations will see
    /// completions it did not push.
    pub fn claim(&mut self, completion: &Completion) -> Option<(T, X)> {
        let user_data = completion.user_data();
        let (token, extra) = self.entries.remove(&user_data)?;
        if let Some(contract) = &mut self.contract {
            contract.observe_completion(user_data);
        }
        match token.claim_if(completion) {
            Ok(payload) => {
                if let Some(contract) = &mut self.contract {
                    contract.observe_claim(user_data);
                }
                Some((payload, extra))
            }
            // Looked up *by* this completion's identity, so a mismatch would
            // mean `claim_if` disagrees with the key it was found under. Put it
            // back rather than dropping it: an unclaimed token that is silently
            // discarded here is exactly the leak this type exists to prevent.
            Err(token) => {
                self.entries.insert(user_data, (token, extra));
                None
            }
        }
    }

    /// Give up on an identity deliberately, recording it as such.
    ///
    /// The escape hatch that keeps `Drop` honest. A consumer tearing down early
    /// has genuinely abandoned these tokens, and the oracle distinguishes that
    /// from having forgotten them.
    pub fn abandon(&mut self, user_data: usize) -> bool {
        let removed = self.entries.remove(&user_data).is_some();
        if removed && let Some(contract) = &mut self.contract {
            contract.observe_deliberate_leak(user_data);
        }
        removed
    }

    /// How many operations are still awaiting their completion.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether every pushed operation has been claimed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The oracle, when this map is checked.
    #[must_use]
    pub fn contract(&self) -> Option<&RingContract> {
        self.contract.as_ref()
    }

    /// Consume the map, returning whatever conservation rules were broken.
    ///
    /// Empty means every token was claimed and the oracle -- if there is one --
    /// saw a well-formed sequence. This is the graceful counterpart to `Drop`:
    /// calling it says the caller has checked, so teardown stays quiet.
    #[must_use]
    pub fn finish(mut self) -> Vec<Violation> {
        let mut violations = match &self.contract {
            Some(contract) => contract.check_quiescent(),
            None => Vec::new(),
        };
        for user_data in self.entries.keys() {
            violations.push(Violation::LeakedToken {
                user_data: *user_data,
            });
        }
        // Checked deliberately, so `Drop` has nothing left to complain about.
        self.entries.clear();
        self.contract = None;
        violations
    }
}

impl<T: Send + 'static, X> Default for Pending<T, X> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static, X> std::fmt::Debug for Pending<T, X> {
    /// Identities and counts, never payloads: a pending token routinely holds
    /// someone's data, and a `Debug` that printed it would put that data
    /// wherever a caller logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ids: Vec<usize> = self.entries.keys().copied().collect();
        ids.sort_unstable();
        f.debug_struct("Pending")
            .field("outstanding", &self.entries.len())
            .field("user_data", &ids)
            .field("checked", &self.contract.is_some())
            .finish()
    }
}

impl<T: Send + 'static, X> Drop for Pending<T, X> {
    /// Panics if tokens are still held.
    ///
    /// # Why this is a panic rather than a log
    ///
    /// A token dropped unclaimed leaks its buffer **on purpose** -- `Token`
    /// forgets the value rather than freeing it, because the kernel may still
    /// be writing there. So this state is not untidy, it is a deliberate leak
    /// that nobody decided to take, and it is invisible at runtime: the program
    /// keeps working and loses memory.
    ///
    /// A caller who means it says so, with [`Pending::abandon`] per identity or
    /// [`Pending::finish`] for the whole map. Both leave this silent.
    ///
    /// Suppressed while already panicking, because a second panic during unwind
    /// aborts the process and replaces the original failure -- which in a test
    /// would hide the assertion that actually fired.
    fn drop(&mut self) {
        if self.entries.is_empty() || std::thread::panicking() {
            return;
        }
        let mut ids: Vec<usize> = self.entries.keys().copied().collect();
        ids.sort_unstable();
        panic!(
            "{} token(s) dropped unclaimed, leaking their buffers: {ids:#x?}. \
             Claim them, or say so with `abandon` or `finish`.",
            ids.len()
        );
    }
}

#[cfg(test)]
mod tests;
