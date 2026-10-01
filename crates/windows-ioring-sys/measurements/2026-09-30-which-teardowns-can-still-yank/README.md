# Which teardowns can still yank

2026-09-30. An audit, not a sweep: which paths in this workspace can still remove
a delivered completion packet, and therefore still reach the fault
[arrivals-no-longer-reach-the-factory](../2026-09-30-arrivals-no-longer-reach-the-factory/README.md)
describes. Listings in [teardown-paths-disasm.txt](teardown-paths-disasm.txt) and
[cleanup-group-vtable.txt](cleanup-group-vtable.txt).

Read from ntdll 10.0.26100.9278.

## The shipped fix is structural, and this is the check of that claim

`M-T4.2` made teardown drain rather than cancel. The claim has always been that
this is a property of the code rather than a probability, and it now has a direct
check: from `TppWorkWait` -- the branch `WaitForThreadpoolWaitCallbacks(FALSE)`
takes -- the entire reachable closure is **7 functions**, it contains **no
indirect dispatch**, and `NtCancelWaitCompletionPacket` is not in it.

The cancel branch reaches it immediately:
`TpWaitForWait -> TppCancelWait -> NtCancelWaitCompletionPacket`.

So the drain cannot remove a delivered packet however the timing falls, and the
gap can only make the race improbable. That difference is the whole reason the
drain is the fix and a delay is not.

## The exposure map

Every path in this crate that can still pass `RemoveSignaledPacket = TRUE`:

| path | reaches the yank? | on a default path? |
|---|---|---|
| `ThreadpoolWait::drop` | no -- drains | yes, and safe |
| `ThreadpoolWait::stop_and_drain` | no -- drains | yes, and safe |
| `CleanupGroup::drop` -> release with cancel FALSE | no | yes, and safe |
| `ThreadpoolWait::cancel_pending` | **yes** | no -- caller must ask |
| `WaitMember::cancel_pending` | **yes** | no -- caller must ask |
| `CleanupGroup::close_members(true)` | **yes** | no -- caller must ask |

**Every default path is safe. The hazard is reachable only when a caller
explicitly asks to cancel rather than drain.**

## M-T4.7: a cleanup-group consumer is NOT immune

The item asked whether owning a wait through a `CleanupGroup` avoids the hazard,
and expected a 20000-run sweep. The answer is analytic, and it is **no**.

The group dispatches member teardown through a vtable. Its wait entry is
[cleanup-group-vtable.txt](cleanup-group-vtable.txt), whose first four slots are
the real table: `TppFreeWait`, `TppAlpcpCallbackEpilog`,
`TppStopWaitCallbackGeneration`, `TppWorkCancelPendingCallbacks`. (The remaining
slots are adjacent data past the symbol's recorded extent, not pointers.)

The third reaches the primitive --
`TppStopWaitCallbackGeneration -> TppCancelWait -> NtCancelWaitCompletionPacket`
-- and it threads the caller's flag straight through:

```
mov  ebx,edx      ; the caller's cancel-pending argument
neg  ebx
sbb  r8d,r8d      ; -1 when set, 0 when clear
and  r8d,2        ; -> 2 when set
call TppCancelWait
```

and `TppCancelWait` turns that `2` into `RemoveSignaledPacket = TRUE`.

So the group has exactly the same drain-versus-cancel structure as a standalone
wait: `CloseThreadpoolCleanupGroupMembers(group, FALSE, ...)` is safe and
`(group, TRUE, ...)` is not. This crate's `Drop` already passes FALSE, so a
consumer who only drops is safe -- not because the group protects them, but
because the default was already the safe one.

## How this was nearly got wrong, and the rule that follows

**A reachability walk over direct calls can prove reachability. It cannot prove
unreachability wherever dispatch is indirect.**

The first run of this audit reported that the group's release *cannot* reach the
primitive, having explored 30 functions. That was wrong, and it was wrong in the
dangerous direction -- it would have closed `M-T4.7` as "immune". The release
dispatches through CFG-guarded indirect calls that the walk does not follow:

```
mov  rax,[rbx+8]          ; vtable
mov  rax,[rax+18h]        ; slot
call <__guard_dispatch_icall>
```

Reading the table converted the unsound negative into a sound enumeration, and
the answer reversed.

**Two controls caught two separate defects in the instrument**, which is the
reason to run them before the question rather than after:

1. An unrestricted walk claimed the *drain* reaches the primitive, via
   `TppBarrierAdjust -> RtlAcquireSRWLockShared -> ... -> RtlAbPreAcquire ->
   TpReleaseWait`. Nonsense: it wandered through generic SRW-lock infrastructure
   and back out via an address misattributed to a symbol whose recorded extent
   is wrong. Fixed by accepting only exact symbol starts as edges and not
   traversing `Rtl*` primitives.
2. The indirect-dispatch blindness above.

Where a negative result is relied on here, the closure was separately checked to
contain no indirect calls. That check is what makes the drain's negative sound
and the group's first negative unsound.
