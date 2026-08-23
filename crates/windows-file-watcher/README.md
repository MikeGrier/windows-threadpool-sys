# windows-file-watcher

Memory-safe Windows path-change watching over `ReadDirectoryChangesW`, with a
`FindFirstChangeNotification` coarse fallback for filesystems that do not
support the detailed API.

This crate is Windows-only: every item is gated behind `cfg(windows)`, so it
resolves to an empty crate on other targets. Platform-independent watching is
meant to be built at a higher layer -- this crate is about excellent Windows
behaviour (path-name and notification-limitation fidelity) with memory safety.

## The model

A [`Monitor`] owns the watching and runs on no threads of its own -- all work
runs on `windows-threadpool-sys`'s thread pool. It hands out [`Session`]s, each
of which bundles a way to make requests with the destination every subscription
made through it delivers to; [`Monitor::session`] returns one together with the
[`Receiver`] its notifications arrive on. [`Session::subscribe`] registers a
path and returns an affine [`Watch`] that cancels when dropped, and
[`Session::answer`] responds to an interactive subscription's retry question.

```rust,no_run
use windows_file_watcher::{Monitor, Notification, WatchOptions};

let monitor = Monitor::new()?;
let (session, receiver) = monitor.session();
let watch = session.subscribe(r"C:\some\directory", WatchOptions::new())?;

while let Some(notification) = receiver.recv() {
    match notification {
        Notification::Batch { changes, .. } => println!("{} change(s)", changes.len()),
        Notification::Desync { cause, .. } => println!("re-scan: {cause:?}"),
        Notification::Completion { outcome, .. } => println!("request: {outcome:?}"),
        _ => {}
    }
}
# drop(watch);
# Ok::<(), std::io::Error>(())
```

A subscription targets either a directory (optionally recursive) or a single
file, watched through its parent directory. Several subscriptions on the same
directory share one coalesced watcher and one kernel read.

## Everything is queued, in both directions

The crate never calls into client code. A request is something the client
enqueues; a notification is something the crate enqueues and the client
collects. Nothing a client does -- blocking, panicking, being slow -- can stall
or unwind the crate's own cadence, and that holds by construction rather than by
asking a callback to behave.

Which thread a client drains on is entirely its own business. A client that
does not want to dedicate one to [`Receiver::recv`] can take
[`Receiver::doorbell`] and wait on it from its own thread pool -- including from
a `ThreadpoolWait` callback: ringing a doorbell is crate-owned queue signaling,
a bounded, non-blocking event, not a callback carrying client data, so this is
not an exception to "the crate never calls into client code" -- there is no
exception.

## Losses are reported, never silent

`ReadDirectoryChangesW` can lose changes -- its buffer overflows under a burst
-- and so can a client that stops draining, or a fault outage the monitor is
still recovering from. Every hole is reported as one cause-tagged
[`Notification::Desync`] meaning *re-scan*. Honest reporting of that limitation
is a core requirement of this crate rather than an afterthought.

## Faults recover on their own, or on your terms

A directory that cannot be opened yet, or a live watch that faults, is never a
terminal state: the monitor retries indefinitely (a target that will never
become watchable is the one exception, reported permanently rather than retried
forever). Each subscription chooses, at registration, how that recovery is
timed:

- **`RetryMode::Defaults`** (the default): the monitor retries autonomously, at
  a fixed 500ms delay per attempt.
- **`RetryMode::Interactive`**: on fault, the monitor asks -- a
  `Notification::RetryQuestion` names the failing operation and carries the
  real `FaultDetail` behind it, and
  [`Session::answer`] supplies the next delay (clamped to a 50ms floor); an
  explicit `answer(watch, None)` declines, which is what counts at the
  default -- never answering at all leaves the question outstanding
  indefinitely. A directory shared by several subscriptions takes the
  earliest answer.

A subscription that opts into `WatchOptions::report_liveness` additionally
receives `Suspended`/`Resumed` brackets around an outage and an
`Established { mode }` report naming which tier (detailed or coarse) is
actually watching.

## Two tiers, chosen automatically

Not every filesystem supports `ReadDirectoryChangesW`. When it does not, the
monitor falls back to the coarse `FindFirstChangeNotification` family, which
reports only that *something* changed within reach, delivered as
`Desync { Coarse }`. Which tier a directory uses is re-resolved on every
establish and re-establish, so a recovered fault always retries the detailed
path first.

## What this crate is not

It does not verify a reported change by re-reading content, does not cache
per-volume capability across process restarts, and does not surface the
extended `ReadDirectoryChangesExW` record format. These are recorded,
deliberate v1 scope decisions (see [DESIGN-NOTES.md](DESIGN-NOTES.md)), not
oversights.

## Stress-testing tool

[`src/bin/run_scenario.rs`](src/bin/run_scenario.rs) replays a persisted JSON scenario file through the
same data-driven stress model the test suite uses; see
[`src/bin/README.md`](src/bin/README.md) for usage and examples. It is
gated behind the `scenario-tool` feature (`serde`/`serde_json` are optional
dependencies), so it never affects an ordinary build of this crate.

## License

MIT. Copyright (c) Mike Grier.

