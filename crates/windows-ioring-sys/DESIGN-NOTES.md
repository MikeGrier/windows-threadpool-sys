# Design notes: windows-ioring-sys (Tier 1)

This crate does not exist yet as compiled code. This file, the checklist beside it, and the design session
it references are the design record that precedes it. Creating the Cargo skeleton is M1.1 in
[CHECKLIST.md](CHECKLIST.md).

## Intent

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
| <a id="d-4"></a>D-4 | **Completion allocates nothing: the token owns the buffer, and the caller supplies the type.** `push()` returns a `Token<B>` that owns the `B` it was given; the ring stores only a generation counter and an in-flight count for rundown. No slab entry, no box, no type erasure -- the caller already knows `B`, so making it say so is free. Dropping a token whose operation is still in flight `mem::forget`s the buffer: leaking is safe, use-after-free is not, and this is the same leak-and-reclaim discipline `windows-overlapped-io-sys`'s `Operation` uses with the leak as the failure mode rather than the normal path. The ergonomic, allocating variant is layered on top of this, never underneath it. |
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
| <a id="d-24"></a>D-24 | **`IOSQE_FLAGS_DRAIN_PRECEDING_OPS` is a full, ring-wide barrier that spans submissions -- not a one-sided wait, and not scoped to a file. Measured.** Operations pushed *after* a drained op are held until it completes, even when they target an entirely different file, which rules out filesystem-level serialization as the cause. The barrier also reaches every outstanding operation on the ring rather than only the current submission batch: results were identical whether the sequence went in one `submit()` or three. This matches `io_uring`'s `IOSQE_IO_DRAIN` and it means **cross-epoch pipelining through a single ring is not available** -- a consumer that closes an epoch with a drained flush stalls the whole ring for its duration. The alternatives, and their costs, are in "Durability on the ring" below. |
| <a id="d-25"></a>D-25 | **Implemented: the flush barrier decision in M12.1 (`FlushCoverage`), the write flags in M12.3 (`WriteCaching`), the flush modes in M12.4 (`FlushMode`).** **Every durability parameter the kernel exposes is exposed by this crate, and the barrier decision is never taken by default.** `BuildIoRingWriteFile` takes `FILE_WRITE_FLAGS` and `BuildIoRingFlushFile` takes a `FILE_FLUSH_MODE`; this crate hardcoded `FILE_WRITE_FLAGS_NONE` and `FILE_FLUSH_DEFAULT`, so a consumer reading the API saw ordering but no way to express durability at all, and reasonably concluded the ring could not express it. That is a PLATFORM INTEGRITY failure -- the platform narrowed to what the crate's own examples needed -- and it is the second instance found in one review cycle, which makes it a pattern rather than an accident. Given [D-23](#d-23), `Batch::flush` additionally must not have a default spelling that produces a non-covering flush: the barrier decision is made explicit at the call site rather than inherited from `PushOptions::default()`. Queued as M12. |
| <a id="d-35"></a>D-35 | **A registered buffer's outstanding-operation count is per buffer, not per registration, so a caller can refill a quiet slot while its neighbours are in flight (M13.2).** Found by writing M13's worked example: `RegisteredBuffers` exposed `get` but no mutable accessor, so a registered arena could only carry bytes the *kernel* produced -- there was no way to put a caller-composed record into one, which is exactly what an arena-backed log does. `ring_copy` never hit it because it reads into a registered buffer and writes back out of the same one. The fix is `get_mut`, and the reason it needed more than an accessor is that **`&mut self` is not sufficient for safety**: an operation in flight holds no borrow (`write_registered` takes `&RegisteredBuffers` for the length of the call; the `Token` keeps only a `RegisteredUse`), so the borrow checker would allow mutating a buffer the kernel is reading through -- a data race with the kernel. `get_mut` therefore pairs `&mut self` with a runtime check of that buffer's own count, refusing with `WouldBlock` while it is busy and `InvalidInput` when the index does not exist. Per-registration counting would have been simpler and useless: it would refuse every slot whenever any slot was in flight, which is the normal state of a pipelined arena. Rejected: a narrow `unsafe fn get_mut_unchecked` seam, which would have been smaller but pushed the hazard onto every consumer of an arena, for a check that costs one relaxed load. **Corrected by code review: `get_mut` hands back `&mut [u8]`, not `&mut B`.** The first version returned the buffer type, which closed the *temporal* hole (no mutation while an operation is in flight) and left the *address-stability* hole wide open: `IoRing` has no unregister call, so the address and length given to `BuildIoRingRegisterBuffers` are live for the ring's remaining life, and `&mut Vec<u8>` lets entirely safe code `reserve`, `resize`, or assign a whole new vector -- each of which frees or moves that allocation, with no `unsafe` anywhere and at a moment when the outstanding count is legitimately zero, so no runtime check could catch it. A byte slice of exactly the length recorded at registration grants the one power a caller needs and none of the powers that break the registration. The same correction moved `checked_span` off the live `bytes_len()` and onto that recorded length, since the kernel's view is the registered one. The lesson worth carrying: a per-operation temporal check is not a substitute for a lifetime-of-the-registration structural one, and reasoning about "what can mutate this" missed "what can *replace* this". |
| <a id="d-39"></a>D-39 | **Non-fixed test data is permitted in this component only when it is seeded, announced, and pinnable -- and M15.2's poison is the first thing admitted under that rule.** The component's standing rule is that tests be reproducible and not use randomized sampling without explicit approval. A poison pattern that never varies would satisfy it trivially and be worth much less: a fixed byte like `0xDD` collides with real payloads, and code can come to depend on the constant without anyone noticing. The terms that reconcile the two: (1) the varying input is a single **seed**, not per-value randomness, so one number reproduces the entire run; (2) the seed is **announced** on stdout by `GuardAlloc::announce_seed` at test start, including the exact command to replay it; (3) it is **pinnable** from the environment (`WINDOWS_GUARD_ALLOC_SEED`, decimal or `0x` hex), verified end-to-end -- two unpinned runs produced different seeds, and two pinned runs reproduced byte-for-byte. A run with a pinned seed is exactly as deterministic as a constant; what varies is *which* deterministic pattern, which is the point. Two implementation constraints worth recording because they are not obvious: the seed must be readable **without allocating** (this code runs inside the global allocator, so `std::env::var` would recurse -- `GetEnvironmentVariableW` into a stack buffer is used instead, and `QueryPerformanceCounter` rather than `SystemTime` for the default), and the mixing function must be a **bijection with a computable inverse**, since identifying which allocation a region of poison came from is done by inverting it rather than by scanning. This rule governs the poison only; **M17.2's separate question of whether randomized *property* testing is permitted is still open**, and should be decided under the same three terms if it is admitted. |
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

## Durability on the ring

Written for consumers, like the two sections that follow it, and for the same reason: the default
spelling is wrong and the failure is invisible until power is lost.

### What the ring offers

Three separate things, which are routinely conflated and must not be:

| Concept | Meaning | On this ring |
|---|---|---|
| **Ordering** | does B start after A completes | `IOSQE_FLAGS_DRAIN_PRECEDING_OPS` only |
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

[D-24](#d-24): **the barrier that fixes that is a full ring-wide stall.** Operations pushed after a
drained flush are held until it completes, even against unrelated files.

Together these mean the correct durability construction is also the expensive one, and a consumer
must choose deliberately how to pay for it.

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

Because [D-24](#d-24) makes the drained flush a ring-wide stall, there are three strategies and no
free one. Which is right depends on epoch size and latency target, which is why this crate exposes
the mechanism and declines to choose ([D-8](#d-8), [D-26](#d-26)):

| Strategy | Cost | Suits |
|---|---|---|
| **Drained flush** (`FlushCoverage::CoversPrecedingOperations`) | ring stalls for the flush's duration | large epochs, where the stall amortizes |
| **Host sequencing** -- observe the epoch's write completions, then push a `FlushCoverage::Unordered` flush | one userspace round trip per epoch (completion must reach your thread: wake, schedule, syscall) | any epoch big enough that ~tens of microseconds is noise |
| **Alternating rings** -- one drains while the other fills | doubled registration, split buffer pools, two completion events to wait on | latency-sensitive work that cannot tolerate either |

Host sequencing looks worst per-operation and is often right per-epoch: group commit means one
ordering point per epoch rather than per write.

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
- The barrier's cost is invisible to a benchmark that awaits each commit before appending again,
  because a ring-wide barrier costs nothing when nothing is queued behind it. Measuring it requires
  the shape a real log has: keep appending while the commit is outstanding.

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
  [D-8](#d-8) and the L3-domain guidance below already recommend; this is the reason underneath them.

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

A better default heuristic is the **last-level cache domain**: `GetLogicalProcessorInformationEx` with
`RelationCache` filtered to `CacheLevel == 3`. On EPYC that is the CCX/CCD boundary, which has a real
latency cliff even inside a single NPS1 node, because crossing it goes out to the IO die over Infinity
Fabric. It is meaningful on Intel and ARM too, where the NUMA node often is not, and it degrades sanely: a
VM reporting one L3 domain yields one ring, which is correct.

**Processor groups are a hard floor.** A thread's affinity is a `GROUP_AFFINITY` and a ring's waiter lives
in exactly one group, so above 64 logical processors the partition is forced whether or not it is wanted.

### Buffer placement probably dominates thread placement

For a storage workload the device DMAs directly into the registered buffer. A buffer on a node remote from
the device means **every byte crosses the interconnect, on every operation, forever**. Where the completion
callback happens to run is a one-time cache-warmth question by comparison.

So `VirtualAllocExNuma` for the pool, on the node closest to the device, registered once into that domain's
ring, is very likely the highest-leverage locality decision available -- and it is independent of
everything above about completion routing.

### What is not reachable

Mapping a **file handle to the NUMA node of the device backing it** has no clean user-mode path. It means
walking volume to disk to device instance and reading `DEVPKEY_Device_Numa_Node`, with real failure modes
(spanned volumes, Storage Spaces, network paths, VHDs) where the question may have no answer. This crate
will not offer an automatic "put this file's I/O on the right ring." It offers "bind a ring to a domain and
submit from there," and leaves the mapping to whoever knows their storage layout.

### The practical shape

Almost nobody runs pure Model B. What works is hybrid: Model B on the hot data path (pinned threads,
per-domain rings, node-local registered pools, run-to-completion continuations, cross-domain work by
explicit message passing rather than shared state), and Model A for the control plane, background, and cold
paths, where the thread pool's quiescence is worth more than locality.

Both paths are therefore first-class in this crate, which is what D-3 records.

On sizing: one domain per physical core (not per SMT sibling) maximizes isolation; one per L3 domain gives
a smaller number of domains that can still share cache-resident state cheaply -- eight rather than
sixty-four on a 64-core EPYC. Fewer domains balance load better and duplicate registered buffers less; more
isolate better. That is a workload call, and this crate does not make it.

`examples/ring_copy` (M7) is where that workload call actually gets made, for exactly one workload: it
implements the `ByL3`/`ByNode`/`ByPackage`/`ByCore`/`Single` policies above as runnable code, over a real
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
