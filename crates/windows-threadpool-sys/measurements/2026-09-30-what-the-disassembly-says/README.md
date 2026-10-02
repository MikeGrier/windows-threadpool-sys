# What the disassembly says

2026-09-30. ntdll 10.0.26100.9278, x64, public PDB symbols. Listings are beside
this file: [tp-wait-disasm.txt](tp-wait-disasm.txt),
[tp-setwaitex-tppcancelwait-disasm.txt](tp-setwaitex-tppcancelwait-disasm.txt),
[tppwaitcompletion-disasm.txt](tppwaitcompletion-disasm.txt).

**This is the first account of the fault that is read out of the implementation
rather than inferred from rates.** It explains two of the three measured effects
outright, and does not explain the third.

## A correction first: ntdll is not the kernel

Earlier reasoning in this investigation treated a call into ntdll as a call into
the kernel. That is wrong, and it matters here, because almost all of the thread
pool is ordinary user-mode code inside ntdll -- `Tp*` and `Tpp*` functions
manipulating fields of the `TP_WAIT` and its cleanup-group member under an SRW
lock. Only the `Nt*` stubs transition. `TpSetWaitEx`, `TppCancelWait`,
`TpReleaseWait` and `TpWaitForWait` contain **no** `syscall` instruction between
them; every transition is a `call` to a named `Nt*` stub, and those calls are
individually visible in the listings.

So the question "is control transferred to the kernel too quickly?" has a
concrete answer: no -- there is a substantial user-mode state machine first, and
that state machine is where the asymmetry lives.

## The one primitive that matters

`NtCancelWaitCompletionPacket(packet, RemoveSignaledPacket)`. Its second argument
decides whether a packet that has **already been delivered to the pool's
completion port** is forcibly pulled back out.

Who passes what, read off the code:

| Public call | ntdll path | `RemoveSignaledPacket` |
|---|---|---|
| `SetThreadpoolWait(wait, NULL, NULL)` -- the disarm | `TpSetWait` -> `TpSetWaitEx` | **FALSE** (`xor edx,edx`) |
| `WaitForThreadpoolWaitCallbacks(wait, TRUE)` -- the cancel | `TpWaitForWait` -> `TppCancelWait(flags=2)` | **TRUE** (`and r8d,2` then `setne dl`) |
| `WaitForThreadpoolWaitCallbacks(wait, FALSE)` -- the drain | `TpWaitForWait` -> `TppWorkWait` | **never called at all** |
| `CloseThreadpoolWait(wait)` -- the close | `TpReleaseWait+0x174` | **TRUE** (`movzx edx,bpl`, `ebp` = 1) |

The drain is the outlier, and it is the outlier *structurally*: on
`fCancelPendingCallbacks == FALSE` the function branches to `TppWorkWait` and
never reaches the cancel at all.

## Why the disarm alone is harmless, and the close is not

`TppCancelWait` and `TpSetWaitEx` both treat the association at `[wait+0x168]` as
the record of whether a packet is outstanding.

- If the packet has **not** yet been delivered, `NtCancelWaitCompletionPacket`
  returns success; the code clears `[wait+0x168]` and the wait is clean.
- If it **has** been delivered, the call returns `STATUS_PENDING` (`0x103`) or
  `STATUS_CANCELLED` (`0xC0000120`). The disarm then takes the path at
  `TpSetWaitEx+0x372`: it sets bit 2 of `[wait+0x1D0]`, takes a **+1** barrier
  reference via `TppBarrierAdjust`, returns FALSE -- and **leaves
  `[wait+0x168]` set**. It does not remove the packet, because it asked with
  FALSE.

So after a disarm that lost the race, the wait object still records an
outstanding packet. `TpReleaseWait` tests exactly that field, and when it is set
it issues the cancel **with TRUE** -- reaching in and removing a packet that is
already sitting in the completion port.

`TppWaitCompletion`, the dispatch path, is what should have consumed it: it
releases the barrier reference, clears `[wait+0x168]`, and clears the flag byte,
before running the callback.

## What this explains

- **Why the cancel call poisons at all.** It is the same forcible removal the
  close performs.
- **Why the cancel call is immune to elapsed time**, measured across four orders
  of magnitude. The removal happens when the call is made; waiting afterwards
  cannot put the packet back.
- **Why the drain is safe**, measured at 0 in 20004 in the model and 0 in 70000
  on the real path. It never calls the primitive.
- **Why the disarm alone is safe**, measured at 0 in 15000. It asks with FALSE.
- **Why the symptom is a worker factory with no workers.** The packet whose
  arrival would have caused the pool to make its first thread is removed, and the
  factory is left reporting no workers, `MayCreate` true, not paused, not shut
  down, and no failed creation -- which is exactly what 12 and 20 captures
  recorded.

## What this does not explain

**The gap curve.** On this account the close is dangerous whenever the disarm
lost its race, and that race is decided between the `SetEvent` and the disarm --
a window the gap experiments held constant. It predicts no dependence on the
disarm-to-close gap, and there is a large one: 0.87 per thousand at no gap, 4.15
at 3us, 0.025 at 30us
([a-short-gap-is-worse-than-none](../2026-09-30-a-short-gap-is-worse-than-none/README.md)).

The dispatch path does clear the association, so a gap long enough for a worker
to dequeue the packet would make the close safe -- but the callback is measured
not to have run in about 999 of 1000 runs at 30us, and dequeue precedes the
callback only slightly. So that is not a sufficient explanation either, and the
non-monotonic peak at 3us is not addressed by any of it.

**Do not read the table above as a complete account.** It is a mechanism for the
cancel and the close; the timing structure remains unexplained.

## The consequence for the shipped fix

The engineer's concern was that the fix might only have moved the fault into the
"extremely unlikely" region rather than removing a cause. On the gap, that concern
is exactly right -- a gap is a race that has been made improbable, and the tail is
not characterised.

The drain is a different kind of answer. `TpWaitForWait` with FALSE does not reach
`NtCancelWaitCompletionPacket` on any path, so it cannot remove a delivered
packet however the timing falls. That is a structural property of the code, not a
probability. It is the reason M-T4.2 is the right fix and a gap would not have
been.

## Method

`iced-x86` disassembling ntdll's mapped bytes in-process, with DbgHelp resolving
names from the public PDB so call targets are named rather than bare addresses
(3361 thread-pool and `Nt*` symbols enumerated). Every claim above is a branch or
an instruction in the committed listings; the flag values are immediate operands
(`xor edx,edx`, `mov r8d,2`, `mov ebp,1`) rather than inferences.

The reading is of **one** ntdll build. Nothing here establishes that the same
code shipped in other versions, and the field offsets are internal and
unstable.
