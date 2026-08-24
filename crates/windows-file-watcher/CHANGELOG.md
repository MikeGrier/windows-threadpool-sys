# Changelog

## [1.0.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-watcher-v1.0.0...windows-file-watcher-v1.0.1) (2026-08-24)


### Bug Fixes

* **file-watcher:** stop asserting creation-time order in the multi-subscription test ([b478b67](https://github.com/MikeGrier/windows-threadpool-sys/commit/b478b679b889bdfc3a5738b2f5d5945efccf5a8a))

## [1.0.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-watcher-v0.1.0...windows-file-watcher-v1.0.0) (2026-08-24)


### ⚠ BREAKING CHANGES

* **file-watcher:** per-subscription volume-change confirmation (M12)
* **file-watcher:** surface FaultDetail (classification + raw code) on every fault report

### Features

* **file-watcher:** add a real Barrier primitive to the scenario model ([204d1b2](https://github.com/MikeGrier/windows-threadpool-sys/commit/204d1b287c782a4bd26e290d880ff424253d43c2))
* **file-watcher:** per-subscription volume-change confirmation (M12) ([2a7feba](https://github.com/MikeGrier/windows-threadpool-sys/commit/2a7feba3842790cc315bb81369c898d421f75483))
* **file-watcher:** reopen by file reference and re-key coalesced watchers (M11) ([9fe0fda](https://github.com/MikeGrier/windows-threadpool-sys/commit/9fe0fdafaf41378aab669ad43254b4a341e136aa))
* **file-watcher:** surface FaultDetail (classification + raw code) on every fault report ([9cfb6ea](https://github.com/MikeGrier/windows-threadpool-sys/commit/9cfb6eaa770e19bafbb2dded5bdba0cec1ea6ac8))


### Bug Fixes

* **file-watcher:** accept even barrier reuse, fix lock-order doc, clean up orphaned volume-change subscriptions ([339c708](https://github.com/MikeGrier/windows-threadpool-sys/commit/339c7086e022c808a7a0e05e45f5c0ead17171d0))
* **file-watcher:** address remaining PR [#20](https://github.com/MikeGrier/windows-threadpool-sys/issues/20) review findings ([87c7049](https://github.com/MikeGrier/windows-threadpool-sys/commit/87c7049d823000c96403fe53dd3597312b31067e))
* **file-watcher:** bound Barrier::wait against the harness deadline ([b47a293](https://github.com/MikeGrier/windows-threadpool-sys/commit/b47a2939fd74852862c5278a3466f22da5921fa5))
* **file-watcher:** break a StandingHold-Shared reference cycle ([7b9dc26](https://github.com/MikeGrier/windows-threadpool-sys/commit/7b9dc26c3dfee8522095543777d4ba665b0e6c4c))
* **file-watcher:** compare volume identity by serial, not mutable label/fs name ([9d51682](https://github.com/MikeGrier/windows-threadpool-sys/commit/9d5168216b6215a8f7e7c43e5569e65387951909))
* **file-watcher:** do not bypass the fault protocol when a route addition widens reach while already faulted ([9f642eb](https://github.com/MikeGrier/windows-threadpool-sys/commit/9f642eb2b81db81fd1b43d8127bf017c93c3f42c))
* **file-watcher:** fix a lost volume-change answer race and a rekey route drop ([83cf922](https://github.com/MikeGrier/windows-threadpool-sys/commit/83cf922ce075d8bf828b51b09c17abac552c3836))
* **file-watcher:** fix a missing tally and a Barrier hang risk in the scenario harness ([c6c4cb1](https://github.com/MikeGrier/windows-threadpool-sys/commit/c6c4cb113a1386f5878a5b55254a4ac0f7a90be0))
* **file-watcher:** multiply Repeat counts into barrier-use tallies, close a DeadlineBarrier reset race ([77766e4](https://github.com/MikeGrier/windows-threadpool-sys/commit/77766e4a877f24decb0b7908a7cc3bfa9d37d42e))
* **file-watcher:** serialize the reopen transaction with a dedicated lock ([af3e5e1](https://github.com/MikeGrier/windows-threadpool-sys/commit/af3e5e1a0ebc050c3b9f311d82a00476d51ebfde))
* **file-watcher:** stop a reopen race and fault-shadowed Established notice ([fc3b175](https://github.com/MikeGrier/windows-threadpool-sys/commit/fc3b1751e014a35b97fa937e60914d3e78751700))
* **file-watcher:** stop two StandingSlot reservation-accounting races ([07d4b75](https://github.com/MikeGrier/windows-threadpool-sys/commit/07d4b75cae618d5ef71da1a75f5c218d2e6052cc))
* **file-watcher:** use inline code instead of a broken private intra-doc link ([d7790c0](https://github.com/MikeGrier/windows-threadpool-sys/commit/d7790c0ef757f3134d4019571f6b1461b337cff5))
* **file-watcher:** use ordinal Unicode case folding for file-route matching ([4bcd551](https://github.com/MikeGrier/windows-threadpool-sys/commit/4bcd5512cdd31164c1f80be9ee5089266a7b758a))
