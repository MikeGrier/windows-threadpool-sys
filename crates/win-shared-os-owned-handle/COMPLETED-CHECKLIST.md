# Completed checklist: win-shared-os-owned-handle

## Moved 2026-10-21 21:40:08 -04:00 -- SH-1+.1 and SH-1+.2: adoption and publication; the checklist is complete

### <a id="sh-1+1"></a>SH-1+.1 -- Every crate that wrapped `Arc<OwnedHandle>` itself now holds a `SharedHandle`, and none is left: the ring crate and, in the Windows driver, `durable-ioring`'s file type. *(completed 2026-10-21 21:40:08 -04:00)*

The item as written, with its last sub-item checked:

**SH-1+.1** -- **Adoption by the crates that wrap `Arc<OwnedHandle>` themselves.** The crate
  exists so that they stop. Each adoption is that crate's own checklist item:
  - [x] `windows-ioring-sys`: its own `SharedFile` is removed, and a safe push takes `SharedHandle`
    (its [D-82](../windows-ioring-sys/DESIGN-NOTES.md#d-82)), landed by #119 -- not the alias first
    planned; its `M31.3`, archived in
    [its COMPLETED-CHECKLIST.md](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m313).
  - [x] `durable-ioring`: its design names `SharedHandle` as the contract's file type (its
    `DI-D-29`), replacing the crate-local `DurableFile` it had first designed; it is built on it
    from `DI-3.1`.

  Done when both have landed. Two further crates wrap a handle the same way and were parked by the
  engineer on 2026-10-07, each in its own checklist and outside this item:
  `windows-namespace-request-sys`' `NR-inf.1` ([its CHECKLIST.md](../windows-namespace-request-sys/CHECKLIST.md)),
  which is not a drop-in replacement and which graduated on 2026-10-18 to that crate's `NR-6`
  ([D-32](../windows-namespace-request-sys/DESIGN-NOTES.md#d-32)), and `windows-impersonation-token-sys`' `IT-inf.1`, done 2026-10-18
  ([its COMPLETED-CHECKLIST.md](../windows-impersonation-token-sys/COMPLETED-CHECKLIST.md#it-inf1)).

**What was found when it was closed.** Both of its sub-items had landed, and the second's wording had become wrong.
`windows-ioring-sys` was done by #119, as the item says. `durable-ioring` is built on the type in a different place than the
item expects: its design first named `SharedHandle` as the contract's file type ([DI-D-29](../durable-ioring/DESIGN-NOTES.md#di-d-29)),
and the target design ([DI-D-50](../durable-ioring/DESIGN-NOTES.md#di-d-50)) then made the portable core name a target by
`IoTargetKey`, so that no handle crosses the contract and the core has no Windows dependency. The type is held by the Windows
driver, `win-durable-ioring`: it depends on this crate, re-exports `SharedHandle`, and its `OpenedFile` is a `SharedHandle`
with the way it was opened recorded beside it. The portable crate depends on nothing here, by design.

**The sweep.** A search of every crate's code for `Arc<OwnedHandle>` finds it only inside this crate, and the crates that depend on
this one are the ring crate, the driver, and the crates the item names as outside it: the thread pool, the overlapped-I/O,
namespace-request, file-enumeration, impersonation-token, platform-probes and sync crates. The two the item records as parked
by the engineer on 2026-10-07 are done in their own checklists: `windows-namespace-request-sys` as `NR-6`
([D-32](../windows-namespace-request-sys/DESIGN-NOTES.md#d-32)) and `windows-impersonation-token-sys` as
[`IT-inf.1`](../windows-impersonation-token-sys/COMPLETED-CHECKLIST.md#it-inf1).

### <a id="sh-1+2"></a>SH-1+.2 -- Published to crates.io as 0.1.0, by the publish workflow on the `win-shared-os-owned-handle-v0.1.0` tag. *(completed 2026-10-07)*

The item as written:

**SH-1+.2** -- **Publish to crates.io.** Done: 0.1.0, by the publish workflow on the
  `win-shared-os-owned-handle-v0.1.0` tag, 2026-10-07.
