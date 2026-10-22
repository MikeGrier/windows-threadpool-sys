# Changelog

## [0.3.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-enumeration-sys-v0.2.2...windows-file-enumeration-sys-v0.3.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* **file-enumeration:** Receiver::doorbell_owned is removed and Receiver::doorbell_waitable, which returns a WaitableHandle sharing the ring's event, takes its place; the crate takes the request crate's path contract in place of its own path preparation.

### Features

* **file-enumeration:** the doorbell is a shared win-sync-sys Event, and the crate takes the request crate's path contract ([849959c](https://github.com/MikeGrier/windows-threadpool-sys/commit/849959cb08ed48c062d4aa9a3ded20fe71167636))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.3.0 to 0.4.0
    * windows-namespace-request-sys bumped from 0.5.0 to 0.5.1
    * win-sync-sys bumped from 0.0.2 to 0.1.0
    * win-text-sys bumped from 0.0.1 to 0.0.2

## [0.2.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-enumeration-sys-v0.2.1...windows-file-enumeration-sys-v0.2.2) (2026-10-17)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.2.2 to 0.3.0

## [0.2.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-enumeration-sys-v0.2.0...windows-file-enumeration-sys-v0.2.1) (2026-10-17)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.2.1 to 0.2.2

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-enumeration-sys-v0.1.4...windows-file-enumeration-sys-v0.2.0) (2026-10-16)


### ⚠ BREAKING CHANGES

* **file-enumeration:** Win32Error::from_io returns Option<Win32Error>.

### Bug Fixes

* **file-enumeration:** Win32Error is win_error_codes::Win32Error, which keeps a code of 2^31 or more ([5f014f5](https://github.com/MikeGrier/windows-threadpool-sys/commit/5f014f59dde27fab31a269cc5ab9cfa5a87ebcea))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * win-error-codes bumped from 0.1.0 to 0.0.2

## [0.1.4](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-enumeration-sys-v0.1.3...windows-file-enumeration-sys-v0.1.4) (2026-10-15)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.2.0 to 0.2.1

## [0.1.3](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-enumeration-sys-v0.1.2...windows-file-enumeration-sys-v0.1.3) (2026-10-04)


### Bug Fixes

* **file-enumeration:** drain the session's servicer and engine at teardown ([60281fe](https://github.com/MikeGrier/windows-threadpool-sys/commit/60281fe936a06d542fc64a435f3b0b1259a6f0e7))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.1.4 to 0.2.0

## [0.1.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-enumeration-sys-v0.1.1...windows-file-enumeration-sys-v0.1.2) (2026-09-25)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-threadpool-sys bumped from 0.1.3 to 0.1.4

## [0.1.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-enumeration-sys-v0.1.0...windows-file-enumeration-sys-v0.1.1) (2026-08-28)


### Features

* **file-enumeration:** add the public request, predicate, entry, error, and completion types ([2cefbd5](https://github.com/MikeGrier/windows-threadpool-sys/commit/2cefbd549ae9fd0dc4667fef688852093f7b6d36))
* **file-enumeration:** add the two-ring session and enumeration admission ([4e5b677](https://github.com/MikeGrier/windows-threadpool-sys/commit/4e5b677a7978fb4411c9c67af6cdb9d0da9acc9f))
* **file-enumeration:** assert refill preconditions before classifying failures ([4aea0dd](https://github.com/MikeGrier/windows-threadpool-sys/commit/4aea0ddf9bf920bf3a5b70e6f7a60b48a48b845f))
* **file-enumeration:** bound each quantum and make backpressure lossless ([aa2a2db](https://github.com/MikeGrier/windows-threadpool-sys/commit/aa2a2db5998c8cfa29620af1f29c04817ec47f30))
* **file-enumeration:** open directories and read their first batch ([f45980d](https://github.com/MikeGrier/windows-threadpool-sys/commit/f45980dc36cf30f83e3c5de074ff04dc409a59a0))
* **file-enumeration:** parse native batches and deliver entries ([9aba14e](https://github.com/MikeGrier/windows-threadpool-sys/commit/9aba14e1b5e66dda16c6fb3d2a2b554c8ebe9ca1))
* **file-enumeration:** split the worker from the control authority ([2443f50](https://github.com/MikeGrier/windows-threadpool-sys/commit/2443f5057034399d1c037fa1cc12d1fdf6f4f50e))


### Bug Fixes

* address remaining PR [#44](https://github.com/MikeGrier/windows-threadpool-sys/issues/44) Copilot review findings ([a892245](https://github.com/MikeGrier/windows-threadpool-sys/commit/a89224596f6d849c6714b6b481fd16c3d496ac54))
* **file-enumeration:** address PR [#44](https://github.com/MikeGrier/windows-threadpool-sys/issues/44) review feedback ([22a821a](https://github.com/MikeGrier/windows-threadpool-sys/commit/22a821a059525e9929ea0a62f36bbc902cb4b252))
* **file-enumeration:** close a missed-wakeup race in completion-ring backpressure ([4a080f6](https://github.com/MikeGrier/windows-threadpool-sys/commit/4a080f6f24afaab86930557c87b7099996d7b7ad))
* **file-enumeration:** give integration tests headroom for CI's default thread pool ([18272dd](https://github.com/MikeGrier/windows-threadpool-sys/commit/18272dd066e8a7fab65d050cc614dab13d8ca4ce))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-impersonation-token-sys bumped from 0.1.0 to 0.1.1

## Changelog

All notable changes to this project will be documented in this file. Releases
are generated by release-please from Conventional Commits.
