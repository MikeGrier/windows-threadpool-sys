// Copyright (c) 2026 Mike Grier
//! A blocking push, built beside durable-ioring rather than into it (`DI-7.4`, `DI-D-56`).
//!
//! A push the ring cannot take right now is refused, and the refusal names its cause: submit
//! first, drain first, or wait for another submission. [`push_blocking`] is the loop that reacts to
//! that for a caller who wants back-pressure rather than a refusal -- try, and while the cause is
//! one that clears, make room and try again, up to a patience. It is written entirely against
//! durable-ioring's public API, and is here to be used as it stands or taken and adapted: a caller
//! that would rather shed load, queue the write itself, or fail fast has the same refusal to react
//! to, which is why the library does not choose for it.
//!
//! **What "make room" means is the caller's, because it depends on how the instance is delivered:**
//!
//! - **Pool delivery** drains on a pool thread, so there is nothing to do but give it a moment. The
//!   instance offers no signal that a drain ran, so this backs off in short sleeps.
//! - **Pumped and signalled delivery** are drained by the caller's own thread, so making room is
//!   `pump`: it drains, and if that delivered nothing it waits for the ring to say there is
//!   something to drain. For a signalled instance that wait is on `completion_signal`, which a caller
//!   with other handles to wait on would wait on itself, and then pump without waiting.
//!
//! **Never call this from inside a callback** -- a provider's `make_durable`, or a handler running
//! on the pool: under pool delivery the drain it waits for runs on the pool, which the blocked call
//! may be holding up, and under pumped delivery it pumps the instance from a thread that may be in
//! the middle of doing so already. The helper cannot tell, so it does not try.
//!
//! Run it with `cargo run --example blocking_push`; its tests run with the crate's, so a change to
//! the public API that breaks it breaks the build.

use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use win_durable_ioring::windows_backend::{Cached, FileOptions, OpenedFile, Win32FileTarget};
use win_durable_ioring::{
    DeliveryMode, Dioring, DurableRing, Entry, IoTargetKey, LineageHandle, OpIdOf, PushError,
    PushRefusal, PushResult, Setup, TargetSetup,
};

#[cfg(test)]
mod tests;

/// The longest one wait for room lasts: short enough that a push whose patience is nearly spent
/// notices, long enough that a drain's worth of work is not spent polling.
const SLICE: Duration = Duration::from_millis(50);

/// Why a blocking push gave up. Every case hands the push back whole.
pub struct Blocked<R: DurableRing> {
    /// The refusal, with the buffer and context it hands back, so the caller can try something
    /// else with them.
    pub refused: PushError<R::Ids, R::Buffer, R::Context>,
    /// Why it stopped.
    pub why: GaveUp,
}

/// Why [`push_blocking`] stopped.
#[derive(Debug)]
pub enum GaveUp {
    /// The refusal is not one waiting clears: no such file, a sealed epoch, an operation the
    /// backend cannot perform. `refused.reason` says which.
    NotRetryable,
    /// The patience passed with the ring still refusing.
    TimedOut,
    /// Making room failed, which is the caller's `make_room` reporting an error.
    MakeRoom(io::Error),
}

/// Push through `attempt`, and while the ring refuses for a cause waiting clears, call `make_room`
/// and try again, for up to `patience`. Returns the operation's identity, or why it gave up with the
/// buffer and context handed back.
///
/// `attempt` is the push itself, given the buffer and the context each time -- a write, a read, with
/// whatever tag and options the caller wants -- and returns the instance's own `PushResult`. It
/// is called with the buffer a refused push handed back, so the buffer is never copied. The
/// buffer must be an owned one: a push of a registered span has none to hand back, and for one an
/// `attempt` that ignores them and a loop of the caller's own is the shape.
///
/// `make_room` is given the longest it should wait, and should drain the instance in the way its
/// delivery needs. It is not called when the first attempt is accepted.
pub fn push_blocking<R, A, M>(
    ring: &mut R,
    buffer: R::Buffer,
    context: R::Context,
    patience: Duration,
    mut attempt: A,
    mut make_room: M,
) -> Result<OpIdOf<R>, Blocked<R>>
where
    R: DurableRing,
    A: FnMut(&mut R, R::Buffer, R::Context) -> PushResult<R>,
    M: FnMut(&mut R, Duration) -> io::Result<()>,
{
    let deadline = Instant::now() + patience;
    let (mut buffer, mut context) = (buffer, context);
    loop {
        let refused = match attempt(ring, buffer, context) {
            Ok(id) => return Ok(id),
            Err(refused) => refused,
        };
        let clears = matches!(&refused.reason, PushRefusal::Backend(cause) if cause.is_retryable());
        if !clears || refused.buffer.is_none() {
            return Err(Blocked {
                refused,
                why: GaveUp::NotRetryable,
            });
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Blocked {
                refused,
                why: GaveUp::TimedOut,
            });
        }
        if let Err(error) = make_room(ring, left.min(SLICE)) {
            return Err(Blocked {
                refused,
                why: GaveUp::MakeRoom(error),
            });
        }
        let PushError {
            buffer: back,
            context: back_context,
            ..
        } = refused;
        buffer = back.expect("checked above: an owned-buffer push hands its buffer back");
        context = back_context;
    }
}

// ---- What follows is the demonstration, over a real file and a real ring. ----

type Ring = Dioring<Vec<u8>>;

const FILE: IoTargetKey = IoTargetKey(1);

/// The delivery modes, as the demonstration names them.
#[derive(Clone, Copy, Debug)]
pub enum Delivery {
    Pool,
    Pumped,
    Signalled,
}

impl Delivery {
    fn mode(self) -> DeliveryMode<'static, 'static> {
        match self {
            Self::Pool => DeliveryMode::Pool(None),
            Self::Pumped => DeliveryMode::Pumped,
            Self::Signalled => DeliveryMode::Signalled(None),
        }
    }

    /// Making room, for an instance delivered this way.
    fn make_room(self, ring: &mut Ring, slice: Duration) -> io::Result<()> {
        match self {
            // The pool drains on its own: give it a moment.
            Self::Pool => {
                std::thread::sleep(slice.min(Duration::from_millis(1)));
                Ok(())
            }
            // Drains, and if that delivered nothing waits for the ring -- or, signalled, for the
            // signal the pool sets -- to say there is something to drain.
            Self::Pumped | Self::Signalled => ring.pump(Some(slice)).map(|_| ()),
        }
    }
}

/// A file that is removed when it goes.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dioring-blocking-push-{}-{tag}.bin",
            std::process::id()
        ));
        std::fs::write(&path, []).expect("a scratch file");
        Self(path)
    }

    fn target(&self) -> TargetSetup {
        let name = win_durable_ioring::Wtf16String::from_os_str(self.0.as_os_str());
        let file = OpenedFile::<Cached>::open(name, FileOptions::new().read().write())
            .expect("open the scratch file");
        TargetSetup::new(FILE, Win32FileTarget::new(file))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// An instance over `scratch`, delivered as `delivery`, with queues of `queue` entries.
pub fn build(delivery: Delivery, scratch: &Scratch, queue: u32) -> Ring {
    Ring::new(Setup {
        submission_queue_size: queue,
        completion_queue_size: queue,
        files: vec![scratch.target()],
        buffers: Vec::new(),
        provider: None,
        diagnostics: None,
        delivery: delivery.mode(),
    })
    .expect("build an instance")
    .0
}

/// A write of `buffer` at `offset`, in epoch 1 of `lineage`, as an `attempt` for [`push_blocking`].
pub fn write_at<'a>(
    lineage: &'a LineageHandle,
    file: IoTargetKey,
    offset: u64,
) -> impl FnMut(&mut Ring, Vec<u8>, ()) -> PushResult<Ring> + 'a {
    move |ring, buffer, context| ring.write(file, offset, buffer, lineage.at(1_u64), context)
}

/// What a demonstration run did.
#[derive(Debug)]
pub struct Report {
    pub pushed: usize,
    /// How many times a push was refused and made room.
    pub made_room: usize,
    pub completed: usize,
}

/// Push `writes` blocks through an instance delivered as `delivery`, with queues far smaller than
/// that, blocking whenever the ring will not take one, then collect every completion.
pub fn run(delivery: Delivery, writes: usize, queue: u32) -> Report {
    let scratch = Scratch::new(&format!("{delivery:?}"));
    let mut ring = build(delivery, &scratch, queue);
    let lineage = ring.default_lineage();
    let mut made_room = 0;
    let mut pushed = 0;
    for i in 0..writes {
        let offset = u64::try_from(i).expect("a small index") * 8;
        let result = push_blocking(
            &mut ring,
            vec![u8::try_from(i % 256).expect("a byte"); 8],
            (),
            Duration::from_secs(10),
            write_at(&lineage, FILE, offset),
            |ring, slice| {
                made_room += 1;
                delivery.make_room(ring, slice)
            },
        );
        match result {
            Ok(_) => pushed += 1,
            Err(blocked) => panic!("a push gave up: {:?}", blocked.why),
        }
    }
    let mut completed = 0;
    let started = Instant::now();
    while completed < writes {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "completions never arrived"
        );
        match ring.pop().expect("pop") {
            Some(Entry::Op(_)) => completed += 1,
            Some(other) => panic!("expected a completion, got {other:?}"),
            None => delivery.make_room(&mut ring, SLICE).expect("make room"),
        }
    }
    Report {
        pushed,
        made_room,
        completed,
    }
}

fn main() {
    for delivery in [Delivery::Pool, Delivery::Pumped, Delivery::Signalled] {
        println!("{delivery:?}: {:?}", run(delivery, 100, 4));
    }
}
