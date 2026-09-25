// Copyright (c) 2026 Mike Grier
//! **Throwaway probe for `M24.1`.** Not a permanent test -- it exists to
//! settle one design question by demonstration, and is deleted once the
//! decision is recorded.
//!
//! # The question
//!
//! [`DESIGN-NOTES.md`]'s "Two techniques deliberately rejected" refuses a mock
//! `IoRing`, because a mock *encodes* an assumption: one written before this
//! crate's two shipped defects were found "would not merely have failed to
//! find them, it would have manufactured evidence they were absent".
//!
//! `M24` wants a hermetic unit suite, and the remedy it would most like is a
//! fake whose **assertions are shared** with the kernel -- one conformance
//! suite, run against both. `M24.1` asks whether sharing escapes the
//! objection, and insists it be settled by demonstration rather than argument,
//! because the argument is what is in doubt.
//!
//! # The two predictions
//!
//! 1. Give the fake a wrong **accounting** model. The shared suite must go red
//!    on the fake and stay green on the kernel.
//! 2. Give the fake a wrong **Windows belief**. The shared suite must *not*
//!    catch it -- the expected result, and the reason `D-49`'s bright line is
//!    a finding rather than a hedge.
//!
//! A co-tested peer is defensible only if both halves behave as predicted.
//! Run with `--nocapture` to read the verdicts.

use std::io;
use std::os::windows::fs::OpenOptionsExt;

use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, IoRing, NumaBuffer, PushOptions, SharedFile, WriteCaching,
};

/// The slice of ring behaviour this probe shares between the two peers.
///
/// Deliberately the *handle-free accounting* `M24.2` measured as separable:
/// push something, submit, pop it, and keep an outstanding count. That is the
/// part a fake could legitimately own.
trait RingLike {
    fn push_one(&mut self) -> io::Result<usize>;
    fn submit(&mut self) -> io::Result<()>;
    fn try_pop(&mut self) -> io::Result<Option<usize>>;
    fn outstanding(&self) -> usize;
    /// A bounded wait. Included because this is where a *real* defect lived:
    /// `SubmitIoRing` reports an expired wait as `ERROR_TIMEOUT`, a failure
    /// HRESULT, so `pop_within` returned `Err` on every ordinary timeout until
    /// `M21.6`. Nobody guessed that; the kernel said it.
    fn pop_within(&mut self, timeout: std::time::Duration) -> io::Result<Option<usize>>;
}

// ------------------------------------------------------------- the kernel ---

struct RealRing {
    ring: IoRing,
    file: SharedFile,
    path: std::path::PathBuf,
    pending: Vec<windows_ioring_sys::Token<SharedFile>>,
    writes: Vec<windows_ioring_sys::Token<(NumaBuffer, SharedFile)>>,
    /// Push sector-aligned writes rather than a flush. A bare flush with
    /// nothing dirty completes inline whatever the handle flags are, which is
    /// how the first version of case 4 failed to discriminate.
    write_mode: bool,
    next_offset: u64,
}

impl RealRing {
    fn new(tag: &str) -> io::Result<Self> {
        Self::with_flags(tag, 0, false)
    }

    /// A real ring over a handle opened with `flags`, optionally over an
    /// extent written beforehand.
    ///
    /// The parameters exist to make one point measurable: whether an operation
    /// has completed by the time `SubmitIoRing` returns is a property of the
    /// *handle*, not of the API. `write-pending-spike.rs` measured that
    /// directly -- buffered never pended over 500 trials, `NO_BUFFERING` over a
    /// pre-written extent pended every time.
    fn with_flags(tag: &str, flags: u32, prewrite: bool) -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "windows-ioring-sys-m24-1-{}-{tag}.tmp",
            std::process::id()
        ));
        if prewrite {
            std::fs::write(&path, vec![0_u8; 64 * 1024])?;
        }
        let file = SharedFile::new(
            std::fs::OpenOptions::new()
                .create(!prewrite)
                .truncate(!prewrite)
                .write(true)
                .custom_flags(flags)
                .open(&path)?
                .into(),
        );
        Ok(Self {
            ring: IoRing::new(64, 128)?,
            file,
            path,
            pending: Vec::new(),
            writes: Vec::new(),
            write_mode: prewrite,
            next_offset: 0,
        })
    }
}

impl Drop for RealRing {
    fn drop(&mut self) {
        let _ = self.ring.run_down();
        let _ = std::fs::remove_file(&self.path);
    }
}

impl RingLike for RealRing {
    fn push_one(&mut self) -> io::Result<usize> {
        if self.write_mode {
            let buffer = NumaBuffer::new(4096, None)?;
            let offset = self.next_offset;
            self.next_offset += 4096;
            let mut batch = Batch::new(&mut self.ring);
            let token = batch.write(
                &self.file,
                buffer,
                offset,
                PushOptions::new(),
                WriteCaching::Cached,
            )?;
            let id = token.id();
            self.writes.push(token);
            return Ok(id);
        }
        let mut batch = Batch::new(&mut self.ring);
        let token = batch.flush(&self.file, FlushCoverage::Unordered, FlushMode::Default)?;
        let id = token.id();
        self.pending.push(token);
        Ok(id)
    }

    fn submit(&mut self) -> io::Result<()> {
        Batch::new(&mut self.ring).submit()?;
        Ok(())
    }

    fn try_pop(&mut self) -> io::Result<Option<usize>> {
        let Some(completion) = self.ring.try_pop()? else {
            return Ok(None);
        };
        let id = completion.user_data();
        if let Some(index) = self.pending.iter().position(|t| t.id() == id) {
            let token = self.pending.swap_remove(index);
            let _ = token.claim_if(&completion);
        } else if let Some(index) = self.writes.iter().position(|t| t.id() == id) {
            let token = self.writes.swap_remove(index);
            let _ = token.claim_if(&completion);
        }
        Ok(Some(id))
    }

    fn outstanding(&self) -> usize {
        self.ring.outstanding()
    }

    fn pop_within(&mut self, timeout: std::time::Duration) -> io::Result<Option<usize>> {
        Ok(self.ring.pop_within(timeout)?.map(|c| c.user_data()))
    }
}

// --------------------------------------------------------------- the fake ---

/// Which wrongness this fake was built with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Flaw {
    /// A faithful peer.
    None,
    /// Prediction 1: outstanding is decremented at push rather than at pop.
    /// This is a bookkeeping rule *we* own, and it is fully specified by us.
    WrongAccounting,
    /// Prediction 2a: a belief about Windows -- that a completion is
    /// available without submitting. No accounting rule is violated; the fake
    /// simply models the platform wrongly.
    WrongWindowsBelief,
    /// Prediction 2b, and the stronger case because it is not invented: the
    /// belief this crate ACTUALLY HELD until M21.6 -- that an expired
    /// bounded wait is an error rather than Ok(None). A fake written before
    /// that discovery would have encoded exactly this.
    PreM216TimeoutBelief,
}

struct FakeRing {
    flaw: Flaw,
    next_id: usize,
    queued: Vec<usize>,
    completed: Vec<usize>,
    outstanding: usize,
}

impl FakeRing {
    fn new(flaw: Flaw) -> Self {
        Self {
            flaw,
            next_id: 1,
            queued: Vec::new(),
            completed: Vec::new(),
            outstanding: 0,
        }
    }
}

impl RingLike for FakeRing {
    fn push_one(&mut self) -> io::Result<usize> {
        let id = self.next_id;
        self.next_id += 1;
        self.queued.push(id);
        self.outstanding += 1;
        if self.flaw == Flaw::WrongAccounting {
            // The sabotage: released here instead of when the completion is
            // observed.
            self.outstanding -= 1;
        }
        if self.flaw == Flaw::WrongWindowsBelief {
            // The sabotage: this fake thinks the kernel starts work as soon as
            // it is queued, so a completion is poppable without a submit.
            self.completed.push(id);
            self.queued.pop();
        }
        Ok(id)
    }

    fn submit(&mut self) -> io::Result<()> {
        self.completed.append(&mut self.queued);
        Ok(())
    }

    fn try_pop(&mut self) -> io::Result<Option<usize>> {
        if self.completed.is_empty() {
            return Ok(None);
        }
        let id = self.completed.remove(0);
        if self.flaw != Flaw::WrongAccounting {
            self.outstanding -= 1;
        }
        Ok(Some(id))
    }

    fn outstanding(&self) -> usize {
        self.outstanding
    }

    fn pop_within(&mut self, _timeout: std::time::Duration) -> io::Result<Option<usize>> {
        if self.completed.is_empty() {
            if self.flaw == Flaw::PreM216TimeoutBelief {
                // What a fake written from the old understanding would do.
                return Err(io::Error::from_raw_os_error(1460));
            }
            return Ok(None);
        }
        self.try_pop()
    }
}

// ------------------------------------------------- the shared conformance ---

/// Every assertion both peers must satisfy.
///
/// This is the *whole* point of the technique under evaluation: one suite, two
/// implementations. Note what it asserts -- push, submit, pop, and the count
/// in between -- and note what it does not think to ask, which is where
/// prediction 2 lives.
fn shared_suite<R: RingLike>(ring: &mut R) -> Result<(), String> {
    if ring.outstanding() != 0 {
        return Err(format!(
            "a fresh ring has {} outstanding, expected 0",
            ring.outstanding()
        ));
    }

    ring.push_one().map_err(|e| e.to_string())?;
    if ring.outstanding() != 1 {
        return Err(format!(
            "after one push, outstanding is {}, expected 1",
            ring.outstanding()
        ));
    }

    ring.submit().map_err(|e| e.to_string())?;
    if ring.outstanding() != 1 {
        return Err(format!(
            "submitting does not complete anything, so outstanding should still be 1, got {}",
            ring.outstanding()
        ));
    }

    // Drain, bounded: the kernel may not have posted it yet.
    let mut popped = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while popped < 1 {
        if ring.try_pop().map_err(|e| e.to_string())?.is_some() {
            popped += 1;
        } else if std::time::Instant::now() > deadline {
            return Err("the completion never arrived".to_string());
        }
    }

    if ring.outstanding() != 0 {
        return Err(format!(
            "claiming the completion releases it, so outstanding should be 0, got {}",
            ring.outstanding()
        ));
    }

    if ring.try_pop().map_err(|e| e.to_string())?.is_some() {
        return Err("an empty ring popped something".to_string());
    }

    Ok(())
}

/// The assertion that was **added after** the kernel taught us to ask.
///
/// This is the whole argument in one function. It is not in
/// [`shared_suite`] above, because the suite above is what someone would
/// write from the understanding this crate held before `M21.6`: push, submit,
/// pop, count. Asking "and what does a bounded wait do when nothing arrives?"
/// only occurs to someone who has been surprised by the answer.
fn timeout_assertion<R: RingLike>(ring: &mut R) -> Result<(), String> {
    match ring.pop_within(std::time::Duration::from_millis(20)) {
        Ok(None) => Ok(()),
        Ok(Some(id)) => Err(format!("an empty ring produced completion {id}")),
        Err(e) => Err(format!("an expired wait reported an error: {e}")),
    }
}

/// The same question, asked from the **wrong** belief -- the one this crate
/// actually held before `M21.6`.
///
/// This is the case the first version of this probe did not test, and it is
/// the crux. If a shared suite encodes a mistaken belief about Windows, what
/// happens when it is run against the kernel? A mock-only world never asks.
fn timeout_assertion_from_the_old_belief<R: RingLike>(ring: &mut R) -> Result<(), String> {
    match ring.pop_within(std::time::Duration::from_millis(20)) {
        // The pre-M21.6 understanding: an expired wait is a failure.
        Err(_) => Ok(()),
        Ok(None) => Err("an expired wait returned Ok(None), not the error we expected".to_string()),
        Ok(Some(id)) => Err(format!("an empty ring produced completion {id}")),
    }
}

/// An assertion that **looks** like a contract and is actually a frozen
/// observation.
///
/// "After submitting, the completion is already there." Nothing in this
/// crate's API promises that, and Windows documents nothing about when a ring
/// operation completes relative to `SubmitIoRing`. It is simply what a
/// developer sees on the handle they happened to test on -- and writing it
/// down turns one run's testimony into a rule the suite will enforce forever.
fn completion_is_already_queued_after_submit<R: RingLike>(ring: &mut R) -> Result<(), String> {
    ring.push_one().map_err(|e| e.to_string())?;
    ring.submit().map_err(|e| e.to_string())?;
    match ring.try_pop().map_err(|e| e.to_string())? {
        Some(_) => Ok(()),
        None => Err("nothing was queued when submit returned".to_string()),
    }
}

/// The same question asked as **our** contract instead of the platform's
/// behaviour: the completion arrives within a bound we specify.
///
/// This is robust to the difference the previous function freezes, because it
/// is a statement about what this crate promises rather than about when the
/// kernel happens to finish.
fn completion_arrives_within_the_bound<R: RingLike>(ring: &mut R) -> Result<(), String> {
    ring.push_one().map_err(|e| e.to_string())?;
    ring.submit().map_err(|e| e.to_string())?;
    match ring.pop_within(std::time::Duration::from_secs(5)) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err("the completion did not arrive within the bound".to_string()),
        Err(e) => Err(format!("the wait failed: {e}")),
    }
}

// ------------------------------------ a resolver over the permitted space ---

/// A fake that does **not** model what Windows does. It models what Windows is
/// *permitted* to do, and a seed picks one resolution out of that space.
///
/// This is the distinction the rest of this probe was circling. A fake built
/// from observation freezes one run's testimony (case 4). A resolver takes the
/// platform's freedom as an **input dimension**: for each submitted operation
/// it decides, from the seed, whether the work finishes inside `SubmitIoRing`
/// or pends, and in what order completions are posted. Then the assertion is
/// about *us* -- does this crate behave correctly under that resolution --
/// rather than about the kernel.
///
/// Nothing here claims the kernel does these things. The claim is that it is
/// **allowed** to, so our code has to survive them. That makes the crate's
/// model of Windows an explicit, reviewable artifact instead of an accident of
/// whichever machine the tests last ran on.
struct Resolver {
    state: u64,
    /// Ops submitted but not yet resolved, in submission order.
    queued: Vec<usize>,
    /// Resolved completions, in the order this resolution posts them.
    posted: Vec<usize>,
    next_id: usize,
    outstanding: usize,
}

impl Resolver {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            queued: Vec::new(),
            posted: Vec::new(),
            next_id: 1,
            outstanding: 0,
        }
    }

    /// SplitMix64, matching the generator this crate already uses (`D-41`):
    /// one number replays a whole run.
    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn push(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.queued.push(id);
        self.outstanding += 1;
        id
    }

    /// Resolve everything queued. Two freedoms the platform actually has, and
    /// that this crate has measured: work may finish inside submit or pend
    /// (`write-pending-spike.rs`), and completion order is unspecified.
    fn submit(&mut self) {
        let mut batch: Vec<usize> = self.queued.drain(..).collect();
        // Fisher-Yates over the seed: any posting order is permitted.
        for i in (1..batch.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            batch.swap(i, j);
        }
        for id in batch {
            // Inline or pending is invisible from here -- either way the
            // completion becomes available. What differs is whether it is
            // there the instant submit returns, which is what a consumer must
            // not assume.
            self.posted.push(id);
        }
    }

    fn try_pop(&mut self) -> Option<usize> {
        if self.posted.is_empty() {
            return None;
        }
        self.outstanding -= 1;
        Some(self.posted.remove(0))
    }
}

/// A consumer with a hidden assumption: that completions arrive in the order
/// the operations were submitted.
///
/// Nothing in this crate promises that, and `DESIGN-NOTES.md` has a whole
/// section on the ring inviting exactly this belief. The question is whether a
/// seeded resolver finds the assumption that a fixed fake cannot.
fn a_consumer_that_assumes_fifo(seed: u64, ops: usize) -> Result<(), String> {
    let mut r = Resolver::new(seed);
    let submitted: Vec<usize> = (0..ops).map(|_| r.push()).collect();
    r.submit();

    let mut observed = Vec::new();
    while let Some(id) = r.try_pop() {
        observed.push(id);
    }
    if observed != submitted {
        return Err(format!("completions arrived as {observed:?}, not {submitted:?}"));
    }
    Ok(())
}

fn verdict(label: &str, result: Result<(), String>) -> bool {
    match &result {
        Ok(()) => println!("  {label:<38} GREEN"),
        Err(why) => println!("  {label:<38} RED   -- {why}"),
    }
    result.is_ok()
}

#[test]
fn m24_1_does_a_shared_suite_escape_the_mock_objection() {
    println!("\nM24.1: can a co-tested fake be trusted?\n");

    println!("Baseline -- a faithful fake and the kernel must agree:");
    let kernel_ok = verdict("kernel", {
        let mut r = RealRing::new("base").expect("a ring");
        shared_suite(&mut r)
    });
    let faithful_ok = verdict(
        "fake (faithful)",
        shared_suite(&mut FakeRing::new(Flaw::None)),
    );

    println!("\nPrediction 1 -- a wrong ACCOUNTING model must be caught:");
    let accounting_caught = !verdict(
        "fake (outstanding decremented early)",
        shared_suite(&mut FakeRing::new(Flaw::WrongAccounting)),
    );

    println!("\nPrediction 2 -- a wrong WINDOWS BELIEF must slip through:");
    let belief_slipped = verdict(
        "fake (pops without submitting)",
        shared_suite(&mut FakeRing::new(Flaw::WrongWindowsBelief)),
    );
    let historical_slipped = verdict(
        "fake (pre-M21.6: timeout is an error)",
        shared_suite(&mut FakeRing::new(Flaw::PreM216TimeoutBelief)),
    );

    println!("\nAnd now the same fake, against an assertion written AFTER the");
    println!("kernel taught us to ask it:");
    let kernel_timeout_ok = verdict("kernel", {
        let mut r = RealRing::new("timeout").expect("a ring");
        timeout_assertion(&mut r)
    });
    let historical_caught_once_asked = !verdict(
        "fake (pre-M21.6: timeout is an error)",
        timeout_assertion(&mut FakeRing::new(Flaw::PreM216TimeoutBelief)),
    );

    println!("\n--- verdict ---");
    println!("\nCase 3 -- the suite itself encodes the WRONG belief, and is run");
    println!("against both. This is what a mock-only world never does:");
    let kernel_refutes_us = !verdict("kernel", {
        let mut r = RealRing::new("oldbelief").expect("a ring");
        timeout_assertion_from_the_old_belief(&mut r)
    });
    let fake_confirms_us = verdict(
        "fake (built from the same wrong belief)",
        timeout_assertion_from_the_old_belief(&mut FakeRing::new(Flaw::PreM216TimeoutBelief)),
    );

    println!("\nCase 4 -- an assertion that LOOKS like a contract but is a frozen");
    println!("observation. Same API, same suite, two handle configurations:");
    const OVERLAPPED: u32 = 0x4000_0000;
    const NO_BUFFERING: u32 = 0x2000_0000;
    let buffered_says_yes = verdict("kernel, buffered handle", {
        let mut r = RealRing::with_flags("frozen-buf", 0, false).expect("a ring");
        completion_is_already_queued_after_submit(&mut r)
    });
    let unbuffered_says_no = !verdict("kernel, NO_BUFFERING + OVERLAPPED", {
        let mut r = RealRing::with_flags("frozen-nb", OVERLAPPED | NO_BUFFERING, true)
            .expect("a ring");
        completion_is_already_queued_after_submit(&mut r)
    });
    println!("  ... and the fake, which encoded whichever one its author saw:");
    let fake_agrees_with_one = verdict(
        "fake (faithful)",
        completion_is_already_queued_after_submit(&mut FakeRing::new(Flaw::None)),
    );

    println!("\n  The same assertion stated as OUR contract instead survives both:");
    let bound_buffered = verdict("kernel, buffered handle", {
        let mut r = RealRing::with_flags("bound-buf", 0, false).expect("a ring");
        completion_arrives_within_the_bound(&mut r)
    });
    let bound_unbuffered = verdict("kernel, NO_BUFFERING + OVERLAPPED", {
        let mut r = RealRing::with_flags("bound-nb", OVERLAPPED | NO_BUFFERING, true)
            .expect("a ring");
        completion_arrives_within_the_bound(&mut r)
    });
    let bound_fake = verdict(
        "fake (faithful)",
        completion_arrives_within_the_bound(&mut FakeRing::new(Flaw::None)),
    );
    println!("\nCase 5 -- a resolver over the PERMITTED space, seeded. The fake no");
    println!("longer models what Windows does; it models what Windows may do:");
    let mut broke = 0;
    let mut first_break = None;
    for seed in 0..200u64 {
        if let Err(why) = a_consumer_that_assumes_fifo(seed, 4) {
            broke += 1;
            if first_break.is_none() {
                first_break = Some((seed, why));
            }
        }
    }
    println!("  a consumer assuming FIFO completion order:");
    println!("    broke under {broke} of 200 seeds");
    match &first_break {
        Some((seed, why)) => println!("    first at seed {seed}: {why}"),
        None => println!("    never broke -- the resolver is not exploring"),
    }
    let resolver_discriminates = broke > 0 && broke < 200;
    println!(
        "  resolver explores rather than asserting one answer: {resolver_discriminates}"
    );
    println!("    (some seeds pass and some fail, which is the point -- a fixed");
    println!("     fake would have reported whichever single answer it encoded)");
    println!("\n--- summary ---");
    println!("  kernel green ......................... {kernel_ok}");
    println!("  faithful fake green .................. {faithful_ok}");
    println!("  wrong accounting caught .............. {accounting_caught}");
    println!("  wrong Windows belief slipped ......... {belief_slipped}");
    println!("  real pre-M21.6 belief slipped ........ {historical_slipped}");
    println!("  ... and caught once the suite asked .. {historical_caught_once_asked}");
    println!("  kernel satisfies the new assertion ... {kernel_timeout_ok}");
    println!("  kernel REFUTES our wrong belief ...... {kernel_refutes_us}");
    println!("  fake CONFIRMS our wrong belief ....... {fake_confirms_us}");
    println!("  frozen obs: buffered says YES ........ {buffered_says_yes}");
    println!("  frozen obs: unbuffered says NO ....... {unbuffered_says_no}");
    println!("  frozen obs: fake picks one ........... {fake_agrees_with_one}");
    println!("  our-contract form holds everywhere ... {}", bound_buffered && bound_unbuffered && bound_fake);

    let as_predicted = kernel_ok
        && faithful_ok
        && accounting_caught
        && belief_slipped
        && historical_slipped
        && historical_caught_once_asked
        && kernel_timeout_ok;
    println!(
        "\n  both halves behaved as predicted: {as_predicted}\n\n\
         Reading it: the shared suite is strong over accounting, which is a rule\n\
         WE own and fully specify. It is blind to any Windows belief it does not\n\
         already assert -- and the last two lines are the point, because the\n\
         assertion that catches the historical defect could only be written\n\
         AFTER the kernel had already revealed it. The fake never could have.\n"
    );

    assert!(kernel_ok, "the kernel must satisfy the shared suite");
    assert!(faithful_ok, "a faithful fake must satisfy it too");
    assert!(
        kernel_timeout_ok,
        "the kernel must satisfy the timeout assertion (M21.6 fixed this)"
    );
}
