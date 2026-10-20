# Changelog

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-multi-file-tail-watcher-v0.1.3...win-multi-file-tail-watcher-v0.2.0) (2026-10-19)


### ⚠ BREAKING CHANGES

* **tail-watcher:** CreateError has a new variant, so an exhaustive match on it no longer compiles. It is #[non_exhaustive] from now on, so the next cause is additive.

### Features

* **tail-watcher:** a queue the allocator will not give is CreateError::Allocation, not a refused capacity ([3bcd90e](https://github.com/MikeGrier/windows-threadpool-sys/commit/3bcd90e539c0850d824642410c8dbfe266c6409b))
* **tail-watcher:** the public reading layer, reader::TailReader ([6bac816](https://github.com/MikeGrier/windows-threadpool-sys/commit/6bac816c7032fbeb4e8adbc9691e527ad50a4def))
* **tail-watcher:** the reading layer is public as reader::TailReader ([f750fba](https://github.com/MikeGrier/windows-threadpool-sys/commit/f750fba17557aa192fcebde9fbe3c0179ccb2d28))
* **tail-watcher:** the reading layer's entries are public in the reader module ([b03c107](https://github.com/MikeGrier/windows-threadpool-sys/commit/b03c1074f747cd3f89129124f3d46ff1b67f1780))
* **tail-watcher:** the reading layer's options, checked before anything is allocated ([69ef06c](https://github.com/MikeGrier/windows-threadpool-sys/commit/69ef06c3a779ad937bd270b35d6d546094bb7ef2))


### Bug Fixes

* **tail-watcher:** a failure to read the ring's limits is a ring failure, not a refused size ([94e4b26](https://github.com/MikeGrier/windows-threadpool-sys/commit/94e4b264a4bc5b32ba06b3116ae12b8f8bb37f9b))
* **tail-watcher:** a poll before the previous one is complete fails ([cc84286](https://github.com/MikeGrier/windows-threadpool-sys/commit/cc84286b12afd4740006a7f4514e0dc2c3a0e2ac))
* **tail-watcher:** a poll refused as in flight wakes the reading engine ([99a4062](https://github.com/MikeGrier/windows-threadpool-sys/commit/99a4062b26210bff389d4dfed7ed1b0edaa53909))
* **tail-watcher:** CreateError words its messages for a submission queue, and two public docs are corrected ([9e78795](https://github.com/MikeGrier/windows-threadpool-sys/commit/9e787958d8f0e0edeee311c7eed066d9b71e0944))
* **tail-watcher:** ReadSq::capacity does not wait for a drain, and a joined design-notes bullet is split again ([0d79524](https://github.com/MikeGrier/windows-threadpool-sys/commit/0d795249763bb35641d1505a94ba1609614357ce))

## [0.1.3](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-multi-file-tail-watcher-v0.1.2...win-multi-file-tail-watcher-v0.1.3) (2026-10-18)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-waitable-queues bumped from 0.1.1 to 0.1.2

## [0.1.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-multi-file-tail-watcher-v0.1.1...win-multi-file-tail-watcher-v0.1.2) (2026-10-17)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.2.2 to 0.3.0
    * windows-ioring-sys bumped from 0.7.2 to 0.8.0
    * windows-waitable-queues bumped from 0.1.0 to 0.1.1

## [0.1.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-multi-file-tail-watcher-v0.1.0...win-multi-file-tail-watcher-v0.1.1) (2026-10-17)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-overlapped-io-sys bumped from 0.2.0 to 0.3.0
    * windows-threadpool-sys bumped from 0.2.1 to 0.2.2
    * windows-ioring-sys bumped from 0.7.1 to 0.7.2

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-multi-file-tail-watcher-v0.0.1...win-multi-file-tail-watcher-v0.1.0) (2026-10-17)


### Features

* **tail-watcher:** a diagnostic sink the caller supplies ([4ffafce](https://github.com/MikeGrier/windows-threadpool-sys/commit/4ffafce0f15d29be08cdf85ca700a8b2f0a7c469))
* **tail-watcher:** a poll stopped on a full queue resumes when the caller rings the resume doorbell ([54b014f](https://github.com/MikeGrier/windows-threadpool-sys/commit/54b014f81b8f7bb2f3cb06f486cfca8aec472c58))
* **tail-watcher:** a terminal entry kind in the SQ ([f9bc33a](https://github.com/MikeGrier/windows-threadpool-sys/commit/f9bc33a1202196d1a5a51671eeeacf26b5d3581d))
* **tail-watcher:** a terminal read failure is delivered on the client's SQ ([468ebad](https://github.com/MikeGrier/windows-threadpool-sys/commit/468ebada6cb2908b051880c169c06e9c774d5816))
* **tail-watcher:** poll runs the engine on the thread pool, one poll at a time ([89f777d](https://github.com/MikeGrier/windows-threadpool-sys/commit/89f777d66e5d12fe9e56badc60269d12e0dcfd3a))
* **tail-watcher:** shutdown stops the watcher for good and poll is refused after it ([2b55128](https://github.com/MikeGrier/windows-threadpool-sys/commit/2b551286977fb33540109740b3a25189da124873))


### Bug Fixes

* **tail-watcher:** answer a refused read, document the callback hazards, and release as 0.1.0 ([ba9bbd8](https://github.com/MikeGrier/windows-threadpool-sys/commit/ba9bbd8671fa162a8940f00ddcfa99a01f877e18))
