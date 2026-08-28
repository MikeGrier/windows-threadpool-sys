# Design notes: windows-thread-ambient-sys

Decisions for this crate. The workspace-level context that produced it -- the
namespace-plane division of labour, and the measurements of what a thread-pool
worker does and does not inherit -- is in the workspace
[DESIGN-NOTES.md](../../DESIGN-NOTES.md), which is authoritative for anything
this file does not cover. Pending work is in the workspace
[CHECKLIST.md](../../CHECKLIST.md), milestones M22 and M23.

## Why the crate exists separately

The composite this crate provides was first designed as an internal type of a
planned namespace-remoting facility, and recorded there as something that "may
be extracted" once it became a genuine cross-crate contract. That precondition
arrived early and from an unanticipated direction: an independent consumer needs
to carry a caller's ambient state onto another thread with none of that facility
around it. Extraction is therefore not preemptive.

Because it now has more than one consumer, the crate is a **level platform**. It
offers every aspect for capture *and* for explicit declaration, and privileges no
combination of them. A consumer running on process-shared threads will force the
dialog-suppressing error-mode bits; a consumer owning a private thread is
entitled to the opposite choice, and must not have to fight this layer to make
it. Policy belongs to the consumer; primitives belong here.

## The scope test

The crate carries **thread-scoped ambient state that changes what a Win32 call
does**. Three consequences of taking that literally:

- **Call parameters are out.** A desired access mask, a share mode, a security
  descriptor and a path are parameters, not ambient state. They belong to a
  request type, and keeping them out is what stops this crate growing into a file
  API. See the sibling `windows-namespace-request-sys` (workspace
  [CHECKLIST.md](../../CHECKLIST.md), M24).
- **Process-scoped state is out.** The current directory is the notable case: it
  is process-wide and mutable by any thread, so it cannot be captured per-thread
  and remoting it would be racy regardless. A consumer resolves paths on the
  calling thread instead.
- **State that rides along is out.** Drive-letter resolution follows the
  impersonation token's logon session, so it is not a separate aspect to capture;
  it arrives with the token or not at all. This is measured, and the consequence
  is a hazard a consumer must know about rather than something this crate can
  fix: under a captured token, the same path string can name a different device
  or nothing at all.

## Two sets, not one

<a id="d-two-sets"></a>

The aspects divide by whether the calling thread's value can be *read*:

| Set | Aspects | Chosen by |
|---|---|---|
| **Capturable** | impersonation, thread error mode, TxF transaction | a capture set, per call |
| **Declared** | WOW64 filesystem redirection, memory priority, I/O priority | the caller states a value; unspecified means leave the target alone |

An earlier statement of this division placed WOW64 redirection with the captured
aspects. That was not implementable rather than merely debatable:
`Wow64DisableWow64FsRedirection` yields an `OldValue` only as a side effect of
*disabling* redirection, and there is no getter, so the current state cannot be
observed without changing it. An aspect that cannot be read cannot be
transplanted.

**A declared aspect is not "an aspect excluded from the capture set".** There is
nothing to collect, so it is not in that vocabulary at all. Conflating the two
would make "unspecified" ambiguous between *leave the target thread alone* and
*install the caller's value*, which are different behaviours.

The thread error mode deliberately appears in **both** sets. It is readable, so
it can be captured -- for diagnostics, or to transplant -- and it is also the
aspect consumers most want to override. Offering only one of those would encode a
consumer's policy in a platform layer.

## Not captured is not the same as captured and absent

<a id="d-three-state"></a>

Each captured aspect is a three-state value: **not captured**, **captured and
absent**, or **captured with a value**. `Option` is insufficient, and the reason
is a real hazard rather than a taste for precision.

Take impersonation. "Impersonation was not in the capture set" and "impersonation
was captured, and the caller had no thread token" both end with the worker
running under the process identity. The observable outcome is identical; the
meaning is not, and only one of them is a deliberate statement about what the
work should run as. Collapsing them makes an omission indistinguishable from a
decision, and a later reader cannot recover which one happened.

The shape is uniform across aspects even where one state is unreachable -- see
the note on impersonation below -- because a per-aspect shape would put the
burden of remembering which aspects can be absent onto every consumer.

## The default capture set is a named constant, not a `Default` impl

<a id="d-named-default"></a>

The workspace decision that this composite is exhaustively enumerated rests on
its field list being contract surface: a silently added field is a silent
semantic change. A *default set* has the same property in a worse form, because
growing it changes behaviour for callers who never named it and have no diff to
review.

So the default is a named constant. Adding an aspect to it is then a visible
change to a named thing rather than an invisible change to an implicit one, a
caller wanting stability names its aspects explicitly, and a caller taking the
default can go read what the default contains.

## Application order, and why impersonation is innermost

<a id="d-guard-order"></a>

Application composes per-aspect guards, applied outermost-first and released in
exact reverse:

1. thread error mode -- outermost, so that hard-error suppression is already in
   force while the remaining aspects are being applied;
2. priority;
3. WOW64 filesystem redirection;
4. TxF transaction;
5. impersonation -- innermost, because its window is the narrowest and its
   restoration is the one that must not be delayed.

Applying a subset must stay expressible. That is not a convenience: the aspects
have genuinely different application windows. A consumer may want impersonation
around an open alone, reverting immediately because later work uses the handle
and needs no token, while the error mode must hold for the whole callback since
any blocking call can raise a hard error.

## Restore failure is fail-fast only where the hazard warrants it

<a id="d-restore-policy"></a>

Failing to restore impersonation is fail-fast, because returning a shared worker
to a pool under an unknown identity is a process-wide security failure. That
semantics is **inherited unchanged** from
[windows-impersonation-token-sys](../windows-impersonation-token-sys/DESIGN-NOTES.md);
this crate does not restate or reimplement it.

The other aspects do not warrant that severity, and imposing it on them would be
the single-strictest-semantics failure the composite exists to avoid. Their
restore failures are **reported to the caller** rather than being either fatal or
silently dropped. Silence is the option specifically rejected: a thread left with
the wrong error mode or redirection state is contaminated, and a consumer that
owns the thread may reasonably decide to retire it.

## Aspect notes

### Impersonation

Consumed from
[windows-impersonation-token-sys](../windows-impersonation-token-sys/DESIGN-NOTES.md),
never reimplemented. Note that its capture never yields an absent token: when the
calling thread has no token it snapshots the process identity as a
`SecurityImpersonation` token. So this aspect's *captured and absent* state is
unreachable by construction. The three-state shape is retained anyway, per the
decision above.

### TxF transaction

`ktmw32` is bound lazily rather than linked, so a consumer that never captures a
transaction does not acquire a dependency nothing else in the workspace has.

The aspect carries an owned duplicate of the transaction handle, so the captured
value does not depend on the caller keeping its own handle open.

One hazard it cannot remove, stated because it is invisible from the API: the
caller may commit or roll the transaction back while the worker is still inside
it. TxF is also deprecated by Microsoft, which is a reason to keep the aspect
optional and out of any minimal default, not a reason to omit it.

### Thread error mode

Which `SEM_` bits are actually settable per-thread decides which bits this crate
can offer as declarable. The documented set for `SetThreadErrorMode` is three
bits and excludes `SEM_NOALIGNMENTFAULTEXCEPT`, which is process-scoped and
sticky once set. That is measured rather than read off the documentation; see the
workspace [CHECKLIST.md](../../CHECKLIST.md), M22.2.
