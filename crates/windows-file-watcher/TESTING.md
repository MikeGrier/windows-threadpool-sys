# Testing your code against `windows-file-watcher`

This guide is for **consumers** of `windows-file-watcher` -- application authors
and test-framework authors -- who want to test the code they build *on top of*
this crate without a real filesystem, a real thread pool, or the timing flakiness
those bring.

The crate exposes a small **test surface** (behind an off-by-default Cargo
feature) that lets your test become the source of notifications. You feed a real
`Receiver` with a scripted sequence of synthetic `Notification`s and assert on
how your own code reacts. Because your test decides *what* arrives and *when*, it
is fully deterministic -- the crate ships no scheduler or virtual clock for you to
steer.

## Philosophy: go below, don't mock above

A consumer reacts to `Notification`s drained from a `Receiver`. There are three
ways you could try to test that reaction logic:

- **Go above** -- wrap the whole `Monitor` API. Messy, and it throws away the
  delivery model you are actually trying to test against.
- **Replace** -- swap the crate for a fake. Also throws the delivery model away.
- **Go below** -- keep the crate's delivery model (`Notification`, `Receiver`,
  queue ordering, the doorbell) and substitute only the *source* of
  notifications. This is what the test surface does.

Going below is the only option that preserves the exact machinery your code runs
against in production, so a test that passes here exercises your real handler on
the real queue.

## Enabling the test surface

The surface is behind the `test-util` feature, which is **off by default**, adds
**no dependencies**, and (like the rest of the crate) is Windows-only. Turn it on
as a dev-dependency so it never touches your production build:

```toml
[dev-dependencies]
windows-file-watcher = { version = "0.1", features = ["test-util"] }
```

If you are building a reusable test framework that other crates depend on, expose
your own feature that forwards to it:

```toml
[features]
# your-crate's opt-in test support
test-support = ["windows-file-watcher/test-util"]
```

Everything below requires `test-util`; without it, the feed channel and the
`for_test` builders are not part of the public API.

## The shape of the seam

```text
  your test                  the crate's real delivery model         your code
  ---------                  -------------------------------         ---------
  Sender::send(n)  -->  [ bounded queue + doorbell + ordering ]  -->  Receiver::recv()
```

You build the channel, mint identities, construct notifications, push them, then
drain and dispatch exactly as a production loop would.

## A first test

```rust
use windows_file_watcher::{
    channel_with_bound, DesyncCause, Notification, Outcome, WatchId, DEFAULT_BOUND,
};

// The reaction logic under test -- your code, in isolation.
fn handle(notification: &Notification, rescans: &mut u32) {
    if matches!(notification, Notification::Desync { .. }) {
        *rescans += 1;
    }
}

#[test]
fn my_handler_counts_rescans() {
    let (sender, receiver) = channel_with_bound(DEFAULT_BOUND);
    let watch = WatchId::from_raw(1);

    // A scripted, deterministic sequence -- no OS involved.
    let _ = sender.send(Notification::Completion { watch, outcome: Outcome::Subscribed });
    let _ = sender.send(Notification::Desync { watch, cause: DesyncCause::Overflow });

    let mut rescans = 0;
    while let Some(notification) = receiver.try_recv() {
        handle(&notification, &mut rescans);
    }
    assert_eq!(rescans, 1);
}
```

See [`examples/test_your_handler.rs`](examples/test_your_handler.rs) for a fuller
worked example covering every notification kind.

## The building blocks

| Item | Purpose |
|---|---|
| `channel_with_bound(bound) -> (Sender, Receiver)` | Build a connected pair yourself. `bound` is a `NonZeroUsize`; `DEFAULT_BOUND` is a sensible default. |
| `Sender::send(Notification) -> Delivery` | Push a best-effort notification. Returns `Delivery::Queued` or `Delivery::Latched` (see backpressure below). |
| `Sender::reserve() -> Option<Reservation>` | Claim a slot for a message that must not be dropped; `Reservation::send` then cannot fail. |
| `Sender::has_room() -> bool` | Whether a best-effort `send` would be accepted right now. |
| `WatchId::from_raw(u64)` | Mint a subscription identity to tag notifications with. Any value is valid; the pairing is yours to choose. |
| `Receiver::try_recv() -> Option<Notification>` | Non-blocking drain; `None` when empty. |
| `Receiver::recv() -> Option<Notification>` | Blocking drain; `None` once the queue is empty *and* every `Sender` is dropped (clean teardown). |
| `Receiver::recv_timeout(Duration)` | Blocking drain with a deadline. |
| `Receiver::doorbell()` | A manual-reset event you can wait on from your own thread instead of blocking in `recv`. |

## Constructing every notification variant

All boundary types are constructible from public items. Two of them --
`RelativeName` (inside a `Change`) and `VolumeIdentity` -- have no production
constructor and gain `for_test` builders under the `test-util` feature.

```rust
use std::num::NonZeroUsize;
use windows_file_watcher::{
    channel_with_bound, Change, ChangeKind, DesyncCause, FailureCode, FaultDetail,
    FaultOperation, Notification, OpenFailure, Outcome, RelativeName, VolumeIdentity,
    WatchMode, WatchId,
};

let watch = WatchId::from_raw(1);

// Batch: the changes one completion carried, in kernel order.
let batch = Notification::Batch {
    watch,
    changes: vec![
        Change { kind: ChangeKind::Added,    name: RelativeName::for_test("new.txt") },
        Change { kind: ChangeKind::Modified, name: RelativeName::for_test("sub\\data.bin") },
    ],
};

// Desync: "you may have missed changes, re-scan".
let desync = Notification::Desync { watch, cause: DesyncCause::Overflow };

// Completion: a request you made was serviced.
let completion = Notification::Completion { watch, outcome: Outcome::Subscribed };

// Liveness bracket (opt-in in production via WatchOptions::report_liveness).
let suspended   = Notification::Suspended { watch };
let resumed     = Notification::Resumed { watch };
let established = Notification::Established { watch, mode: WatchMode::Detailed };

// RetryQuestion: an interactive subscription is asked how long to wait.
let question = Notification::RetryQuestion {
    watch,
    operation: FaultOperation::Open,
    detail: FaultDetail { failure: OpenFailure::NotFound, code: FailureCode::Win32(2) },
};

// VolumeChanged: a reopen landed on a different volume than before.
let volume = Notification::VolumeChanged {
    watch,
    previous: VolumeIdentity::for_test(0x1111, "NTFS", "System"),
    current:  VolumeIdentity::for_test(0x2222, "FAT32", "Removable"),
};
```

`RelativeName` also offers `for_test_os(&OsStr)` (lossless) and
`for_test_units(&[u16])` (the exact shape the kernel reports, lone surrogates and
all), for names a `&str` cannot express.

## Backpressure and loss

The queue is bounded, and its saturation behaviour is observable -- useful if your
code reasons about loss. Send into a full queue and the notification is dropped
and a `Desync { QueueFull }` is latched instead; the return value tells you which
happened:

```rust
use std::num::NonZeroUsize;
use windows_file_watcher::{channel_with_bound, Delivery, DesyncCause, Notification, Outcome, WatchId};

let (sender, _receiver) = channel_with_bound(NonZeroUsize::new(1).unwrap());
let watch = WatchId::from_raw(1);

assert!(matches!(
    sender.send(Notification::Completion { watch, outcome: Outcome::Subscribed }),
    Delivery::Queued,
));
// The queue (capacity 1) is now full; the next best-effort send is latched.
assert!(matches!(
    sender.send(Notification::Desync { watch, cause: DesyncCause::Coarse }),
    Delivery::Latched,
));
```

For a message whose loss your code cannot tolerate, reserve first:

```rust,ignore
if let Some(reservation) = sender.reserve() {
    // ... produce the message ...
    reservation.send(control_notification); // cannot fail: the slot is already held
}
```

## Writing a reusable test framework

The channel is a plain in-memory queue with no OS dependency, so a framework can
build freely on top of it:

- **You own ordering and timing.** Drive the sequence from a single thread for a
  strictly reproducible test. There is no hidden concurrency to control.
- **Reproducible variation.** If you want to explore many orderings, drive the
  choice points from a *seeded* PRNG so a given seed always replays identically.
  A tiny splitmix64-style generator on one thread is enough.
- **Multi-threaded rendezvous.** If your framework spawns threads (for example to
  model a producer racing a consumer), coordinate them with barriers/latches at
  the points that matter rather than relying on the scheduler; that keeps the
  interesting interleaving reproducible while the rest runs freely.
- **Doorbell integration.** If your harness drains on its own thread pool rather
  than blocking in `recv`, wait on `Receiver::doorbell()` -- a manual-reset event
  -- and drain with `try_recv` when it signals.
- **Teardown.** Dropping every `Sender` makes `recv` return `None`, so a drain
  loop terminates cleanly; use that to end a framework's collector thread.

## The fidelity limit (read this)

This surface tests **your reactions**, not whether the crate would ever emit the
sequence you fed it. Two consequences:

- The `for_test` builders are **valid by construction**, so you cannot mint an
  impossible *value* -- but an impossible *ordering* (a `Resumed` with no prior
  `Suspended`, say) is your responsibility to avoid, exactly as with any
  hand-authored test double.
- Do **not** use this surface to convince yourself you are calling the crate's
  *own* API correctly. That is a different question, and only a real `Monitor`
  answers it.

For end-to-end fidelity against the operating system -- that a real directory
change produces the notifications you expect -- drive a real `Monitor` against a
temporary directory instead. The crate's own integration tests under
[`tests/`](tests) and the runnable
[`examples/`](examples) show that shape.

## See also

- The crate-level "Testing your consumer code" section in the API docs
  (a compile-tested version of the first example above).
- [`examples/test_your_handler.rs`](examples/test_your_handler.rs) -- a fuller
  worked example.
- [`DESIGN-NOTES.md`](DESIGN-NOTES.md) -> "Consumer test surface" (decisions
  D-81/D-82/D-83) for the rationale, including why the feed channel and the
  `for_test` builders are feature-gated.
