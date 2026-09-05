# Changelog

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
