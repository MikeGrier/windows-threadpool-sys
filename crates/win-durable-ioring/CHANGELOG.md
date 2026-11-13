# Changelog

## [0.6.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.6.0...win-durable-ioring-v0.6.1) (2026-11-13)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-ioring-sys bumped from 0.9.1 to 0.9.2
    * windows-threadpool-sys bumped from 0.4.0 to 0.4.1
    * windows-namespace-request-sys bumped from 0.5.2 to 0.5.3

## [0.6.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.5.2...win-durable-ioring-v0.6.0) (2026-11-06)


### ⚠ BREAKING CHANGES

* **win-durable-ioring:** a file target's domains were exactly what the consumer declared, and now include the disks of the file's volume. Failure scoping follows: files on one disk share fate, as their disk does.

### Features

* **win-durable-ioring:** derive a file's flush domains from its handle when it is opened ([2e799a1](https://github.com/MikeGrier/windows-threadpool-sys/commit/2e799a1c7ea425e8ec40e6a6b5afd08f241d6243))
* **win-durable-ioring:** predict a directory's flush domains and confirm a file's against the prediction ([79de376](https://github.com/MikeGrier/windows-threadpool-sys/commit/79de37649518d435993638e0d687866a51e1c58a))


### Bug Fixes

* **win-durable-ioring:** derive raw targets' domains from the retained handle ([b119ce3](https://github.com/MikeGrier/windows-threadpool-sys/commit/b119ce36ae9f559e1785edb022f92a7326518876))
* **win-durable-ioring:** raw virtual targets carry their host's flush domains ([6b678e4](https://github.com/MikeGrier/windows-threadpool-sys/commit/6b678e438cafbe0d9119cadaf29d5eff36a65903))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * win-volume-sys bumped from 0.1.2 to 0.1.3
    * windows-namespace-request-sys bumped from 0.5.1 to 0.5.2

## [0.5.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.5.1...win-durable-ioring-v0.5.2) (2026-11-03)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * durable-ioring-events bumped from 0.1.1 to 0.2.0

## [0.5.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.5.0...win-durable-ioring-v0.5.1) (2026-11-03)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-ioring-sys bumped from 0.9.0 to 0.9.1

## [0.5.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.4.0...win-durable-ioring-v0.5.0) (2026-11-03)


### ⚠ BREAKING CHANGES

* **durable-ioring:** a refusal carrying a backend fault passes the backend's report through
* **durable-ioring:** DurableRing loses failure_suspects and failure_markings, UnknownFailure and Entry::Marked are removed, and ConformanceOracle loses suspects and observe_from in favour of observer and reading; a consumer reads a failure's suspect writes and markings through a FailureObserver given at construction.
* **durable-ioring:** win-durable-ioring's Setup gains a failure_observer field.
* **durable-ioring:** give the failure inventory to a visitor  failures() no longer hands out a Vec of copies. On the DurableRing trait and the core it takes a visitor and gives each unresolved failure, in the order observed, as a FailureInfo<'a, V> whose cause is lent for the visit as &'a FailureCause -- the way LineageInfo already lends its description (DI-D-103). Leftover::Failure from close() carries the lent form. The visitor runs under the instance's lock and must not call back into it. Neither the inventory nor a close with failures allocates: the "failures queried" phase of the allocation test is now zero, and its three allocation tags are gone.  win-durable-ioring implements the new signature in Dioring, the delivery wrapper and the fake; its tests count the inventory, and its close helper keeps failure identities. The core's tests read the inventory through a test-only failures_listed() that copies each out, so their assertions kept their shape.  Swept "failures()"/"FailureInfo"/"inventory" across CONTRACT.md, DESIGN-NOTES.md, both READMEs and both crates' src: CONTRACT.md "The inventory" (1), DI-D-103 as-built (1), the trait and core rustdoc (2), win-durable-ioring's README example (1); API.md left as the superseded sketch.  Sabotage: one entry added (the core's inventory visiting nothing, caught by the driver's tests) and the two existing inventory entries re-swept; all as declared.  BREAKING CHANGE: DurableRing::failures and DurableIoCore::failures take a visitor instead of returning Vec[FailureInfo]; FailureInfo gains a lifetime and lends its cause.  Completed item: DI-23.12.3.7: The inventory as a visitor (DI-D-103, DI-D-108): failures() gives each unresolved failure to a visitor, lending its cause, rather than handing out a Vec of copies, as the lineages and the holds already are; close() lends its failures the same way.  Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
* **durable-ioring:** Failed loses suspect; Abandoned and Healed carry only the failure; FailureInfo loses suspect and markings; SuspectSet is removed; DurableRing gains failure_suspects and failure_markings; a resolved failure's holds, and an import's count, are let go when its resolution entry is popped; the oracle requires each Failed's suspect set.
* **durable-ioring:** LineageInfo is LineageInfo<'a, V> with description: Option<&'a str>, and Leftover is Leftover<'a, V, B, C>. DurableRing::lineages and close take visitors over the lent types, so a visitor written as a bare function over one lifetime, such as drop, no longer fits; |_| {} does.
* **durable-ioring:** LineageHandle::mint returns None when its home is gone or has no slot free, and the driver's Home trait gains slots(). A handle whose home is gone, or a detached one, counts no copies, so it is never the last.
* **durable-ioring:** DurableRing::lineages and close take a visitor and return nothing; close gives a Leftover enum in place of Leftovers; target_holds and lineage_holds are new required trait methods; IoTargetBusy loses lowest_gates and uncovered_through and its type parameter, and its failures and LineageBusy's are counts; RemoveTargetError loses its type parameter; the engine's take_requests is drain_requests.
* **durable-ioring:** DurableIoCore::new and with_provider take a Capacity and return a Result; DuplicateDomain is replaced by ConstructionError; mint_lineage returns a Result; import_failure returns ImportError; PushRefusal and AddTargetCause gain AtCapacity; Setup gains a capacity field and SetupRefusal gains Construction and AtCapacity.
* **durable-ioring:** IoFault.error is an ErrorReport, not an Arc of io::Error; DomainCompletion::fail and DomainOutcome::Failed take an ErrorReport; DurableRing, RegisteredBufferRing and Waitable report IoFault where they reported io::Error; PumpedSetup's error is an IoFault; scripted_fault takes a static str. In win-durable-ioring, DeliveryEvent::Failure carries an IoFault and the registered-buffer methods of DeliveryHandle report IoFault.

### Features

* **durable-ioring:** a fault's report is an rc-errors ErrorReport, and io::Error leaves the portable boundary ([92c34e7](https://github.com/MikeGrier/windows-threadpool-sys/commit/92c34e799d0740831ee667d5f742681a630f1f3a))
* **durable-ioring:** a refusal carrying a backend fault passes the backend's report through ([f4123ef](https://github.com/MikeGrier/windows-threadpool-sys/commit/f4123efd7e95476f824c30517419877b4a5cac6b))
* **durable-ioring:** count lineage handles' copies in slots made at construction ([5c07f50](https://github.com/MikeGrier/windows-threadpool-sys/commit/5c07f50f49111834e98de020f5c581dbae09e264))
* **durable-ioring:** declare a capacity at construction and refuse past it ([81ab43b](https://github.com/MikeGrier/windows-threadpool-sys/commit/81ab43bbfe06b1d01dd0d7532da4b55e5065e5f7))
* **durable-ioring:** give the failure inventory to a visitor  failures() no longer hands out a Vec of copies. On the DurableRing trait and the core it takes a visitor and gives each unresolved failure, in the order observed, as a FailureInfo&lt;'a, V&gt; whose cause is lent for the visit as &'a FailureCause -- the way LineageInfo already lends its description (DI-D-103). Leftover::Failure from close() carries the lent form. The visitor runs under the instance's lock and must not call back into it. Neither the inventory nor a close with failures allocates: the "failures queried" phase of the allocation test is now zero, and its three allocation tags are gone.  win-durable-ioring implements the new signature in Dioring, the delivery wrapper and the fake; its tests count the inventory, and its close helper keeps failure identities. The core's tests read the inventory through a test-only failures_listed() that copies each out, so their assertions kept their shape.  Swept "failures()"/"FailureInfo"/"inventory" across CONTRACT.md, DESIGN-NOTES.md, both READMEs and both crates' src: CONTRACT.md "The inventory" (1), DI-D-103 as-built (1), the trait and core rustdoc (2), win-durable-ioring's README example (1); API.md left as the superseded sketch.  Sabotage: one entry added (the core's inventory visiting nothing, caught by the driver's tests) and the two existing inventory entries re-swept; all as declared.  BREAKING CHANGE: DurableRing::failures and DurableIoCore::failures take a visitor instead of returning Vec[FailureInfo]; FailureInfo gains a lifetime and lends its cause.  Completed item: DI-23.12.3.7: The inventory as a visitor (DI-D-103, DI-D-108): failures() gives each unresolved failure to a visitor, lending its cause, rather than handing out a Vec of copies, as the lineages and the holds already are; close() lends its failures the same way.  Co-authored-by: Copilot &lt;223556219+Copilot@users.noreply.github.com&gt; ([cb5b55e](https://github.com/MikeGrier/windows-threadpool-sys/commit/cb5b55e6106f4f3a0525a2cd1c2743eb5f5402bb))
* **durable-ioring:** hand the clean paths' collections to visitors ([a39a6ce](https://github.com/MikeGrier/windows-threadpool-sys/commit/a39a6ce01d53de0b9617d734b36079521f6c2a01))
* **durable-ioring:** keep a lineage's description as given and lend it to visitors ([cd5d957](https://github.com/MikeGrier/windows-threadpool-sys/commit/cd5d95746009bbb67b575175f5cd265b6fd0b95f))
* **durable-ioring:** lend a failure's records until its resolution is popped ([d92d2a5](https://github.com/MikeGrier/windows-threadpool-sys/commit/d92d2a525653cd95df3632320ade0caac9ece594))
* **durable-ioring:** read a failure's detail only through its observer ([8614f37](https://github.com/MikeGrier/windows-threadpool-sys/commit/8614f37ad6da5f49dcc71b141f43a70c80ba8f25))
* **durable-ioring:** tell a failure observer given at construction ([fab1810](https://github.com/MikeGrier/windows-threadpool-sys/commit/fab1810ef63c10a8575a9485b555d20f2bcb7c0c))


### Bug Fixes

* **win-durable-ioring:** seal and pop as the blocking-push demonstration goes, so it can write past its capacity ([994df28](https://github.com/MikeGrier/windows-threadpool-sys/commit/994df28dbc7da8b4fe397bd377510256a150a839))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * durable-ioring bumped from 0.3.0 to 0.4.0
    * durable-ioring-events bumped from 0.1.0 to 0.1.1

## [0.4.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.3.2...win-durable-ioring-v0.4.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring's target and submit traits gain a declared access and carry it in a typed Key<A>, Diagnostic and DiagnosticKind gain the begin/end pair variants, and a sink is given every pair's end; win-durable-ioring asks for file access as a typed set of named rights, takes data access as the type parameter of every handle it opens, takes a share mode as a typed set, takes OpenedFile::open's path as a Wtf16String, uses Win32's own disposition enum, and removes HandleKindQuery and HandleKind.

### Features

* **durable-ioring:** a typed Key&lt;A&gt; carries a target's access, every sink takes the begin/end pairs, and the ETW provider writes each pair as one activity ([341858e](https://github.com/MikeGrier/windows-threadpool-sys/commit/341858ed566c12a38bfde2dd06a7d9c1a9da1a09))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * durable-ioring bumped from 0.2.0 to 0.3.0
    * windows-ioring-sys bumped from 0.8.0 to 0.9.0
    * win-sync-sys bumped from 0.0.2 to 0.1.0
    * windows-threadpool-sys bumped from 0.3.0 to 0.4.0
    * win-volume-sys bumped from 0.1.1 to 0.1.2
    * windows-namespace-request-sys bumped from 0.5.0 to 0.5.1
    * win-handles-sys bumped from 0.1.0 to 0.1.1
    * win-debugging-sys bumped from 0.0.1 to 0.0.2
    * durable-ioring-events bumped from 0.0.1 to 0.1.0

## [0.3.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.3.1...win-durable-ioring-v0.3.2) (2026-10-20)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-namespace-request-sys bumped from 0.4.2 to 0.5.0

## [0.3.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.3.0...win-durable-ioring-v0.3.1) (2026-10-20)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * win-volume-sys bumped from 0.1.0 to 0.1.1

## [0.3.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.2.1...win-durable-ioring-v0.3.0) (2026-10-17)


### ⚠ BREAKING CHANGES

* **win-durable-ioring:** the CallbackEnviron accepted by DeliveryMode::Pool, DeliveryMode::Signalled and EntryDelivery::new now comes from windows-threadpool-sys 0.3.

### Features

* **win-durable-ioring:** release as 0.3.0, paired with windows-threadpool-sys 0.3 ([1bbea90](https://github.com/MikeGrier/windows-threadpool-sys/commit/1bbea905b3ded93d7e18a1595ceb2fb2d6d9e24b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-ioring-sys bumped from 0.7.2 to 0.8.0
    * windows-threadpool-sys bumped from 0.2.2 to 0.3.0
    * windows-namespace-request-sys bumped from 0.4.1 to 0.4.2

## [0.2.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.2.0...win-durable-ioring-v0.2.1) (2026-10-17)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-ioring-sys bumped from 0.7.1 to 0.7.2
    * windows-threadpool-sys bumped from 0.2.1 to 0.2.2
    * windows-namespace-request-sys bumped from 0.4.0 to 0.4.1

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.1.0...win-durable-ioring-v0.2.0) (2026-10-16)


### ⚠ BREAKING CHANGES

* **namespace-request:** OpenFile::new, WatchDirectory::new and ResolveFullPath::new take a Wtf16String and return a Result; prepare, PreparedPath, PathError, PathFailure and the path module are removed.
* **win-durable-ioring:** Win32FileTarget::new takes an OpenedFile, not a SharedHandle, and the warranty, geometry and capabilities setters are removed.

### Features

* **namespace-request:** open, watch and resolve by a Wtf16String, and retire prepare ([3083c05](https://github.com/MikeGrier/windows-threadpool-sys/commit/3083c050319c9bbee663c1f784ebbdd6084c2be2))
* **win-durable-ioring:** a file target is made from a file this crate opened ([b49d421](https://github.com/MikeGrier/windows-threadpool-sys/commit/b49d4212150c67d2fc67bd9da96b2e9511798557))
* **win-durable-ioring:** a raw volume is a file target, opened here and its disks found ([81dbdb7](https://github.com/MikeGrier/windows-threadpool-sys/commit/81dbdb78ca7a21927ae226de9db3794fa42656a5))
* **win-durable-ioring:** add a whole physical disk as a file target ([fd281df](https://github.com/MikeGrier/windows-threadpool-sys/commit/fd281df87aeed800348d406657497ddf659c3898))
* **win-durable-ioring:** add error_code_from_win32 and win32_code_of ([0758381](https://github.com/MikeGrier/windows-threadpool-sys/commit/075838108d3fe8765f6ba293b1375739411c0946))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * durable-ioring bumped from 0.1.0 to 0.2.0
    * windows-namespace-request-sys bumped from 0.3.0 to 0.4.0
    * win-error-codes bumped from 0.1.0 to 0.0.2

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.0.1...win-durable-ioring-v0.1.0) (2026-10-15)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring no longer contains Dioring, the Windows backend, delivery, the readiness check or the Windows re-exports; they are win-durable-ioring's. ErrorCode::of, the default DioringIds time type and the TimeBase bound on a win-time-sys clock are replaced as described above.

### Features

* **win-durable-ioring:** Dioring::completion_event, a wait-only handle on the ring's own event ([747e46f](https://github.com/MikeGrier/windows-threadpool-sys/commit/747e46f6eb619c388faf2ada9aab44085d1bfb54))


### Bug Fixes

* **win-durable-ioring:** a push only queues, and a failed doorbell or an unreadable ring reaches the consumer ([71f3562](https://github.com/MikeGrier/windows-threadpool-sys/commit/71f35626011941ad59e6a6e3efbfd49b5e86d794))
* **win-durable-ioring:** end and retire refuse a lineage handle the instance did not mint ([00fb4a4](https://github.com/MikeGrier/windows-threadpool-sys/commit/00fb4a4bde839651646eeb31c177f465076c9a73))
* **win-durable-ioring:** Model A's handler is told when the instance's pop fails ([622848e](https://github.com/MikeGrier/windows-threadpool-sys/commit/622848e92dc4b1adbfa1536ea0016f5a6854112a))
* **win-durable-ioring:** refuse a buffer beyond a u32 at check; no safe CompletionEvent from any handle ([f640c2c](https://github.com/MikeGrier/windows-threadpool-sys/commit/f640c2ccb2d883be0055d537e4e51239752bec3d))
* **win-durable-ioring:** report a lineage release the core finds not live ([c98ff3b](https://github.com/MikeGrier/windows-threadpool-sys/commit/c98ff3b56190b408c8935409e18168fed89da0cc))
* **win-durable-ioring:** the backend's ring is an owned one, so a completion it held nothing for is not a case ([ceb3b72](https://github.com/MikeGrier/windows-threadpool-sys/commit/ceb3b720e8054ef8ff711736da8f8d0e077df03d))


### Code Refactoring

* **durable-ioring:** split the Windows driver out into win-durable-ioring ([e8ff929](https://github.com/MikeGrier/windows-threadpool-sys/commit/e8ff92981e05304c897a20e0836778c5f209f8c5))
