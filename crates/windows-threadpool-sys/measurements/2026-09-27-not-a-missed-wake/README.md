# It is not a missed wake -- 2026-09-27

Raised in review, and the best-supported hypothesis this failure has had: it
smells like a lost wakeup, the same shape as the auto-reset-event and
arming-order problems that [D-19](../../../windows-ioring-sys/DESIGN-NOTES.md#d-19),
[D-68](../../../windows-ioring-sys/DESIGN-NOTES.md#d-68) and [D-77](../../../windows-ioring-sys/DESIGN-NOTES.md#d-77) each
addressed a version of.

It is not. Re-signalling the very event the wait is armed on, during the stall,
changes nothing.

## Why the hypothesis was strong

Everything about the signature fits a lost wakeup. `callbacks run: 0`. The
completion event is auto-reset ([D-21](../../../windows-ioring-sys/DESIGN-NOTES.md#d-21)), so a
signal is consumed rather than left pending. It is edge-triggered on the
completion queue going empty to non-empty ([D-19](../../../windows-ioring-sys/DESIGN-NOTES.md#d-19)),
so a ring whose queue is already non-empty is signalled by nothing else. And
the stall is now known to be **permanent**, which is exactly what a lost
wakeup on such an event looks like. This crate has had that bug twice.

## The test

If the wait is armed and healthy and merely never woken, then setting that same
event again must release it -- with no work submitted to the pool at all.

`SetEvent` is safe to use here where a zero-timeout poll would not be: setting
an auto-reset event can only add a signal, never consume one, so unlike the
poll this cannot manufacture the bug it is looking for.

The shape of one post-mortem:

```text
quiet 2s        confirms the stall is still live and silent
resignal        SetEvent on every delivery's own completion event
observe 3s      did it release?
control         a work submit, known to release it
```

## What was observed

5 captured stalls. In **all five**, the delivery arrives only at the control:

| Capture | SetEvent returns | re-signalled at | first delivery |
|---|---|---|---|
| stall-0295 | 0,1,1,0,1,1 | 7.012s | 10.013s |
| stall-1146 | 1,1,1,1,1,1 | 7.006s | 10.007s |
| stall-1283 | 1,1,1,1,1,1 | 7.005s | 10.006s |
| stall-1978 | 1,1,1,1,1,1 | 7.007s | 10.008s |
| stall-3250 | 0,1,1,0,1,1 | 7.004s | 10.005s |

Three full seconds pass between the signal and the delivery, and the delivery
lands at the work submit, not at the signal.

## The control, which is why the zeroes matter

A `SetEvent` that quietly failed would look exactly like a signal that did
nothing, so every call's return value is recorded. Two captures show `0` for
the *first* handle of each pass -- that is the trigger's delivery, which has
already been dropped, so its duplicated handle is closed. Expected, and the
reason it is worth seeing.

**Both victims' handles returned 1 in every capture.** The events that matter
were successfully signalled, every time, and the stall continued anyway.

## What this means

The wait is armed. Its event can be signalled successfully. The callback still
does not run. So the fault is not in the signal and not in the arming -- it is
that the pool does not dispatch, which is what
[2026-09-27-which-poke-releases-the-stall](../2026-09-27-which-poke-releases-the-stall/README.md)
showed from the other direction when a fresh wait, a fresh timer and a fresh
I/O completion all failed to dispatch during the same window.

It also closes off the reading that `D-19`, `D-68` or `D-77` left something
behind. Those decisions stand on their own evidence; none of them is implicated
here.