# Unresolved test failures: windows-ioring-sys

Pre-existing failures that do not block an unrelated commit, recorded per the repository's
checklist-execution rules. When one is resolved, move its entry into a sibling
[RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md) (append-only) rather than deleting it.

## a backlog is not delivered when the caller attached the event and consumed its signal

**Found 2026-09-25**, by Copilot review on PR #108, which reported the narrower form: that
`EventDelivery::new` signals only when it attached the event itself, so a caller who attached it
earlier arms a wait on an already-non-empty, edge-triggered queue with no wakeup owing.

The reproducer is
`a_backlog_is_delivered_even_when_the_caller_attached_the_event_first` in
[event_delivery.rs](tests/event_delivery.rs), `#[ignore]`d because it fails. It attaches the event,
**consumes** the signal attaching raised, submits work, consumes the signal the completions raise,
and only then hands the ring over -- leaving a non-empty queue with nothing pending, which is the
state the guarantee is about. An earlier version of the test omitted the two consuming waits and
passed against the defect, because the leftover signal fired the wait; that version proved nothing.

**The reported repair does not work, which is why nothing is applied.** Signalling unconditionally
after arming leaves the test failing 6 of 6. What does make it pass is a 50 ms sleep between
`wait.arm` and the signal: 3 of 3. Building with `--features trace` also makes it pass, which is the
same schedule perturbation by another route.

So the wakeup is lost in a window *after* arming, rather than never being raised. That is wider than
the review finding, and it bears on [D-68](DESIGN-NOTES.md#d-68): `M26.9` fixed the delivery stall
by ordering the arm before the signal, measured at 0 failures in 3600 runs, and this says that
ordering alone is not sufficient to close the window -- only to narrow it.

**Not established:** why the window exists. `SetThreadpoolWait` is documented as registering the
wait, so a signal after a completed `arm` should be observed. Whether the pool's wait thread
re-issues its `WaitForMultipleObjects` asynchronously, and whether an auto-reset signal can be
consumed and discarded during that re-issue, is a guess and is recorded here as one. Queued as
`M26.12`.
