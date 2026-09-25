# Changelog

## [0.4.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-ioring-sys-v0.3.1...windows-ioring-sys-v0.4.0) (2026-09-25)


### ⚠ BREAKING CHANGES

* **ioring:** IoRing will become generic over the token payload, so every consumer names the type. Not in this commit -- this records the decision and the plan; M28.3 makes the change.
* NumaBuffer::new takes Option<NumaNode> rather than Option<u32>. windows_ioring_sys::NumaBuffer still resolves -- the type is re-exported, so a path binding is unaffected -- but a call site passing a bare integer must wrap it. That is the newtype doing its job at the one boundary where a wrong u32 silently allocates on the wrong node, which windows-placement-probe documents as a real defect it had.
* **ioring:** ask which cache level partitions the machine, never filter on L3

### Features

* create win-numa-sys and move NumaBuffer into it ([947b252](https://github.com/MikeGrier/windows-threadpool-sys/commit/947b252b065629352b7eeaa1a2000209112ccbac))
* **ioring:** add a bounded pop, generic over the wait the caller supplies ([c6703f3](https://github.com/MikeGrier/windows-threadpool-sys/commit/c6703f3bc22b52f8f14a2f5532ab8a077623d3df))
* **ioring:** build the seeded resolver over the kernel response space ([6e53bbe](https://github.com/MikeGrier/windows-threadpool-sys/commit/6e53bbe23be57d08c0f0e6f08298fb6bd1a59682))
* **ioring:** D-54 -- this crate owns one flush, not durability groups ([4319688](https://github.com/MikeGrier/windows-threadpool-sys/commit/4319688af60af2b4d0e6287a9606e3861deeebc4))
* **ioring:** D-55 -- the ring owns the pending inventory ([fdc3f7f](https://github.com/MikeGrier/windows-threadpool-sys/commit/fdc3f7f8f28f7f68e6928ac26b3560ff7aa9a377))
* **ioring:** decide RS-P-8, a completion may report fewer bytes than requested ([f4d4f0e](https://github.com/MikeGrier/windows-threadpool-sys/commit/f4d4f0e35ae57ba77513986cfa6b32a9ca06216d))
* **ioring:** give epoch-log records a sector stride, both ends ([a8a2a3e](https://github.com/MikeGrier/windows-threadpool-sys/commit/a8a2a3e3f810e0f9fe7e741bdab5bceb5b573f1b))
* **ioring:** hand rundown's retry policy to the caller, and fix an expired wait reported as a failed submit ([2585180](https://github.com/MikeGrier/windows-threadpool-sys/commit/2585180b47e48a7ad3880d6336150d6c79a518eb))
* **ioring:** measure a commit as submit / blocking / deferral ([2b2e75c](https://github.com/MikeGrier/windows-threadpool-sys/commit/2b2e75c74009c4d0263f595f627a07d70ffb2f95))
* **ioring:** open the epoch log NO_BUFFERING | OVERLAPPED over a written extent ([e75a9d7](https://github.com/MikeGrier/windows-threadpool-sys/commit/e75a9d7a80105bcca874854daa9bfe649e887024))
* **ioring:** provide NumaBuffer, and place the epoch-log arena deliberately ([896252e](https://github.com/MikeGrier/windows-threadpool-sys/commit/896252ec650d4b515b183dd28452c185157a167e))
* **ioring:** route the submission-path kernel calls through a seam ([73b60c9](https://github.com/MikeGrier/windows-threadpool-sys/commit/73b60c99202854932785d6f5d6de42f2214374a2))
* **ioring:** spike Pending&lt;T, X&gt; for M23.3, and report what it cannot do ([bc856bd](https://github.com/MikeGrier/windows-threadpool-sys/commit/bc856bde2756738ad73ef46c165a7b53274692e8))
* **ioring:** state that the ring, not the log, is the durability unit ([d845bb2](https://github.com/MikeGrier/windows-threadpool-sys/commit/d845bb28579fdc57da1ff14dba575522c3d331e2))
* **ioring:** state the epoch log's handle requirements and enforce the checkable one ([1b975af](https://github.com/MikeGrier/windows-threadpool-sys/commit/1b975af1ac1c5f2b0bcb29cfeb569232bcdb4521))
* **ioring:** trace the delivery path, and prove the pool is alive when it stalls ([ef3ada4](https://github.com/MikeGrier/windows-threadpool-sys/commit/ef3ada4ef51bb7bd16b78270a6214cbe43ba27a8))


### Bug Fixes

* **ioring:** address PR [#108](https://github.com/MikeGrier/windows-threadpool-sys/issues/108) review, and record what one finding uncovered ([04c329b](https://github.com/MikeGrier/windows-threadpool-sys/commit/04c329b3622dd0191e8103984def6f25ee7f65bb))
* **ioring:** address the two findings Copilot raised without a thread ([95fc3e3](https://github.com/MikeGrier/windows-threadpool-sys/commit/95fc3e3328a8d485fa95678533d0840b4bcb9bf7))
* **ioring:** an expired wait is a result, not an error ([9c9c5ff](https://github.com/MikeGrier/windows-threadpool-sys/commit/9c9c5ff72578c97a345c6cc31836b0e0562db08c))
* **ioring:** arm the delivery wait before signalling the completion event ([6d6956d](https://github.com/MikeGrier/windows-threadpool-sys/commit/6d6956d2a458498531011d7fd6cd9d25cc744bfe))
* **ioring:** ask which cache level partitions the machine, never filter on L3 ([3433fe0](https://github.com/MikeGrier/windows-threadpool-sys/commit/3433fe0cd67dfd07beaf27e307705dbb482b611d))
* **ioring:** bound every wait that could hang, not the two that were named ([665edab](https://github.com/MikeGrier/windows-threadpool-sys/commit/665edab61502791676da300edfcea717f3708190))
* **ioring:** count a strategy's preparation as part of its commit, and re-run M20.6 ([6c012e6](https://github.com/MikeGrier/windows-threadpool-sys/commit/6c012e63dfb921bf0889adf4a598809258fee89e))
* **ioring:** derive the epoch-log sample's free slots instead of tracking them ([834c7af](https://github.com/MikeGrier/windows-threadpool-sys/commit/834c7afab45df7b8344030274d2a0881b72c08e9))
* **ioring:** keep Drop guards silent while already panicking ([74022a3](https://github.com/MikeGrier/windows-threadpool-sys/commit/74022a3900664a705ae756d758d9700d08eef440))
* **ioring:** make bounded_pop wait on a pipe, not on a fast enough device ([8f079ca](https://github.com/MikeGrier/windows-threadpool-sys/commit/8f079cac45d333325774f31451c0258e09dc4c9e))
* **ioring:** resolve the intra-doc links this branch broke ([02bb88f](https://github.com/MikeGrier/windows-threadpool-sys/commit/02bb88fbdcc463a3913e9ee06e4b359312b9d4ee))
* **ioring:** separate the barrier's scope from the flush's in the contract ([4adea67](https://github.com/MikeGrier/windows-threadpool-sys/commit/4adea675b93409026f02ac2b6d3ced98397cc7f6))


### Performance Improvements

* **ioring:** batch an epoch's appends into one submission, and measure the claim ([7c12708](https://github.com/MikeGrier/windows-threadpool-sys/commit/7c12708e1cfc39ecbeff74418f19d45b63e8d35d))
* **ioring:** size the zero-fill chunk from measurement, not habit ([c7bf651](https://github.com/MikeGrier/windows-threadpool-sys/commit/c7bf6510010eed933eff9746055f0f6c028ce210))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.1.3 to 0.1.4

## [0.3.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-ioring-sys-v0.3.0...windows-ioring-sys-v0.3.1) (2026-09-06)


### Bug Fixes

* **ioring:** finish the sweep by reading every barrier mention, not by guessing phrasings ([78ab9fd](https://github.com/MikeGrier/windows-threadpool-sys/commit/78ab9fd3de4f479026934410992193be77bdcd99))
* **ioring:** name whose cost the drain is, everywhere the old vocabulary survived ([4263a2a](https://github.com/MikeGrier/windows-threadpool-sys/commit/4263a2a1e1fd1a94cae18c8f5f2c917ae5947adc))
* **ioring:** pop order witnesses posting order; only execution order is hidden ([275bd1e](https://github.com/MikeGrier/windows-threadpool-sys/commit/275bd1e586acab9e17c21da16876077c307a12fd))
* **ioring:** state the barrier by completion, not by when an operation starts ([255f6d3](https://github.com/MikeGrier/windows-threadpool-sys/commit/255f6d3cc7b9623179fe7995ffeb14f2a58f51c1))
* **ioring:** sweep the withdrawn ring-wide-stall claim through the example and notes ([dfee26b](https://github.com/MikeGrier/windows-threadpool-sys/commit/dfee26b1bc7f0ca167964152f7f644f7e34574f6))
* **ioring:** the covering-flush ordering is observable; say why this oracle skips it ([8813327](https://github.com/MikeGrier/windows-threadpool-sys/commit/8813327df3b37d9de8034382eeef4251901ba337))
* **ioring:** the drain flag is one-sided, and the docs said otherwise ([868768d](https://github.com/MikeGrier/windows-threadpool-sys/commit/868768df8316516742a624a74a5e818b2c791932))

## [0.3.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-ioring-sys-v0.2.0...windows-ioring-sys-v0.3.0) (2026-09-05)


### ⚠ BREAKING CHANGES

* **topology:** reshape the topology model around observed domains

### Features

* **topology:** reshape the topology model around observed domains ([a775600](https://github.com/MikeGrier/windows-threadpool-sys/commit/a77560054ba35c2ca8eb6ea46d7a3efc2390674f))


### Bug Fixes

* **topology:** write test buffers through as_mut_ptr, keep the observed node id ([0f49ab6](https://github.com/MikeGrier/windows-threadpool-sys/commit/0f49ab690cff8b28e9b9ffe7cb21d7fd8957f026))


### Dependencies

* The following workspace dependencies were updated
  * dev-dependencies
    * windows-topology-sys bumped from 0.1.0 to 0.2.0

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-ioring-sys-v0.1.2...windows-ioring-sys-v0.2.0) (2026-08-30)


### ⚠ BREAKING CHANGES

* **ioring:** `RegisteredBuffers::get` now takes `&mut self`. Callers holding the registration behind a shared reference, or holding the returned slice across a submission, must adjust. Appropriate for the 0.2.0 release.
* **ioring:** `EventDelivery::ring() -> &Mutex<IoRing>` is removed in favour of `EventDelivery::scope() -> RingScope`. Callers that locked the mutex and built a `Batch` now call `delivery.scope()` and `scope.batch()`.
* **ioring:** adds the public module `contract`. Additive, but new public surface in a crate already cutting 0.2.0.
* **ioring:** `RegisteredBuffers::get` now returns `io::Result<&[u8]>` rather than `Option<&B>`.
* **ioring:** RegisteredBuffers::get_mut returns io::Result<&mut [u8]> rather than io::Result<&mut B>. Callers writing bytes are unaffected; callers relying on &mut B were relying on the unsound behaviour.
* **ioring:** RegisteredUse is now a struct rather than a newtype over Arc<AtomicUsize>. It is only ever produced by this crate and consumed as an opaque Token payload, so callers that treat it as opaque are unaffected.
* **ioring:** write, write_raw, write_registered and write_registered_raw take a trailing WriteCaching argument; flush and flush_raw take a trailing FlushMode argument. WriteCaching::Cached and FlushMode::Default reproduce the previous behaviour exactly.
* **ioring:** Batch::flush and Batch::flush_raw take FlushCoverage instead of PushOptions. Callers pass FlushCoverage::CoversPrecedingOperations to make preceding writes durable, or FlushCoverage::Unordered for the previous (non-covering) behaviour of PushOptions::default().
* **ioring:** keep the IORING_BUFFER_INFO array alive until the kernel reads it

### Features

* **guard-alloc:** add a guard-page global allocator and calibrate it against D-32 ([983afbc](https://github.com/MikeGrier/windows-threadpool-sys/commit/983afbc1832e974a34302a51406a79a6e2cb28e4))
* **guard-alloc:** fill fresh allocations with a tracked, seeded poison pattern ([36ecd8a](https://github.com/MikeGrier/windows-threadpool-sys/commit/36ecd8ad3d983b6e5cd38d0dde69777f9a5d9614))
* **ioring:** add a fault-injection seam that transforms real completions ([16bb39b](https://github.com/MikeGrier/windows-threadpool-sys/commit/16bb39bd2198b6b502e3f15fd03f58991d839ed2))
* **ioring:** add RingContract, this crate's conservation rules made executable ([bfbb840](https://github.com/MikeGrier/windows-threadpool-sys/commit/bfbb84071cd45de1c174fae84373fcc04a93cabc))
* **ioring:** commit epoch-log records by group commit ([f15e504](https://github.com/MikeGrier/windows-threadpool-sys/commit/f15e50431e99a4951b23167fd33f1da8b95daa82))
* **ioring:** count registered-buffer uses per buffer, and add get_mut ([222bde1](https://github.com/MikeGrier/windows-threadpool-sys/commit/222bde1bf725873c05863434c1c427dd0245ba9c))
* **ioring:** expose the kernel's write flags and flush modes ([1c21394](https://github.com/MikeGrier/windows-threadpool-sys/commit/1c21394cee1e4098ea3d0a5b477efb9ddb31aa4a))
* **ioring:** gate windows-threadpool-sys behind a default-on feature ([d374cc6](https://github.com/MikeGrier/windows-threadpool-sys/commit/d374cc681bd4e1b941b550cf3eee5742af04123a))
* **ioring:** hand back the ring's completion event without surrendering the ring ([0ed5e87](https://github.com/MikeGrier/windows-threadpool-sys/commit/0ed5e87672e863bd27c53898cb83d4f01160ea90))
* **ioring:** implement all three epoch-commit strategies behind one interface ([3bf25bf](https://github.com/MikeGrier/windows-threadpool-sys/commit/3bf25bf9bb4a87e3a744af06230c1419cf42deb7))
* **ioring:** let a RegisteredFile be pushed without unsafe ([d2373b6](https://github.com/MikeGrier/windows-threadpool-sys/commit/d2373b64041bd993dc3ee3e956660be613a5a4ed))
* **ioring:** measure the three commit strategies on the running machine ([8399753](https://github.com/MikeGrier/windows-threadpool-sys/commit/83997533a69fd9f0285fc513a6a451a0b322d983))
* **ioring:** name the ring conditions a consumer has to branch on ([8d51124](https://github.com/MikeGrier/windows-threadpool-sys/commit/8d51124761dd6d1a0ec173e8162292a5261a9bfd))
* **ioring:** order a non-ring FSCTL against ring epochs in the epoch-log sample ([22c11de](https://github.com/MikeGrier/windows-threadpool-sys/commit/22c11de1da2d39363e4a068b3f3a6bc679f7e755))
* **ioring:** replace EventDelivery::ring with a narrowed RingScope ([21f1203](https://github.com/MikeGrier/windows-threadpool-sys/commit/21f120300b58025c4089c1346527f277e8cf5083))
* **ioring:** replay and verify the epoch log against its own contract ([002f87d](https://github.com/MikeGrier/windows-threadpool-sys/commit/002f87d8443ef1b2891607c5083d839b81ff8711))
* **ioring:** run the epoch log's control plane on the thread pool ([e90e90a](https://github.com/MikeGrier/windows-threadpool-sys/commit/e90e90afe98e37cc6a264ab85106354785cafbc1))
* **ioring:** wait the epoch log on its completion event and a shutdown latch ([b96a6be](https://github.com/MikeGrier/windows-threadpool-sys/commit/b96a6be9106f3c585a7a7f03ef20667e1b629347))


### Bug Fixes

* **ioring:** deliver completions queued before EventDelivery handover ([f1b48c8](https://github.com/MikeGrier/windows-threadpool-sys/commit/f1b48c817b6428c611d0c32481f446bc07a89ac9))
* **ioring:** keep the IORING_BUFFER_INFO array alive until the kernel reads it ([66272ff](https://github.com/MikeGrier/windows-threadpool-sys/commit/66272ffdc6e452f14164a532a73c0c4f18c6dc84))
* **ioring:** make RegisteredBuffers::get refuse a buffer a read is landing into ([49d928c](https://github.com/MikeGrier/windows-threadpool-sys/commit/49d928c6438a99b4ca76e2e502a7663cdc1c9a24))
* **ioring:** RegisteredBuffers::get takes &mut self, closing a borrow hole ([7da99a7](https://github.com/MikeGrier/windows-threadpool-sys/commit/7da99a721ec46d32a248125eda02f1359ac88dec))
* **ioring:** remove a dead guard in strategy.rs and correct what catches what ([e33161d](https://github.com/MikeGrier/windows-threadpool-sys/commit/e33161d4cca6dcb2b5ece7494a77afcf33be3ba3))
* **ioring:** require an explicit barrier decision on every flush ([5f8577b](https://github.com/MikeGrier/windows-threadpool-sys/commit/5f8577b42ccd46b1392e6652935b57bcf105df5c))
* **ioring:** require both checkpoint operations to succeed before authorising a reclaim ([982b291](https://github.com/MikeGrier/windows-threadpool-sys/commit/982b2916dc32be7e56a7f24314e5f64146ca41b2))
* **ioring:** stop get_mut handing out the registered buffer itself ([ffed97c](https://github.com/MikeGrier/windows-threadpool-sys/commit/ffed97c42f351d50a77b68fbeb797631e5c5a225))
* **ioring:** unbreak CI clippy, and queue the get() borrow hole as M19 ([a38f754](https://github.com/MikeGrier/windows-threadpool-sys/commit/a38f75409d82854bdf2f6f9a93d263043c0f8809))

## [0.1.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-ioring-sys-v0.1.1...windows-ioring-sys-v0.1.2) (2026-08-27)


### Bug Fixes

* **file-watcher:** derive the resume wake edge from the same quantity has_room tests ([8855e1d](https://github.com/MikeGrier/windows-threadpool-sys/commit/8855e1d4237aa34284f1752f17f1aa937265ed3a))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.1.2 to 0.1.3

## [0.1.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-ioring-sys-v0.1.0...windows-ioring-sys-v0.1.1) (2026-08-24)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.1.1 to 0.1.2

## Changelog
