# Design notes: windows-ioring-sys (Tier 1)

This file is the crate's current design record: what was decided, and what forced each choice. It was
written before the implementation, and the design session it references is where its earliest decisions
came from.

It deliberately states no release or milestone status. That is derivative of
[CHANGELOG.md](CHANGELOG.md), the git tags, and [CHECKLIST.md](CHECKLIST.md), and a copy of it here would
be one more thing to keep true by hand.

## Intent

**Why this crate exists at all is recorded one level up**, in the workspace's
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) -> [The adoption thesis](../../DESIGN-NOTES.md#the-adoption-thesis).
Read it before proposing to remove a design option here: it is the reason this crate keeps
alternatives alive that no measurement on the development machine can justify, and the reason its
samples hand a consumer data rather than a verdict. The operational form of that posture is OPTION
INTEGRITY in [copilot-instructions.md](../../.github/copilot-instructions.md).

Windows 11 / Server 2022 added `IoRing`: a submission/completion ring for file I/O, closer in shape to
`io_uring` than to anything else Windows offers. This crate raises those primitives into memory-safe Rust
with the minimum additional CPU and memory cost, in the same spirit as the rest of this repository.

The goal is **not** to solve every consumer's I/O architecture for them. It is to provide a safe toolkit
they can build their own answer with, plus honest guidance on the patterns that actually matter -- how to
construct rings, how to give them the right affinities, and what the trade-offs are. Consumers of these
crates are trying to maximize I/O throughput; the information in "Two delivery architectures" below is
written for them, not only for this crate's maintainers.

Where a choice would impose a policy on the consumer (how many rings, how to partition them, which thread
runs a continuation), this crate exposes the mechanism and documents the trade-off rather than picking.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **IoRing lives in its own crate, not as a third backend inside `windows-overlapped-io-sys`.** Duplicate-then-decide, per the repository's PLATFORM INTEGRITY rule: the ring path is speculative, and building it beside the working IOCP path keeps that path stable. The genuinely shared surface turned out to be small (see D-2), which strengthens rather than weakens the separation. `IoBuf`/`IoBufMut` are duplicated initially; the extract-or-share decision is deferred until the ring path is proven, and tracked as M6+ rather than left implicit. |
| <a id="d-2"></a>D-2 | **IoRing is a file data plane, not a general completion backend, and the division is forced by the kernel rather than chosen by us.** The op table is fixed: `NOP`, `READ`, `WRITE`, `FLUSH`, `REGISTER_FILES`, `REGISTER_BUFFERS`, `CANCEL`. Verified by spike against `IsIoRingOpSupported` on a fully current machine (`MaxVersion` 400). There is no ioctl op, no socket op, and no directory-change op, and unlike Linux's `io_uring` -- which grew to roughly fifty opcodes including full socket support -- Windows IoRing has not grown beyond file I/O. So `windows-overlapped-io-sys` remains the crate for arbitrary I/O (any handle, any operation), and this crate covers a strict subset of one of its three families. Neither can subsume the other. |
| <a id="d-3"></a>D-3 | **There are two delivery architectures, both first-class; neither is a degraded form of the other.** See the detail section below. This supersedes an earlier framing in which the thread-pool path was "primary" and the pinned-thread path was a "fallback" for a missing capability. That framing was wrong twice over: the pinned-thread path is the high-performance architecture, and the capability fallback is only its least interesting justification. **Amended by [D-20](#d-20):** the two architectures remain the two, but this decision was read -- reasonably -- as also fixing each one's *wakeup mechanism*, which coupled reaching the completion event to surrendering the ring. Model B's wakeup source is separable from Model B's identity; see D-20. |
| <a id="d-4"></a>D-4 | **Completion allocates nothing, and the caller supplies the type.** **No slab entry, no box, no type erasure** -- the caller already knows what the buffer is, so making it say so is free. That is the load-bearing half, it is unchanged, and [D-55](#d-55) and [D-73](#d-73) both rest on it. **What moved is who holds the buffer.** As written, `push()` returned a `Token<B>` owning the `B` it was given, and the ring stored only a generation counter and an in-flight count for rundown; dropping a token whose operation was still in flight `mem::forget`ed the buffer, because leaking is safe and use-after-free is not -- the same leak-and-reclaim discipline `windows-overlapped-io-sys`'s `Operation` uses, with the leak as the failure mode rather than the normal path. `M28` moved the buffer into the ring's inventory (`D-73`), so a push hands it over and the pop that observes its completion hands it back. **The forget mechanism did not disappear; it became the crate's** -- [`Drop for IoRing`](src/ring.rs) forgets the inventory rather than dropping it when rundown fails, for exactly the reason a dropped token used to forget its value. The ergonomic, allocating variant is still layered on top of this, never underneath it. |
| <a id="d-5"></a>D-5 | **The submission queue is ring state, not batch state, so buffers are owned from `Build*` and not from `Submit`.** Once `BuildIoRingReadFile` returns, the SQE is queued and there is no rewind. If a batch could be abandoned and its buffers freed, a later unrelated `submit()` would hand the kernel freed memory. A `Batch` therefore submits on drop, and holds `&mut IoRing` so that two concurrent batches do not compile -- which turns Win32's "you must serialize submission" footnote into a compiler-enforced guarantee. |
| <a id="d-6"></a>D-6 | **Capability is negotiated and cached, never assumed.** The ring version is `min(highest we understand, caps.MaxVersion)`, stored and exposed, because the spike found an OS reporting `MaxVersion = 400` while `windows-sys` 0.61.2 names only up to `IORING_VERSION_3 = 300`; hardcoding a version would cap us permanently. `IsIoRingOpSupported` is probed once per op at construction into a capability set, so per-call cost is a bit test. `QueryIoRingCapabilities` needs no ring at all, so capability inspection is free and side-effect-free. |
| <a id="d-7"></a>D-7 | **The op set will grow, and the API is shaped so that growth is additive.** The public op enum is `#[non_exhaustive]` so a consumer cannot write an exhaustive `match` that a new op would break; new ops arrive as new builder methods; `supports_raw(op_code)` answers for ops the OS has but this crate has not wrapped; and a narrow unsafe raw-SQE seam lets a consumer use such an op before we wrap it -- the same shape, and the same justification, as the `device` family's unsafe arbitrary-control-code `ioctl` in `windows-overlapped-io-sys`. Honest limit: this covers new ops that reuse existing parameter types. An op needing genuinely new structs still requires a `windows-sys` bump, and no API shape avoids that. |
| <a id="d-8"></a>D-8 | **Locality is the consumer's decision. This crate makes a ring cheap and correct, makes its affinity explicit, and documents the trade-offs -- it does not partition anything.** Baking "one ring per NUMA node" into the layer would be policy in a primitive, and would also be wrong: see "Why the NUMA node is the wrong key" below. A `RingFleet`-style abstraction may come later, once there is evidence about what sharding actually helps; it is deliberately not in the initial plan. |
| <a id="d-9"></a>D-9 | **An IoRing cannot feed an I/O completion port, and no amount of userspace bridging recovers what is lost.** The only completion hook in the entire API is `SetIoRingCompletionEvent`, which takes an event; there is no port variant, and the CQ is a userspace ring the consumer pops. More decisively: the device-to-CPU association is lost inside the kernel, before any userspace code runs. By the time a wait callback could call `PostQueuedCompletionStatus`, the packet enters the port from an already-arbitrary processor, so the port routes on where the post came from rather than where the device completed. A bridge is therefore not merely two kernel transitions for nothing -- it is structurally incapable of delivering the associativity that would motivate it. No such bridge is provided, and no example demonstrates one. |
| <a id="d-10"></a>D-10 | **Recorded as an explicitly unverified assumption: we do not believe IOCP performs NUMA-local completion dispatch either.** Its documented and relied-upon property is LIFO thread wakeup for cache warmth, which is a different thing. The indirect evidence is that the standard high-performance IOCP pattern is one port per node with threads explicitly affinitized -- which nobody would build by hand if the kernel did it for them. This is belief, not measurement: settling it needs a multi-node machine, a device whose interrupts are affinitized to a known node, and instrumentation correlating the completing node with the callback's processor. It is recorded rather than resolved because **the design consequence is the same either way** -- a consumer who needs guaranteed locality affinitizes their own threads (Model B below). No work is scheduled against this decision. |
| <a id="d-11"></a>D-11 | **`IoBuf`/`IoBufMut`'s safety contract is extended, at duplication time, to also cover a buffer registered for many operations, not only one in flight.** `windows-overlapped-io-sys`'s original contract only had to hold for the lifetime of a single operation, because that crate has no registration concept. This crate's M5 (`RegisterIoRingBuffers`) will hand the kernel a buffer's address for the life of the *registration*, which can span many submissions. Rather than silently reinterpreting the inherited contract when M5 lands, M2.1 states the wider requirement up front in `buf.rs`'s doc comments: a `stable_ptr`/`stable_mut_ptr` implementation must not move for as long as *any* outstanding use exists, whether that use is one `Token` or a standing registration. This is D-1's "duplicate-then-decide" playing out concretely: the duplicate is not a frozen copy, it is free to diverge the moment this crate's actual needs diverge from the original's. |
| <a id="d-12"></a>D-12 | **Completion retrieval is one primitive, `IoRing::try_pop`, used by both delivery architectures rather than each growing its own.** It pops one `Completion` (an identity plus a `Result` over `ResultCode`/`Information`) without blocking, added during M3 once M3.6's own tests showed nothing exposed a *typed* completion outside the untyped rundown drain (a re-plan, not an omission -- see M3.7 in `CHECKLIST.md`). Model B's pinned thread calls it in a loop after `submit_and_wait`; Model A's event callback (M4) will call it in the same drain-to-empty pattern. Neither needs its own popping logic. The matching raw-SQE seam, `IoRing::push_raw` (M3.5), follows the same shape as `windows-overlapped-io-sys`'s unsafe `ioctl`: the mechanics of building an SQE need nothing unsafe, but this crate cannot audit an arbitrary caller-supplied `Build*` call, so the seam itself is `unsafe`, and a failed `push_raw`/`Batch` push releases its reservation immediately rather than waiting for a rundown to notice an operation that never queued. |
| <a id="d-13"></a>D-13 | **`EventDelivery`'s quiesce-then-close teardown (M4.3) is Rust's own struct-field-drop order, not a hand-written `Drop` impl.** Its `wait: ThreadpoolWait` field is declared before its `ring: Arc<Mutex<IoRing>>` field; fields drop top-to-bottom, so `ThreadpoolWait`'s own `Drop` (disarm, suppress re-arming, drain any in-flight callback, close, then free its context -- releasing its captured `Arc` clone) always finishes before `ring`'s last strong reference drops and runs `IoRing`'s own `run_down` then `CloseIoRing`. No callback can be touching the ring when it closes, and no new `Drop` logic had to be written to guarantee it. This is also why `EventDelivery` cannot be placed in a `CleanupGroup`: a group only knows how to bulk-release objects it created itself, and `EventDelivery` owns a ring with its own teardown obligation a group's `CloseThreadpoolCleanupGroupMembers` has no way to run -- the same reasoning `windows-threadpool-sys` already applies to exclude `ThreadpoolIo`. |
| <a id="d-14"></a>D-14 | **Dissolved by [D-31](#d-31) (M10.3) -- the assumption below is no longer load-bearing, because the failure mode it reasons about became unreachable.** Retained as written for the record. Originally: **recorded as an explicitly unverified assumption (mirroring D-10): registration bookkeeping (`IoRing::registered_file_count`/`registered_buffer_count`) advances the instant a `BuildIoRingRegisterFileHandles`/`BuildIoRingRegisterBuffers` call successfully queues, not once its completion is observed.** Neither function takes an `IORING_SQE_FLAGS` parameter, so this crate cannot force a drain barrier around them the way `Batch`'s other pushes can. Whether the kernel actually claims the assigned indices synchronously at build time or only when the op later runs is not documented anywhere this crate could verify. Advancing eagerly is the safe direction regardless: it can only ever waste indices (skip ahead too far), never collide two registrations onto the same index (the only failure mode that would actually corrupt a later registration's base index). Like D-10, the design consequence -- eager, monotonic advancement -- is the same whichever way the truth turns out. (That last clause is what [D-31](#d-31) makes decisive: a second registration was forbidden the day after this was written, so there is no "later registration" left to corrupt.) |
| <a id="d-15"></a>D-15 | **`Token<B: IoBuf>` was generalized to `Token<T: Send + 'static>` to build M5's registration types on the exact same forget-unless-claimed mechanism, rather than a parallel one.** Nothing inside `Token` ever called an `IoBuf` method; the bound only ever documented intent. `Batch::register_files`/`register_buffers` return plain data (`PendingFileRegistration`/`PendingBufferRegistration<B>`) with their own `claim_if`, because there is no buffer to forget-or-free for a registration *push* itself. `Batch::read_registered`/`write_registered` reuse `Token<RegisteredUse>` for the *use* of an already-registered buffer: `RegisteredUse`'s own `Drop` decrements `RegisteredBuffers`'s outstanding-use count, so it fires only when a completion is actually observed and claimed (D-4's rule -- an unclaimed, dropped token forgets its value) -- never merely because a caller gave up on the token. `RegisteredBuffers` itself extends the same "leak is safe, use-after-free is not" philosophy one level up: since Win32's `IoRing` has no unregister call at all, `RegisteredBuffers::drop` refuses to free its `ManuallyDrop`-held buffers while that count is nonzero (loud via `debug_assert!` in debug builds, a silent permanent leak in release) rather than freeing memory a still-outstanding `IORING_BUFFER_REF` might address. |
| <a id="d-16"></a>D-16 | **`FileRef::Raw(HANDLE)`'s lifetime hole (PR #20 review finding, M8) is closed by making the raw-handle-taking pushes `unsafe fn`, paired with a safe, `Arc<OwnedHandle>`-backed `SharedFile` wrapper for the common case -- not by forcing every raw handle through an owning wrapper the way `windows-overlapped-io-sys`'s endpoints do.** A bare `HANDLE` carries no lifetime, so nothing stopped a caller from closing or reusing it before the kernel finished with it; the existing `SAFETY` comments already said "the caller's to keep alive" on functions with no `unsafe` keyword, the textbook shape of an unsound safe API. `windows-overlapped-io-sys` never has this hazard because every endpoint owns its handle -- but forcing that shape here would eliminate `FileRef::Raw`'s reason to exist: zero-setup addressing for a handle used across many concurrent pushes, which an owning-endpoint model cannot express without wrapping every file first. `SharedFile` instead shares by reference count: each `*_shared` push clones the `Arc` into the same `Token` that already tracks the operation's own payload (bundled as a tuple with the buffer for `read`/`write`-shaped pushes, or as `Token<SharedFile>` alone for `flush`/`cancel`, which have no buffer of their own), so the underlying handle survives until that token is claimed or leaked regardless of what the caller does with its own clone -- the same discipline `Token`/`RegisteredBuffers` already apply, adapted for a resource with multiple simultaneous holders instead of one. `register_files` gets no `_shared` counterpart: its handles must stay valid for the ring's remaining life, a lifetime no single push's `Token` can express. |
| <a id="d-17"></a>D-17 | **Every `Token`, `RegisteredFile`, and `RegisteredBuffers` now carries the identity of the ring that minted it (PR #20 review finding), and every popped `Completion` carries the identity of the ring that produced it, so a value from one ring can never be mistaken for one from another.** `UserData` is a plain counter this crate assigns starting at zero per ring, so two different rings routinely hand out the same value -- a `Token`'s `id == completion.user_data()` check alone cannot tell those apart, and a `RegisteredFile`/`RegisteredBuffers` index is only meaningful against the specific table it was assigned in. `RingId` is a monotonic, process-lifetime-unique counter (`AtomicU64`, starting at 1) rather than the ring's own `HANDLE`: Windows is free to hand a closed ring's numeric handle value to the next object it creates, which would let a stale identity collide with a genuinely new ring. `Token::claim_if` now requires both identities to match; `Batch`'s pushes reject a `FileRef::Registered`/`RegisteredBuffers` argument whose `RingId` differs from `self.ring`'s own with an `InvalidInput` error, checked before any `Build*` call runs (so a rejected push never reserves `UserData` or counts against rundown). |
| <a id="d-18"></a>D-18 | **`PendingBufferRegistration` now leaks its buffers on an unclaimed drop instead of freeing them, mirroring `Token`/`RegisteredBuffers` (PR #20 review finding).** `Batch::register_buffers` queues `BuildIoRingRegisterBuffers` -- and hands the buffer addresses to the kernel -- the instant it returns, before any completion is ever observed; a caller that drops the returned `PendingBufferRegistration` without matching a completion to it (via `claim_if`) has no proof the kernel is done deciding whether to retain those addresses. Freeing them anyway would risk handing memory the kernel still references back to the allocator, so `buffers` moved behind a `ManuallyDrop` and `PendingBufferRegistration`'s own `Drop` is now deliberately empty, exactly like `Token`'s. `claim_if` explicitly takes the buffers back out of the `ManuallyDrop` once a *matching* completion proves the kernel has decided one way or the other -- success or failure -- so the previously-documented "dropped normally on a failed registration" behavior is unchanged; only the never-observed-a-completion case changed, from an unsound free to a safe leak. |
| <a id="d-19"></a>D-19 | **The ring's completion event is edge-triggered on the completion queue going empty -> non-empty, not level-triggered and not one signal per completion. Measured, not inferred.** See "The completion event is an edge, not a level" below for the measurements and the two rules that follow (drain to empty before waiting again; a wake with nothing to pop is normal). This was found by spike during the M11 exchange, and it is not inferable from the Win32 API surface: `SetIoRingCompletionEvent` takes an event and documents nothing about when it fires. The consequence is severe rather than cosmetic -- a waiter that waits again without draining to empty blocks until some *later* completion arrives after the queue has been emptied, which is a lost-wakeup deadlock, not a latency wobble. |
| <a id="d-20"></a>D-20 | **Fully implemented: `IoRing::completion_event` in M11.1, `EventDelivery`'s consolidation onto it in M11.3.** **The completion event is reachable without surrendering the ring, via `IoRing::completion_event() -> io::Result<OwnedHandle>`: the ring creates and owns the event, and hands the caller a duplicate.** This opens the shape D-3 accidentally closed off -- a caller that owns its ring *and* can wait on the ring alongside other handles (`WaitForMultipleObjects`), which is what any consumer mixing ring I/O with non-ring I/O needs, since `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` orders SQEs against SQEs only and cannot order across the two paths. This is not a third delivery architecture: it is **Model B with a multiplexed wakeup source**, changing only what the domain thread blocks on, never who owns, submits, or drains. Rejected: taking a caller-supplied `BorrowedHandle<'_>` (the borrow ends at return but the kernel retains the handle for the ring's life -- a use-after-free reachable from safe code), and promoting `raw_handle()` to `pub` (exports the handle for arbitrary use, moves the capability check out of the crate, and pushes `unsafe` onto every consumer -- while also forfeiting the D-19 protections below). The event is signalled once before the method returns, so a caller that had already submitted never misses the backlog; the cost is one spurious wakeup at setup, which the contract requires callers to tolerate anyway. `EventDelivery` is re-expressed on top of this rather than remaining the only route to it. |
| <a id="d-21"></a>D-21 | **Auto-reset, and exactly one waiter per ring.** Forced by D-19 rather than chosen: a manual-reset event would stay signalled after the drain and spin the waiter, and two threads waiting on one ring's event cannot be made correct, because the drain that restores the empty state -- and therefore re-arms the edge -- must run to empty exactly once. The consumer whose request prompted this confirmed a single waiter, serialized behind their own lock, and separately flagged that a future move to less serialization on their side would reopen the question. Recorded so that a later multi-waiter request is recognised as a genuine design change rather than a flag. No work is scheduled against that possibility. |
| <a id="d-22"></a>D-22 | **Implemented in M11.4.** **`windows-threadpool-sys` becomes an optional dependency behind a default-on `threadpool` feature.** `EventDelivery` is its only consumer, so a Model B consumer currently links a thread pool it never uses. Default-on keeps the change additive: no existing consumer is affected, and a caller opts out with `default-features = false`. Recorded with its rationale corrected: the requesting consumer argued a runtime "correctness-of-posture" cost, and that is false -- linking the crate creates no threads, since the Win32 default pool is a process-wide facility instantiated lazily on first use. The gate is justified on layering alone (a ring wrapper does not intrinsically depend on a thread pool), and its real cost is that CI must build and test both feature combinations or the `default-features = false` path rots silently. |
| <a id="d-23"></a>D-23 | **A flush is not a durability barrier unless it carries `IOSQE_FLAGS_DRAIN_PRECEDING_OPS`. Measured.** A flush pushed after a batch of writes, with no barrier flag, routinely completes while many of those writes are still outstanding -- observed at 17 of 32 and 23 of 32 writes finishing *after* the flush's own completion. So the natural spelling, "push the writes, then push a flush", silently does not make those writes durable. See "Durability on the ring" below. This is a property of the operation, not of any wrapper, and it is the single most dangerous undocumented fact this crate has found: the failure is invisible except after power loss. **Amended by M12.2's measurement:** which *direction* the reordering shows in is device-dependent -- a second machine showed 0 of 32 preceding writes finishing after the flush, while 11 of 32 writes queued after it finished first. The rule is unchanged and the trap is sharper: seeing your flush land last is incidental behavior of one stack, never evidence the barrier can be omitted. See "The two measured facts" below. |
| <a id="d-24"></a>D-24 | **Corrected 2026-09-06: `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` drains what precedes it and does NOT hold back what follows.** The flag is one-sided. An operation pushed *after* a drained flush **can and does complete before it** -- observed in roughly 4,500 trials at a rate between 0.03% and 0.8%, and in the worst case *all 32* post-flush writes completed ahead of the flush. The half that holds is the half that matters: across every one of those trials, **not once** did a write queued *before* the flush complete after it, so [D-23](#d-23)'s durability guarantee is intact and a consumer closing an epoch is served correctly. The barrier does still reach every outstanding operation on the ring rather than only the current submission batch, and it is still a real cost. What is withdrawn is the claim that it stalls *subsequent* work: see [D-47](#d-47) for the measurement, what was originally concluded and why it looked true, and what a consumer may rely on. |
| <a id="d-25"></a>D-25 | **Implemented: the flush barrier decision in M12.1 (`FlushCoverage`), the write flags in M12.3 (`WriteCaching`), the flush modes in M12.4 (`FlushMode`).** **Every durability parameter the kernel exposes is exposed by this crate, and the barrier decision is never taken by default.** `BuildIoRingWriteFile` takes `FILE_WRITE_FLAGS` and `BuildIoRingFlushFile` takes a `FILE_FLUSH_MODE`; this crate hardcoded `FILE_WRITE_FLAGS_NONE` and `FILE_FLUSH_DEFAULT`, so a consumer reading the API saw ordering but no way to express durability at all, and reasonably concluded the ring could not express it. That is a PLATFORM INTEGRITY failure -- the platform narrowed to what the crate's own examples needed -- and it is the second instance found in one review cycle, which makes it a pattern rather than an accident. Given [D-23](#d-23), `Batch::flush` additionally must not have a default spelling that produces a non-covering flush: the barrier decision is made explicit at the call site rather than inherited from `PushOptions::default()`. Queued as M12. |
| <a id="d-35"></a>D-35 | **A registered buffer's outstanding-operation count is per buffer, not per registration, so a caller can refill a quiet slot while its neighbours are in flight (M13.2).** Found by writing M13's worked example: `RegisteredBuffers` exposed `get` but no mutable accessor, so a registered arena could only carry bytes the *kernel* produced -- there was no way to put a caller-composed record into one, which is exactly what an arena-backed log does. `ring_copy` never hit it because it reads into a registered buffer and writes back out of the same one. The fix is `get_mut`, and the reason it needed more than an accessor is that **`&mut self` is not sufficient for safety**: an operation in flight holds no borrow (`write_registered` takes `&RegisteredBuffers` for the length of the call; the `Token` keeps only a `RegisteredUse`), so the borrow checker would allow mutating a buffer the kernel is reading through -- a data race with the kernel. `get_mut` therefore pairs `&mut self` with a runtime check of that buffer's own count, refusing with `WouldBlock` while it is busy and `InvalidInput` when the index does not exist. Per-registration counting would have been simpler and useless: it would refuse every slot whenever any slot was in flight, which is the normal state of a pipelined arena. Rejected: a narrow `unsafe fn get_mut_unchecked` seam, which would have been smaller but pushed the hazard onto every consumer of an arena, for a check that costs one relaxed load. **Corrected by code review: `get_mut` hands back `&mut [u8]`, not `&mut B`.** The first version returned the buffer type, which closed the *temporal* hole (no mutation while an operation is in flight) and left the *address-stability* hole wide open: `IoRing` has no unregister call, so the address and length given to `BuildIoRingRegisterBuffers` are live for the ring's remaining life, and `&mut Vec<u8>` lets entirely safe code `reserve`, `resize`, or assign a whole new vector -- each of which frees or moves that allocation, with no `unsafe` anywhere and at a moment when the outstanding count is legitimately zero, so no runtime check could catch it. A byte slice of exactly the length recorded at registration grants the one power a caller needs and none of the powers that break the registration. The same correction moved `checked_span` off the live `bytes_len()` and onto that recorded length, since the kernel's view is the registered one. The lesson worth carrying: a per-operation temporal check is not a substitute for a lifetime-of-the-registration structural one, and reasoning about "what can mutate this" missed "what can *replace* this". |
| <a id="d-39"></a>D-39 | **Non-fixed test data is permitted in this component only when it is seeded, announced, and pinnable -- and M15.2's poison is the first thing admitted under that rule.** The component's standing rule is that tests be reproducible and not use randomized sampling without explicit approval. A poison pattern that never varies would satisfy it trivially and be worth much less: a fixed byte like `0xDD` collides with real payloads, and code can come to depend on the constant without anyone noticing. The terms that reconcile the two: (1) the varying input is a single **seed**, not per-value randomness, so one number reproduces the entire run; (2) the seed is **announced** on stdout by `GuardAlloc::announce_seed` at test start, including the exact command to replay it; (3) it is **pinnable** from the environment (`WINDOWS_GUARD_ALLOC_SEED`, decimal or `0x` hex), verified end-to-end -- two unpinned runs produced different seeds, and two pinned runs reproduced byte-for-byte. A run with a pinned seed is exactly as deterministic as a constant; what varies is *which* deterministic pattern, which is the point. Two implementation constraints worth recording because they are not obvious: the seed must be readable **without allocating** (this code runs inside the global allocator, so `std::env::var` would recurse -- `GetEnvironmentVariableW` into a stack buffer is used instead, and `QueryPerformanceCounter` rather than `SystemTime` for the default), and the mixing function must be a **bijection with a computable inverse**, since identifying which allocation a region of poison came from is done by inverting it rather than by scanning. This rule governed the poison only; the separate question of whether randomized *property* testing is permitted has since been **settled by [D-41](#d-41)**, which admits it and generalises these same three terms to any generator. |
| <a id="d-37"></a>D-37 | **Heap instrumentation for this crate's tests is a guard-page global allocator in-process, not Application Verifier / PageHeap -- because IFEO is keyed by image file name and cargo rehashes test binaries.** Measured rather than assumed, since PageHeap was the obvious first answer. What was established: `gflags` is **not** preinstalled on GitHub's `windows-2022` runners (though the runners do run as administrator); `gflags /p /enable <exe> /full` writes exactly two `REG_SZ` values under `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\<exe>` -- `GlobalFlag = 0x02000000` and `PageHeapFlags = 0x3` -- so `reg add` alone is sufficient and the SDK dependency is avoidable (verified on a fresh image name with no `gflags` involvement: the use-after-free went from a silent stale read to `0xC0000005`). The blocker is not availability but **keying**: IFEO matches on image file name, and one test target produced six distinct hashed names (`registration-<hash>.exe`) during a single day's work, so CI would have to enumerate, register and unregister every built test binary on every job, degrading silently to testing nothing whenever the enumeration missed one. A guard-page `GlobalAlloc` -- `VirtualAlloc` per allocation with a trailing `PAGE_NOACCESS` guard page, right-aligned so the allocation abuts it, and the address never reused after free -- gets the same detection (measured: clean `0`, use-after-free `0xC0000005`, overrun `0xC0000005`, against a system-allocator baseline that silently read a freed byte and exited `0`) with no admin, no SDK, no registry and no cleanup, and is immune to renaming because it lives inside the binary. Two honest limits recorded so they are not rediscovered: it governs only allocations made through Rust's global allocator (sufficient here -- [D-32](#d-32)'s `IORING_BUFFER_INFO` array is a `Vec`), and never reusing addresses means free must `MEM_DECOMMIT` rather than merely `VirtualProtect`, or a long suite grows without bound. Also corrected while measuring: PageHeap's "immediate access violation" is true for *user-mode* access, but a **kernel** read of freed memory probes and returns `STATUS_ACCESS_VIOLATION`, surfacing as a completion error rather than a user-mode crash -- so the win over the status quo is determinism, not a louder failure. |
| <a id="d-38"></a>D-38 | **Guard pages cannot see the kernel writing into a live allocation, so registered buffers are poisoned with a tracked pattern and verified after every operation.** A guard page catches *access to memory that should not be touched at all*; it is structurally blind to the kernel writing into a committed, valid buffer -- which is exactly the shape of this crate's two unverified promises: `write_registered` says the kernel only **reads** the slot, and `read_registered` says it writes only within the declared `RegisteredSpan`. Both are asserted in prose and checked nowhere. The poison is **tracked** rather than a fixed constant like `0xDD`: derived from a per-run seed plus a per-allocation ordinal, so the bytes identify *which* allocation they came from instead of merely being "not real data", and so they cannot be confused with a real payload that happens to contain the constant. Verification points, chosen by which party is the suspect: after a registered *write* completes the slot must be byte-identical to what was submitted (the kernel may only read it); after a registered *read* completes every byte outside `[span.offset, span.offset + information)` must still be poison (which binds the kernel to the declared span **and** catches this crate's own `checked_span` or offset arithmetic being wrong); and at quiescence every never-written region still holds its pattern. The tracking is also what reconciles this with the component rule that tests be reproducible: the seed is logged at test start and accepted from the environment, so a failure replays exactly rather than being a one-off, which is the term under which non-fixed test data is permitted here. Credit where due: the gap and the tracked-poison remedy were the engineer's, raised as "if there's a gap" after the guard-page measurements came back clean -- a reminder that a technique passing its own demonstration says nothing about the cases it cannot express. |
| <a id="d-36"></a>D-36 | **`RegisteredBuffers::get` refuses while a *read* is outstanding into that buffer, and only a read -- the direction of the kernel's access is what decides, so the count is split in two.** Found by a security review of the M14 branch, which correctly scoped it out of its findings (the line predates the branch, `26af335`) and flagged it anyway. `get` returned `&B` with no check at all, which made a data race with the kernel reachable from entirely safe code: `read_registered` takes `&RegisteredBuffers` and leaves no borrow behind, so `read_registered(...); submit(); arena.get(i)` reads memory the kernel is concurrently filling, with no `unsafe` anywhere. Demonstrated rather than argued -- with the check removed the regression test reads back `[0xEE, ...]`, the file's own bytes, out of a buffer whose read was still in flight. The asymmetry with [`get_mut`](#d-35) is the substance of this decision and is deliberate: mutating races the kernel whichever way it is touching the buffer, so `get_mut` refuses on *any* outstanding operation; reading races only a kernel that is **writing into** the buffer, so `get` refuses only on an outstanding read. A caller may read the bytes of a write it has in flight, because the kernel is reading them too and two readers do not race. That required splitting `BufferState`'s single count into `outstanding` plus a `kernel_writes` subset, and threading a `KernelAccess` through `begin_use`'s four call sites -- named from the kernel's point of view precisely because "a read means the kernel writes" is the inversion that is easy to get backwards. Rejected: refusing on `outstanding` for both, which is three lines and sound, but denies a sound operation and so narrows the platform to make the fix easy -- the thing PLATFORM INTEGRITY forbids. Both directions are sabotage-verified, the over-strict variant included, so a later "simplification" back to one counter fails the test rather than passing it quietly. Breaking (`get` now returns `io::Result<&[u8]>` rather than `Option<&B>`), and folded into the 0.2.0 that [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47) and [#48](https://github.com/MikeGrier/windows-threadpool-sys/issues/48) already force -- deferring it would have cost a 0.3.0 for a three-line change. The lesson worth carrying, and it is [D-35](#d-35)'s twin: that decision asked "what can mutate this" and missed "what can *replace* this"; this one asked "is anything in flight" and missed "in flight in which direction". |
| <a id="d-26"></a>D-26 | **Windows mechanism belongs in this crate; durability policy belongs to the consumer; an example is how the knowledge crosses between them.** The dividing test is whether the answer depends on *how Windows behaves* or on *the consumer's workload and contract*. Ours: exposing the kernel's parameters, stating the measured contracts ([D-19](#d-19), [D-23](#d-23), [D-24](#d-24)), and making the footguns hard to hold. Theirs: epoch bookkeeping, which barrier strategy to pay for, how durability is reported, and how non-ring operations are sequenced against ring ones -- all of which depend on epoch sizes and latency targets, and so would be policy baked into a primitive ([D-8](#d-8) refuses exactly this). The residue is that a consumer must otherwise rediscover the same composition, so the transfer vehicle is a worked example (M13/M14), not a library: it demonstrates the pattern without this crate owning the policy. |
| <a id="d-27"></a>D-27 | **One ring per thread is userspace's proxy for one ring per CPU, and pinning is what makes the proxy real.** Kernels affine hot structures to *CPUs*, not threads, because per-CPU exclusion is free (disable preemption, or raise IRQL) and because interrupt context has no meaningful owning thread. The hardware agrees: NVMe queue pairs are per-CPU with their completion interrupt vector routed to that CPU. Userspace has no per-CPU primitive and cannot disable preemption, so the only durable ownership unit available is the thread -- which is why the SPDK/Seastar discipline pins one. This is the reason under guidance this crate already gives ([D-8](#d-8), and the L3-domain advice): an *unpinned* per-thread ring still gets the single-producer safety the SQ/CQ protocol requires, but none of the locality that motivated the structure, and the interesting count is therefore cores or LLC domains rather than threads. |
| <a id="d-28"></a>D-28 | **`IoRing::supports` answers what the kernel's op table contains, not what this crate's safe push surface reaches; and every legality check runs before anything is reserved (category 3 audit, M10.1).** Six of the seven named ops gate one or more `Batch` methods; `Op::Nop` gates none and is reachable only through `push_raw`, because a nop owns no buffer for a `Token` to hand back. Reading `supports` as "a push for this op will be accepted" is therefore wrong for exactly the op a consumer reaches for to wake a parked `submit_and_wait` (M6+.3). `supports_raw` is the same truth uncached rather than a different question -- it accepts named codes too and agrees with `supports` on them -- and it never widens the push surface, since an op outside `Op` has no builder regardless. Separately, the registration one-shot is enforced against the registered count rather than a flag, which makes the real rule "at most one registration that assigned an index" (a zero-length registration does not spend it) and makes a *failed* registration unretryable on that ring (the count advances at queue time, [D-14](#d-14)). |
| <a id="d-29"></a>D-29 | **Implemented in M10.4 via the sealed [`FileTarget`] trait; the resolution is [D-33](#d-33).** Originally: **`FileRef::Registered` must be reachable without `unsafe`, because a registered index carries no lifetime obligation for a caller to get wrong (category 1 audit, M10.2).** Every safe push method hardcodes `FileRef::Raw` internally and only the six `unsafe fn` `_raw` variants accept an `impl Into<FileRef>`, so today the *only* route to a registered file is an `unsafe` call. That inverts the safety argument [D-16](#d-16) built: `read_raw`'s own contract says a `FileRef::Registered` target "needs none of this", which means the `unsafe` obligation is vacuous for exactly that input -- and vacuous `unsafe` is worse than none, since it trains a caller to discharge safety contracts by rote. The index is minted by this crate, checked against the minting ring ([D-17](#d-17)), and names a table the ring itself owns; there is nothing left for the caller to keep alive. Queued as M10.4. Note this is a PLATFORM INTEGRITY instance rather than a mere ergonomic one: registration is a platform capability that the safe surface currently does not reach at all, so the safe API is narrower than the platform for no reason the design supports. |
| <a id="d-30"></a>D-30 | **Implemented in M10.5; the resolution is [D-34](#d-34).** Originally: **`io::Error::kind()` discriminates this crate's own rejections and never the kernel's, and that asymmetry is stated rather than papered over (category 9 audit, M10.2).** `check` wraps every failing `HRESULT` as `io::Error::other(IoRingError)`, so a kernel-reported failure always surfaces as `ErrorKind::Other` while this crate's own refusals carry `Unsupported`/`InvalidInput`/`AlreadyExists`. The `HRESULT` is not lost -- it survives behind `downcast_ref::<IoRingError>()` -- but the derived form a consumer naturally matches on is lossy, which is category 9 applied to an error type rather than a name. The alternative, mapping `IORING_E_*` onto `io::ErrorKind` variants, is refused: the kinds are not a faithful target (there is no "submission queue full" kind, and `WouldBlock` would misdescribe a condition that is not retryable without draining), so the mapping would trade an honest `Other` for a lossy guess. Instead the downcast recipe is documented on `IoRingError`, and a named predicate for the one condition consumers must branch on -- `IORING_E_SUBMISSION_QUEUE_FULL`, which the push rustdoc already names as the backpressure signal -- is queued as M10.5. |
| <a id="d-31"></a>D-31 | **[D-14](#d-14)'s unverified registration-index continuity assumption is dissolved rather than verified: the failure mode it reasoned about became unreachable a day after it was written, and what remains is a naming question this crate can answer entirely from its own code (M10.3).** D-14 justified advancing the registered count at queue time on the grounds that erring early "can only ever waste indices, never collide two registrations onto the same index". That collision requires a *second* registration, and the PR #20 review response later made a second registration of either kind impossible -- so `base_index` is now always zero, no later base index is ever computed from the count, and the kernel's actual claim timing has no observable consequence for index assignment. Measuring it would establish a fact with nothing downstream of it. What *is* observable, and is now stated on the public API rather than left to the accessor's name, is that `registered_file_count`/`registered_buffer_count` report a **reserved** count rather than a confirmed one: they advance when the `Build*` call queues, so they are already advanced before any completion is popped, and they stay advanced after a registration whose completion reported failure (which is why such a registration cannot be retried on that ring, [D-28](#d-28)). The `base_index` machinery is kept rather than folded to a constant, because it is the correct shape if the one-registration rule is ever relaxed; it is documented as currently always zero so its presence is not misread as evidence that multiple registrations work. |
| <a id="d-32"></a>D-32 | **`BuildIoRingRegisterBuffers` reads its `IORING_BUFFER_INFO` array when the registration op *runs*, not when the `Build*` call returns -- the opposite of `BuildIoRingRegisterFileHandles`, and measured rather than assumed.** This was a live use-after-free in shipped 0.1.2: `Batch::register_buffers` built the array in a local `Vec` and dropped it before `SubmitIoRing`, so the kernel read freed heap and the registration completed with `ERROR_NOACCESS` (`0x800703E6`). Because `register_buffers` is a **safe** `pub fn`, safe code could make the kernel dereference a dangling pointer -- a soundness hole, not merely a failing test. The spike (`.scratch/ioring-bufreg-spike`) crossed array-lifetime against buffer alignment and found alignment irrelevant in both directions (an align-1 `Vec<u8>` succeeds with the array alive; a page-aligned buffer still fails with it dropped), and separately established that the array may be released once `SubmitIoRing` returns, and that `BuildIoRingRegisterFileHandles` genuinely *does* read synchronously. **The array is therefore held by the `IoRing`, not by the `Batch` that built it**: a failed submit leaves the SQE queued as ring state ([D-5](#d-5)), so a later unrelated submit can be what finally runs it, after that batch is gone. A ring accepts at most one buffer registration, so the cost is one small allocation per ring. |
| <a id="d-33"></a>D-33 | **[D-29](#d-29) is resolved by making the safe pushes generic over a *sealed* `FileTarget` trait with an associated `Guard` type, rather than by adding a parallel family of `*_registered_file` methods (M10.4).** The two safe targets differ in exactly one respect -- what the operation's `Token` must hold until its completion is observed -- so that difference, and nothing else, becomes the associated type: `SharedFile::Guard = SharedFile` (a clone of its `Arc`, which is what keeps the raw handle open) and `RegisteredFile::Guard = RegisteredFile` (nothing needs keeping alive; the index is handed back for symmetry). This makes the change **non-breaking**: `read(&SharedFile, ..)` still resolves to `Token<(B, SharedFile)>` exactly as before, and every existing call site compiles untouched. The generic parameters are ordered `<B, F>` rather than `<F, B>` so an existing `read::<Vec<u8>>` turbofish still resolves. The alternative -- six new concrete methods -- was refused because the naming does not survive the combinatorics: `read_registered` already means *registered buffer*, so a registered-file sibling would need a name like `read_registered_file_registered_buffer` to stay unambiguous. **Sealing is load-bearing, not tidiness:** an outside implementation could return `FileRef::Raw(arbitrary_handle)` from `as_file_ref` with a guard that keeps nothing alive, reintroducing precisely the unsoundness D-16 closed -- so the trait is public to *name* in bounds, and closed to implement. |
| <a id="d-34"></a>D-34 | **[D-30](#d-30) is resolved with a complete `RingCondition` enum plus a sealed `IoRingErrorExt` on `io::Error`, and the `HRESULT` -> name mapping is defined once rather than twice (M10.5).** Three parts, each answering a different half of the problem D-30 named. (1) `RingCondition` is `#[non_exhaustive]` and covers **every** `IORING_E_*` this crate names, not only the ones a submission loop branches on -- narrowing it to the actionable three would be exactly the "narrow the platform to serve the visible goal" failure PLATFORM INTEGRITY forbids. (2) Predicates (`is_submission_queue_full`, `is_completion_queue_too_full`, `is_submit_in_progress`) exist only for the runtime-actionable conditions, because a predicate asserts that a branch exists; the rest stay reachable through `condition()`, so nothing is unreachable, merely unsugared. (3) `IoRingErrorExt` puts those answers on `io::Error` itself, which is what actually removes the hand-rolled `get_ref().downcast_ref::<IoRingError>()` from call sites -- the crate's own integration tests had two such helpers, and deleting them was the first use of the new API. `IoRingError::name` is now **derived** from `condition()` rather than matching the `HRESULT` a second time, per CONTRACT INTEGRITY's "prefer a derived fact to a restated one": the mapping had been a single `match` that a new condition could be added to while `name` silently kept the old answer. D-30's refusal to map onto `io::ErrorKind` stands unchanged. |
| <a id="d-40"></a>D-40 | **A read from a cached file completes synchronously inside `submit_and_wait`, so the handover tests reach the genuinely-in-flight state with unbuffered I/O and construct a mixed queue deterministically rather than racing for one.** M17.1 was planned as a race: submit without waiting, attach immediately, and sweep the attach across the window in which the kernel might still be working. Measured before being believed, that window does not exist for **buffered** reads -- 512 reads of 64 KiB, plus four smaller shapes, left the completion queue **full** at attach time in 80 of 80 attempts, with no partial split at any size. A sweep across it therefore samples one cell repeatedly and only looks like coverage; the first draft was in effect a slower restatement of `attaching_to_a_ring_whose_queue_is_already_non_empty_still_signals`, which is exactly what its sabotage revealed by failing on *attempt 0*. Two things follow, and the handover tests do both. **(1) The in-flight state is reachable, just not that way:** unbuffered (`FILE_FLAG_NO_BUFFERING`) reads are genuinely asynchronous, so `attaching_while_unbuffered_reads_are_still_in_flight_strands_nothing` attaches while real device reads are outstanding. It **verifies its own precondition** rather than assuming it -- counting what was already queued at the instant of attach and failing if no attempt caught a read in flight -- so it cannot silently decay into the already-queued case on faster hardware. That guard is itself calibrated: dropping only the `FILE_FLAG_NO_BUFFERING` flag makes it fail with exactly that message, which re-derives this decision's central measurement from the test suite. The buffer is the same over-allocate-and-offset shape [tests/flush_barrier.rs](tests/flush_barrier.rs) already uses, extended to `IoBufMut`, because a `Vec<u8>` cannot carry a sector-aligned allocation safely -- its layout alignment is 1, so a hand-aligned block would be freed under the wrong layout. **(2) The union of the two wakeup mechanisms is tested deterministically:** one attach serving a backlog queued *before* it (covered by [D-20](#d-20)'s deliberate setup signal) **and** a wave submitted *after* it into the still-non-empty queue (covered by [D-19](#d-19)'s edge plus the drain-to-empty rule), which is where a seam would strand work exactly as [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47) did. The mechanisms are calibrated separately and their sabotages produce **disjoint** failures, so no test is passing on another's behalf. |
| <a id="d-41"></a>D-41 | **Randomized property testing is permitted in this component, under two conditions: seed it whenever it can be seeded, and where non-determinism is inherent and outside our control, randomness is fair game.** This settles the question [D-39](#d-39) left open when it admitted seeded poison. **Condition 1 -- seed what can be seeded.** A randomized run must be reproducible from a single number, and varying that number is how permutations of whatever variance was in play get generated. That is [D-39](#d-39)'s rule generalised from the poison pattern to any generator, and its three terms carry over unchanged: **seeded** (one number reproduces the whole run, rather than per-value randomness), **announced** (the seed is printed with the command to replay it, since a seed nobody can learn after a CI failure is not reproducibility), and **pinnable** from the environment. **Condition 2 -- inherent non-determinism is fair game.** Multiprocessor scheduling, kernel completion order, and thread-pool dispatch are not ours to control, and a test that depends on them cannot be seeded. Such tests are legitimate; pretending otherwise would only produce a test that is deterministic in its inputs while remaining nondeterministic in the behaviour it actually observes. **The corollary that keeps condition 2 from becoming a loophole: inherent non-determinism excuses irreproducibility, never unverifiability.** A test that cannot be replayed must instead **prove it reached the state it claims to exercise**, because the failure mode of a timing-dependent test is not a crash but a silent decay into testing something easier. M17.1 is the worked example on both halves: the racing draft looked like coverage of the in-flight handover state while in fact sampling the already-queued one, and its replacement cannot make that mistake because it counts what was already queued at the instant of attach and fails if no attempt ever caught a read in flight -- a guard itself calibrated by dropping `FILE_FLAG_NO_BUFFERING` and watching it fire ([D-40](#d-40)). Two operational terms follow for property tests specifically: any failure a generator discovers becomes a **named, seed-free regression test** committed alongside the corpus, since a property test that finds a bug and then forgets it has bought one debugging session rather than a guarantee; and they are **integration** tests, because they cross the OS boundary and will exceed the one-second unit budget. |
| <a id="d-42"></a>D-42 | **A test that collects completions by polling `try_pop` cannot observe a lost wakeup, so any test exercising an attached completion event must wait for the signal it is owed -- and wait *before* draining, not after.** Measured, and the measurement is the reason M17.4 exists: with [D-20](#d-20)'s deliberate setup signal removed -- [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47) exactly as it shipped -- M17.3's generator reported **green**. It attached the event and sampled the right states, but drained with unconditional `try_pop`, which recovers every completion whether or not the ring ever signalled. The wakeup contract was simply not among the things it could observe. This is the sharpest available illustration that **sampling the right state is not the same as being sensitive to the defect that lives in it**, and it is why a generator's green result is worthless until a known-real defect has been shown to turn it red. The ordering matters as much as the waiting: a backlog queued *before* the attach is reachable only through the setup signal, so a drain-then-wait loop consumes that backlog by polling and hides the signal's absence, while wait-then-drain does not. With the fix, the generator catches #47 in 10 of 10 runs on fresh seeds, always within six sequences. Any future test that attaches a completion event inherits this rule; polling is legitimate only when no event is attached, where it is the whole consumer model rather than a hole in one. |
| <a id="d-43"></a>D-43 | **Fixed in M18.6: `EventDelivery` hands out a `RingScope` -- every read-only part of `IoRing` plus batch construction, and no `&mut IoRing` -- because any `&mut IoRing` permits whole-value assignment, which let safe code replace the ring and silently stop delivery.** The defect, for the record: `EventDelivery::ring` returned `&Mutex<IoRing>`. Found by [M18.1's borrow-surface audit](#borrow-surface-audit-m181) and **measured, not argued**: `*delivery.ring().lock().unwrap() = IoRing::new(64, 64)?` compiles, and a probe recorded one completion delivered before the swap and **none** after it, despite four further operations being submitted and completed on the replacement. The mechanism is that the pool's wait holds its own duplicate of the *original* ring's completion event ([D-20](#d-20)); replacing the ring drops that ring and attaches nothing to the new one, so the armed wait can never be signalled again. **This is [D-35](#d-35)'s shape at a different layer** -- there, `&mut Vec<u8>` permitted `reserve` and reassignment where only byte writes were intended, and the fix was to narrow the returned type to `&mut [u8]`. Here the returned type permits replacing the whole ring where only submitting work was intended. Note the trap in the obvious fixes: a `Deref`/`DerefMut` newtype does **not** close it, because `*guard = ...` works through `DerefMut` just as well, and neither does a `with_ring(\|ring: &mut IoRing\| ...)` closure, for the same reason. Closing it meant never letting a `&mut IoRing` escape -- `RingScope` hands out a `Batch` instead -- which changed all nine call sites including the `epoch_log` example. The refusal is now enforced by a `compile_fail` doctest, itself verified by adding a `DerefMut` impl and watching that doctest fail, which is also the empirical proof of the claim above that a `Deref` newtype would not have closed the hole. **Severity was silent correctness, not unsoundness.** No use-after-free is reachable: the old ring runs down normally, the wait's duplicate handle stays valid, and completions on the replacement are still claimable. Delivery simply stops, which is the failure mode hardest to notice. The existing rustdoc warns against calling `completion_event` on the shared ring but says nothing about replacing it. |
| <a id="d-44"></a>D-44 | **A spike against the real kernel is a budgeted, first-class technique for every new Win32 surface this crate wraps -- not something that happens after a test fails mysteriously.** The full argument is in [Testing strategy](#testing-strategy-m185); the decision is that the budget is allocated *before* the wrapper is written. Two of the eight defects behind M15-M18 exist because a Win32 contract was assumed rather than measured: the completion event is edge-triggered ([D-19](#d-19)) and `BuildIoRingRegisterBuffers` reads its array when the operation *runs* ([D-32](#d-32)). No oracle, generator, allocator or mutation run supplies that knowledge, because each of them checks code against **our** stated contract -- and in both cases our stated contract was the thing that was wrong. What they detect is a *consequence*, and only on a path some test already walks: the guard allocator does turn D-32 into a hard `STATUS_ACCESS_VIOLATION`, measured in M17.4's calibration, but that is the crash after the mistake, not the knowledge that would have prevented it. A spike is also the only technique here that can be run *before* there is code to test. Two obligations follow, both learned the hard way and recorded in [design-sessions/spikes/README.md](design-sessions/spikes/README.md): a spike must carry a **control case**, because the first two drain-ordering spikes could not discriminate and would have returned confidently wrong answers; and it must be **kept**, as a standalone single-file program depending only on `windows-sys`, so that what it measures stays the operating system's behaviour rather than ours. |
| <a id="d-45"></a>D-45 | **A borrow-returning method must be audited on two questions, not one: what the returned value *permits*, and how long the *borrow* lasts. `RegisteredBuffers::get` therefore takes `&mut self`.** [M18.1's audit](#borrow-surface-audit-m181) asked only the first, of all nineteen items, and the second is where [D-36](#d-36)'s fix was still open: `get` checked `kernel_writes` at the instant of the call but returned a slice living as long as the borrow, and `Batch::read_registered` takes the registration by **shared** reference -- so safe code could take the borrow while the buffer was quiet, then submit a read into that same buffer and keep reading. Measured before being believed: a probe watched the bytes change from `0x11` to `0xEE` through the live slice while a fresh `get(0)` at that same instant correctly refused with `WouldBlock`. The guard worked; the borrow outlived it. **`&mut self` costs nothing real**, because no caller needs to read a buffer during the window it is refused -- while a read is in flight the bytes are indeterminate and only become meaningful once the completion is observed, so earlier or later is always available. That is not merely an argument: all ~40 read sites in this crate's tests, examples and the epoch-log sample already read at a quiescent point, and converting them needed nothing but `mut` on a local. The concession D-36 deliberately kept (reading a buffer whose own *write* is in flight, where the kernel only reads) is given up with it, and is likewise unused. The arena pattern survives, because a [`Token`] holds a [`RegisteredUse`] rather than a borrow of the registration, so quiet neighbours stay readable while operations are outstanding. Enforced by a `compile_fail` doctest, itself verified by reverting the signature and watching it fail, and paired with a `no_run` doctest asserting the neighbour case still compiles so the guard cannot become over-constraining unnoticed. `get_mut` never had the defect: `&mut self` already conflicted with the shared borrow. |
| <a id="d-47"></a>D-47 | **`IOSQE_FLAGS_DRAIN_PRECEDING_OPS` is one-sided, and [D-24](#d-24)'s claim that it holds back subsequent operations is withdrawn. Measured over ~4,500 trials.** The drain half is solid: **not once** did an operation queued *before* a drained flush complete after it. The hold-back half is false: post-flush writes overtake the flush at 0.03%-0.8% depending on conditions, and in the worst observed trial *all 32* did. The rate is why this read as a flaky test for three days rather than as a contract defect -- at roughly one run in a thousand, it surfaces every few days in a full-workspace run and never in isolation. Contention raises the rate but is not required (it reproduces on an idle machine); ring depth does not move it (128, 256 and 512 were indistinguishable). Every violation was confined to a single drain of the completion queue, so it is the queue's own posting order rather than an artifact of sampling it twice. **What a consumer may rely on:** a drained flush's completion means everything outstanding when it was reached is durable. It does **not** mean later work has been held. See [D-47 in detail](#d-47-detail). |
| <a id="d-48"></a>D-48 | **A shipping ARM consumer laptop reports no L3 cache domains at all, and zero `Win32_NumaNode` instances. Measured, and it is an ordinary consumer shape rather than an exotic one.** A Snapdragon X2 Elite (X2E80100, Qualcomm Oryon), 12 cores and no SMT, reports L1 and L2 only: L2 forms two domains of six processors, agreeing with the two `Module` domains the same probe returned, and WMI reports an L3 size of zero. The capture is Measurement M-1 in [DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md](../../design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md). **This is the sibling of the zero-node observation these notes already carried** in [Why the NUMA node is the wrong key](#why-the-numa-node-is-the-wrong-key), and the pair is the point: on one machine the NUMA node is absent because a hypervisor did not present it, on the other the *cache level this crate's guidance names* is absent because the silicon has none. Neither machine is unusual. **What it falsifies is a justification, not the heuristic.** These notes say the last-level-cache domain "is meaningful on Intel and ARM too, where the NUMA node often is not"; on this part it is not meaningful, because it does not exist. That a cache domain beats the node is untouched. **The rule was restated and every restatement swept by `M20.1`/`SH-4.12` (2026-09-22)**, which also replaced the consumer that bound to the level number. Measuring that consumer while fixing it found a second shape this decision did not anticipate: an L3 that spans *every* processor above a real L2 partition, where a `level == 3` filter matches, does **not** degrade, and reports one whole-machine domain as a successful cache-aware partition -- see [Why the NUMA node is the wrong key](#why-the-numa-node-is-the-wrong-key). **Not decided here:** whether the two-cluster L2 structure this part does report is worth sharding on. |
| <a id="d-49"></a>D-49 | **The unit suite is not hermetic, and that is a defect rather than a property of wrapping Win32. 63 of 131 lib tests open a real kernel ring.** The repository's Quality rule already classifies this: it reserves integration tests for work that must cross "a real process, filesystem, network, device, **operating-system API**, or other external boundary", and `CreateIoRing` is an operating-system API. So those 63 are integration tests sitting in the unit-test location, and `cargo test --lib` does not mean what its name implies. **This was surfaced by a load-dependent failure and initially mis-diagnosed.** `M21.6` removed five wall-clock assertions from four lib tests, which made their *outcome* independent of load -- measured at 30x duration variation with zero outcome variation under 2x CPU saturation -- and that was reported as the fix. It was not: outcome-stable and hermetic are different properties, and those tests still open a kernel ring. **What is decided here is the defect and the classification, not the remedy.** Three remedies are costed in [DESIGN-SESSION-2026-09-21-hermetic-unit-tests.md](design-sessions/DESIGN-SESSION-2026-09-21-hermetic-unit-tests.md) and the choice between them is gated on one unresolved question: whether a fake whose assertions are *shared* with the kernel escapes the objection in [Two techniques deliberately rejected](#two-techniques-deliberately-rejected), which refuses a mock that would "manufacture evidence" a kernel-behaviour bug was absent. **`M24.1` settled that (2026-09-22) and the answer was that the instrument was wrong**: a shared suite is strong over what this crate specifies and blind to the platform's incidental behaviour, and an assertion about the latter is a frozen observation rather than a contract -- see [D-52](#d-52). The rejection stands with its scope sharpened; the technique that replaces it is the response-space resolver, scheduled as `M26`. **A bright line holds under every remedy:** a fake never answers a question about Windows. Edge-triggered delivery ([D-19](#d-19)), one waiter per ring ([D-21](#d-21)), flush coverage and drain ordering ([D-23](#d-23), [D-24](#d-24), [D-47](#d-47)), the registration array read at run time ([D-32](#d-32)), `ERROR_TIMEOUT` and `E_INVALIDARG` from `SubmitIoRing`, and inline completion on a synchronous handle all stay kernel-tested forever -- which is precisely the set of findings that produced this crate's defects, and the argument for drawing the line exactly there. **`M24` has since run, and the figures above are the ones it started from.** Measured after it: **41 of 151 lib tests open a ring, 110 do not**. The remainder is not movable without `M26.2`'s FFI seam -- `event_delivery` needs the thread pool, `ring`'s injected-failure cluster transforms a *real* completion by design, `batch` needs the handle, and several reach `#[cfg(test)] pub(crate)` helpers. A zero-check is therefore the wrong rung, and [D-53](#d-53) records what replaced it. |
| <a id="d-50"></a>D-50 | **The epoch-log sample places its registered arena on the NUMA node its own log file's volume reports, and says in the same breath that the placement cannot pay at this workload.** The arena was `vec![0_u8; SLOT_LEN]` -- heap, no alignment, no node -- while this crate's front page told every consumer that placing the registered pool near the device "is very likely the highest-leverage locality decision available". That silence read as an oversight. The node is asked of the log's own handle through `FSCTL_QUERY_VOLUME_NUMA_INFO`, which [What is not reachable](#what-is-not-reachable) already established as the documented mechanism; a volume that names no node yields no preference, and the log runs on. **What is deliberately not claimed is any benefit.** The arena is eight slots of four kilobytes and this workload is bound by a per-epoch device flush costing hundreds of microseconds -- `M22.1` measured that directly. So the sample demonstrates *how the decision is made and reported*, and `examples/ring_copy` remains where placement is put under a load that could show it. The report line is qualified by `GetNumaHighestNodeNumber` for the same reason: on a one-node machine "placed on node 0" is true and misleading, so the sample says the choice was never available. **This is a sample's local policy, not a retraction of [D-8](#d-8):** the library still maps no file to a node, because a volume may span devices and its node is not where a file's extents live. |
| <a id="d-51"></a>D-51 | **`NumaBuffer` moves from `examples/ring_copy` into the library, because recommending an allocation while making every caller write it is what produced the second copy.** This crate's front page names `VirtualAllocExNuma` on the device's node as the highest-leverage locality decision available, and then supplied nothing; the first consumer wrote the allocator in a sample, and `M22.3` was about to make a second. The type is a thin owned mapping implementing [`IoBuf`]/[`IoBufMut`], so it registers like any other buffer. **It decides no policy** -- which node is still the caller's answer, per [D-8](#d-8) -- and its own documentation records that `nndPreferred` is a preference, so a successful allocation is not evidence the pages landed there. The move surfaced a packaging defect that `cargo check --all-targets` cannot see: dev-dependency features are unified into that build, so the missing `Win32_System_Memory` on the *library* dependency only appears in a lib-only build or `cargo doc`. A consumer would have hit it on first compile. |
| <a id="d-52"></a>D-52 | **Test this crate against the *space* of responses the platform is permitted to give, not against one observation of what it gave. A fake that models what Windows does freezes one run's testimony; a seeded resolver over the permitted space has no belief to be wrong about.** `M24.1` set out to ask whether a fake with *shared* assertions escapes the mock rejection, and demonstrated that it does not: a fake built from this crate's own pre-`M21.6` belief passed the shared suite green, and the assertion that catches it could only be written after the kernel had already revealed the answer. The deeper finding is that an assertion about platform behaviour is a **frozen observation** rather than a contract -- "after submitting, the completion is already queued" gave *opposite answers on two handles of the same API*, and [write-pending-spike.rs](design-sessions/spikes/write-pending-spike.rs) reported one condition as 5/500 in one run and 271/500 minutes later. Freezing such an observation into a suite, then building a fake to satisfy it, leaves three artifacts agreeing -- which reads as corroboration but is one observation restated three times, the same failure [D-47](#d-47) already cost this crate once. **What replaces it:** the resolver decides from a seed which operations complete inside `SubmitIoRing` and which pend, and in what order completions are posted; the assertions are then about this crate's behaviour under that resolution. Non-reproducibility stops being a threat and becomes the expected case. **The permitted space must be wider than anything observed and must not be derived from observation**, which makes it a deliberate, reviewable specification of what we tolerate -- and it must state the constraints that *do* hold, or the tests demand code defending against impossible kernels. This is a sixth technique beside the five in [What none of them cover](#what-none-of-them-cover): it still cannot tell you the stated contract is wrong, but it detects brittleness to variation inside the space, which none of the five can. Reasoned, not yet measured, against `D-47` and `M21.6`; scheduled as `M26` in [CHECKLIST.md](CHECKLIST.md), whose calibration item exists because `D-41`'s corollary demands it. Session: [DESIGN-SESSION-2026-09-22-kernel-response-space.md](design-sessions/DESIGN-SESSION-2026-09-22-kernel-response-space.md). |
| <a id="d-53"></a>D-53 | **The rung guarding the hermetic lib suite is an inventory of *which* lib tests open a ring, not a zero-check and not a count.** `M24.5` originally assumed that after the extraction and the relocation "the lib tests should construct no ring at all", so a zero-check would do. That rule is false and cannot be made true by effort: `event_delivery` needs the thread pool, `ring`'s injected-failure cluster transforms a **real** completion on purpose (fabricating one is the unsoundness the seam exists to avoid), `batch` needs the handle for its `Build*` calls, and several tests reach `#[cfg(test)] pub(crate)` helpers that exist only inside the crate. A zero-check would fail on day one and could only be satisfied by deleting real coverage. **Per-test rather than per-file**, because two thirds of the remaining 41 live in `ring/tests.rs` and a file-level allow-list would let that file grow without limit -- which is where a new ring-opening test would most naturally land. **An inventory rather than a count**, because add-one-remove-one nets to zero and passes, and a bare number is derived data no reader can check. The mechanism is [check-borrow-surface.ps1](../../tools/check-borrow-surface.ps1)'s, deliberately: a committed list regenerated from source, failing when the two disagree, so an addition obliges the question *does this test need the kernel, or only a ring-shaped thing?* A removal is progress and needs only regeneration. **The guard's own bidirectional check found a defect in it**: a plain helper defined after the last test in a file was being swallowed into that test's body, reporting an innocent test as ring-opening -- the body now ends at a column-0 `}` rather than at the next attribute. Inventory: [RING-OPENING-LIB-TESTS.txt](RING-OPENING-LIB-TESTS.txt); script: [check-ring-tests.ps1](../../tools/check-ring-tests.ps1). |
| <a id="d-54"></a>D-54 | **This crate owns what *one* flush means. It owns nothing about durability *groups*, and that is a deferral rather than a gap.** The primitives are here because they are facts about a single operation: [`FlushCoverage`] (the barrier is ring-wide), [`FlushMode`] (which modes sync the device), [`WriteCaching`], and the scope distinction between them -- a barrier over the ring, a flush over one file. **Grouping is not here and is not coming here**: what a set of operations that commit together costs, whether two such sets contend, what a co-flush regime implies, and the `Epoch` concept itself. That layer is [C-3](../../design-sessions/DESIGN-SESSION-2026-08-30-numa-sharded-io-execution-domains.md)'s durability crate, queued as `M33+.5` in [CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md); today `Epoch` exists only in `examples/epoch_log`, and no grouping concept appears anywhere in `src/`. **The test for a proposal is whether it needs the concept of a set of operations that become durable together** -- if it does not, it may belong here and must be justified on its own merits; if it does, it is deferred. Recorded because the boundary was re-litigated three times in one day and because a conversational aside -- calling `M23.3` a "down-payment" on the durability crate -- left the impression that the layer had been folded into this one. It has not been, and building *toward* it from here is what this decision forbids. |
| <a id="d-55"></a>D-55 | **The pending-token inventory becomes the ring's, and `IoRing` becomes generic to hold it. The break is accepted.** This crate hands a caller a `Token` from one call and a `Completion` from another, and connects them with nothing -- its own rustdoc twice instructs a caller to "match it against a held `Token`". Every consumer with more than one outstanding tokened operation must therefore build an identity map, and the *correct* one encodes four rules a `HashMap` cannot express; two measured defects in this repository came from the obvious one. Offering a `Pending<T, X>` beside the ring closes the duplication but not the mechanism: nothing would force a minted token into it. A generic `IoRing<T>` owning the map does, because the consumer never holds a token to lose. **The type-erasure objection is withdrawn as false**: per-ring monomorphisation holds for every real consumer here, a closed `enum` serves the rest -- `tests/generated_sequences.rs` already carries eight token types on one ring that way -- and [D-4](#d-4) independently rules type erasure out ("no slab entry, no box, no type erasure"), so the objection contradicted a decision already on the books. Implementation and migration are `M28`. **Landed in `M28.4.1d.3`, and two things this row asserted need correcting.** First, the evidence -- "its own rustdoc twice instructs a caller to match it against a held `Token`" -- was slightly over-broad. One of those two sites was [`IoRing::push_raw`](src/ring.rs), and it still tells a caller to match by hand, *correctly*: a raw push creates no inventory entry, which is [D-75](#d-75)'s subject, so matching is genuinely the caller's job there. The defect was in the **tokened** path, not the raw seam; the raw one only looked the same. Second, `tests/generated_sequences.rs` no longer "carries eight token types on one ring" -- that enum was the strongest evidence the erasure objection was false, and `M28.4.1d.3` deleted it, because the ring holds all eight shapes behind the two `Option`s `D-73` describes. The argument it supported still stands; the artifact is gone, so a reader sent to look for it should know that before going. |
| <a id="d-56"></a>D-56 | **A benchmark that defers its await measures the deferral, not the operation -- and the number survived three rounds of correction because every round corrected the conclusion instead of the instrument.** `epoch_log`'s harness published a commit latency measured from pushing a flush to observing its completion. `M20.6` decomposed it and found **blocking p50 and p99 of 0 us for all three strategies**: the figure was entirely the interval in which the program went on appending, so a design that deferred further reported a worse commit while being no slower. Three separate rounds of work had already re-read the *conclusion* drawn from that number -- the harness's serialisation, the `UserData` collision, the per-record submit -- and none had asked whether the number measured what its name said. **The fix is structural: a commit's cost is now reported in parts** (`prepare`, `submit`, `blocking`) with `deferral` beside them and excluded from the total, so the two cannot be read as each other. `M25.5` then found the same defect one layer down in that fix -- the clock started after `HostSequenced`'s host round trip, making it look six times cheaper -- which is why `prepare` exists. **What generalises:** a measurement whose parts are not separately reported can be wrong in a way that no amount of re-reading its output will reveal, and "the conclusion still holds" is not evidence that the instrument does. See [measurements/2026-09-24-commit-decomposed/](measurements/2026-09-24-commit-decomposed/README.md). |
| <a id="d-57"></a>D-57 | **A flag whose requirements the caller's data layout cannot satisfy is not a flag change.** Making `epoch_log`'s commit separately observable needed `FILE_FLAG_NO_BUFFERING`, which constrains the transfer's buffer address, file offset **and length** -- and the log wrote variable-length records at packed offsets, satisfying none of them. The work was therefore a change to the log's **on-disk format** (`M25.1`: one record per sector-sized block, tail zeroed, in both writers) before a single flag could move (`M25.3`). [write-pending-spike.rs](design-sessions/spikes/write-pending-spike.rs) predicted exactly this from its conditions, and the prediction is the reusable part: when a configuration's preconditions reach into a caller's data layout, costing it as a flag underestimates it by the size of a format migration. The stride's price is write amplification, which the sample now measures and prints rather than describing, and the reason a real log pays it anyway is sector atomicity -- a record sharing a sector with its neighbour can be torn by that neighbour's write. |
| <a id="d-58"></a>D-58 | **A contract checker's failure vocabulary decides its interface, and `epoch_log`'s replay keeps a `&[u8]` for that reason rather than for simplicity.** The walk is strictly forward one block at a time and never looks back, so it has no need of the whole file -- and a real log is larger than memory, which makes reading the whole file the wrong reflex to teach at precisely the point a reader is learning to verify one. The slice is kept anyway because `replay` returns an `Outcome`, not a `Result`: every way it can end is a statement about the log -- verified, tolerated, or a `Violation`. A streaming reader introduces a third kind of ending, `io::Error`, into the one component whose entire job is to distinguish "the log broke its promise" from "the log kept it", and a signature returning both through one channel invites exactly the conflation the file exists to prevent -- an unreadable file reported as a missing durable record. **So the streaming version is a different interface, not a smaller allocation**, and the cost of declining it is stated where it is paid rather than hidden: 140 KiB for the log, 8 MiB per strategy in the harness. A consumer building a real verifier wants the other shape *and* wants the two failure kinds kept apart inside it. What was reducible without touching that interface was reduced: the cross-strategy comparison kept a whole reference log in memory for the length of the comparison and now keeps a digest, which halves the peak and loses nothing a reader had, since the assertion could already only say *that* two logs differed. |
| <a id="d-59"></a>D-59 | **The permitted kernel response space is specified in [RESPONSE-SPACE.md](RESPONSE-SPACE.md), as a statement of what this crate will tolerate rather than a record of what Windows did.** [D-52](#d-52) settled that testing against a *space* dissolves the mock objection; this is that space, written down. Eight clauses say what a resolver **may** do -- an operation may complete inside `SubmitIoRing` or pend (`RS-P-1`), completion order is unconstrained (`RS-P-2`), an operation may fail individually (`RS-P-3`), a wait may expire (`RS-P-4`) or return with nothing poppable (`RS-P-5`), a completion behind another need produce no signal (`RS-P-6`), a failed submit leaves operations queued for a later one (`RS-P-7`), and a successful transfer may report fewer bytes than requested (`RS-P-8`, added by `M26.10`). **Four say what it may not**, because a resolver free to violate everything makes this crate defend against a platform that does not exist: completions are conserved and identified (`RS-C-1`, `RS-C-2`), nothing completes before submission (`RS-C-3`), and **the drain half of `DRAIN_PRECEDING_OPS` holds (`RS-C-4`)**. That last is the call `M26.1` demanded rather than defaulted: [D-47](#d-47) measured roughly 4,500 trials without a single violation, the drain is what this crate's durability story rests on, and a resolver permitted to break it would make the primitive useless -- so a Windows that broke it is caught by the kernel tests instead, which is the division of labour they were repointed to in `M26.6` and is now enforced by a census rather than intended. The hold-back half stays unconstrained, since `D-47` withdrew it. **Every clause is tagged `Observed`, `Over-provision`, or `Decided`**, so a reader can tell a measurement from an extrapolation from a call, and the space is deliberately wider than anything observed -- deriving it from observation would close the trap `D-52` was opened to escape. Rates, partial transfers, failure-code sets and timing are listed as deliberately undecided so an omission cannot be mistaken for a choice. |
| <a id="d-60"></a>D-60 | **The kernel seam is module indirection, not a type parameter, because [D-55](#d-55) has already spent `IoRing`'s.** `M26.3`'s resolver has to be able to answer the crate's kernel calls, and the textbook shape for that is a generic `IoRing<K>` over a kernel trait. That shape is unavailable here: `D-55` commits `IoRing`'s type parameter to `M28`'s token inventory, so a kernel generic would publish `IoRing<T, K>` on a shipped crate -- a second public parameter whose only purpose is letting the crate test itself. **The eight submission-path calls therefore route through [sys.rs](src/sys.rs) instead** -- `SubmitIoRing`, `PopIoRingCompletion`, and the six `Build*` entry points -- each an `#[inline(always)]` wrapper whose `through_seam!` macro expands to the bare FFI call when the `kernel-seam` feature is off. The public surface is unchanged and the type parameter stays free. **The responder is thread-local rather than process-global** for the reason this workspace is not on nextest: `cargo test` runs tests as threads in one process, so a global responder would let one test answer another's calls -- the same hazard that puts `DROP_RUNS` inside its test function. `with()` falls through to the kernel on a re-entrant borrow rather than panicking, and `Installed::drop` uses `try_with` so teardown cannot abort ([M23.4](COMPLETED-CHECKLIST.md#m234)). **Four lifecycle calls are deliberately left direct** -- `CreateIoRing`, `CloseIoRing`, `GetIoRingInfo`, `IsIoRingOpSupported`. (This decision originally listed five; `M26.3` moved `SetIoRingCompletionEvent` behind the seam, because it is how a completion becomes *observable* and `RS-P-6` is therefore a clause about it -- see [D-61](#d-61). The correction is recorded here rather than only there, since a reader arriving at this row would otherwise take the superseded list as current.) `M26` justifies itself on the *response space*: what the kernel may answer to submitted work. Routing construction and teardown through the seam too would be hermeticity for its own sake, which is `M24`'s subject and not this one. The boundary is stated here so a later reader can tell it from an oversight -- and `M26.3` carries the converse, that a clause needing one of those five extends the seam rather than working around it. |
| <a id="d-61"></a>D-61 | **The resolver's permissions are configurable and its constraints are not, and that asymmetry is what keeps it a specification rather than a fake.** `M26.3` builds the resolver [RESPONSE-SPACE.md](RESPONSE-SPACE.md) was written for: it answers the seam's calls itself, choosing a point in that space from a seed. Every freedom cites the `RS-P-n` permitting it and every restriction cites the `RS-C-n` requiring it, so a clause no code cites is visibly unimplemented and a behaviour citing no clause is the resolver inventing a platform. `ResolverConfig` therefore has a switch per permission and **none for any constraint**: narrowing a freedom is how a test isolates another, while a knob relaxing a constraint would let a test quietly assert against a platform that cannot exist. The default is the **widest** point, so an unconsidered test fails loudly under a freedom it did not handle rather than passing while exercising nothing. **Where a permission and a constraint collide, the constraint wins** -- `RS-C-4` holds a drain-flagged operation back even when `RS-P-1` chose to complete it now, and `RS-C-1` forces a post an operation's coin kept deferring. **Two mechanisms exist only to make `RS-C-1` finite** (a per-operation deferral bound and a bound on consecutive declined submits); both are properties of the resolver rather than of the space, which carries no rates deliberately. **A submit resolves and a pop only rescues**, because a pop that flipped coins would make `RS-P-1`'s pending case unobservable to a polling consumer -- the freedom would be implemented and untestable. `SetIoRingCompletionEvent` moved behind the seam for this item: it is how a completion becomes *observable*, so `RS-P-6` is a clause about it, and a resolver unable to make that call does not satisfy the clause vacuously but hangs every [`EventDelivery`](src/event_delivery.rs) consumer instead. **The resolver's first contact with a real ring found a live defect**, queued as `M26.8`: a submit declined under `RS-P-7` propagates out of `IoRing::run_down` leaving work outstanding, which is `M21.6`'s hazard reachable through a different `HRESULT`. |
| <a id="d-62"></a>D-62 | **The properties that must hold under every resolution are checked by *asking* [`RingContract`](src/contract.rs), not by restating it -- and the same rule decides where the expected outstanding count comes from.** `M26.4` states five properties over the resolver `M26.3` built: conservation, no hang, `pop_within` honours its bound, `outstanding` is accurate, and no use-after-free. Only two needed anything new. Conservation is already this crate's own oracle, so the harness reports to it and asks it for the verdict, on the rule that the layer owning an invariant owns the oracle for it -- a copy in a harness is a second implementation, and when the two disagree it is the harness that gets "fixed". **The accurate-`outstanding` property follows the same rule rather than counting for itself**: the expected value is read back out of the contract through its own `Outstanding` violation, because a counter in the harness would be a third party to the disagreement. **"No hang" is a step budget**, since a resolver answers instantly and a hang here is therefore an unterminating loop rather than a block. **Two weaknesses are declared rather than papered over.** `pop_within`'s upper bound is nearly free under an ordinary resolution -- the resolver rarely makes it approach its deadline -- so the non-vacuous case uses a separate degenerate responder in which nothing completes during the window; that is not an `RS-C-1` violation, because no finite observation can distinguish "eventually" from "never", but the prefix of a satisfying resolution in which the eventually has not happened yet. And **no-use-after-free covers this crate's memory handling, not the kernel's**, since under a resolver nothing external ever writes into a buffer; the kernel-side half stays with [generated_sequences.rs](tests/generated_sequences.rs) against a real ring. **Coverage counters are asserted, not printed**: all five properties are satisfied trivially by a run that does nothing, so a vacuity guard is what separates "the properties held" from "nothing reached the states they are about". |
| <a id="d-63"></a>D-63 | **The resolver is calibrated against two defects that really happened, and the two suites' sensitivities were measured rather than assumed -- they differ, and the difference is the reason the calibration is its own file.** [D-41](#d-41)'s corollary is the rule: a green result from an instrument nobody has shown can go red is not evidence, and this repository has already produced one instrument of exactly that shape -- `M17.4` reverted issue #47 as it shipped and the generated suite reported **green**, because it sampled the right state while draining by a poll that recovers completions whether the ring signalled or not. **The two defects are calibrated differently because they live in different places.** `M21.6`'s -- an expired wait treated as a failure -- was in this crate, so it is re-injected by [sabotage.json](sabotage.json) and swept; what [calibration.rs](tests/calibration.rs) adds is the precondition that makes that sabotage mean anything, namely that `RS-P-4` reaches `pop_within` at all. [D-47](#d-47)'s is in a **consumer**, so there is nothing to mutate and the defective consumer is written out: a believer in the hold-back `D-24` claimed and `D-47` withdrew, which the resolver must break. **Measured, and the two suites are not equivalent.** `M21.6`'s defect is caught by both the calibration and `M26.4`'s property suite -- the latter only because that suite distinguishes a declined submit from a genuine error, which makes the detection deliberate rather than lucky. A *narrowed* resolver -- one enforcing the hold-back -- is caught by the calibration alone, and the property suite correctly stays green, because a narrower resolution is still a valid one and conservation still holds under it. **That is why a calibration file exists rather than a calibration assertion inside the property suite**: only a test that demands sensitivity can detect an instrument going blind, and such a test fails when the *instrument* regresses rather than when the crate does. |
| <a id="d-64"></a>D-64 | **The standard seeded-sweep size is 2048, and a coverage threshold must be stated against the space a test can reach rather than against the sweep size.** The sweeps were widened from 64 (and the property suite's 240 plans) for breadth, on the measurement that seed count is not what these suites cost: a seed is tens of microseconds -- a whole ring, a batch, a drain and a rundown -- so the resolver's 22 unit tests sweep 2048 seeds each inside a lib suite that runs in well under a tenth of a second, far inside this repository's sub-second budget for a submodule. Only [properties_under_every_resolution.rs](tests/properties_under_every_resolution.rs) moved materially, to a little over a second, and even there roughly an eighth of the original cost was a deliberate sleep in the bound test rather than the plans. **The trap the widening exposed is the part worth keeping.** `different_seeds_reach_different_resolutions` asserted that distinct completion orders exceeded *half the seed count*, which is satisfiable only while the seeds are fewer than the outcomes: six operations admit `6! = 720` orders, so that threshold becomes arithmetically impossible past 1440 seeds and the test would have failed with nothing regressed. Thresholds of that kind are now phrased against the achievable space. **Saturation was measured rather than assumed**, because the obvious response -- cap the sweep where it stops gaining -- turned out not to apply: over the resolver's own mixer, 1024 seeds reach 539 of the 720 orders and 2048 reach 670, so the sweep is still gaining breadth at its current size and only around 8192 exhausts the space. **The property suite's vacuity guards are fractions of the plan count** for the same reason in reverse: an absolute floor chosen for 240 plans is a twelvefold margin at 2048, which would let the suite lose most of its work without complaint. Each is set near half the minimum observed over repeated runs, since that file's seeds are clock-derived and its counts therefore vary; the fixed-seed suites are deterministic and need no such margin. |
| <a id="d-65"></a>D-65 | **The kernel tests' job is confirming a real Windows stays *inside* the specified space, and that division of labour is enforced by a census that reads markers rather than mentions.** `M26` split one job in two: the resolver sweeps the permissions (`RS-P-n`), and tests against a real ring confirm the constraints (`RS-C-n`). The split is load-bearing for exactly one clause -- `RS-C-4`, which the resolver is forbidden to violate, so a Windows that broke the drain would be caught by nothing the resolver does. **A hole was found where that mattered most**: [flush_barrier.rs](tests/flush_barrier.rs) asserted the clause but sat behind an early return taken when its control could not discriminate, so on such a machine the constraint was untested on both sides at once. The fix separates two questions that one gate had been answering together -- *is the barrier doing work*, a comparative claim that genuinely needs the control, and *did the kernel stay inside `RS-C-4`*, a conformance question where skipping can only hide a violation. The conformance assertion now runs on every machine and only the comparative claim is withheld. **The census was green and useless on its first build, and trying to make it go red is what found that.** It searched each file for the clause ID anywhere in its text; removing `RS-C-4`'s check from the only test performing it did not turn it red, because a second file mentioned the clause only to *disclaim* it -- and under a substring search a disclaimer is indistinguishable from a claim. This is the same trap already recorded for a probe whose only mention of a tag was a comment. A claim is now a structured `CONFIRMS:` / `EXERCISES:` marker naming a clause and nothing else, verified to go red in three directions. **Two blind spots are declared rather than hidden**: a census over source proves a clause is *claimed*, never that the file's assertion still runs or still means anything; and dropping the covering flag does **not** fire the `RS-C-4` assertion on every machine, because a device stack that orders a flush behind its file's outstanding writes by itself produces no violation to see -- so that sabotage is a portable demonstration of nothing, and the assertion's conformance value (reporting a violation if one occurs) is separate from its sensitivity (proving it would notice). **The toolkit's "five techniques" framing becomes six**, and the sixth is different in kind: the other five check this crate against its own stated contract and cannot tell you that contract is wrong, whereas the resolver checks it against a written specification of what the platform may do, and the kernel tests check the platform against that same specification. |
| <a id="d-66"></a>D-66 | **The suite's frozen observations were one class, it was 31 assertions wide, and the restatement is guarded by a source census because no run objects to it.** `M26.7` audited the suite for assertions that pin the platform's incidental behaviour rather than this crate's contract -- the shape [D-52](#d-52) demonstrated, where one assertion gave *opposite answers on two handles of the same API*. **The census came from a command**, as the item required, and the answer was not the file anyone guessed: 31 sites across five kernel tests read `try_pop()` straight after `submit_and_wait` and unwrapped the `Option`, asserting the kernel had **already** queued the completion. `pop_within`'s own documentation denies that in this crate's words -- a submit-side wait's return "promises nothing about poppability" -- and [RESPONSE-SPACE.md](RESPONSE-SPACE.md) states it as `RS-P-5`. They passed for the reason [D-40](#d-40) measured: a buffered read completes inside the submit in 80 of 80 attempts, while an unbuffered one genuinely pends. **The restatement is the one `D-52` prescribed** -- ask for the completion within a bound this crate chooses, which holds on every handle -- and it also removed an unbounded `try_pop` spin found in the same sweep. **Nothing catches a regression by running**, which is the part worth keeping: reverting a site leaves its test green on any machine where the observation is true, so the guard is a census that refuses the shape at the source, plus a resolver-driven test that makes the pending case reachable on demand and shows `try_pop` failing where `pop_within` succeeds. **A second, narrower class was found and deliberately not settled**: five assertions require a complete transfer, which the space lists as *deliberately undecided* and `Completion::result` never promises. That is a specification gap the suite silently answers, queued as `M26.10` rather than decided here, since the space's own instruction is to decide it before a resolver relies on either answer. One of the five is already in the honest form -- [flush_barrier.rs](tests/flush_barrier.rs) checks the transfer as its own precondition and says so -- and is left alone. |
| <a id="d-67"></a>D-67 | **This crate does not implement retry policy. It supplies bounded primitives and reports what the platform documented, and the caller owns backoff, attempt counts and when to give up.** `M26.8` began as "what should `run_down` do when its submit is refused" and was settled by reading `SubmitIoRing`'s reference page rather than by measuring, which is the correction worth keeping: **no amount of measurement constitutes a contract.** The documentation gives a return-value row for `IORING_E_WAIT_TIMEOUT` -- *"All operations were submitted without error and the subsequent wait timed out"* -- and Remarks stating *"If this function returns an error other than IORING_E_WAIT_TIMEOUT, then all entries remain in the submission queue"*, plus that a per-entry failure arrives as a completion rather than as a submit failure. Three things follow. **`Batch::submit_and_wait` had `M21.6`'s defect** at the one site that sweep did not reach: `do_submit` passed a timed-out wait to `check`, reporting a fully successful submission as an error -- and because any *other* error means the entries are still queued, an `Err` was ambiguous between "your buffers are free" and "the kernel still owns them", which is [D-5](#d-5)'s hazard with the sign hidden. **[`IoRing::run_down`](src/ring.rs) was the only waiting API in this crate shaped wrongly**: it waited in 50 ms segments with no period to sit inside, which made "how long to keep trying" this crate's policy rather than its caller's. [`IoRing::run_down_within`](src/ring.rs) is the primitive -- the caller supplies the bound, `Ok(true)` means safe to drop, `Ok(false)` means call again -- and `run_down` is now that with an unbounded period, which is a choice a caller makes by calling it. Note the contrast that makes this precise: [`IoRing::pop_within`](src/ring.rs) also waits in segments and is *correct*, because its segments sit inside a deadline the caller supplied. **An error from rundown is not terminal and says so**: the entries remain queued, the ring is resumable, and the one thing a caller must not do is drop it. **`RS-P-7` and `RS-P-3` moved from inference to citation** -- `M26.1` had written `RS-P-7` as a consequence clause with no authority for submits failing at all, and `M26.3`'s resolver had read it as a permission anyway; the space now carries a `Documented` tag that outranks `Observed`, because a measurement describes one run of one build. |
| <a id="d-68"></a>D-68 | **A thread-pool wait must be armed before the event it watches is signalled, so the ring's completion event is attached unsignalled and the setup signal is raised after arming.** `M26.9`'s intermittent stall was settled the same way [D-67](#d-67) was -- by reading the reference page rather than by measuring. `SetThreadpoolWait`'s Remarks state *"You must re-register the event with the wait object before signaling it each time to trigger the wait callback."* [`IoRing::completion_event`](src/ring.rs) raises its deliberate setup signal as it attaches, and `EventDelivery::new` then built a `ThreadpoolWait` around the returned duplicate and armed it -- signal first, register second, which is the order the sentence forbids. **Three properties compounded to make a dropped signal permanent rather than late.** The event is auto-reset ([D-21](#d-21)), so a signal is consumed rather than left pending for a later arming to observe. It is edge-triggered on the completion queue going empty to non-empty ([D-19](#d-19)), so a ring whose queue is already non-empty is signalled by nothing else. And the setup signal exists precisely to serve the already-non-empty case, so it is the only wakeup such a ring will ever get. The fix separates the two steps: `attach_completion_event_unsignalled` attaches and reports whether the signal is still owed, `raise_setup_signal` raises it, and `EventDelivery::new` arms in between; `completion_event` is the two composed, unchanged, for a caller doing its own waiting. **The ordering was the defect, so [D-21](#d-21) stands.** A manual-reset event would also have survived the wrong order, by not consuming the signal, but that trades the arming rule for a reset the drain has to get right, and nothing measured here argued for reopening a decision whose own rationale is about the drain. **Measured before and after** on the reproducer recorded in [RESOLVED-TEST-FAILURES.md](RESOLVED-TEST-FAILURES.md): 5 failures in 600 and 2 in 600 for the two co-running triggers, against 0 in 3600 after. The rule itself is now stated where a caller meets it -- on `ThreadpoolWait::arm` and `WaitActivation::rearm` in [windows-threadpool-sys](../windows-threadpool-sys/src/wait.rs), on `IoRing::completion_event` with a worked remedy for a caller who holds the handle, and on `EventDelivery::new` -- because every other arming site in this workspace already had the order right, so what was missing was the statement rather than the practice. |
| <a id="d-69"></a>D-69 | **A completion may report fewer bytes than requested, and the permission is open because this crate does not constrain the handle type. A consumer that narrows its handle type earns a stronger guarantee, and states it in its own contract.** `M26.10` asked whether a short transfer is possible; the answer has two halves that belong to different layers, and collapsing them is what the question had been deferred over. **The general ring permits it** (`RS-P-8`). `WriteFile`'s Remarks state that *"when writing to a non-blocking, byte-mode pipe handle with insufficient buffer space, WriteFile returns TRUE with \*lpNumberOfBytesWritten < nNumberOfBytesToWrite"*; sockets report a short send against a full transmit buffer, and a communications handle with a write timeout can report a partial count. So the clause is `Documented` rather than `Observed`, which matters because nothing in this repository had measured it and `M26.7` had flagged five assertions quietly depending on the opposite. The reason the permission cannot be narrowed is not that files behave badly -- for an ordinary file on a local volume a successful completion is expected to carry the full length, and a full volume is `ERROR_DISK_FULL` rather than a short success -- but that **`IoRing` takes a handle and never asks what kind it is**. A pipe, a socket and a serial port are all handles, so a consumer of *this* crate has to read the count. **Continuation stays with the caller**, per [D-67](#d-67): the count is reported and nothing reissues the remainder, because how many times to retry and when to give up are policy. A short count may be zero, so a consumer that loops must tolerate making no progress. **The durability layer is the other half.** [examples/epoch_log](examples/epoch_log) makes guarantees -- epoch commits, flush barriers, FUA -- that are only meaningful on a real file on a real volume: a flush barrier means nothing on a socket, and a short write would break "the whole record landed" silently rather than loudly. It therefore has to *constrain* the handle types it accepts, and thereby earn the completeness its accounting already assumes, rather than inherit it from a ring that does not promise it. That constraint is not yet enforced; it is queued as `M26.11` rather than recorded here, because a decision is not a work queue. **The kernel tests were corrected in the honest direction** -- three sites that asserted a full count bare now say that completeness is a property of the temp file they opened, which is the form [flush_barrier.rs](tests/flush_barrier.rs) already used and `M26.7` had singled out as right. |
| <a id="d-70"></a>D-70 | **The epoch log states the capabilities it requires of a handle and does not check for them. A pre-flight check points the wrong way: it catches the failures that were already loud and misses every failure that is silent.** [D-69](#d-69) left the durability layer owing a handle constraint, and the obvious discharge was a gate -- `GetFileType`, then `GetDriveType` or `FileRemoteProtocolInfo` for the distinctions the first is too coarse to make. Working the gate out is what showed it was not worth having. **Sort the failures by how they present.** A console handle, a pipe, a socket or a closed handle is exactly what `GetFileType` names -- and every one of them already fails at the first positioned write, so the check buys a clearer message and nothing else. A RAM disk, a remote share, or a volume whose write cache is not power-protected **succeeds at every API call this log makes** and silently fails to be durable; `GetDriveType` can name the first two and nothing in Win32 settles the third from a handle, because write-cache state is a property of the device rather than of the handle. So the gate guards the loud cases and misses the silent ones, which is the inverse of what a durability layer needs. **The cost is not the call, it is the claim.** A check that cannot establish the property still reads to a later maintainer as though the property was established -- the "decoration that reads like enforcement" the repository's FAIL FAST rules name -- and that is worse than no check, because it discourages the reader from asking the question themselves. **So the requirement is documented and the caller warrants it.** This is [D-67](#d-67)'s shape applied to handles rather than to retries: we state what we need, the caller chooses what to hand us, and the platform enforces what it is able to. Two requirements are singled out in the contract: that a successful write of `N` bytes transfers `N`, which this log *can* check and does, and that a completed flush reaches stable media, which it cannot check and says so in those words. **Two alternatives were considered and declined**, both recorded so they are not re-proposed as new: refusing `FILE_TYPE_CHAR` and `FILE_TYPE_UNKNOWN` as a cheap early error, declined because it improves only the already-loud path; and a caller-declared mask of intended handle types, whose value would be the acknowledgment rather than the validation, declined as ceremony every ordinary caller pays for. The work of writing the contract is queued as `M26.11`, since a decision is not a work queue. |
| <a id="d-71"></a>D-71 | **A push returns a `Copy` identity that owns nothing, and the ring returns the payload at its own pop. `Token` is split rather than moved.** `M28.1` asked whether a caller receives *an identity it matches later* or *a claim returning `(T, X)` from the ring*, and the census answer is **both**, because today's `Token` is two things welded together. It is an **ownership guard** -- dropping it unclaimed is treated as still-outstanding and leaks, and claiming it requires a `Completion` -- and it is an **identity**, [`Token::id`](src/token.rs), which [`RingContract`](src/contract.rs) observes, consumers correlate on, and `Batch::cancel` targets. Only the ownership half is what [D-55](#d-55) exists to move. The identity half must survive: `cancel(file, target: usize)` makes the caller *name* an outstanding operation, so a consumer that cannot name one cannot cancel it. **So `Batch::write` returns an `OperationId` -- `Copy`, no `Drop`, and losing it is harmless -- while the payload comes back only from the ring's own pop.** **The safety argument is strengthened rather than merely preserved, which is the test `M28.1` said to settle this against.** [`Token::claim_if`](src/token.rs) takes a `&Completion` rather than a `usize` precisely because `Completion` has no public constructor: the only proof the kernel has finished with a buffer is one that was popped, and accepting a caller-supplied integer would let safe code free memory the kernel is still writing into. Under this shape **there is no claim call left to misuse** -- the payload is produced by the pop that observed the completion and by nothing else. The cross-ring hazard that PR #20 added a runtime ring-identity check for becomes structural too: a completion popped from another ring cannot name an entry in this ring's map. `OperationId` is therefore a **name, not a capability**, and must carry no power to retrieve a payload; handing one to `cancel` stays safe because a cancel returns no memory. **The sidecar `X` stays, on census evidence rather than taste**: of the twelve sites keeping an identity map, only about a third hold a bare buffer and the rest carry a slot index, an expected length, a phase or a sequence number ([pending.rs](src/pending.rs)). **Validated against the second consumer rather than reasoned about alone.** The epoch-durability ring wraps this ring and injects flushes of its own, which it must *not* surface to its client; with the payload returned at pop it matches a closed enum and filters them, which is the `Held` pattern [generated_sequences.rs](tests/generated_sequences.rs) already uses for eight token types on one ring. **What this does not settle.** The generic signature is `M28.3`'s. `M28.5`'s tokenless push is informed but left open: every push yielding an identity, and a push with no payload simply yielding none at pop, is the shape `flush_raw`'s bare `usize` was already approximating. And it names a migration cost for `M28.4` -- [`EventDelivery`](src/event_delivery.rs)'s callback takes `Fn(Completion)` today and would receive the payload alongside it. |
| <a id="d-72"></a>D-72 | **The contract oracle's memory follows operations in flight, not operations ever performed. A terminal outcome leaves the tracking map, and the last 1024 finished identities are kept for one purpose only: telling a duplicate completion from an unrecognised one.** `M28.2` asked three things and the code answered the first two. [`RingContract::check_quiescent`](src/contract.rs) mapped `Completed` and `DeliberatelyLeaked` to `None`, so it **demonstrably never read a terminal entry** -- yet `observe_claim` rewrote the entry in place and kept it, retaining one per operation for the life of the process. Nothing documented it, and the sample could not show it because it appends 24 records. So a consumer following this crate's own recommendation grew without limit, which is also why `M23.3` could not reach for an always-on checked inventory: a check that cannot be left running is not a check. **Naive pruning was rejected as worse than the leak.** Retiring a finished entry makes a later completion for it fall to `None`, which reported [`Violation::UnexpectedCompletion`] -- and that variant's own documentation says it means no observed push produced it, while `DuplicateCompletion` exists separately *because* "a duplicate is a much stronger signal ... it cannot be explained by a caller forgetting to report a push". Collapsing them would have had the oracle report a **wrong reason**, not a coarser one, and two tests named for the distinction (`a_completion_after_a_claim_is_a_duplicate_not_a_fresh_operation`, `a_tokenless_operation_completing_twice_is_still_a_duplicate`) encode it deliberately. An oracle that misnames a cause is worth less than one that grows. **So the identity outlives the entry.** `State` keeps only what quiescence reads -- `Pushed`, `PushedTokenless`, `Leaked` -- and a bounded queue holds the last `FINISHED_HISTORY` finished identities. Both tests pass unchanged. **The price is stated rather than discovered**: a duplicate that has fallen out of the window is still reported, under the weaker name, and a test asserts both halves so the limit is honest rather than aspirational. The window is a diagnostic parameter, not a correctness one, and changing it is not a breaking change. |
| <a id="d-73"></a>D-73 | **The inventory is `IoRing<T, X = ()>` -- the caller's payload and its sidecar -- and the file guard is not generic, because `FileTarget` is sealed. Amends [D-71](#d-71) one layer down.** `M28.3.3` found what `D-71` had not: the eleven push sites hold **five** structurally different things, and only some are the caller's. `B` and the sidecar are, but `F::Guard` and `RegisteredUse` are **manufactured by the crate during the push**, so there is no obvious way for them to enter a caller-chosen type. **The seal is what resolves it.** `FileTarget` is sealed to `SharedFile` and `RegisteredFile` ([batch.rs](src/batch.rs)'s `mod sealed`), so the guard set is closed and crate-owned: an entry carries the caller's `(T, X)` beside a concrete internal `Held { guard: Option<..>, registration: Option<RegisteredUse> }`, and the five shapes collapse into two `Option`s the consumer never names. **This makes the seal load-bearing in a way it was not.** It was an API-stability choice; it now underpins the inventory, because unsealing `FileTarget` would force either type erasure -- which [D-4](#d-4) forbids outright -- or a third generic parameter every consumer must name. **The correction that matters most is about `Drop`.** It is tempting to say a ring that pops its own completions no longer needs `Token`'s leak-on-unclaimed-drop, since it can prove the kernel is finished. That is true only on the path where rundown succeeds. [`Drop for IoRing`](src/ring.rs) runs `run_down` **best-effort** -- on failure it `debug_assert!`s and closes anyway, as its own SAFETY comment says -- and today that is survivable precisely because the *caller* holds the buffers and a dropped `Token` forgets its value, so nothing is freed while the kernel may still be writing. Moving the buffers into the ring removes that protection unless it is rebuilt, so **the forget mechanism does not disappear, it becomes the crate's**: the inventory must be forgotten rather than dropped when rundown fails. **It also reaches back into [D-72](#d-72).** With no token a caller can lose, `Violation::LeakedToken`, `observe_deliberate_leak` and `State::Leaked` are all written around a hazard that changes shape, so whether `RingContract` narrows or partly retires is a decision `M28.4` must take rather than discover. **Residue for `M28.5`:** a tokenless push has no `T` but still needs an `X`, so either `X: Default` or tokenless pushes name one. **Constraint worth stating:** `T` and `X` are both per-ring and both want `Send + 'static`, so a borrowed sidecar is not expressible and a consumer mixing shapes writes a closed `enum` for each. **`M28.4.1d.2b` settled the same question for `T`, and built nothing.** The enum this row already prescribes for a mixed sidecar is also the answer for a mixed payload, alongside a ring per buffer type; both are now stated on [`IoRing::with_inventory`](src/ring.rs), where a consumer meets the parameter rather than a type error. No `IoBuf` enum helper was added. The evidence was the conversion itself: of the seventeen ring types `M28.4.1d.2` introduced across this crate's consumers, **one** carried two buffer types through what had been a single ring ([epoch_log/logfile/tests.rs](examples/epoch_log/logfile/tests.rs), a `NumaBuffer` and a `Vec<u8>`), and its two writes were already sequential, so a ring each changed nothing it asserted. Two other files hold two ring types apiece and are **not** instances: [handover.rs](tests/handover.rs)'s are used by different tests, and [submission_lifecycle.rs](tests/submission_lifecycle.rs)'s share `Vec<u8>` and differ in `X`. That census covers this crate's own consumers, which are predominantly tests; it bounds what has been observed, not what an embedder will do. An embedder mixing a registered arena with ad-hoc records would meet this on its first ring. |
| <a id="d-74"></a>D-74 | **The oracle's leak rules retire rather than narrow, because the hazard they describe stops existing; and the conservation they were approximating becomes something the ring can check about itself.** `M28.4.1c`. `Violation::LeakedToken`, `State::Leaked`, `RingContract::observe_claim` and `RingContract::observe_deliberate_leak` all describe one situation: a caller held a `Token`, dropped it without claiming, and so lost whatever it owned while the kernel may still have been using it. Once a push hands back only an [`OperationId`](src/token.rs) -- `Copy`, no `Drop` ([D-71](#d-71)) -- **there is nothing for a caller to drop unclaimed**, and no claim step for it to skip. A rule against an impossible act is not a weaker rule, it is a misleading one: a reader meeting `LeakedToken` would go looking for the way to leak a token, and there would not be one. **`State::Pushed` and `State::PushedTokenless` collapse for the same reason.** That distinction existed only to say whether a token was *owed* -- the `_raw` entry points returned a bare `usize` and would have drawn a `LeakedToken` nobody could satisfy. With the inventory no push owes a token, so every push is the same shape and the pair becomes one state. **What replaces them is stronger and structurally cheaper.** [`accounting.rs`](src/accounting.rs) increments `outstanding` at `reserve_user_data` and decrements it at `record_completion`, and the inventory gains its entry at the same push and loses it at the same pop -- so once `M28.4.1d` retires the token pushes, `IoRing::held` and `IoRing::outstanding` must agree, and **the ring owns both numbers**. That is the same conservation the leak rules were approximating, checkable without a caller driving an oracle at all. It answers the structural complaint [pending.rs](src/pending.rs) recorded against the whole approach: an oracle "whose value depends on being driven correctly by the very code it checks". **`RingContract` narrows, it does not go.** `UnexpectedCompletion`, `DuplicateCompletion`, `Outstanding` and `BufferStillInUse` are claims about what the *kernel* did, and a consumer validating its own harness still needs to drive them from outside ([D-72](#d-72)'s bounded history is what keeps the duplicate distinguishable). **Whether the ring should absorb that checking too is deliberately not decided here** -- it could, since a pop already knows whether the identity was stowed -- and is queued as `M28.7` rather than taken in passing. **Applied in `M28.4.1d.3`, and one prediction it made was wrong.** The retirement landed as written -- `Violation::LeakedToken`, `State::Leaked`, `observe_claim` and `observe_deliberate_leak` are gone, the two push states collapsed to one, and completion became terminal instead of provisional. What this row did not anticipate is how far the removal reached into *tests*: two generated-sequence harnesses sampled claim-or-drop as a **generated dimension**, each asserting its own coverage of it, so retiring the rule deleted a dimension of the space those oracles explore rather than a line from each. `generated_sequences.rs` also carried an exception carved out of that dimension -- a registered buffer could not be dropped unclaimed without pinning its arena slot -- and the exception dissolved with the axis it qualified. Three tests whose whole subject was the leak model were deleted rather than converted, and replaced by the pair that states what took its place: a completion settles its operation, and a push without one is still `Outstanding`. Both directions, because a test showing only the first would pass against an oracle that settled everything. **`M28.4.2` then found that `M28.4.1d.3` had broken the sabotage manifest**, which is worth recording because of *how*: two cases patched `src/pending.rs`, deleted by that commit, so the harness aborted before testing anything -- and two more had gone silently stale, matching zero times. The commit that retired the token API passed its own gate cleanly; what it did not do was run the thing whose whole purpose is re-checking earlier guarantees. A guard that is only run when someone remembers is not on the ladder. |
| <a id="d-75"></a>D-75 | **A `_raw` push creates no inventory entry, and the ring does not try to tell that apart from a completion it never stowed -- the caller already can, because it chose the push.** `M28.5` asked what the inventory does with an operation that has no token, and most of the question had already dissolved: `D-74` collapsed the two push states, which left `RingContract::observe_tokenless_push` **byte-identical** to `observe_push` and its doc describing a leak violation that no longer exists. Two names for one behaviour is the shape `FAIL FAST` rule 1 is about, so the tokenless one retires. What genuinely remained is what the outer `None` from a pop means, and the honest answer is that it has two causes the ring cannot separate: a `_raw` flush or cancel, which takes a borrowed handle and deliberately creates no entry, and an identity this ring never stowed, which is a contract violation. **Closing that gap was considered and rejected.** Giving every raw push an entry needs an `X` the caller never supplied -- the residue [D-73](#d-73) named -- and would mean either an `X: Default` bound on the ring or a second argument on a call whose whole point is that it carries nothing. The `_owned` forms already exist for a caller who wants an entry, so choosing `_raw` *is* choosing not to have one. The distinction is therefore documented on [`IoRing::try_pop`](src/ring.rs) and bound by a test that asserts both directions on one ring -- a raw push holds nothing, an owned push holds its payload -- because a test showing only one side would pass against a ring that answered the same way every time. Verified by sabotage, which also demonstrated the separation `M28.4.2` asserts: patching `try_pop` left the test green, because it pops with `pop_within`. |

## Durability on the ring

Written for consumers, like the two sections that follow it, and for the same reason: the default
spelling is wrong and the failure is invisible until power is lost.

### What the ring offers

Three separate things, which are routinely conflated and must not be:

| Concept | Meaning | On this ring |
|---|---|---|
| **Ordering** | does B run only after A completes | `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` only, and then only as far as user mode can see it: what is observable is that B's *completion* follows A's, and only where B is the flag-carrying operation itself. It is one-sided, and execution start is not observable at all ([D-47](#d-47-detail)) |
| **Durability** | data is on non-volatile media | the flush operation only |
| **Atomicity** | a torn write is impossible across power loss | not exposed; a device property (NVMe `AWUN`/`AWUPF`) |

**There is no FUA.** `BuildIoRingWriteFile`'s entire flag set is `{FILE_WRITE_FLAGS_NONE,
FILE_WRITE_FLAGS_WRITE_THROUGH}`, and write-through is a cache-bypass directive to the OS, not a
device-level durability guarantee -- whether it becomes a Force Unit Access bit on the underlying
command depends on the driver, the volume, and whether the device's write cache is enabled. It is
useful as a latency-shaping knob (data already at the device shortens the subsequent flush) and must
never be treated as a durability marker.

**So the flush operation is the only durability primitive the ring has.** That is a narrowing
constraint, and it is worth stating plainly rather than leaving a consumer to discover it by
elimination.

### The two measured facts

[D-23](#d-23): **an unflagged flush does not cover preceding writes.** It is an ordinary operation
competing with them, and it frequently wins.

[D-24](#d-24): **the flag that fixes that drains what precedes it, and does not hold back what
follows.** A drained flush waits for every operation outstanding on the ring when it is reached --
that is the durability guarantee, and it is solid. Operations pushed *after* it are **not** held:
they can and do complete first ([D-47](#d-47), measured).

Together these mean the correct durability construction is still the expensive one, and it is worth
being exact about where the expense lands. **The cost is the flush's own latency, not a stall
imposed on anything else**: the flush waits on every operation outstanding on the ring when it is
reached, however unrelated to the caller's epoch, so it takes as long as the slowest of them. What
it does *not* do is hold up work submitted after it. A consumer that needs later work to observe the
flush's completion must arrange that itself, because the ring will not.

**How the reordering shows up is device-dependent, and that is a trap rather than a detail.** M12.2
re-ran the D-23 shape as a permanent test and measured a second machine behaving differently from the
spike's: there, *no* preceding write ever completed after an unflagged flush (0 of 32, against the
spike's 17 and 23), yet 11 of 32 writes queued *after* the flush completed before it. Reordering was
plainly happening; it simply did not manifest as the flush overtaking the writes ahead of it, because
that stack appears to order a flush behind its own file's outstanding writes on its own.

The consumer-facing consequence is the important part: **observing that your flush lands last is not
evidence that you can omit the barrier.** It is incidental behavior of one device stack, exactly the
kind of thing PLATFORM INTEGRITY says never to bind to, and it can change with the drive, the driver,
the filesystem, or the virtualization layer underneath. The barrier is what makes it a guarantee.
This is also why M12.2's test treats *either* direction of reordering as its control and skips when
it sees neither -- requiring D-23's specific observable would have made it silently vacuous on the
second machine.

### The construction this implies

Durability is a property of an **epoch**, never of an individual write, because there is no per-write
primitive to make it one:

1. Writes stream with no durability flag and are tagged with an epoch number.
2. Closing epoch *N* pushes a flush with `FlushCoverage::CoversPrecedingOperations`, carrying *N* as
   its identity.
3. When that flush's completion is observed, every write in epochs `<= N` is durable.
4. Callers wait on epochs, not on writes.

One expensive operation amortized over many writes -- the group-commit shape every write-ahead log
uses. Note that step 2's barrier is not optional decoration: without it, step 3 is false. Since M12.1
that is enforced by the signature rather than left to a default -- `Batch::flush` has no spelling that
omits the decision.

### Paying for the barrier

The drained flush waits for everything outstanding on the ring, so **the flush is expensive even
though the ring is not stalled** ([D-47](#d-47) corrected this: subsequent operations are not held
back, so later work does proceed). What you pay is a long-latency operation whose completion is the
epoch's ordering point, and the strategies differ in how that is paid. Which is right depends on
epoch size and latency target, which is why this crate exposes the mechanism and declines to choose
([D-8](#d-8), [D-26](#d-26)):

| Strategy | Cost | Suits |
|---|---|---|
| **Drained flush** (`FlushCoverage::CoversPrecedingOperations`) | the flush waits for every outstanding op, so its own latency is the epoch's; later submissions are not blocked | large epochs, where one long flush amortizes |
| **Host sequencing** -- observe the epoch's write completions, then push a `FlushCoverage::Unordered` flush | one userspace round trip per epoch (completion must reach your thread: wake, schedule, syscall) | any epoch big enough that ~tens of microseconds is noise |
| **Alternating rings** -- one drains while the other fills | doubled registration, split buffer pools, two completion events to wait on | latency-sensitive work that cannot tolerate either |

Host sequencing looks worst per-operation and is often right per-epoch: group commit means one
ordering point per epoch rather than per write.

**[D-47](#d-47) weakens the case for alternating rings, and that row has not been rewritten.** The
strategy exists because a drained flush was believed to stall the whole ring, so a second ring was
the only way to keep working through a commit. Later operations are not in fact held, so a single
ring can continue submitting during a drained flush and the second ring buys less than this table
claims. What it may still buy is a *clean* ordering point -- with one ring, work submitted during the
flush completes in an order the flush does not constrain, so a consumer that wants "everything after
this commit" to be identifiable still needs to arrange it. Whether that is worth a second ring is a
design decision rather than a documentation fix, so it is queued as `M20.6` in
[CHECKLIST.md](CHECKLIST.md) rather than settled here.

### Two device facts worth querying before doing any of this

- **Volatile write cache disabled?** Then writes are already durable and flushes are unnecessary. A
  consumer that flushes anyway is paying commit latency for nothing.
- **Atomic write unit.** A write larger than the device's power-fail atomic unit can tear, which
  decides how large a commit record can be before it needs its own checksum and replay.

Neither is exposed by this crate today, and neither is reachable through the ring API; a consumer
that needs them queries the device directly.

### A worked implementation of everything above

[examples/epoch_log/](examples/epoch_log/) builds this section as a running program, and is the
place to look when the prose above is clear but the composition is not. It carries a written-down
durability contract (authored before the code that implements it), group commit over a registered
arena, a multiplexed wait that services a non-ring `FSCTL` alongside ring completions, a thread-pool
control plane on a second ring, replay with a negative control, and all three commit strategies from
the table above implemented behind one interface and measured against each other.

Two of its findings belong here rather than only in the sample:

- Measured on the machine this was written on, the three strategies are **indistinguishable**: the
  spread across strategies is the same size as one strategy's run-to-run spread, because all three
  pay exactly one device flush per epoch at hundreds of microseconds while their actual differences
  land in the tens. The table's distinctions are real, and at that workload they sit two orders of
  magnitude below the dominant term. A device with a fast flush, a log committing far more often, or
  an arena under real pressure moves the balance -- which is why the sample measures rather than
  quotes.
- A benchmark that awaits each commit before appending again measures a workload no real log runs.
  Measuring requires the shape a real log has: keep appending while the commit is outstanding.
  **This bullet originally justified that shape by saying a ring-wide barrier costs nothing when
  nothing is queued behind it.** [D-47](#d-47-detail) withdrew the stall that reasoning assumed, so
  the overlap now exposes the strategies' other costs rather than a stall. The measurements stand;
  what they mean is [`M20.6`](CHECKLIST.md).

The sample is a demonstration of a pattern, not supported API surface -- it makes exactly the policy
choices [D-8](#d-8) and [D-26](#d-26) say this crate must not make.

## The completion event is an edge, not a level

This is the second section written for consumers rather than for maintainers, for the same reason as
"Two delivery architectures" below: getting it wrong produces a hang, and nothing in the Win32 surface
warns you.

**The event is signalled when the completion queue transitions from empty to non-empty.** It is not
signalled once per completion, and it is not level-triggered. Measured directly against the Win32 API
(`IoRing` version 400, real kernel ring, `UM_EMULATION` absent):

| Case | Result |
|---|---|
| Completion arrives into an **empty** CQ | event **is** signalled |
| Completion arrives into a **non-empty** CQ | event is **not** signalled |
| CQ drained to empty, next completion arrives | event **is** signalled again |
| Event attached while the CQ is **already non-empty** | never signalled -- and subsequent completions do not signal it either, because the queue never returns to empty |
| 8 completions submitted at once into an empty CQ | exactly **one** wakeup; a single drain-to-empty retrieved all 8 |
| Event still signalled after a full drain | no -- no spurious leftover signal |

Two rules follow, and they are part of this crate's published contract rather than advice:

1. **A waiter must drain to empty before waiting again** -- `try_pop` until it yields `None`, on *every*
   pass through a multiplexed wait loop, not only on the pass where the ring's own handle signalled. A
   wait entered with entries still in the CQ blocks until some later completion arrives after the queue
   has been emptied, which may be never.
2. **A wake with nothing to pop is normal** and must not be treated as an error or as evidence of a
   spurious wakeup. `completion_event` deliberately produces one at setup (D-20).

The same measurements also settled what `SetIoRingCompletionEvent` permits, none of which is documented:
it may be called at any time including with operations in flight; calling it again replaces the event;
passing `NULL` clears it and leaves the ring fully usable via `SubmitIoRing`'s own wait; and a
`DuplicateHandle`'d copy is still signalled after the original handle is closed, which is what makes
D-20's hand-back-a-duplicate shape sound.

**This bit us before it bit anyone else.** `EventDelivery::new` attached the event and armed the wait
with no initial drain, while its rustdoc claimed delivery covered completions "already queued when
`ring` was handed over". Rule 2's attach case makes that false: a ring handed over with a non-empty CQ
stranded those completions permanently, because nothing would drain the queue back to empty and no
later completion could signal. The existing M4 test only ever handed over a *fresh* ring, which is why
it passed. Fixed in M11.3, in the same change that re-expressed `EventDelivery` on top of
`completion_event` -- the signal-once-on-attach in D-20 is what closes it. The repro is kept as
`completions_queued_before_handover_are_still_delivered` in `tests/event_delivery.rs`, and it was
watched failing (a five-second delivery timeout) against the old implementation before the fix landed,
so it is known to bind rather than merely to pass.

## Specifying this contract: the ten gap categories

This crate publishes a completion contract consumers build reliability on, so it is exposed to the same
under-specification failure `windows-file-watcher` measured in PR #42: a contract written as prose is true but
incomplete, and the gaps stay invisible until something has to obey it mechanically rather than read it. The
ten categories and the evidence behind them are recorded once in
[the workspace design notes](../../DESIGN-NOTES.md#specifying-a-delivery-contract). This section records where
this crate sits against them.

**Two it already got right, and which are worth citing as the pattern rather than treating as routine.**

- **[D-17](#d-17)'s `RingId` is category 4/5 (cross-object identity and cross-field relationship) handled
  correctly.** `UserData` is a per-ring counter starting at zero, so two rings routinely hand out the same
  value: a `Token`'s `id == completion.user_data()` check alone cannot distinguish them, and a registered
  index is only meaningful against the table it was assigned in. Stamping every `Token`, `RegisteredFile`,
  `RegisteredBuffers`, and `Completion` with the minting ring's identity is exactly the "an identity must be
  durable, not merely descriptive" rule `windows-overlapped-io-sys` reached independently with generation
  stamping. Using a monotonic counter rather than the ring's `HANDLE` is the same reasoning one step further:
  Windows may reissue a closed ring's handle value, so the handle is not durable either.
- **`Completion::synthetic` being `#[cfg(test)]`-only is category 10 ("valid by construction" overclaimed)
  handled correctly.** Its own comment states the rule: production code has no legitimate reason to fabricate
  a completion, because `Token::claim_if`'s safety argument depends on every `Completion` in existence
  tracing back to a real `IORING_CQE`. That is precisely the restriction `windows-file-watcher`'s D-83 had to
  learn and `windows-overlapped-io-sys`'s `post`/`post_raw` still lacks -- a test seam confined to test
  builds, rather than a public one documented as "do not misuse".

**One recorded as an assumption, which is category 4 (cross-message continuity) -- and which the audit then
dissolved.** [D-14](#d-14) stated that registration bookkeeping advances when a `BuildIoRingRegister*` call
queues rather than when its completion is observed, and said outright that this was unverified because
neither function takes an `IORING_SQE_FLAGS` parameter to force a drain barrier. The continuity rule -- that
the next registration's base index follows the previous one's -- is exactly the shape of invariant that lives
*between* two messages and so has no natural home in either, which is why recording it explicitly was right.

M10.3 then found that the rule had stopped being load-bearing without anyone noticing: D-14's safety argument
turns on never colliding *two* registrations, and a second registration of either kind was forbidden the day
after D-14 was written. `base_index` is therefore always zero and no later base index is ever derived, so the
kernel's claim timing has no observable consequence and measuring it would settle nothing. The assumption is
dissolved rather than verified ([D-31](#d-31)); what survives is the *reserved-not-confirmed* meaning of the
public counts, which is now stated on them. Worth noting as a category-4 lesson in its own right: the
decision did not become wrong, it became irrelevant, and nothing in the process would have surfaced that if
the audit had not gone looking.

### Completion ordering is unspecified, and a ring invites the opposite assumption

The gap this audit found is an omission: **nothing here says whether completions may be assumed to arrive in
submission order.** They may not, and this crate is more exposed to the wrong assumption than its siblings,
because the word *ring* and the `io_uring` comparison in the Intent section both suggest an ordered queue.

- The completion queue is a userspace ring the consumer pops with `try_pop`, but the *order entries enter it*
  is the kernel's, not the submission order. Nothing in the spike findings established otherwise, and the
  spike deliberately recorded what it did establish.
- `drain_preceding` (`IOSQE_FLAGS_DRAIN_PRECEDING_OPS`) exists precisely because ordering is otherwise not
  guaranteed. Its presence in the API is itself evidence: a barrier flag is only meaningful where there is
  no order to rely on without it. That inference is currently available to a reader who notices the flag,
  and to nobody else.
- Model A and Model B observe completions through different paths (an event-driven drain-to-`S_FALSE` loop
  versus a pinned thread looping after `submit_and_wait`), and neither imposes an order the other shares.

**The contract is therefore: completion order is unspecified except where `drain_preceding` establishes a
barrier. An operation is identified by matching its `Token` against a popped `Completion`'s identity, never
by its position in the completion stream.** This is the same rule `windows-overlapped-io-sys` now states for
its own stream, reached independently on both sides -- which is the point of writing the category down rather
than the instance.

**The barrier's scope stops at the ring's edge.** `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` orders SQEs against
SQEs. A completion that is not an SQE -- an overlapped `DeviceIoControl`, anything issued through
`windows-overlapped-io-sys` -- is outside the barrier entirely, in both directions: the flag can neither
make a ring op wait for an overlapped op nor make an overlapped op wait for ring ops. A consumer that
needs ordering across both paths must enforce it in its own code, and this crate's job is to make that
expressible without blocking (D-20's `completion_event`) rather than to provide the barrier itself,
which belongs to whoever knows the semantics of the operations being ordered. The sentence above was
previously available to be read as stronger than it is; this states the limit explicitly, since a
consumer mixing both paths is exactly the case D-2 says is normal.

### Category 3: which pushes are legal is per-ring runtime state

Category 3 asks: the contract lists the modes; which messages is each mode *capable* of emitting? Here the
modes are ring capability states and the messages are pushes. [D-6](#d-6) makes the legal op set a per-ring
*runtime* property -- `IsIoRingOpSupported` is probed once per op at construction -- so which `Batch` methods
can succeed is state neither the type system nor the prose carried. The mapping, previously derivable only by
reading every `self.require(..)` call in `batch.rs`:

| Probed op | `Batch` methods it gates |
|---|---|
| `Op::Read` | `read`, `read_raw`, `read_registered`, `read_registered_raw` |
| `Op::Write` | `write`, `write_raw`, `write_registered`, `write_registered_raw` |
| `Op::Flush` | `flush`, `flush_raw` |
| `Op::Cancel` | `cancel`, `cancel_raw` |
| `Op::RegisterFiles` | `register_files` |
| `Op::RegisterBuffers` | `register_buffers` |
| `Op::Nop` | **none** -- see below |

Four rules follow, and each is a place the prose was silent.

**The capability set answers for the kernel, not for this crate's push surface.** `supports(Op::Nop)` is true
on every ring this crate has run on, and there is no `Batch::nop`: `IORING_OP_NOP` is reachable only through
`IoRing::push_raw`'s unsafe seam. That asymmetry is deliberate rather than a gap to close on demand -- a nop
owns no buffer, so there is nothing for a `Token` to hand back -- but it does mean `supports` must not be read
as "this ring will accept a push for this op through the safe API". It answers what the kernel's op table
contains. The distinction is not academic: it is exactly the op a consumer reaches for to wake a thread parked
in `submit_and_wait`, which is the shutdown problem [CHECKLIST.md](CHECKLIST.md) -> M6+.3 records.

**`supports_raw` is not restricted to ops outside `Op`, and agrees with `supports` where they overlap.** Its
name and its stated purpose ([D-7](#d-7): reach an op this crate has not wrapped) invited the reading that
passing a named op's `code()` is out of contract. It is not -- the two answer identically for every named op,
which the `capability_reporting_never_claims_more_than_is_io_ring_op_supported_reports` test already asserted
against a live ring while the rustdoc still read as excluding the case. The difference between them is cost
and caching, not truth: `supports` is a bit test against the set probed at construction, `supports_raw` is an
`IsIoRingOpSupported` call every time. What `supports_raw` never does is widen what `Batch` can push: an op
outside `Op` has no builder method whatever it answers, so `push_raw` stays the only route to one.

**Legality is decided before anything is reserved.** Every pre-`Build*` rejection -- an unsupported op, a
cross-ring `RegisteredFile`/`RegisteredBuffers` ([D-17](#d-17)), an out-of-range span, an oversized buffer, a
second registration -- returns before `reserve_user_data` runs. A rejected push therefore consumes no
`UserData`, counts nothing against `IoRing::run_down`, and leaves the ring exactly as it found it. D-17 states
this for the cross-ring check specifically; it holds for every legality check in `batch.rs`, and it is the
property that makes "just attempt the push and read the error" a safe probing strategy for a consumer rather
than one that silently strands an identity.

**A registration is a one-shot per ring, and the shot is spent by queueing, not by succeeding.**
`register_files` and `register_buffers` each refuse once the corresponding count is non-zero, because
`BuildIoRingRegister*` replaces the whole table rather than appending, so a second call would invalidate every
index the first handed out. Two consequences the prose did not state, both from the guard testing the *count*
rather than a flag:

- A zero-length registration does not spend the shot: it advances the count by zero, so a later registration
  is still accepted. This is correct rather than an oversight -- an empty registration hands out no index, so
  a later replacement invalidates nothing -- but the enforced rule is "at most one registration that assigned
  an index", not the "at most one call" the rustdoc claimed.
- The count advances when the `Build*` call queues ([D-14](#d-14)), so a registration whose *completion*
  reports failure has still spent the ring's one registration. There is no retry: a consumer whose
  registration fails must build it on a new ring. That is a real constraint on a consumer's error path, and
  it was previously discoverable only by reading `reserve_registered_files`'s call site.

### Category 1: independent options read as one concept

Two axes look independent on the push surface, and one pair genuinely is. **File addressing**
(`FileRef::Raw` vs `FileRef::Registered`) and **buffer addressing** (an owned buffer vs a registered one) are
fully orthogonal: all four combinations are legal and reachable, because `read_raw`/`write_raw` take
`impl Into<FileRef>` with an owned buffer and `read_registered_raw`/`write_registered_raw` take the same
`impl Into<FileRef>` with a registered span. A reader could reasonably have guessed the registered forms
pair up -- that registered buffers require registered files -- and they do not.

**What was *not* independent, when this audit ran, was file addressing and safety -- and the coupling ran
the wrong way.** Every safe method (`read`, `write`, `flush`, `cancel`, `read_registered`,
`write_registered`) took a `SharedFile` and hardcoded `FileRef::Raw(..)` internally; only the six
`unsafe fn` `_raw` variants accepted an `impl Into<FileRef>`. So `FileRef::Registered` -- the addressing
mode with *no* handle-lifetime hazard at all, since the ring holds the table and the caller passes only an
index this crate minted -- could be reached only through an `unsafe fn` whose own safety contract says, of
that very case, "A `FileRef::Registered` target needs none of this." The requirement was vacuous and the
`unsafe` unearned. That is a real API gap rather than a statement gap, which is why it became
[D-29](#d-29) and was implemented in M10.4 rather than merely written down.

**Fixed:** the safe pushes are now generic over the sealed `FileTarget` trait ([D-33](#d-33)), so a
`RegisteredFile` is pushed without `unsafe` and the fully-registered combination (registered file *and*
registered buffer) is expressible for the first time. The change is non-breaking -- `SharedFile` call sites
resolve exactly as before.

A smaller one: `PushOptions` is not universal, though its presence on most pushes implies it. Neither
`cancel` nor `cancel_raw` takes one, because `BuildIoRingCancelRequest` has no SQE-flags parameter, and
neither registration takes one either -- which is not an oversight but the precise root of [D-14](#d-14),
since it is what stops this crate from forcing a drain barrier around a registration.

### <a id="one-sqe-one-completion"></a>Category 2: unconditional read as probabilistic

The rule the prose never stated, and that `run_down`'s termination silently depends on: **every SQE that
successfully queues produces exactly one completion -- always, not usually.** `try_pop` returning `Option`
describes whether the completion queue has an entry *at this instant*, never whether one is ever coming.
`IoRing::run_down` loops until `outstanding` reaches zero and would not terminate if this were probabilistic.
The one case that produces no completion is the push that never queued at all: a `Build*` failing
synchronously releases its reservation (`cancel_reservation`), so it is not merely uncompleted, it is
un-counted.

Two consequences worth stating in the same breath, because both are places "may" reads too weakly:

- **`submit` and `submit_and_wait` return entries *submitted*, not completed.** `submit_and_wait(n, timeout)`
  returning does not mean `n` completions are poppable -- the timeout can expire first, and the return value
  counts submissions regardless. A consumer still drains with `try_pop` and still counts for itself.
- **A cancel is a request, not a guarantee, and it does not replace its target's completion.** The target may
  complete normally anyway; either way the cancel produces its *own* completion in addition to the target's,
  so a cancelled operation yields two. `ERROR_NOT_FOUND` on the cancel's own result means the target was no
  longer outstanding, which is a normal race rather than a caller error.

### Category 6: which state a transition is entered from

**A popped `Completion` matching no live `Token` is normal, not a bug**, and a drain loop that treats it as
one is wrong. There are four distinct ways to reach that state, and only the last is a mistake:

- a **registration** completion, which is claimed by `PendingFileRegistration`/`PendingBufferRegistration`
  rather than by a `Token`;
- a **`flush_raw` or `cancel_raw`** completion, for which no `Token` was ever created -- both return a bare
  `usize` identity, because neither op owns a buffer to hand back;
- a **cancel's own** completion, distinct from its target's;
- a completion whose `Token` the caller **dropped** unclaimed, which by [D-4](#d-4) forgets the buffer rather
  than freeing it.

Relatedly, completions can arrive for work the caller never explicitly submitted: `Batch` submits on `Drop`
([D-5](#d-5)), so abandoning a batch queues its pushes rather than discarding them.

### Category 8: values deliberately never correlated

**This crate joins nothing, deliberately.** Matching a completion to what it completes is the consumer's
loop, and every place a pairing could have been offered is left to them on purpose:

- a `Completion` is never joined to its `Token` -- `claim_if` is offered so the consumer can, and [D-4](#d-4)
  is why the crate does not do it for them (it would require the ring to retain a map keyed by `UserData`,
  which is exactly the per-operation allocation this design exists to avoid);
- a cancel's completion is never paired with its target's, though the consumer holds both identities;
- a registration's completion is never paired with the reads and writes that later address that registration;
- `Completion::information` is returned uninterpreted, since its meaning is per-op.

Stated plainly so the absence reads as a decision rather than an omission.

### Category 9: boundary-type fidelity lost at the consumer

Most boundaries here are lossless or narrow loudly. `UserData` is `usize` from `IORING_CQE` through `Token`
with no conversion. Buffer lengths narrow `usize` to `u32` through `checked_len`, which *reports*
`InvalidInput` rather than truncating. `RegisteredBuffers::len`'s saturating `unwrap_or(u32::MAX)` is
unreachable by construction, because `register_buffers` runs the same `checked_len` over the same `Vec`
before the registration is ever built. `RingVersion` wraps a raw `i32` precisely so a version this crate
cannot name survives ([D-6](#d-6)).

**The exception is `io::Error::kind()`, and it is the shape category 9 describes exactly: the value is
lossless, the derived form the consumer actually matches on is not.** Every kernel-reported failure goes
through `check`, which produces `io::Error::other(IoRingError)` -- so `kind()` is **always**
`ErrorKind::Other` and discriminates nothing, while the `HRESULT` itself survives intact behind
`downcast_ref::<IoRingError>()`. This crate's *own* rejections, by contrast, carry meaningful kinds
(`Unsupported`, `InvalidInput`, `AlreadyExists`). So `kind()` reliably answers "did this crate refuse the
push?" and never answers "why did the kernel refuse it?" -- including for `IORING_E_SUBMISSION_QUEUE_FULL`,
which the push rustdoc names as the expected backpressure signal and which is therefore the one a consumer
most needs to match. See [D-30](#d-30).

**Fixed:** M10.5 added the `RingCondition` enum, predicates for the runtime-actionable conditions, and the
sealed `IoRingErrorExt` that puts them on `io::Error` itself ([D-34](#d-34)), so the downcast is named once
in the crate rather than hand-rolled per call site -- `error.is_submission_queue_full()`. The lossiness of
`kind()` itself is unchanged and deliberate: mapping `IORING_E_*` onto `ErrorKind` would trade an honest
`Other` for a lossy guess, so the fix is a faithful second channel rather than a distortion of the first.

### Audit status

All ten categories have now been examined against this crate's surface (M10.1, M10.2). Categories 4, 5, and
10 were reached by the first pass above; 3 by M10.1; 1, 2, 6, 8, and 9 by M10.2. Category 7 (branch and
terminal paths documented by omission) is answered jointly by the category-2 and category-6 sections: the
terminal paths are "exactly one completion per queued SQE" and "no completion for a push that never queued",
and the branch paths are the four ways a completion can match no token.

## Two delivery architectures

This is the section written for consumers rather than for maintainers, and the reason it sits in a design
note rather than only in a commit message.

There are two coherent high-performance shapes, and they are mutually exclusive on the hot path.

**Model A -- shared queue, kernel load-balances.** A pool of threads waits; work is handed to whichever
thread the system picks. Load balancing is automatic, locality is incidental. Classic Windows IOCP is this,
and the Win32 thread pool *is* this, architecturally. In this crate, Model A is
`IoRing::completion_event` plus a `ThreadpoolWait` from `windows-threadpool-sys`: the ring signals an
event, the pool wakes a thread, the callback drains the completion queue. (`EventDelivery` reached the
event by calling `SetIoRingCompletionEvent` itself until M11.3 consolidated it onto the primitive; see
[D-20](#d-20).)

**Model B -- shared-nothing execution domains.** One pinned thread per domain, owning its ring, its buffer
pool, and its shard of the application's state, with no cross-thread synchronization on the data path.
This is SPDK, Seastar, and essentially every serious `io_uring` deployment. In this crate, Model B is a
pinned thread parked directly in `SubmitIoRing(ring, wait_n, timeout, &submitted)` -- the fused
submit-and-wait *is* the event loop. No event, no wait object, no wakeup indirection, and no drain/re-arm
race, because there is nothing to re-arm.

**IoRing is shaped for Model B.** The submission queue not being thread-safe, registration being per-ring,
and there being exactly one completion event per ring are not limitations to work around; they are the API
assuming a shared-nothing consumer.

### Model B's wakeup source is separable from Model B's identity ([D-20](#d-20))

The paragraph above describes Model B's *usual* wakeup source, and an earlier revision of this section
offered no other, which is how the framing came to be read as fixing it. It does not. Model B's identity
is **who owns, submits, and drains** -- one pinned thread per domain, no sharing on the data path. What
that thread happens to *block on* is a separate axis, and there are two answers:

| Wakeup source | The thread blocks in | Use when |
|---|---|---|
| **Fused submit-and-wait** | `Batch::submit_and_wait` (`SubmitIoRing` with `wait_n`) | The domain's only I/O is ring I/O. Nothing to re-arm, nothing to multiplex, lowest overhead. |
| **Multiplexed wait** | `WaitForMultipleObjects` over `IoRing::completion_event` plus other handles | The domain must also service non-ring handles: a shutdown event, a socket, an overlapped operation, a timer. |

Both are Model B. Switching between them changes neither ownership nor the submission path, and neither
one is a degraded form of the other -- picking the second does not make a consumer "Model A with extra
steps", and does not cost the locality that motivated Model B in the first place.

The second row exists because of a limit stated in full under Category 2 above and worth repeating
here, since this is where a consumer decides: **`IOSQE_FLAGS_DRAIN_PRECEDING_OPS` stops at the ring's
edge.** It orders SQEs against SQEs and is powerless in both directions across the ring boundary -- it
can neither make a ring op wait for an overlapped one nor make an overlapped op wait for ring ops. A
consumer mixing both paths therefore cannot get its ordering from the barrier flag and must enforce it
itself; the multiplexed wait is what lets it do so without either surrendering the ring or parking a
thread in a blocking drain.

The cost of the multiplexed row is that the waiter inherits [D-19](#d-19)'s edge-trigger contract in
full: drain to empty before waiting again, on every pass, and treat a wake with nothing to pop as
normal. The fused row has no such obligation, which is the honest reason to prefer it when it fits.
`examples/model_b_multiplexed.rs` (M11.6) is the worked shape, including shutdown with I/O still
outstanding; sabotaging its drain-to-empty into a single `try_pop` reproduces the lost-wakeup deadlock
directly.

### Why per-thread, and why pinning is not optional ([D-27](#d-27))

Model B's "one ring per thread" is usually presented as a convention. It is not -- it is userspace
reconstructing a discipline that exists one layer down, and knowing that changes how you size it.

**Kernels affine hot structures to CPUs, not threads.** Per-CPU state gets mutual exclusion for free
(disable preemption on Linux, raise IRQL on Windows) with no atomics and no contended cache line, and much
of the hot work has no owning thread to speak of -- an interrupt or DPC runs in whatever context the CPU
was in. Hence per-CPU run queues, per-CPU allocator caches, per-CPU deferred-work queues.

**The hardware agrees.** NVMe queue pairs are per-CPU, with each pair's completion interrupt routed by its
own MSI-X vector to that same CPU, so a completion lands where the command was submitted and the context is
still cache-warm. The affinity that produces the benefit is CPU-to-queue-to-interrupt-vector, established
in the device's programming. Threads are nowhere in it.

**Userspace has no per-CPU primitive.** It cannot disable preemption and its threads migrate. The only
durable ownership unit available is the thread -- so a *pinned* thread is the best available proxy for a
CPU, and that is the whole content of the SPDK/Seastar discipline.

Two consequences follow, and they are why this matters beyond terminology:

- **An unpinned per-thread ring keeps the safety and loses the point.** The SQ/CQ head/tail protocol is
  single-producer, and per-thread ownership satisfies that whether or not the thread is pinned. But the
  cache and NUMA locality that motivated the whole structure comes from the pinning, not from the
  per-thread split. This is a configuration people ship by accident.
- **The interesting count is cores, or LLC domains -- not threads.** Which is exactly what
  [D-8](#d-8) and the cache-domain guidance below already recommend; this is the reason underneath them.

### The two models are Windows' own two completion mechanisms

Worth noting because it makes the taxonomy less arbitrary than it looks. Windows has long had exactly two
ways to finish an I/O:

- **A special kernel APC delivered to the originating thread** -- work returns to the thread that issued
  it. That is Model B's shape, and it is the direct analogue of Linux's `task_work`.
- **A completion packet posted to an I/O completion port's queue**, taken by whichever pool thread is
  available. That is Model A, and it is why [D-9](#d-9) is right that the device-to-CPU association is
  already gone by the time a packet enters the port.

So Model A and Model B are not this crate's invention, nor `io_uring`'s. They are the two shapes the
platform has always had, showing up again at the ring.

### Why the three-way tension dissolves in Model B

A ring is three things at once, and in Model A they want different granularities:

| Role | Wants |
|---|---|
| Serialization domain (submission is not thread-safe) | finest possible -- per submitting thread |
| Dispatch domain (one completion event, one waiter set) | whatever is being affinitized |
| Registration domain (registered buffers and files are per-ring) | coarsest -- registration pins pages |

Registration is the axis that punishes over-sharding, and it is easy to miss: registering one buffer pool
into sixteen rings means sixteen separate pinnings of that memory, or sixteen pools each a sixteenth the
size. There is no partition that is optimal on all three axes -- which is a further argument for D-8.

In Model B all three coincide, because one thread per domain means per-thread and per-domain are the same
partition, and the buffer pool is per-domain anyway. The tension is an artifact of trying to share
something.

So the unit is not "a NUMA node." It is an **execution domain**: one pinned thread, its ring, its
node-local registered buffer pool, and its shard of the work.

### Why the NUMA node is the wrong key

Node count is a firmware setting, not a hardware property. AMD's NPS (Nodes Per Socket) presents the same
EPYC silicon as one node or four; Intel's Sub-NUMA Clustering does the same. A design keyed on node gets a
different partition on identical hardware depending on a BIOS option no process can see. On an NPS1 EPYC,
sharding "per node" puts 64 cores in one ring and calls it NUMA-aware.

It is worse in virtualized deployments, which is where most of this code will run: the machine this was
investigated on reported **zero** `Win32_NumaNode` instances. Any strategy keyed on node must degrade to
"one ring" when the answer is unknowable, which is the common case.

**And it is not only virtualization.** A shipping ARM consumer laptop reports zero `Win32_NumaNode`
instances too, and reports no L3 cache domains at all -- see [D-48](#d-48). The two observations are
siblings, and together they describe the machines most consumers actually have: one where the node is
absent because a hypervisor did not present it, one where it is absent on bare metal.

A better default heuristic is the **outermost cache level that actually partitions the machine**, which
`MachineMemoryTopology::outermost_partitioning_cache` answers -- defined in
[windows-topology-sys](../windows-topology-sys/README.md), so that every consumer asks rather than
restating. On EPYC that is the L3/CCX boundary,
which has a real latency cliff even inside a single NPS1 node, because crossing it goes out to the IO die
over Infinity Fabric. It degrades sanely: a VM whose caches partition nothing yields one ring, which is
correct.

**The rule is not "L3", and the level number is not the ordering.** Two measurements forced that wording,
and each falsifies a different half of the old one:

- A shipping Snapdragon X2 Elite reports **no L3 at all**, with its natural cluster boundary at L2
  ([D-48](#d-48)). So "the last-level cache is L3" is false on a part that ships today.
- The machine this workspace is developed on reports an L3 that spans **all 16 processors** over a real
  8-way L2 partition. So an L3 can exist and still partition nothing -- and a consumer filtering on
  `level == 3` there does not degrade, because it matched something. It reports one whole-machine domain
  as a successful cache-aware partition.

That second case is why the rule is stated as a question to ask rather than a level to match: the failure
is silent, and it collapses an eight-domain machine to a single ring while reporting success. The finding
that a cache domain beats the NUMA node is untouched by either.

**Choosing a specific level is still available; it is just not the default.** What was withdrawn is a
*policy* that matched on a level number, because that policy computed the wrong partition on two of the
three machines above. The underlying capability is untouched: `MachineMemoryTopology::cache_levels` and
`cache_partitions_at_level` let a consumer who knows their part ask about any level directly, and
`examples/cache_domains.rs` prints every level's distinct processor sets beside the heuristic's choice,
so the comparison the old policy got wrong is visible rather than asserted. On the development host that
output is `L1: 8`, `L2: 8` (chosen, checked pairwise disjoint), `L3: 1` -- a consumer can see in one
glance why matching `level == 3` there collapses the machine, and equally that on a part where L3 does
partition, choosing it is theirs to make. A consumer is given the data and the means to decide; what they
are not given is a preset that answers wrongly without saying so.

**Processor groups are a hard floor.** A thread's affinity is a `GROUP_AFFINITY` and a ring's waiter lives
in exactly one group, so above 64 logical processors the partition is forced whether or not it is wanted.

### Buffer placement probably dominates thread placement

For a storage workload the device DMAs directly into the registered buffer. A buffer on a node remote from
the device means **every byte crosses the interconnect, on every operation, forever**. Where the completion
callback happens to run is a one-time cache-warmth question by comparison.

So `VirtualAllocExNuma` for the pool, on the node closest to the device, registered once into that domain's
ring, is very likely the highest-leverage locality decision available -- and it is independent of
everything above about completion routing.

That allocation is `NumaBuffer`, which this crate provides as of [D-51](#d-51). *Which* node is still the
caller's answer, per [D-8](#d-8); the type supplies the allocation and decides no policy.

### What is not reachable

What is not reachable is the **answer** -- "which ring should this file's I/O go to". The **mechanism** is
reachable, and an earlier version of this section had that wrong: it said the mapping has "no clean
user-mode path" and "means walking volume to disk to device instance and reading
`DEVPKEY_Device_Numa_Node`". There is one documented call, and it takes the handle a caller already holds.

- **`FSCTL_QUERY_VOLUME_NUMA_INFO`** is documented in the IFS docs, accepts a handle to a **file or
  directory** directly, and returns `FSCTL_QUERY_VOLUME_NUMA_INFO_OUTPUT { ULONG NumaNode }`. No device-tree
  walk.
- **`GetNumaNodeNumberFromHandle`** is the other path: a Win32 wrapper over `NtQueryInformationFile` with
  `FileNumaNodeInformation` (class 53, Windows 7 and later), yielding
  `FILE_NUMA_NODE_INFORMATION { USHORT NodeNumber }`. PHNT and the WDK mark that class **reserved for
  system use**, so this crate must not build on it. It is named here so the next reader does not rediscover
  it and assume it is available.

Both were observed to succeed on an ordinary NTFS data file, and on a directory handle, and to agree --
so an ordinary file is not the no-association case. That run was on a single-node host, so neither call is
shown to name a node that distinguishes anything; the run and its limits are recorded in the spikes
[README.md](design-sessions/spikes/README.md), and
[file-handle-numa-spike.rs](design-sessions/spikes/file-handle-numa-spike.rs) is the instrument. Settling
what a multi-node host reports needs storage whose PDO advertises a proximity domain, which is a hardware
gap rather than a deferred decision.

**The conclusion this section has always drawn survives, on different grounds than it used to rest on.**
What either call returns is the node the *volume* resides on, not where the file's extents live. It is
absent whenever the device layer advertised no proximity domain -- `IoGetDeviceNumaNode` on the PDO, or
`DEVPKEY_Numa_Proximity_Domain` with `GetNumaProximityNode` from user mode. And one volume may sit on
several devices, which is the ordinary case for a spanned volume or a Storage Spaces set. So this crate
will not offer an automatic "put this file's I/O on the right ring." It offers "bind a ring to a domain and
submit from there," and leaves the mapping to whoever knows their storage layout.

**A sample now exercises the mechanism, which is not the same as the crate adopting it.** The epoch-log
sample asks `FSCTL_QUERY_VOLUME_NUMA_INFO` of its own log handle and places its arena on whatever comes
back ([D-50](#d-50)), so the call above has a runnable consumer rather than living only in a spike. That is
a sample making a local policy choice with the caveats in this section attached to it; the library's
position is unchanged.

### The practical shape

Almost nobody runs pure Model B. What works is hybrid: Model B on the hot data path (pinned threads,
per-domain rings, node-local registered pools, run-to-completion continuations, cross-domain work by
explicit message passing rather than shared state), and Model A for the control plane, background, and cold
paths, where the thread pool's quiescence is worth more than locality.

Both paths are therefore first-class in this crate, which is what D-3 records.

On sizing: one domain per physical core (not per SMT sibling) maximizes isolation; one per cache domain gives
a smaller number of domains that can still share cache-resident state cheaply -- eight rather than
sixty-four on a 64-core EPYC. Fewer domains balance load better and duplicate registered buffers less; more
isolate better. That is a workload call, and this crate does not make it.

`examples/ring_copy` (M7) is where that workload call actually gets made, for exactly one workload: it
implements the `ByCache`/`ByNode`/`ByPackage`/`ByCore`/`Single` policies above as runnable code, over a real
file copy, so the guidance here has something executable behind it rather than staying prose. The policy
lives in the sample, not the library (D-8); the library still makes none of these choices for a caller.

## What the spike established

A throwaway spike (see the design session) probed a current machine directly. Findings that the design
above depends on:

- `QueryIoRingCapabilities` succeeds with no ring; `MaxVersion` 400, max SQ 65536, max CQ 131072.
- `FeatureFlags` reported `SET_COMPLETION_EVENT` present and `UM_EMULATION` absent -- a real kernel ring
  rather than user-mode emulation.
- All seven ops supported, and only those seven.
- `PopIoRingCompletion` returns `S_FALSE` on an empty queue.
- **A file handle does not need `FILE_FLAG_OVERLAPPED`.** Reads succeed on an ordinary handle, which means
  `UnassociatedEndpoint` is not the required input type and this crate need not depend on that model.
- The completion event signals correctly and auto-resets.
- Registered file handles and registered buffers both work, including a read addressing both by index.
- A batch of eight reads submitted in one call reports `submittedEntries = 8` with all `UserData`
  preserved.
- Overflowing a 64-entry submission queue fails at entry 64 with `0x80460002` -- clean build-time
  backpressure, which is what D-5's design leans on.
- Cancelling a target that is not outstanding succeeds at build time and reports `0x80070490`
  (`ERROR_NOT_FOUND`) in the completion, not at build time.

## <a id="borrow-surface-audit-m181"></a>Borrow-surface audit (M18.1)

Population C -- what safe code is *permitted* to do -- is the one no runtime
technique reaches, because nothing has to execute for the hole to exist. Both
of the M14 review round's most severe findings were of this kind:
[D-35](#d-35) (`get_mut` returned `&mut Vec<u8>`, which permits `reserve`,
`resize` and reassignment where only byte writes were intended) and
[D-36](#d-36) (`get` returned an unchecked `&[u8]` while the kernel might still
be writing into it).

This is a pass over every public item that hands out a borrow or an owned
value, against one mechanical question:

> **What can safe code do with this, and does the registration or the kernel
> still hold anything it could invalidate?**

**Every item is recorded, including the ones where the answer is "nothing".**
An audit that lists only its findings cannot be distinguished, later, from an
audit that stopped early -- so the absence of a hole is written down as
evidence rather than left as silence.

| Item | Hands out | What safe code may do with it | Finding |
|---|---|---|---|
| `RegisteredBuffers::get` | `&[u8]`, from `&mut self` | Read the bytes. Cannot resize or reassign -- the slice, not the `Vec`, is the returned type. Cannot start an operation against the registration while the slice is alive. | **Was the audit's blind spot, fixed in M19 -- see [D-45](#d-45).** Refuses with `WouldBlock` while `kernel_writes > 0`, so a read cannot race a kernel write ([D-36](#d-36)); direction-aware, since a *write* in flight means the kernel only reads. The row as first written stopped there, and that was the omission: the check held at the instant of the call while the returned slice lived as long as the borrow, so `&self` let a caller take the borrow, then submit a read into that same buffer. `&mut self` makes the borrow itself conflict with the push. |
| `RegisteredBuffers::get_mut` | `&mut [u8]` | Write bytes in place. Cannot `reserve`, `resize`, or assign a new `Vec` -- that is exactly what [D-35](#d-35) narrowed. Cannot start an operation against the registration while the slice is alive. | **Guarded, and never had `get`'s second hole.** Refuses with `WouldBlock` while `outstanding > 0`. Per-buffer rather than per-registration, so a quiet buffer stays writable while its neighbours are busy. M19.3 re-checked it on the duration question and found it immune for free: `&mut self` already conflicts with the shared borrow a submission needs, so the analogous sequence is `E0502` rather than a hole. |
| `RegisteredBuffers::outstanding` | `Option<usize>` | Read a count. | **No hole.** A snapshot of an atomic; inherently stale the moment it is returned, and callers cannot act on it soundly -- which is why `get`/`get_mut` do their own checks rather than inviting a check-then-use. |
| `RegisteredFiles::get` | `RegisteredFile` (a `Copy` index + `RingId`) | Copy it, keep it past the registration, submit with it. | **No hole.** Every submission path validates `ring_id`, so a file from another ring is rejected rather than dereferenced. Keeping it past the *handle's* life is covered by `register_files`'s existing `unsafe` contract: the caller promises the handles stay open. |
| `RegisteredFile::index` | `u32` | Read the raw index; pass it to `push_raw`. | **No hole here; the hazard is `push_raw`'s.** A raw index used in a hand-built SQE is already inside that method's `unsafe` contract. |
| `Token::claim_if` | `T` (the buffer, plus any file guard) | Take back the buffer and use it freely. | **Sound by construction.** Handing the value back requires a `Completion` whose `user_data` *and* `ring_id` match, which is the proof the kernel is finished. A mismatched completion returns the token instead. |
| `Token` (dropped, not claimed) | nothing | Drop it. | **Safe but terminal for registered buffers.** `Token`'s drop is deliberately empty ([D-4](#d-4)), so the payload leaks rather than freeing memory the kernel may still touch. For a `Token<RegisteredUse>` that means the buffer index stays outstanding for ever and the registration can never drop cleanly -- which is why `read_registered_raw`'s rustdoc states the token **must** be claimed. Found the hard way in M17.3, whose generator emitted the drop and tripped the registration's own drop guard. |
| `PendingBufferRegistration::claim_if` | `io::Result<RegisteredBuffers<B>>` | Take the registration, or observe the failure. | **Sound, with a documented sharp edge.** A *failed* completion is treated as proof the kernel did not retain the addresses, so the buffers are dropped. M16.4 recorded that injecting a synthetic failure here would therefore free memory the kernel genuinely holds -- inert only because nothing does so outside the fault-injection seam. |
| `PendingFileRegistration::claim_if` | `io::Result<RegisteredFiles>` | Take the registration. | **No hole.** `BuildIoRingRegisterFileHandles` reads its array synchronously ([D-32](#d-32)), so nothing outlives the call that could be invalidated. |
| `EventDelivery::scope` (was `ring`) | `RingScope` | Submit work through [`RingScope::batch`], read the ring's read-only state. **Cannot** obtain a `&mut IoRing`, and so cannot replace the ring. | **WAS THE ONE FINDING -- [D-43](#d-43), fixed in M18.6.** The previous `ring -> &Mutex<IoRing>` let safe code assign a whole new ring through the guard, which compiled and silently stopped delivery: measured at one completion delivered before the swap and none after. [D-35](#d-35)'s shape at a different layer, and fixed the same way -- by narrowing the returned type to exactly what the caller needs. Now enforced by a `compile_fail` doctest. |
| `Pending::contract` | `Option<&RingContract>` | Read the oracle's record of what it has observed -- counts and per-operation states. **Cannot** mutate it, and cannot obtain one at all from an unchecked map, which is what the `Option` reports. | **No hole.** `RingContract` is a pure record: it owns no handle, no buffer, and no index into a registration, so there is nothing here the kernel or a registration could invalidate. The borrow is a plain `&self` borrow of the `Pending`, so nothing can be submitted through that `Pending` while it is alive -- every push takes `&mut self`. Added in M23.3 and audited in M26.2, which is late: the gate reported it on the next run rather than on the change that introduced it, because that change did not run the gate. |
| `IoRing::completion_event` | `OwnedHandle` | Wait on it, close it, hand it elsewhere. | **No hole.** The returned handle is a *duplicate*; the ring keeps its own, so closing the caller's does not stop the ring signalling ([D-20](#d-20)). Repeat calls duplicate the same event rather than attaching a second. The one real hazard -- two waiters on one ring -- is a documented misuse, not a memory-safety hole. |
| `IoRing::try_pop` | `Option<Completion>` | Read `user_data`, `code`, `result`; use it to claim a token. | **No hole.** `Completion` is a plain value carrying no borrow of ring state. Its power is that it authorises a claim, and that power is bounded by the id/ring checks in `claim_if`. |
| `Completion::with_injected_failure` | `Completion` | Rewrite the result of a *real* completion. | **Sound because it transforms rather than fabricates.** Same `user_data` and `ring_id`, so the "a completion exists therefore the kernel is done" argument is untouched. `Completion::synthetic`, which *would* fabricate, is `#[cfg(test)] pub(crate)` for exactly this reason. Behind the off-by-default `fault-injection` feature. |
| `Batch::{read,write,flush,cancel,*_registered}` | `Token<...>` | Hold, claim, or drop it. | **No hole beyond the `Token` row above.** The token is the only handle to the in-flight buffer, which is what keeps the buffer alive for the kernel's benefit. |
| `Batch::{flush_raw,cancel_raw}` | `usize` (a bare `UserData`) | Match it against a completion. | **No hole.** These operations own nothing a claim could hand back, which is why they return an id rather than a token -- and why `RingContract` has a separate `observe_tokenless_push`. |
| `IoRing::{info,version,supports,supports_raw,registered_*_count,outstanding}` | plain values | Read them. | **No hole.** Copies of state, no borrow of anything the kernel holds. |
| `capabilities()` | `Capabilities` | Read it. | **No hole.** A cached snapshot of a process-wide probe. |
| `IoRingError::{name,code,condition}` | `&'static str`, `HRESULT`, `RingCondition` | Read them. | **No hole.** `&'static str` borrows a literal, not ring state. |
| `contract::RingContract::{violations,check_quiescent}` | `&[Violation]`, `Vec<Violation>` | Read or keep them. | **No hole.** The oracle is pure bookkeeping over values the caller already reported; it holds no kernel resource. |

**One finding in nineteen items**, and it is the same shape as the two that
prompted the audit: a returned type that permits an operation nobody intended.
That is the pattern worth carrying forward into M18.2's recurring rule -- the
question that finds these is not "is this correct?" but "what else does this
type allow?"

### <a id="borrow-surface-audit-m21plus1"></a>Four more items, surfaced by widening the check (M21+.1)

The audit above is a point-in-time pass, and nineteen was its count. These four
are not corrections to it; they are items the *check* could not see, and so
never put to anyone. A 2026-09-21 review of the `M21.2` surface found that
[check-borrow-surface.ps1](../../tools/check-borrow-surface.ps1) inspected only
the text after the last `->` on lines matching `pub fn`, which leaves two shapes
invisible: **a method of a `pub trait`** (declared `fn`, not `pub fn`) and **a
borrow-carrying type in parameter position**. Widening it reported exactly four
entries, answered here before the inventory was regenerated.

Worth stating plainly: the check was not wrong about what it covered, and the
control still passes -- a `pub fn` returning `&[u8]` was caught throughout. It
was narrow, and nothing said so.

| Item | Shape the old check missed | What safe code may do with it | Finding |
|---|---|---|---|
| `IoRingErrorExt::as_ioring_error` | trait method, borrow **returned** | Read `code`, `name`, `condition` through a shared reference. | **No hole, and it is the honest catch of the four.** This predates the widening by months and was simply never inventoried, which is the blind spot made concrete rather than a new risk. The borrow is of the `io::Error` the caller already owns -- not of ring state, not of anything the kernel holds -- and `&IoRingError` permits reads only. |
| `CompletionWait::wait` | trait method, borrow **parameter** | Call `RingWait::block` and `RingWait::outstanding`, and nothing else. | **No hole, and the narrowing is the reason.** This is the wider exposure of the two directions: the wrapper goes to arbitrary safe code implementing the trait, not to a known caller. `RingWait` exposes no pop -- one would consume the completion its own caller is waiting for -- and no way to build work. It cannot be retained: the `'ring` lifetime is fresh per call and unconstrained by `Self`. Nothing else can touch the `IoRing` while it is alive, because the pop loop holds `&mut self` across the call. Handing out a bare `&mut IoRing` here would have been [D-43](#d-43) again. |
| `Batch::new` | borrow **parameter** with an explicit lifetime | Nothing it could not already do: the caller supplied the `&mut IoRing`. Re-asked at `M28.3.2`, when the parameter became `&'ring mut IoRing<T>`: the answer is unchanged, and the reason it is unchanged is the point. `T` names what the ring's *inventory* holds, so it varies what the ring owns on the caller's behalf without touching who may borrow the ring or for how long. The borrow is still exclusive and still for the batch's life, and safe code gains no new way to reach registered memory or a kernel-held address -- an `IoRing<T>` is reachable only through the same `&mut` the caller already had. Re-asked again at `M28.3.3` for `IoRing<T, X>`: unchanged for the same reason, since `X` is the caller's own sidecar and is neither read nor reachable through the batch. Worth noting what *did* change and why it does not bear on this row -- the ring now holds the caller's buffers, so `read_owned` moves one in and `try_pop_held` is the only way out. That narrows what safe code can reach rather than widening it: there is no longer a `Token` a caller can hold, and so no way to reach a buffer the kernel may still be writing into without first popping the completion that proves it is finished. | **No hole; this is [D-5](#d-5)'s mechanism, not a leak of one.** The exclusive borrow is what makes two concurrent batches fail to compile, which is the point of taking it. The borrow travels *into* the crate and is released when the `Batch` drops. |
| `EventDelivery::new` | borrow **parameter** with an explicit lifetime | Nothing; the callback environment is forwarded to `ThreadpoolWait::new` and not retained. | **No hole.** `EventDelivery` stores only `wait` and `ring`, so the `&mut CallbackEnviron<'_>` does not outlive the call. Re-asked at `M28.3.4`, when this and `scope`/`batch` gained `<T, X>`: unchanged. The parameters describe what the **ring** holds for its caller, so they alter neither the callback environment's lifetime nor what the delivery retains. `scope` and `batch` still confine their borrow to the returned value, and the payload now leaving through the callback is the same value the ring would otherwise have handed back at a pop -- reached only after a completion proves the kernel is finished with it. |

**A plain `&T` parameter is deliberately not reported**, or the inventory would
list every method in the crate and say nothing. Lending a reference *to* a
callee is the caller's business; what this defect class is about is a
borrow-carrying wrapper whose lifetime the crate chose. The explicit-lifetime
test is what separates the two, and it is a heuristic -- it would miss a
hypothetical `&dyn Trait` parameter carrying no named lifetime.

### <a id="borrow-surface-rows-retired"></a>Which rows of the M18.1 table describe retired items (M28.6)

The table above is a point-in-time pass and is not edited to match later code,
for the same reason the M21+.1 section below it was added rather than folded in:
the record of what was asked, and of what the answer was *then*, is the thing
worth keeping. But a reader looking up an item should not have to discover from
a compile error that it no longer exists.

`M28.4.1d.3` retired the token API, so five rows now describe items that are
gone: `Token::claim_if`, `Token` (dropped, not claimed), `Pending::contract`,
and the `Batch::{read,write,flush,cancel,*_registered}` row that hands out a
`Token<...>`. `IoRing::try_pop` survives but no longer hands out a bare
`Option<Completion>` -- it returns the payload alongside it, which is the whole
of `D-71`.

Two rows are **not** affected and are easy to mistake for the retired ones:
`PendingBufferRegistration::claim_if` and `PendingFileRegistration::claim_if`
are a different mechanism from the operation token, and both are live.
`Batch::{flush_raw,cancel_raw}` is also live, and its finding is now
[D-75](#d-75)'s subject rather than a note about `observe_tokenless_push`,
which `M28.5` retired.
### <a id="borrow-surface-audit-m2841d3"></a>One item removed, none added (M28.4.1d.3)

`M28.4.1d.3` retired the token API, and the only borrow-surface movement was a
**removal**: `Pending::contract -> Option<&RingContract>` went with
`Pending<T, X>` itself.

A removal still has to be put to someone, because the question this audit asks
is about what safe code may reach, and an answer can be wrong in either
direction -- a surface that shrinks can still leave a caller reaching for the
same state by a worse route. It does not here. `Pending<T, X>` was a
*caller-side* inventory: it lent a reference to an oracle the caller had handed
it, borrowing nothing of the ring's and nothing the kernel held. Its last
consumer was `epoch_log`'s `Appender`, which now owns a `RingContract` outright
and lends it through its own `contract(&self)` -- the same borrow, of the same
value, one layer nearer its owner.

Nothing was added. That is worth stating rather than leaving implicit: the
commit deleted ten push methods and two types, and a reader checking whether a
retirement of that size widened anything should find the answer written down
instead of inferring it from the absence of a row.
## <a id="testing-strategy-m185"></a>Testing strategy (M18.5)

Eight defects came out of the 0.1.x line and the M11-M14 branch. M15 through
M18 were built by sorting them by **what would have caught them**, rather than
by adding whichever technique was closest to hand. This section records the
result: which population each technique reaches, what each one actually found
when run, and -- the part worth reading if you read nothing else -- what none of
them reach at all.

### The three populations

| | The defects | What finds them | Built in |
|---|---|---|---|
| **A -- preconditions never varied** | [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47): every `event_delivery` test handed over a *fresh* ring, so "completion queue non-empty at handover" was never a test input | Generated operation sequences, so the state space is sampled rather than enumerated by hand. **`M26` added a second technique for this population**, on the observation that *the kernel's response* is a precondition too and was never varied either: a seeded resolver over a specified space | M17, M26 |
| **B -- failure paths never taken** | The checkpoint path authorising a reclaim after a failed write; [#48](https://github.com/MikeGrier/windows-threadpool-sys/issues/48) surfacing as a *lucky* `ERROR_NOACCESS` rather than corruption | Deterministic memory instrumentation, an executable contract oracle, and a seam that injects failure into a real completion | M15, M16 |
| **C -- permissions, not behaviour** | [D-35](#d-35) (`&mut Vec<u8>` permits `reserve`), [D-36](#d-36) (`&B` handed out while the kernel writes), and [D-43](#d-43), found by the audit itself | Review, made recurring by a mechanical trigger; mutation testing for the weaker cousin of the same problem | M18 |

Population C is the one worth dwelling on: **no runtime technique reaches it at
all.** Nothing has to execute for the hole to exist, so a fuzzer, an oracle, an
allocator and a chaos harness are all looking in the wrong place. That is why
review is a *primary* technique for this crate rather than a backstop, and why
M18.2 gave it a written question and a CI trigger instead of an exhortation.

### What each technique actually found

Recorded as measured, because the honest numbers are more useful than the
hoped-for ones.

| Technique | Found |
|---|---|
| Guard-page allocator + tracked poison (M15) | **No new defects.** Calibrated against D-32, which it turns into a hard `STATUS_ACCESS_VIOLATION` where 0.1.2 got a survivable `ERROR_NOACCESS` |
| `RingContract` oracle + fault-injection seam (M16) | **No new defects in shipping code.** Found two gaps in its own design (tokenless pushes, a claim path that frees on failure) |
| Generated sequences (M17) | **No new defects.** Found an API precondition the generator was violating, and -- once calibrated -- rediscovers #47 in 10 of 10 runs |
| Borrow-surface audit (M18.1) | **One defect: [D-43](#d-43)**, in 19 items audited. Same shape as the two that prompted the audit |
| Mutation testing (M18.3/4/7) | **A third vacuous test** four review rounds had read past, plus 36 further weak assertions. 79.7% to 95.8% |
| Seeded resolution over a specified space (M26) | **No defect in shipping code yet**, and the calibration is why that is reportable rather than reassuring: re-injecting `M21.6`'s expired-wait defect turns it red, and so does narrowing the resolver itself. It did find one live defect on its first contact with a real ring -- a declined submit propagating out of `IoRing::run_down` with work still outstanding, queued as `M26.8` |

Two observations that only appear once the table is read as a whole.

**M15 and M16 found nothing, and that is not reassurance.** They are *passive*:
they check invariants during whatever operations the existing tests happen to
perform. Zero findings meant the detectors had only ever seen the twenty or so
hand-written scenarios that already passed. M17 exists to feed them, which is
why its milestone is titled that way rather than "more testing".

**Every one of these instruments was wrong the first time, and only sabotage
found it.** M17.3's generator reported green with #47 reintroduced, because it
polled `try_pop` instead of waiting for the wakeup it was owed ([D-42](#d-42)).
Four of M18.4's mutation-killing tests did not kill their mutant, each for a
different and individually plausible reason. M15.2's poison inverse was
fabricated twice. The rule that falls out of this is stated in
[D-41](#d-41)'s corollary and is the single most transferable thing in M15-M18:
**a green result from an instrument nobody has shown can go red is not
evidence.** Budget the calibration, not just the instrument.

### What none of them cover

**Five of the six check this crate's code against this crate's stated
contract.** None of those five can tell you the stated contract is wrong -- and
in the two most expensive defects, that is exactly what happened. The
completion event is edge-triggered ([D-19](#d-19)) and
`BuildIoRingRegisterBuffers` reads its array when the operation runs rather
than when `Build*` returns ([D-32](#d-32)). Both were discovered by a spike
against the real kernel, and neither could have come from anywhere else in that
toolkit.

**The sixth is different in kind, and `M26.6` is what makes the difference
real.** The resolver tests this crate against a *written specification of what
the platform may do* ([RESPONSE-SPACE.md](RESPONSE-SPACE.md)) rather than
against our beliefs about what it does -- so it catches code that is brittle to
platform variation inside the permitted space, which is a class the other five
cannot reach. It still cannot tell you the specification is wrong. What can is
the **other half of the pair**: the kernel tests now confirm that a real Windows
stays *inside* the declared space, and a kernel observed outside it is a finding
about the platform rather than a regression in this crate. `RS-C-4` is the
clause that makes that job load-bearing rather than nominal, since the resolver
is forbidden to break the drain half of `DRAIN_PRECEDING_OPS` and therefore
cannot be what notices if Windows does. The division is enforced by
[response_space_census.rs](tests/response_space_census.rs), which fails when a
clause is claimed by nothing on the side that owes it a check.

So the honest statement is narrower than "a spike is the only way to learn the
platform is not what we assumed", and it is still true: a spike remains the only
technique that runs **before there is any code to test**, and the space itself
was written from what spikes established.

Be precise about the failure mode, because "the allocator would not have caught
D-32" is not quite true and the imprecision matters. The guard allocator *does*
catch it, loudly, once a test walks the registration path -- M17.4 measured
that. What no technique here supplies is the *knowledge* that the kernel reads
late, and without that knowledge the code is written wrong in the first place.
These techniques detect consequences on paths that already exist. A spike
produces the platform knowledge that determines what the code should be, and it
is the only technique that can run **before there is any code to test**.

Hence [D-44](#d-44): a spike is a budgeted, first-class technique for every new
Win32 surface, allocated before the wrapper is written. It carries two
obligations, both learned by getting them wrong -- a **control case**, because
the first two drain-ordering spikes could not discriminate and would have
returned confidently wrong answers; and it is **kept** as a standalone program
depending only on `windows-sys`, so what it measures stays the operating
system's behaviour and not ours. The surviving spikes and the reasoning behind
their shape are in
[design-sessions/spikes/README.md](design-sessions/spikes/README.md), and what
they established is summarised under
[What the spike established](#what-the-spike-established).

### <a id="two-techniques-deliberately-rejected"></a>Two techniques deliberately rejected

**The first was re-examined by [D-49](#d-49) / `M24.1` and now **stands, with its scope sharpened**
by [D-52](#d-52). The re-examination did not find an escape; it found that the instrument was the
wrong one.**

Recorded so they are not re-proposed as obvious wins.

**A mock `IoRing`.** Both shipped defects were the kernel behaving differently
from this crate's assumptions. A mock *encodes* the assumption, so one written
before those discoveries would have passed both bugs green -- it would not
merely have failed to find them, it would have manufactured evidence they were
absent. A model belongs here as an **oracle over observed sequences**
([`RingContract`](src/contract.rs)), never as a substitute for the kernel.

> **What `M24.1` demonstrated, rather than argued** (2026-09-22, the apparatus is
> [kernel-response-space-probe.rs](design-sessions/kernel-response-space-probe.rs)). Sharing a
> suite's assertions between a fake and the kernel does **not** rescue a mock. A fake built from the
> belief this crate held before `M21.6` -- that an expired wait is an error -- passed the shared
> suite green; the assertion that catches it could only be written after the kernel had already
> revealed the answer. Running the *wrong* assertion against both is the one case that helps: the
> kernel refutes it while the fake confirms it, which is the manufactured-evidence mechanism made
> visible.
>
> And the line is not "accounting versus Windows behaviour", which was the first answer. It is **our
> specified contract versus the platform's incidental behaviour**. "After submitting, the completion
> is already queued" reads like a contract and gave *opposite answers on two handles of the same
> API*; the same question stated as this crate's own contract -- "arrives within a bound we specify"
> -- holds everywhere. See [D-52](#d-52) for what replaces the technique.

**Application Verifier / PageHeap**, rejected after measuring rather than
assuming ([D-37](#d-37)): it works, and needs no SDK, but it is keyed by *image
file name* and cargo rehashes test binaries on every meaningful rebuild -- so
it would degrade silently to instrumenting nothing.

## <a id="d-47-detail"></a>D-47: the drain flag is one-sided, and how a rate of one-in-a-thousand hid that

[D-24](#d-24) recorded `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` as a **full** ring-wide barrier: preceding
operations drained, *and* subsequent ones held. The first half is right. The second is not, and this
records what was measured, why the original conclusion looked sound, and what changes.

### What was measured

[tests/flush_barrier.rs](tests/flush_barrier.rs) queues 32 large writes, one flush, then 32 tiny
writes, submits them as one batch, and reads the completion queue in order. Two observables:

- **D-23's**: a phase-A write (queued *before* the flush) completing *after* it. The drain forbids
  this.
- **D-24's**: a phase-B write (queued *after* the flush) completing *before* it. The claimed
  hold-back forbids this.

Run about 4,500 times across quiet, contended, and concurrent-ring conditions:

| condition | trials | D-23 failures | D-24 failures |
|---|---|---|---|
| idle machine, ring depth 256 | 3,000 | **0** | 1 |
| contended, ring depth 128 | 250 | **0** | 2 |
| contended, ring depth 256 | 250 | **0** | 1 |
| contended, ring depth 512 | 250 | **0** | 2 |
| eight concurrent rings, and earlier rounds | ~1,000 | **0** | 8 |

**D-23 never failed.** Every failure was D-24's, and in the worst trial all 32 post-flush writes
completed ahead of the flush -- the barrier absent entirely rather than leaking at its edge.

Three things the measurement rules out:

- **It is not contention.** The idle run failed. Contention appears to raise the rate (0.03% idle
  against roughly 0.4% loaded at the same depth) but is not necessary.
- **It is not queue depth.** 128, 256 and 512 gave 2, 1 and 2 violations per 250 -- indistinguishable.
- **It is not an artifact of how the queue is sampled.** Every violation was confined to a single
  drain: the flush and the writes that overtook it were already in the completion queue together, so
  their order is the order the kernel posted them in.

And one thing it deliberately does **not** settle. The completion queue is FIFO, so pop order is a
faithful witness of the order the kernel *posted* those completions -- that is the observable above,
and it is solid. It is not a witness of the order the operations *began executing*. Whether the
kernel actually let a post-flush write start early, or ran it in order and merely posted its
completion early, cannot be distinguished from user mode with any instrument available here. The
decision is therefore stated observationally throughout -- "can and does complete before it" --
because that is both what was measured and the only form a consumer can act on. A consumer needing
later work to follow the flush must sequence it on the flush's *completion* either way, so the
distinction does not change any advice; it bounds what this decision claims to know.

### Why the original conclusion looked sound

The spike behind D-24 ran the sequence a handful of times and saw the hold-back hold every time. At
the rate now measured -- often nearer one in a thousand than one in a hundred -- that is exactly what
a handful of runs would show. The conclusion was not careless; it was under-sampled, and nothing in
a passing run distinguishes "guaranteed" from "usually".

That is also why it survived three days as a suspected flaky test. A defect that appears in a
full-workspace run every few days and never in isolation looks like timing noise, and the natural
next step -- make the assertion less strict, or mark the test serial -- would have suppressed the
only evidence that a documented guarantee was false. The lesson worth keeping is that **a rare
failure in a test of a platform guarantee should be characterised before it is stabilised**, because
"flaky test" and "the platform does not do what we wrote down" produce the same symptom.

### What a consumer may rely on

**Guaranteed.** When a `FlushCoverage::CoversPrecedingOperations` flush completes, every operation
outstanding on the ring when the flush was reached has completed. That is the durability property,
it is what closing an epoch needs, and it held in every trial.

**Not guaranteed.** That operations submitted after the flush have *not* run. They may already have
completed. A consumer needing later work to observe the flush's completion must sequence it itself
-- submit it after the flush's completion is popped, rather than assuming the ring holds it.

The cost story in [Durability on the ring](#durability-on-the-ring) is unchanged, and is the flush's
own: it still waits for everything outstanding on the ring when it is reached, so it is still a long
operation and still a reason to choose the coverage deliberately. That cost is what the flush pays,
not something it imposes on later submissions.

### What the test asserts now

The contract test asserts D-23 and no longer asserts D-24, because D-24 is false and an assertion
that fails one run in a thousand teaches nothing except to distrust the suite. It still *observes*
the D-24 counter and prints it, so the rate stays visible and a future platform that did hold the
line would show up as a run of zeros rather than silence.
