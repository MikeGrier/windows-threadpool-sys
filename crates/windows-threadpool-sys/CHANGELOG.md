# Changelog

All notable changes to this project will be documented in this file.

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
