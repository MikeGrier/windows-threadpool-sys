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
//! 1. **Establish first.** Emit `Completion { Subscribed }`, and -- for a watch
//!    that reports liveness -- an `Established { mode }`, before any data. Nothing
//!    else precedes establishment (schedule docs: establishment-before-data).
//! 2. **Live loop.** A weighted mix of: a `Batch` (with rename changes emitted as
//!    an old/new *pair*, in order); a loss `Desync` (`Overflow`/`QueueFull`, the
//!    D-29 loss shape); for a liveness watch, a `Suspended` -> `Desync {
//!    Reestablished }` -> `Resumed` (-> optional re-`Established`) bracket; for an
//!    interactive watch, a single `RetryQuestion` or `VolumeChanged`, which is
//!    treated as resolved before the next event so at most one question is ever
//!    outstanding (schedule docs: one-question-per-watch).
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
        low + self.below(high - low + 1)
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
    /// Percent chance a watch is interactive (can be asked a `RetryQuestion` /
    /// `VolumeChanged`).
    pub interactive_percent: u32,
    /// Percent chance a watch ends with `Completion { Cancelled }`.
    pub cancel_percent: u32,
    /// Relative weight of a `Batch` in the live loop.
    pub weight_batch: u32,
    /// Relative weight of a loss `Desync` in the live loop.
    pub weight_desync: u32,
    /// Relative weight of a `Suspended`/`Resumed` bracket (liveness watches only).
    pub weight_suspend_resume: u32,
    /// Relative weight of a question (interactive watches only).
    pub weight_question: u32,
}

impl Default for GeneratorConfig {
    fn default() -> Self {
        Self {
            watches: 3,
            steps_per_watch: 16,
            liveness_percent: 50,
            interactive_percent: 40,
            cancel_percent: 25,
            weight_batch: 6,
            weight_desync: 2,
            weight_suspend_resume: 1,
            weight_question: 1,
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
        loop {
            let live: Vec<usize> = queues
                .iter()
                .enumerate()
                .filter(|(_, queue)| !queue.is_empty())
                .map(|(index, _)| index)
                .collect();
            if live.is_empty() {
                break;
            }
            let pick = live[self.pick_index(&mut rng, live.len())];
            let spec = queues[pick]
                .pop_front()
                .expect("chosen queue was non-empty");
            schedule.steps.push(spec);
        }
        schedule
    }

    /// One watch's legal lifecycle, in order.
    fn generate_watch(&self, rng: &mut Rng, watch: u64) -> Vec<NotificationSpec> {
        let mut out = Vec::new();
        let liveness = rng.chance(self.config.liveness_percent);
        let interactive = rng.chance(self.config.interactive_percent);

        // 1. Establish first.
        out.push(NotificationSpec::Completion {
            watch,
            outcome: OutcomeSpec::Subscribed,
        });
        if liveness {
            out.push(NotificationSpec::Established {
                watch,
                mode: WatchModeSpec::Detailed,
            });
        }

        // 2. Live loop.
        for _ in 0..self.config.steps_per_watch {
            match self.pick_event(rng, liveness, interactive) {
                Event::Batch => out.push(self.gen_batch(rng, watch)),
                Event::Desync => out.push(NotificationSpec::Desync {
                    watch,
                    cause: gen_loss_cause(rng),
                }),
                Event::SuspendResume => {
                    out.push(NotificationSpec::Suspended { watch });
                    out.push(NotificationSpec::Desync {
                        watch,
                        cause: DesyncCauseSpec::Reestablished,
                    });
                    out.push(NotificationSpec::Resumed { watch });
                    if rng.chance(50) {
                        out.push(NotificationSpec::Established {
                            watch,
                            mode: gen_mode(rng),
                        });
                    }
                }
                Event::Question => {
                    if rng.chance(50) {
                        out.push(NotificationSpec::RetryQuestion {
                            watch,
                            operation: gen_operation(rng),
                            detail: gen_detail(rng),
                        });
                    } else {
                        out.push(NotificationSpec::VolumeChanged {
                            watch,
                            previous: gen_volume(rng),
                            current: gen_volume(rng),
                        });
                    }
                    // The question is treated as resolved before the next event,
                    // so a second question never overlaps the first.
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

    /// Choose a live-loop event, honoring which kinds this watch may emit.
    fn pick_event(&self, rng: &mut Rng, liveness: bool, interactive: bool) -> Event {
        let candidates = [
            (Event::Batch, self.config.weight_batch),
            (Event::Desync, self.config.weight_desync),
            (
                Event::SuspendResume,
                if liveness {
                    self.config.weight_suspend_resume
                } else {
                    0
                },
            ),
            (
                Event::Question,
                if interactive {
                    self.config.weight_question
                } else {
                    0
                },
            ),
        ];
        let weights: Vec<u32> = candidates.iter().map(|(_, w)| *w).collect();
        candidates[rng.weighted(&weights)].0
    }

    /// A `Batch` of 1..=3 changes, with renames emitted as ordered old/new pairs.
    fn gen_batch(&self, rng: &mut Rng, watch: u64) -> NotificationSpec {
        let count = rng.range(1, 3);
        let mut changes = Vec::new();
        for _ in 0..count {
            match rng.below(4) {
                0 => changes.push(ChangeSpec {
                    kind: ChangeKindSpec::Added,
                    name: gen_name(rng),
                }),
                1 => changes.push(ChangeSpec {
                    kind: ChangeKindSpec::Removed,
                    name: gen_name(rng),
                }),
                2 => changes.push(ChangeSpec {
                    kind: ChangeKindSpec::Modified,
                    name: gen_name(rng),
                }),
                _ => {
                    // A rename is a pair: old name then the matching new name.
                    let old = gen_name(rng);
                    let new = gen_name(rng);
                    changes.push(ChangeSpec {
                        kind: ChangeKindSpec::RenamedOldName,
                        name: old,
                    });
                    changes.push(ChangeSpec {
                        kind: ChangeKindSpec::RenamedNewName,
                        name: new,
                    });
                }
            }
        }
        NotificationSpec::Batch { watch, changes }
    }
}

/// A live-loop event kind (internal to the generator).
#[derive(Clone, Copy)]
enum Event {
    Batch,
    Desync,
    SuspendResume,
    Question,
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

/// A faulting operation.
fn gen_operation(rng: &mut Rng) -> FaultOperationSpec {
    if rng.chance(50) {
        FaultOperationSpec::Open
    } else {
        FaultOperationSpec::Arm
    }
}

/// A plausible fault detail (a classification plus a real Win32 code).
fn gen_detail(rng: &mut Rng) -> FaultDetailSpec {
    let failure = match rng.below(6) {
        0 => OpenFailureSpec::NotFound,
        1 => OpenFailureSpec::NotADirectory,
        2 => OpenFailureSpec::Unsupported,
        3 => OpenFailureSpec::Retryable,
        4 => OpenFailureSpec::InvalidPath,
        _ => OpenFailureSpec::RetryUnavailable,
    };
    // A handful of common Win32 error codes.
    const CODES: [u32; 5] = [2, 3, 5, 32, 1231];
    let code = FailureCodeSpec::Win32(CODES[rng.below(CODES.len() as u64) as usize]);
    FaultDetailSpec { failure, code }
}

/// A plausible volume identity.
fn gen_volume(rng: &mut Rng) -> VolumeSpec {
    const FS: [&str; 3] = ["NTFS", "ReFS", "FAT32"];
    const LABELS: [&str; 4] = ["System", "Data", "Removable", "Backup"];
    VolumeSpec {
        serial: (rng.next_u64() & 0xFFFF_FFFF) as u32,
        filesystem: FS[rng.below(FS.len() as u64) as usize].to_string(),
        label: LABELS[rng.below(LABELS.len() as u64) as usize].to_string(),
    }
}
