# Changelog

All notable changes to this project will be documented in this file.

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-threadpool-sys-v0.1.4...windows-threadpool-sys-v0.2.0) (2026-10-04)


### ⚠ BREAKING CHANGES

* **threadpool:** delete the inline hooking facility; ship no undocumented APIs
* **threadpool:** restore TimerMember's Send and Sync
* **threadpool:** no public API changes, but `self-heal`'s observable trace changes shape -- `repair-unnecessary` is gone and `repair-in-flight` is new, and a pool is no longer credited as healthy by an unrelated callback dispatching on it.
* **threadpool:** `CleanupGroup::close_members` no longer takes a `bool`. Draining is `close_members()`; cancelling is `close_members_cancelling()` behind `self-heal`, or `close_members_cancelling_no_heal_tracking()` unsafely without it. `trace::poke_completion_ports` is now an `unsafe fn`.
* **threadpool:** trace::worker_factory_snapshot returns Vec<WorkerFactorySnapshot> rather than Vec<(usize, u32, u32, u32)>.
* **threadpool:** cancel_pending is renamed try_cancel_pending to say in its name that it is best effort, and ThreadpoolIo's Drop no longer merely reports a skipped rundown. Every type that drains now exposes stop_and_drain, so a caller can pay the blocking cost at a point they choose rather than meeting it in Drop.

### Features

* **threadpool:** delete the inline hooking facility; ship no undocumented APIs ([1eca1e1](https://github.com/MikeGrier/windows-threadpool-sys/commit/1eca1e16423ef2841f510662af898e5d712a8e5c))
* **threadpool:** detect an arity change on the one hook that reads its arguments ([e3db01d](https://github.com/MikeGrier/windows-threadpool-sys/commit/e3db01d92a40926d9f5a4f2a44e065b00cc0ca85))
* **threadpool:** fail closed on enumeration, retry a refused quiesce, and gate both cancelling teardowns ([127700d](https://github.com/MikeGrier/windows-threadpool-sys/commit/127700d889379da559b93793bc9b76ab372f0687))
* **threadpool:** retry a repair the pool has not taken, then stop or keep trying ([e53899b](https://github.com/MikeGrier/windows-threadpool-sys/commit/e53899bb9ac54203d1091c40a8aa3c43b1899717))
* **threadpool:** teardown drains rather than cancels, and reports what it had to drain ([567702b](https://github.com/MikeGrier/windows-threadpool-sys/commit/567702b3f9b56138004d1c49cc6282188dea2b31))


### Bug Fixes

* **threadpool:** arm the exception assertion, stop the healer parking, and narrow the writable window ([57e59ec](https://github.com/MikeGrier/windows-threadpool-sys/commit/57e59ecb790c24866f3ade86557031474c74c197))
* **threadpool:** clear the residue two earlier deletions left behind ([e9913a1](https://github.com/MikeGrier/windows-threadpool-sys/commit/e9913a14d247699873e051e641876a7af8516922))
* **threadpool:** close a cancel-time use-after-free, unify retirement, and check the patch window ([799d509](https://github.com/MikeGrier/windows-threadpool-sys/commit/799d509eb7ad2953d2cadfe528b5d52d093fa764))
* **threadpool:** count outstanding repairs, publish stamps with fetch_max, and stop a failed claim re-registering ([430b2db](https://github.com/MikeGrier/windows-threadpool-sys/commit/430b2dbd017428a6154d11305b30266f9153e7cd))
* **threadpool:** derive pool health from three stamps instead of a mark that could lose a cancellation ([0d3ced8](https://github.com/MikeGrier/windows-threadpool-sys/commit/0d3ced89a2c76017b9ecad6eec7f8a14abf93e3f))
* **threadpool:** drain the repair work object before closing it ([7fb2861](https://github.com/MikeGrier/windows-threadpool-sys/commit/7fb286107fba70be9382debbcc6e327bcefacbd0))
* **threadpool:** enforce the one-thread hook-installation window at the installer ([b539085](https://github.com/MikeGrier/windows-threadpool-sys/commit/b539085824a43fe5ddd005289375e74d1d07ac33))
* **threadpool:** give each hook its stub's arity, and make the quiesce see threads it never suspended ([16c95b1](https://github.com/MikeGrier/windows-threadpool-sys/commit/16c95b129e128266089022c8d13130c281d958ad))
* **threadpool:** give the periodic timer one drain body instead of two ([64a3b8f](https://github.com/MikeGrier/windows-threadpool-sys/commit/64a3b8f5ffcac96391510005046fd1e213d4cab3))
* **threadpool:** handle the failures the patch path assumed away ([f727371](https://github.com/MikeGrier/windows-threadpool-sys/commit/f7273711adc8e06ab3ce9f8baf50630ac02038c4))
* **threadpool:** mark a cancelling group release's repairs after the cancellation ([4991bb0](https://github.com/MikeGrier/windows-threadpool-sys/commit/4991bb0a03ec9b4e34e052a1cf193e9062c35610))
* **threadpool:** name the worker-factory snapshot fields, and make the lifecycle trace test run ([d664fc9](https://github.com/MikeGrier/windows-threadpool-sys/commit/d664fc9f6a866d712e271803e0bbb475f7a9b527))
* **threadpool:** publish the submission stamp monotonically, and drain only where an obligation survives ([19bb98c](https://github.com/MikeGrier/windows-threadpool-sys/commit/19bb98cc439bfa22021f8862afb74795f7a64c9e))
* **threadpool:** record the drain obligation before the arming is published ([6e0a1cd](https://github.com/MikeGrier/windows-threadpool-sys/commit/6e0a1cd8a5b0378ad353e575d7741bec57c03b81))
* **threadpool:** register a pool at cancel time rather than giving up on it ([eeabeca](https://github.com/MikeGrier/windows-threadpool-sys/commit/eeabecaa182cc93d2f5addade6c15179fff43565))
* **threadpool:** release a trampoline page when its install batch is refused ([5882dad](https://github.com/MikeGrier/windows-threadpool-sys/commit/5882dad99af5eaee80d4f0d23c0e33886acf7a89))
* **threadpool:** restore page protection per page, and stop three tests passing without testing ([21f63b4](https://github.com/MikeGrier/windows-threadpool-sys/commit/21f63b46be04721e2d1a31677a7840208fd24d53))
* **threadpool:** restore TimerMember's Send and Sync ([bc11d0a](https://github.com/MikeGrier/windows-threadpool-sys/commit/bc11d0a30a084cf94786cc3496f654a0b3e3a2f6))
* **threadpool:** serialize the hook suspension, and stop the canary reading a patched stub ([a609192](https://github.com/MikeGrier/windows-threadpool-sys/commit/a60919290d2a001d63ec2743cda7155883a86b11))
* **threadpool:** settle the drain obligation on a cancel, and stop cancelling tests from risking the default pool ([189bdaf](https://github.com/MikeGrier/windows-threadpool-sys/commit/189bdafddd2a8db4c93d01359ee8a9481b3926fa))
* **threadpool:** submit the repair inline when the healer cannot be started ([416a64e](https://github.com/MikeGrier/windows-threadpool-sys/commit/416a64e18bdd94f5d8ffec8b4b84b002663f94c9))
* **threadpool:** take no process lock while the other threads are suspended ([105a66c](https://github.com/MikeGrier/windows-threadpool-sys/commit/105a66c311e8038e0791fc147c8bb09373da44c9))


### Performance Improvements

* **threadpool:** evict the trace buffer in constant time ([cc15bdf](https://github.com/MikeGrier/windows-threadpool-sys/commit/cc15bdf783fc3255a3e113f5775c619f5858de66))

## [0.1.4](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-threadpool-sys-v0.1.3...windows-threadpool-sys-v0.1.4) (2026-09-25)


### Features

* **threadpool:** add a trace for defects that only appear under concurrency ([2522bbf](https://github.com/MikeGrier/windows-threadpool-sys/commit/2522bbf6e54db5d4fc592ffd6daade09ec28e852))


### Bug Fixes

* **ioring:** address PR [#108](https://github.com/MikeGrier/windows-threadpool-sys/issues/108) review, and record what one finding uncovered ([04c329b](https://github.com/MikeGrier/windows-threadpool-sys/commit/04c329b3622dd0191e8103984def6f25ee7f65bb))
* **ioring:** resolve the intra-doc links this branch broke ([02bb88f](https://github.com/MikeGrier/windows-threadpool-sys/commit/02bb88fbdcc463a3913e9ee06e4b359312b9d4ee))

## [0.1.3](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-threadpool-sys-v0.1.2...windows-threadpool-sys-v0.1.3) (2026-08-27)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-overlapped-io-sys bumped from 0.1.2 to 0.1.3

## [0.1.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-threadpool-sys-v0.1.1...windows-threadpool-sys-v0.1.2) (2026-08-24)


### Bug Fixes

* **build:** use table headers for windows-sys so release-please can parse the manifests ([76a0d53](https://github.com/MikeGrier/windows-threadpool-sys/commit/76a0d53f3b06db73d6a2567933f66bcd4edac260))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-overlapped-io-sys bumped from 0.1.1 to 0.1.2

## [0.1.0] - 2026-08-16

- Specialized the repository and package metadata for `windows-threadpool-sys`.
- Established the initial crate and documentation for a memory-safe Windows
	thread pool API.
