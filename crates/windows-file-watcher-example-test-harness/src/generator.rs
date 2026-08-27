// Copyright (c) 2026 Mike Grier
//! A seeded generator of **contract-legal** schedules.
//!
//! This is where the legal-envelope discipline (crate DESIGN-NOTES D-5) becomes
//! concrete. The generator only ever emits schedules a real file-watcher
//! substrate could actually produce, so a pathology found by driving one is a
//! real bug in the handler, not a phantom manufactured by an impossible sequence
//! (the wire format itself would happily let you build an impossible one -- see
//! the [`schedule`](crate::schedule) module docs, D-7).
//!
//! # The per-watch state machine it respects
//!
//! Each subscription is generated as an independent, legal lifecycle, and the
//! per-watch lifecycles are then interleaved (cross-watch order is free; per-watch
//! order is preserved). For one watch:
//!
//! 1. **Establish first.** For a watch that reports liveness, emit an
//!    `Established { mode }` *before* `Completion { Subscribed }` --
//!    `windows-file-watcher` itself sends the initial `Established` from
//!    inside route establishment, and only afterward turns that into the
//!    `Completion` its caller reports. A non-liveness watch has only the
//!    `Completion`. Nothing precedes establishment (schedule docs:
//!    establishment-before-data).
//! 2. **Live loop.** A weighted mix of: a `Batch` (each record's kind, including
//!    `RenamedOldName`/`RenamedNewName`, drawn independently rather than forced
//!    into a pair -- `windows-file-watcher` never joins a rename, D-9); a loss
//!    `Desync` (`Overflow`/`QueueFull`, the D-29 loss shape); or a fault
//!    recovery, modeled as one unit because a real fault and its resolution are
//!    never independent events: `Suspended` (liveness only) -> an unconditional
//!    `RetryQuestion` for an interactive watch (`enter_fault`, watcher.rs, asks
//!    every interactive route on every fault with no probability involved)
//!    and/or, independently and only probabilistically, a `VolumeChanged` for
//!    a volume-confirming watch whose confirming reopen actually landed on a
//!    different volume (`RetryMode::Interactive` and `VolumeChangePolicy::
//!    Confirm` are separate `WatchOptions` fields, never conflated) -> the
//!    resolution, always a `Desync { Reestablished }` (D-12: unconditional,
//!    never gated on liveness) followed by `Resumed` then `Established`
//!    (liveness only, and always together -- `resolve_fault_success` never
//!    sends one without the other). A bare `RetryQuestion`/`VolumeChanged` with
//!    no bracket, or ordinary data delivered while a watch is faulted, is not a
//!    schedule `windows-file-watcher` could produce.
//!
//!    A re-establishment can (re)select `Coarse` tier, whether or not liveness
//!    reporting exposes it (watcher.rs picks a mode on every re-establishment
//!    regardless of `report_liveness`). From that point until the watch's next
//!    re-establishment, its live loop is restricted the way a real Coarse
//!    endpoint is: no `Batch`, and every loss `Desync` reports
//!    `DesyncCauseSpec::Coarse` rather than `Overflow`/`QueueFull`, both
//!    Detailed-only concepts (watcher.rs:535-563, D-17).
//! 3. **Optional terminal.** End with `Completion { Cancelled }`; nothing for that
//!    watch follows it (schedule docs: Cancelled-as-terminator).
//!
//! To extend the generator safely, keep every new event inside those rules: the
//! [`schedule`](crate::schedule) module documents the full set of data and
//! control-flow dependencies a legal schedule must respect.
//!
//! # Reproducibility
//!
//! Every choice is drawn from a seeded [`Rng`] (splitmix64), and nothing else is
//! consulted -- no clock, no addresses, no hash iteration -- so a given seed and
//! [`GeneratorConfig`] produce a byte-identical [`Schedule`] every run.

use std::collections::VecDeque;

use crate::schedule::{
    ChangeKindSpec, ChangeSpec, DesyncCauseSpec, FailureCodeSpec, FaultDetailSpec,
    FaultOperationSpec, NotificationSpec, OpenFailureSpec, OutcomeSpec, Schedule, VolumeSpec,
    WatchModeSpec,
};

/// A tiny reproducible PRNG: the splitmix64 step function.
///
/// Deterministic and dependency-free -- the same seed always yields the same
/// sequence. It is `pub` because a reader extending the generator needs it, but
/// it is not cryptographic and makes no such claim.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// Seed the generator. A zero seed is folded with the golden-ratio constant
    /// so it does not degenerate.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// The next pseudo-random `u64`.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform integer in `[0, n)`, unbiased by rejecting the top partial
    /// block. Panics if `n == 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "Rng::below(0)");
        let limit = u64::MAX - (u64::MAX % n);
        loop {
            let draw = self.next_u64();
            if draw < limit {
                return draw % n;
            }
        }
    }

    /// A uniform integer in `[low, high]` inclusive. Panics if `low > high`.
    pub fn range(&mut self, low: u64, high: u64) -> u64 {
        assert!(low <= high, "Rng::range with low > high");
        // `high - low + 1` overflows exactly when the requested range is the
        // full width of `u64` (`low == 0 && high == u64::MAX`) -- the only
        // case in which `wrapping_add` yields 0, since `high - low` itself
        // cannot overflow given the assertion above. Every `u64` is in range
        // then, so the raw output already is one; no modulus is needed.
        let span = (high - low).wrapping_add(1);
        if span == 0 {
            return self.next_u64();
        }
        low + self.below(span)
    }

    /// `true` with probability `percent`/100 (clamped to `[0, 100]`).
    pub fn chance(&mut self, percent: u32) -> bool {
        let percent = u64::from(percent.min(100));
        self.below(100) < percent
    }

    /// Pick an index into `weights` proportional to each weight. Panics if the
    /// weights sum to zero.
    pub fn weighted(&mut self, weights: &[u32]) -> usize {
        let total: u64 = weights.iter().map(|w| u64::from(*w)).sum();
        assert!(total > 0, "Rng::weighted with zero total weight");
        let mut pick = self.below(total);
        for (index, weight) in weights.iter().enumerate() {
            let weight = u64::from(*weight);
            if pick < weight {
                return index;
            }
            pick -= weight;
        }
        weights.len() - 1
    }
}

/// The tunable shape of a generated schedule. Every field has a sane default
/// (see [`Default`]); adjust to bias toward the traffic you want to stress.
#[derive(Clone, Debug)]
pub struct GeneratorConfig {
    /// How many subscriptions to model. Their lifecycles are interleaved.
    pub watches: u32,
    /// Approximate number of live-loop events per watch (before establishment
    /// and the optional terminal).
    pub steps_per_watch: usize,
    /// Percent chance a watch reports liveness (can emit
    /// `Suspended`/`Resumed`/`Established`).
    pub liveness_percent: u32,
    /// Percent chance a watch is interactive (`RetryMode::Interactive`, can be
    /// asked a `RetryQuestion`). Independent of
    /// [`volume_confirm_percent`](Self::volume_confirm_percent):
    /// `windows-file-watcher`'s `RetryMode` and `VolumeChangePolicy` are two
    /// separate `WatchOptions` fields (`watch.rs`), not one "interactive"
    /// concept.
    pub interactive_percent: u32,
    /// Percent chance a watch opts into volume-change confirmation
    /// (`VolumeChangePolicy::Confirm`, can be asked a `VolumeChanged`).
    /// Independent of
    /// [`interactive_percent`](Self::interactive_percent) -- a watch can
    /// confirm volume changes without being interactive, or vice versa.
    pub volume_confirm_percent: u32,
    /// Percent chance a watch ends with `Completion { Cancelled }`.
    pub cancel_percent: u32,
    /// Relative weight of a `Batch` in the live loop.
    pub weight_batch: u32,
    /// Relative weight of a loss `Desync` in the live loop.
    pub weight_desync: u32,
    /// Relative weight of a fault-recovery event in the live loop. Available to
    /// every watch, not only liveness/interactive ones: even a fully-default
    /// watch (no liveness, `RetryMode::Defaults`) legally sees the bare
    /// resolution `Desync { Reestablished }` when it silently recovers.
    pub weight_fault_recovery: u32,
    /// Given a fault recovery on a volume-confirming watch, the percent
    /// chance the confirming reopen actually landed on a different volume
    /// (and so surfaces a `VolumeChanged`, file-watcher D-78) rather than
    /// reopening the same volume. Has no effect on an interactive watch's
    /// `RetryQuestion`, which is unconditional (`enter_fault`, watcher.rs:
    /// every interactive route is asked on every fault, with no
    /// probability), nor on a watch that is not volume-confirming.
    pub question_percent: u32,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            watches: 3,
            steps_per_watch: 16,
            liveness_percent: 50,
            interactive_percent: 40,
            volume_confirm_percent: 40,
            cancel_percent: 25,
            weight_batch: 6,
            weight_desync: 2,
            weight_fault_recovery: 1,
            question_percent: 50,
        }
    }
}

/// Produces contract-legal [`Schedule`]s from a seed.
#[derive(Clone, Debug, Default)]
pub struct Generator {
    config: GeneratorConfig,
}

impl Generator {
    /// A generator with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A generator with the given configuration.
    #[must_use]
    pub fn with_config(config: GeneratorConfig) -> Self {
        Self { config }
    }

    /// The configuration in effect.
    #[must_use]
    pub fn config(&self) -> &GeneratorConfig {
        &self.config
    }

    /// Generate a legal schedule from `seed`. The same seed and configuration
    /// always produce the same schedule.
    #[must_use]
    pub fn generate(&self, seed: u64) -> Schedule {
        let mut rng = Rng::new(seed);
        // Generate each watch's legal lifecycle, then interleave the per-watch
        // queues -- cross-watch order is free, per-watch order is preserved by
        // always taking the front of the chosen queue.
        let mut queues: Vec<VecDeque<NotificationSpec>> = (1..=u64::from(self.config.watches))
            .map(|watch| self.generate_watch(&mut rng, watch).into())
            .collect();

        let mut schedule = Schedule::new();
        // A mutable index of non-empty queues, rather than rescanning all
        // `queues` on every selection: with `watches` queues and O(total
        // notifications) selections, a full rescan per selection makes
        // generation O(notifications * watches) -- quadratic as `watches`
        // grows. `swap_remove` drops a drained queue's index in O(1).
        let mut live: Vec<usize> = (0..queues.len())
            .filter(|&i| !queues[i].is_empty())
            .collect();
        while !live.is_empty() {
            let position = self.pick_index(&mut rng, live.len());
            let pick = live[position];
            let spec = queues[pick]
                .pop_front()
                .expect("chosen queue was non-empty");
            schedule.steps.push(spec);
            if queues[pick].is_empty() {
                live.swap_remove(position);
            }
        }
        schedule
    }

    /// One watch's legal lifecycle, in order.
    fn generate_watch(&self, rng: &mut Rng, watch: u64) -> Vec<NotificationSpec> {
        let mut out = Vec::new();
        let liveness = rng.chance(self.config.liveness_percent);
        let interactive = rng.chance(self.config.interactive_percent);
        let volume_confirm = rng.chance(self.config.volume_confirm_percent);
        // Tracks the watch's *actual* tier, whether or not liveness reporting
        // exposes it: `watcher.rs` picks a mode on every (re-)establishment
        // regardless of whether the client asked to be told about it, and a
        // Coarse watcher can only ever report `Desync { Coarse }` for
        // activity (watcher.rs:535-563, D-17) -- never a `Batch` or an
        // `Overflow`/`QueueFull` loss `Desync`, both Detailed-only concepts.
        let mut current_mode = WatchModeSpec::Detailed;
        // Tracks the volume identity `WatcherInner::install` last confirmed
        // for this watch (watcher.rs:1137-1139): the *next* VolumeChanged's
        // `previous` must be that same identity, not a fresh independent
        // draw, or the generated pair would claim a continuity file-watcher
        // never breaks. `None` until the first VolumeChanged, which has
        // nothing earlier to continue from.
        let mut current_volume: Option<VolumeSpec> = None;

        // 1. Establish first. windows-file-watcher sends the initial
        // `Established` (liveness only) from inside route establishment, and
        // only afterward turns the result into the `Completion` its caller
        // reports -- so the real order is Established, then Completion, never
        // the reverse.
        if liveness {
            out.push(NotificationSpec::Established {
                watch,
                mode: WatchModeSpec::Detailed,
            });
        }
        out.push(NotificationSpec::Completion {
            watch,
            outcome: OutcomeSpec::Subscribed,
        });

        // 2. Live loop.
        for _ in 0..self.config.steps_per_watch {
            let Some(event) = self.pick_event(rng) else {
                // All three weights are zero: an ordinary, if unusual, public
                // configuration (D-5's "never panic on a legal input"), not a
                // schedule-authoring error. Nothing to generate this step.
                continue;
            };
            match event {
                Event::Batch => {
                    if current_mode == WatchModeSpec::Coarse {
                        // A coarse endpoint has no way to report what
                        // changed -- substitute the one signal it can
                        // legally send instead of skipping the step.
                        out.push(NotificationSpec::Desync {
                            watch,
                            cause: DesyncCauseSpec::Coarse,
                        });
                    } else {
                        out.push(self.gen_batch(rng, watch));
                    }
                }
                Event::Desync => {
                    let cause = if current_mode == WatchModeSpec::Coarse {
                        DesyncCauseSpec::Coarse
                    } else {
                        gen_loss_cause(rng)
                    };
                    out.push(NotificationSpec::Desync { watch, cause });
                }
                Event::FaultRecovery => {
                    // A real fault and its resolution are never independent
                    // events, so this is modeled as one atomic unit: nothing
                    // else may be interleaved between entering a fault and
                    // resolving it (schedule docs: no ordinary data while
                    // faulted).
                    if liveness {
                        out.push(NotificationSpec::Suspended { watch });
                    }
                    // RetryQuestion (RetryMode::Interactive) and VolumeChanged
                    // (VolumeChangePolicy::Confirm) are independent options in
                    // production (watch.rs) -- checked independently here, so
                    // a recovery may surface neither, either, or both.
                    // RetryQuestion is unconditional for an interactive watch:
                    // `enter_fault` (watcher.rs) puts every interactive route
                    // in the awaiting set and asks it on *every* fault, with
                    // no probability involved -- a watch that sometimes
                    // recovers silently despite being interactive is not a
                    // schedule file-watcher could produce. The operation is
                    // always Arm: every `enter_fault` call site but one
                    // passes `FaultOperation::Arm` (watcher.rs); the sole
                    // `Open`-class site (`retry_reestablish`) only re-enters
                    // an *already unresolved* bracket via that bracket's own
                    // retry timer, never a live watch's first fault entry --
                    // and this generator models one question per bracket, so
                    // that later-retry case never applies here.
                    if interactive {
                        out.push(NotificationSpec::RetryQuestion {
                            watch,
                            operation: FaultOperationSpec::Arm,
                            detail: gen_detail(rng),
                        });
                    }
                    // VolumeChanged is genuinely probabilistic: it is only
                    // sent when a confirming reopen actually lands on a
                    // different volume (file-watcher D-78), which is not
                    // guaranteed on every recovery.
                    if volume_confirm && rng.chance(self.config.question_percent) {
                        let previous = current_volume.clone().unwrap_or_else(|| gen_volume(rng));
                        let current = gen_changed_volume(rng, &previous);
                        current_volume = Some(current.clone());
                        out.push(NotificationSpec::VolumeChanged {
                            watch,
                            previous,
                            current,
                        });
                    }
                    // The resolution: always a Desync (D-12, never gated on
                    // liveness), then -- for a liveness watch, and always
                    // together, never one without the other -- Resumed and a
                    // fresh Established.
                    out.push(NotificationSpec::Desync {
                        watch,
                        cause: DesyncCauseSpec::Reestablished,
                    });
                    // Mode selection happens on every re-establishment,
                    // liveness or not -- liveness only gates whether the
                    // client is *told* the tier, not whether the tier can
                    // change (watcher.rs's `finish_reopen`/`route_established`
                    // do not consult `report_liveness`).
                    let mode = gen_mode(rng);
                    current_mode = mode.clone();
                    if liveness {
                        out.push(NotificationSpec::Resumed { watch });
                        out.push(NotificationSpec::Established { watch, mode });
                    }
                }
            }
        }

        // 3. Optional terminal.
        if rng.chance(self.config.cancel_percent) {
            out.push(NotificationSpec::Completion {
                watch,
                outcome: OutcomeSpec::Cancelled,
            });
        }
        out
    }

    /// A uniform index in `[0, len)` -- factored out so the interleave and any
    /// future weighting share one path.
    fn pick_index(&self, rng: &mut Rng, len: usize) -> usize {
        rng.below(len as u64) as usize
    }

    /// Choose a live-loop event. `liveness`/`interactive` do not gate whether a
    /// fault recovery can occur at all (even a fully-default watch legally
    /// sees the bare resolution `Desync`) -- they only shape *what a recovery
    /// contains*, decided in [`Self::generate_watch`].
    ///
    /// `None` if every weight is zero: an ordinary public `GeneratorConfig`
    /// (not a schedule-authoring error), so this is a legal "generate nothing
    /// this step" rather than [`Rng::weighted`]'s zero-total panic.
    fn pick_event(&self, rng: &mut Rng) -> Option<Event> {
        let candidates = [
            (Event::Batch, self.config.weight_batch),
            (Event::Desync, self.config.weight_desync),
            (Event::FaultRecovery, self.config.weight_fault_recovery),
        ];
        let weights: Vec<u32> = candidates.iter().map(|(_, w)| *w).collect();
        if weights.iter().all(|weight| *weight == 0) {
            return None;
        }
        Some(candidates[rng.weighted(&weights)].0)
    }

    /// A `Batch` of 1..=3 raw change records. `RenamedOldName`/`RenamedNewName`
    /// are drawn independently, like every other kind, rather than forced into
    /// an adjacent pair: `windows-file-watcher` never joins a rename (D-9), so
    /// a legal batch may carry a lone half, both halves, or neither.
    fn gen_batch(&self, rng: &mut Rng, watch: u64) -> NotificationSpec {
        let count = rng.range(1, 3);
        let changes = (0..count)
            .map(|_| ChangeSpec {
                kind: gen_change_kind(rng),
                name: gen_name(rng).into(),
            })
            .collect();
        NotificationSpec::Batch { watch, changes }
    }
}

/// A live-loop event kind (internal to the generator).
#[derive(Clone, Copy)]
enum Event {
    Batch,
    Desync,
    FaultRecovery,
}

/// One raw change kind, drawn uniformly. `RenamedOldName`/`RenamedNewName` are
/// independent draws, matching D-9 -- neither implies or requires the other.
fn gen_change_kind(rng: &mut Rng) -> ChangeKindSpec {
    match rng.below(5) {
        0 => ChangeKindSpec::Added,
        1 => ChangeKindSpec::Removed,
        2 => ChangeKindSpec::Modified,
        3 => ChangeKindSpec::RenamedOldName,
        _ => ChangeKindSpec::RenamedNewName,
    }
}

/// A deterministic relative name from a small pool.
fn gen_name(rng: &mut Rng) -> String {
    const EXTS: [&str; 4] = ["txt", "log", "tmp", "dat"];
    let stem = rng.below(20);
    let ext = EXTS[rng.below(EXTS.len() as u64) as usize];
    format!("file{stem:02}.{ext}")
}

/// A loss cause for the live loop: the two ways changes actually go missing.
fn gen_loss_cause(rng: &mut Rng) -> DesyncCauseSpec {
    if rng.chance(50) {
        DesyncCauseSpec::Overflow
    } else {
        DesyncCauseSpec::QueueFull
    }
}

/// A watch tier for a (re-)establishment.
fn gen_mode(rng: &mut Rng) -> WatchModeSpec {
    if rng.chance(20) {
        WatchModeSpec::Coarse
    } else {
        WatchModeSpec::Detailed
    }
}

/// The real Win32 error codes `directory::classify` maps to each
/// `OpenFailure`, named per the repo's no-bare-numeric-constants rule (not
/// imported from `windows-sys`, to avoid a dependency for six well-known,
/// stable `WinError` values). **Changing any of these values is a breaking
/// change**: each is a protocol identity this generator's own contract-legal
/// guarantee (D-5) depends on matching `directory::classify`'s real mapping
/// exactly.
mod win32 {
    pub const ERROR_FILE_NOT_FOUND: u32 = 2;
    pub const ERROR_PATH_NOT_FOUND: u32 = 3;
    pub const ERROR_INVALID_FUNCTION: u32 = 1;
    pub const ERROR_NOT_SUPPORTED: u32 = 50;
    pub const ERROR_ACCESS_DENIED: u32 = 5;
    pub const ERROR_SHARING_VIOLATION: u32 = 32;
}

/// A legal fault detail for a `RetryQuestion`.
///
/// `windows-file-watcher`'s `retry_reestablish` only re-enters the fault loop
/// (and so only ever asks a question) for an open failure that
/// `OpenFailure::is_retryable()` -- `NotFound`, `Unsupported`, or `Retryable`;
/// the permanent classifications (`NotADirectory`, `InvalidPath`,
/// `RetryUnavailable`) only ever reach `Completion::Failed`, never
/// `RetryQuestion`. The code paired with each classification matches
/// `directory::classify`'s real mapping exactly, so a (classification, code)
/// pair this crate could never itself produce is not generated.
fn gen_detail(rng: &mut Rng) -> FaultDetailSpec {
    match rng.below(3) {
        0 => FaultDetailSpec {
            failure: OpenFailureSpec::NotFound,
            code: FailureCodeSpec::Win32(if rng.chance(50) {
                win32::ERROR_FILE_NOT_FOUND
            } else {
                win32::ERROR_PATH_NOT_FOUND
            }),
        },
        1 => FaultDetailSpec {
            failure: OpenFailureSpec::Unsupported,
            code: FailureCodeSpec::Win32(if rng.chance(50) {
                win32::ERROR_INVALID_FUNCTION
            } else {
                win32::ERROR_NOT_SUPPORTED
            }),
        },
        _ => FaultDetailSpec {
            failure: OpenFailureSpec::Retryable,
            code: FailureCodeSpec::Win32(if rng.chance(50) {
                win32::ERROR_ACCESS_DENIED
            } else {
                win32::ERROR_SHARING_VIOLATION
            }),
        },
    }
}

/// A plausible volume identity. Its `serial` alone is not guaranteed distinct
/// from any other draw -- use [`gen_changed_volume`] for a `VolumeChanged`
/// pair, which needs that guarantee.
fn gen_volume(rng: &mut Rng) -> VolumeSpec {
    const FS: [&str; 3] = ["NTFS", "ReFS", "FAT32"];
    const LABELS: [&str; 4] = ["System", "Data", "Removable", "Backup"];
    VolumeSpec {
        serial: (rng.next_u64() & 0xFFFF_FFFF) as u32,
        filesystem: FS[rng.below(FS.len() as u64) as usize].to_string(),
        label: LABELS[rng.below(LABELS.len() as u64) as usize].to_string(),
    }
}

/// A `current` identity that continues from `previous`, with a **serial
/// guaranteed distinct by construction**: `windows-file-watcher` only emits
/// `VolumeChanged` when a reopen's volume identity actually differs from the
/// one previously confirmed (D-78), and identity compares by serial alone
/// (D-50). A fresh independent draw could coincidentally collide with
/// `previous`'s serial, which would make an illegal, impossible
/// `VolumeChanged` -- so `current`'s serial is offset from `previous`'s by a
/// nonzero amount (mod 2^32) rather than drawn independently and hoped to
/// differ.
fn gen_changed_volume(rng: &mut Rng, previous: &VolumeSpec) -> VolumeSpec {
    let mut current = gen_volume(rng);
    // A nonzero offset in [1, u32::MAX - 1] added mod 2^32 can never land back
    // on the original value, so this is a guarantee, not a low-probability draw.
    let offset = 1 + rng.below(u64::from(u32::MAX) - 1) as u32;
    current.serial = previous.serial.wrapping_add(offset);
    current
}
