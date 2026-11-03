# Changelog

## [0.4.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-v0.3.0...durable-ioring-v0.4.0) (2026-11-03)


### ⚠ BREAKING CHANGES

* **durable-ioring:** an import scoped to domains allocates nothing
* **durable-ioring:** lend a provider request from a buffer made at construction
* **durable-ioring:** refuse a gate cycle without naming it
* **durable-ioring:** build without std
* **durable-ioring:** a refusal carrying a backend fault passes the backend's report through
* **durable-ioring:** epoch_state reports an abandoned epoch at or below the high-water mark as Durable, and a gate on one is no longer refused.
* **durable-ioring:** DurableRing loses failure_suspects and failure_markings, UnknownFailure and Entry::Marked are removed, and ConformanceOracle loses suspects and observe_from in favour of observer and reading; a consumer reads a failure's suspect writes and markings through a FailureObserver given at construction.
* **durable-ioring:** win-durable-ioring's Setup gains a failure_observer field.
* **durable-ioring:** give the failure inventory to a visitor  failures() no longer hands out a Vec of copies. On the DurableRing trait and the core it takes a visitor and gives each unresolved failure, in the order observed, as a FailureInfo<'a, V> whose cause is lent for the visit as &'a FailureCause -- the way LineageInfo already lends its description (DI-D-103). Leftover::Failure from close() carries the lent form. The visitor runs under the instance's lock and must not call back into it. Neither the inventory nor a close with failures allocates: the "failures queried" phase of the allocation test is now zero, and its three allocation tags are gone.  win-durable-ioring implements the new signature in Dioring, the delivery wrapper and the fake; its tests count the inventory, and its close helper keeps failure identities. The core's tests read the inventory through a test-only failures_listed() that copies each out, so their assertions kept their shape.  Swept "failures()"/"FailureInfo"/"inventory" across CONTRACT.md, DESIGN-NOTES.md, both READMEs and both crates' src: CONTRACT.md "The inventory" (1), DI-D-103 as-built (1), the trait and core rustdoc (2), win-durable-ioring's README example (1); API.md left as the superseded sketch.  Sabotage: one entry added (the core's inventory visiting nothing, caught by the driver's tests) and the two existing inventory entries re-swept; all as declared.  BREAKING CHANGE: DurableRing::failures and DurableIoCore::failures take a visitor instead of returning Vec[FailureInfo]; FailureInfo gains a lifetime and lends its cause.  Completed item: DI-23.12.3.7: The inventory as a visitor (DI-D-103, DI-D-108): failures() gives each unresolved failure to a visitor, lending its cause, rather than handing out a Vec of copies, as the lineages and the holds already are; close() lends its failures the same way.  Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
* **durable-ioring:** a heal takes effect at the first successful seal made after it that reaches every write pushed before it still to be committed; one that failed or is already covered is no longer waited for.
* **durable-ioring:** an ended lineage counts against the lineages declared until its LineageEnded is popped, so mint_lineage can be refused until then; retiring a live lineage withdraws its un-popped Durable; an un-popped Blocked is withdrawn when a later entry answers its request.
* **durable-ioring:** Failed loses suspect; Abandoned and Healed carry only the failure; FailureInfo loses suspect and markings; SuspectSet is removed; DurableRing gains failure_suspects and failure_markings; a resolved failure's holds, and an import's count, are let go when its resolution entry is popped; the oracle requires each Failed's suspect set.
* **durable-ioring:** LineageInfo is LineageInfo<'a, V> with description: Option<&'a str>, and Leftover is Leftover<'a, V, B, C>. DurableRing::lineages and close take visitors over the lent types, so a visitor written as a bare function over one lifetime, such as drop, no longer fits; |_| {} does.
* **durable-ioring:** LineageHandle::mint returns None when its home is gone or has no slot free, and the driver's Home trait gains slots(). A handle whose home is gone, or a detached one, counts no copies, so it is never the last.
* **durable-ioring:** keep what a call makes due in storage made at construction
* **durable-ioring:** a recorded seal holds a place of the window, and the seal queue is on fixed storage
* **durable-ioring:** DurableRing::lineages and close take a visitor and return nothing; close gives a Leftover enum in place of Leftovers; target_holds and lineage_holds are new required trait methods; IoTargetBusy loses lowest_gates and uncovered_through and its type parameter, and its failures and LineageBusy's are counts; RemoveTargetError loses its type parameter; the engine's take_requests is drain_requests.
* **durable-ioring:** DurableIoCore::new and with_provider take a Capacity and return a Result; DuplicateDomain is replaced by ConstructionError; mint_lineage returns a Result; import_failure returns ImportError; PushRefusal and AddTargetCause gain AtCapacity; Setup gains a capacity field and SetupRefusal gains Construction and AtCapacity.
* **durable-ioring:** a consumer that has not popped a Durable when a later one for its lineage is queued no longer sees the older; it sees one entry naming the later epoch, which says the mark has passed the earlier too.
* **durable-ioring:** a request to seal whose span holds no write, replaced before it is answered by a later such request of its lineage, gets no entry of its own; the later request's Durable entry answers it, as CONTRACT.md guarantee 4 now says. A diagnostic sink sees its seal pair end Superseded.
* **durable-ioring:** IoFault.error is an ErrorReport, not an Arc of io::Error; DomainCompletion::fail and DomainOutcome::Failed take an ErrorReport; DurableRing, RegisteredBufferRing and Waitable report IoFault where they reported io::Error; PumpedSetup's error is an IoFault; scripted_fault takes a static str. In win-durable-ioring, DeliveryEvent::Failure carries an IoFault and the registered-buffer methods of DeliveryHandle report IoFault.

### Features

* **durable-ioring:** a fault's report is an rc-errors ErrorReport, and io::Error leaves the portable boundary ([92c34e7](https://github.com/MikeGrier/windows-threadpool-sys/commit/92c34e799d0740831ee667d5f742681a630f1f3a))
* **durable-ioring:** a lineage has at most one un-popped Durable ([fdd11f8](https://github.com/MikeGrier/windows-threadpool-sys/commit/fdd11f88495618558dbde793cae43582cca6f45b))
* **durable-ioring:** a recorded seal holds a place of the window, and the seal queue is on fixed storage ([326de22](https://github.com/MikeGrier/windows-threadpool-sys/commit/326de2252e260ae342969bcecfad0829502159e5))
* **durable-ioring:** a refusal carrying a backend fault passes the backend's report through ([f4123ef](https://github.com/MikeGrier/windows-threadpool-sys/commit/f4123efd7e95476f824c30517419877b4a5cac6b))
* **durable-ioring:** a seal that covers no writes makes no record ([e5e25c9](https://github.com/MikeGrier/windows-threadpool-sys/commit/e5e25c941ef822a753d0d8de4a5525950ad8ae35))
* **durable-ioring:** an import scoped to domains allocates nothing ([d8c8bf7](https://github.com/MikeGrier/windows-threadpool-sys/commit/d8c8bf7ba8430e24f3bb6ccc3113c72266b75b87))
* **durable-ioring:** build without std ([bcb0266](https://github.com/MikeGrier/windows-threadpool-sys/commit/bcb02667942fe89e4ed6e5864a8446468bba5029))
* **durable-ioring:** count lineage handles' copies in slots made at construction ([5c07f50](https://github.com/MikeGrier/windows-threadpool-sys/commit/5c07f50f49111834e98de020f5c581dbae09e264))
* **durable-ioring:** count the submit window's places per operation ([f562fde](https://github.com/MikeGrier/windows-threadpool-sys/commit/f562fdea7dc0d69ba846256d19a4596b733a8866))
* **durable-ioring:** declare a capacity at construction and refuse past it ([81ab43b](https://github.com/MikeGrier/windows-threadpool-sys/commit/81ab43bbfe06b1d01dd0d7532da4b55e5065e5f7))
* **durable-ioring:** forget an abandoned epoch once the mark passes it ([7fb26be](https://github.com/MikeGrier/windows-threadpool-sys/commit/7fb26be39b18bc6026fd294fc2d7b15108777414))
* **durable-ioring:** give the failure inventory to a visitor  failures() no longer hands out a Vec of copies. On the DurableRing trait and the core it takes a visitor and gives each unresolved failure, in the order observed, as a FailureInfo&lt;'a, V&gt; whose cause is lent for the visit as &'a FailureCause -- the way LineageInfo already lends its description (DI-D-103). Leftover::Failure from close() carries the lent form. The visitor runs under the instance's lock and must not call back into it. Neither the inventory nor a close with failures allocates: the "failures queried" phase of the allocation test is now zero, and its three allocation tags are gone.  win-durable-ioring implements the new signature in Dioring, the delivery wrapper and the fake; its tests count the inventory, and its close helper keeps failure identities. The core's tests read the inventory through a test-only failures_listed() that copies each out, so their assertions kept their shape.  Swept "failures()"/"FailureInfo"/"inventory" across CONTRACT.md, DESIGN-NOTES.md, both READMEs and both crates' src: CONTRACT.md "The inventory" (1), DI-D-103 as-built (1), the trait and core rustdoc (2), win-durable-ioring's README example (1); API.md left as the superseded sketch.  Sabotage: one entry added (the core's inventory visiting nothing, caught by the driver's tests) and the two existing inventory entries re-swept; all as declared.  BREAKING CHANGE: DurableRing::failures and DurableIoCore::failures take a visitor instead of returning Vec[FailureInfo]; FailureInfo gains a lifetime and lends its cause.  Completed item: DI-23.12.3.7: The inventory as a visitor (DI-D-103, DI-D-108): failures() gives each unresolved failure to a visitor, lending its cause, rather than handing out a Vec of copies, as the lineages and the holds already are; close() lends its failures the same way.  Co-authored-by: Copilot &lt;223556219+Copilot@users.noreply.github.com&gt; ([cb5b55e](https://github.com/MikeGrier/windows-threadpool-sys/commit/cb5b55e6106f4f3a0525a2cd1c2743eb5f5402bb))
* **durable-ioring:** hand the clean paths' collections to visitors ([a39a6ce](https://github.com/MikeGrier/windows-threadpool-sys/commit/a39a6ce01d53de0b9617d734b36079521f6c2a01))
* **durable-ioring:** keep a lineage's description as given and lend it to visitors ([cd5d957](https://github.com/MikeGrier/windows-threadpool-sys/commit/cd5d95746009bbb67b575175f5cd265b6fd0b95f))
* **durable-ioring:** keep a suspected write's record and its marking once ([571d6c0](https://github.com/MikeGrier/windows-threadpool-sys/commit/571d6c0010695e2b490ee135b379a69918bdf347))
* **durable-ioring:** keep failure tokens' liveness in slots made at construction ([ab300cc](https://github.com/MikeGrier/windows-threadpool-sys/commit/ab300ccd77ee1ca7af4aaa3c03cc0a0029f4e24f))
* **durable-ioring:** keep flush-domain lists in rows made at construction ([1deffac](https://github.com/MikeGrier/windows-threadpool-sys/commit/1deffac326e823dd77e10953fcf9d8361113dc68))
* **durable-ioring:** keep the completion queue within a derived bound ([8d5d476](https://github.com/MikeGrier/windows-threadpool-sys/commit/8d5d476babdd3c1d2bfd0fea32e5567f7d86ab78))
* **durable-ioring:** keep the unresolved failures in a table bounded by derivation ([968df95](https://github.com/MikeGrier/windows-threadpool-sys/commit/968df954e9f035b383474c1cf9a723d6e7c75dfa))
* **durable-ioring:** keep what a call makes due in storage made at construction ([0a71d4f](https://github.com/MikeGrier/windows-threadpool-sys/commit/0a71d4f7c9f72ce17b36c455f95b0173e636bfab))
* **durable-ioring:** lend a failure's records until its resolution is popped ([d92d2a5](https://github.com/MikeGrier/windows-threadpool-sys/commit/d92d2a525653cd95df3632320ade0caac9ece594))
* **durable-ioring:** lend a provider request from a buffer made at construction ([24471fc](https://github.com/MikeGrier/windows-threadpool-sys/commit/24471fc12ddb49583a1b9c6588a09d4822a7eda7))
* **durable-ioring:** make the core's keyed tables fixed-capacity, allocated when it is built ([77c1cbc](https://github.com/MikeGrier/windows-threadpool-sys/commit/77c1cbcf1c6c9b8ff799ac388c6c827ae6d060af))
* **durable-ioring:** put the ordered tables on fixed storage made at construction ([8e4e1fd](https://github.com/MikeGrier/windows-threadpool-sys/commit/8e4e1fd77c3adc1c8d84bbcd9cb5decd134b16c5))
* **durable-ioring:** read a failure's detail only through its observer ([8614f37](https://github.com/MikeGrier/windows-threadpool-sys/commit/8614f37ad6da5f49dcc71b141f43a70c80ba8f25))
* **durable-ioring:** read a failure's suspects from the write records by its rule ([1f7355d](https://github.com/MikeGrier/windows-threadpool-sys/commit/1f7355d68282cf4c9cc0bb7d62333e46e3909453))
* **durable-ioring:** read a heal's releases from the write records ([ddef179](https://github.com/MikeGrier/windows-threadpool-sys/commit/ddef179af781c06871d3f38388d915981045be91))
* **durable-ioring:** refuse a gate cycle without naming it ([77c885d](https://github.com/MikeGrier/windows-threadpool-sys/commit/77c885db3bd57032a5295b3410f749e925e70f1e))
* **durable-ioring:** tell a failure observer given at construction ([fab1810](https://github.com/MikeGrier/windows-threadpool-sys/commit/fab1810ef63c10a8575a9485b555d20f2bcb7c0c))


### Bug Fixes

* **durable-ioring:** an ended lineage keeps the places of writes whose commit is still out ([f669a1c](https://github.com/MikeGrier/windows-threadpool-sys/commit/f669a1c67c418ed92990c5aab8c01c4e20f88cf1))
* **durable-ioring:** count a failure observer's allocations under the core's call to it ([1dc56fb](https://github.com/MikeGrier/windows-threadpool-sys/commit/1dc56fb7fba9b6e4f005097d8d49471608f9ab70))
* **durable-ioring:** hold every phase after construction at zero allocations ([cc84c45](https://github.com/MikeGrier/windows-threadpool-sys/commit/cc84c45735d6f6b16153a2eadbf7161679794f00))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rc-errors bumped from 0.0.1 to 0.0.2
    * fixed-hash-table bumped from 0.0.1 to 0.0.2

## [0.3.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-v0.2.0...durable-ioring-v0.3.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring's target and submit traits gain a declared access and carry it in a typed Key<A>, Diagnostic and DiagnosticKind gain the begin/end pair variants, and a sink is given every pair's end; win-durable-ioring asks for file access as a typed set of named rights, takes data access as the type parameter of every handle it opens, takes a share mode as a typed set, takes OpenedFile::open's path as a Wtf16String, uses Win32's own disposition enum, and removes HandleKindQuery and HandleKind.

### Features

* **durable-ioring:** a typed Key&lt;A&gt; carries a target's access, every sink takes the begin/end pairs, and the ETW provider writes each pair as one activity ([341858e](https://github.com/MikeGrier/windows-threadpool-sys/commit/341858ed566c12a38bfde2dd06a7d9c1a9da1a09))

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-v0.1.0...durable-ioring-v0.2.0) (2026-10-16)


### ⚠ BREAKING CHANGES

* **durable-ioring:** ErrorCode::from_win32 and ErrorCode::win32 are removed.

### Features

* **durable-ioring:** ErrorCode no longer wraps or reads Win32 codes ([5291eaa](https://github.com/MikeGrier/windows-threadpool-sys/commit/5291eaacaafa42294559621694efd0a995c48948))

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-v0.0.1...durable-ioring-v0.1.0) (2026-10-15)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring no longer contains Dioring, the Windows backend, delivery, the readiness check or the Windows re-exports; they are win-durable-ioring's. ErrorCode::of, the default DioringIds time type and the TimeBase bound on a win-time-sys clock are replaced as described above.
* **durable-ioring:** add pumped delivery, the consumer's own thread waiting on the backend's notification and draining

### Features

* **durable-ioring:** add pumped delivery, the consumer's own thread waiting on the backend's notification and draining ([d3832e7](https://github.com/MikeGrier/windows-threadpool-sys/commit/d3832e742df0e7c5161ff6141bffb9b60bb9b874))
* **durable-ioring:** add signalled delivery, the pool telling the consumer through an event while the consumer drains ([6691c23](https://github.com/MikeGrier/windows-threadpool-sys/commit/6691c23018d48c8cb02a1490a9ab0f8f1c8d056f))


### Bug Fixes

* **durable-ioring:** the engine rings the doorbell, and tells the consumer once when it fails ([dfebddf](https://github.com/MikeGrier/windows-threadpool-sys/commit/dfebddf7045d3cc00e9016e58a709f168df2f156))
* **durable-ioring:** the in-memory backend refuses a write a Vec cannot hold, reports one it cannot grow for, and keeps its registered buffers ([b56236d](https://github.com/MikeGrier/windows-threadpool-sys/commit/b56236deae38a9472304eccdf8586a8c87411b02))
* **win-durable-ioring:** end and retire refuse a lineage handle the instance did not mint ([00fb4a4](https://github.com/MikeGrier/windows-threadpool-sys/commit/00fb4a4bde839651646eeb31c177f465076c9a73))


### Code Refactoring

* **durable-ioring:** split the Windows driver out into win-durable-ioring ([e8ff929](https://github.com/MikeGrier/windows-threadpool-sys/commit/e8ff92981e05304c897a20e0836778c5f209f8c5))
