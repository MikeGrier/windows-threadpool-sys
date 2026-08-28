# Design notes: windows-impersonation-token-sys (Tier 1)

This file is authoritative for the crate. The cross-component decision is also
recorded in the workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md), its historical
reasoning is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), and implementation is
scheduled by M4 in the workspace [CHECKLIST.md](../../CHECKLIST.md).

## Intent

Provide one memory-safe Rust abstraction for capturing the calling thread's
effective Windows impersonation state, carrying it to another thread, applying it
for a bounded operation, and restoring the exact prior state.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **The crate is an independently publishable platform layer.** File enumeration and later traversal both need the same security-context transport, while the primitive is independent of either consumer and of the Windows thread pool itself. |
| <a id="d-2"></a>D-2 | **The public abstraction is the opaque owned `ImpersonationToken`.** A raw `HANDLE` cannot express ownership, capture timing, required rights, immutability, cross-thread transport, or scoped restoration. The implementation still uses the corresponding Microsoft `windows-sys` APIs and types. |
| <a id="d-3"></a>D-3 | **Capture is explicit and synchronous on the submitting thread.** No worker may infer or recapture the submitter's context. A consumer may capture once and reuse the resulting token for later operations. |
| <a id="d-4"></a>D-4 | **Application restores the exact prior thread-token state.** This includes nested impersonation and unwind paths. Restoration failure panics from the application guard's `Drop` because returning a shared worker under an unknown identity is a process security failure. |
| <a id="d-5"></a>D-5 | **This is not a general security-helper collection.** New surface belongs only when it is required to preserve the capture, transport, apply, or restore lifecycle of `ImpersonationToken`. |
| <a id="d-6"></a>D-6 | **Capture duplicates the effective token into an immutable `TOKEN_IMPERSONATE`-only handle.** The thread token is opened with `OpenAsSelf` and only query/duplicate rights; its identification, impersonation, or delegation level is preserved. No-thread-token context falls back to a process-token snapshot at `SecurityImpersonation`. Anonymous context is rejected synchronously. Clones share ownership of the captured handle rather than acquiring broader rights. |
| <a id="d-7"></a>D-7 | **Scoped application is exposed only as a closure operation backed by a private thread-bound guard.** The guard saves an exact `TOKEN_IMPERSONATE` handle or explicit no-token state, applies the captured handle with `SetThreadToken`, and restores the saved state before return and during unwind. Its `Rc` marker makes it `!Send` and `!Sync`. Restoration failure panics from `Drop`; callers cannot obtain or forget the guard. |
| <a id="d-8"></a>D-8 | **`ImpersonationToken` implements no equality, and this is deliberate.** Object-identity equality is buildable (`Arc::ptr_eq`, or `CompareObjectHandles`, neither needing access rights) but is a trap, because `DuplicateTokenEx` mints a *new* token object -- two captures of one context are interchangeable yet would compare unequal. Identity equality is worse: it needs `TOKEN_QUERY`, which [D-6](#d-6) deliberately withholds from the captured handle, and even via capture-time metadata a same-user, same-LUID comparison returns true for a *restricted* token derived from the same logon. `==` on a security principal reads as "same rights" and nothing implementable means that. **No work is scheduled by this decision**; it records why an obvious-looking addition is refused. See [Equality is refused, not deferred](#equality-is-refused). |
## Publication boundary

The crate is Windows-only, independently versioned, and published to crates.io.
Its manifest carries the workspace's normal metadata, Windows docs.rs target, and
only the `windows-sys` features needed by the token lifecycle. Release automation
is registered at the workspace level.

## <a id="equality-is-refused"></a>Equality is refused, not deferred

<a id="d-8-detail"></a>

The question arises naturally -- a consumer writing tests wants `assert_eq!` on a
`Captured<ImpersonationToken>` rather than `matches!` -- and it should be
answered once rather than re-litigated. Three routes exist, and all three are
declined.

**Introspecting the captured handle is closed by [D-6](#d-6).** The stored handle
is duplicated with `TOKEN_IMPERSONATE` and nothing else. Every question a token
can answer about itself -- user, groups, privileges, statistics, integrity level
-- goes through `GetTokenInformation`, which needs `TOKEN_QUERY`. Widening the
captured handle to permit it would contradict the invariant that no safe API
permits rights expansion, in exchange for a convenience.

**Object identity is implementable and is still the wrong contract.** Clones
share the handle through an `Arc`, so `Arc::ptr_eq` answers clone-identity for
free, and `CompareObjectHandles` extends that to separately duplicated handles
without needing rights on either. But `DuplicateTokenEx` mints a **new token
object**, so capturing the same context twice produces two tokens that are
semantically interchangeable and compare **unequal**. An `Eq` that fails on two
captures of one context misleads in the direction a reader will not check.

**Capture-time metadata is feasible and is the most dangerous of the three.** The
thread-token path already opens the source with `TOKEN_QUERY`, so a user SID and
authentication LUID could be recorded beside the handle without widening the
stored rights. The resulting comparison would be wrong in the *unsafe*
direction: a restricted token derived from the same logon carries the same user
and the same LUID while differing in group membership, enabled privileges,
integrity level, restricting SIDs, and AppContainer. `token_a == token_b` reads
as "same rights", and that comparison would return true when the rights differ.

The general rule this crate follows: **`==` on a security principal is a
predicate consumers will use to skip work, so it must not exist unless it means
what they will assume it means.** Nothing implementable here does.

If a real need appears -- most plausibly a facility asking "is this the context
already applied on this worker?", to avoid re-application -- the answer is a
narrowly named inherent method such as `is_same_object_as`, whose name states
exactly what it compares and cannot be mistaken for identity equality. That is
not queued as work, because no consumer needs it today and adding it
speculatively would enlarge a published security surface on a guess. It is
recorded here so the option is found rather than rediscovered.
