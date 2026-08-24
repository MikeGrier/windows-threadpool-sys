# Changelog

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-topology-sys-v0.1.0...windows-topology-sys-v0.2.0) (2026-08-24)


### ⚠ BREAKING CHANGES

* reset all crate versions to the lowest available pre-1.0 numbers

### Bug Fixes

* sync all workspace crate versions to match main (ioring 1.0.0, topology 0.2.0) ([a160260](https://github.com/MikeGrier/windows-threadpool-sys/commit/a1602600625bda7b4b0fa5cdcdfebf83728b71e5))
* **topology:** preserve exact integer precision in AttributeValue ([d5c0464](https://github.com/MikeGrier/windows-threadpool-sys/commit/d5c04647b511e6fea2110aaa6fd10f41c97b5fd4))
* **topology:** use inline code instead of broken intra-doc links to a private constant ([b41d86f](https://github.com/MikeGrier/windows-threadpool-sys/commit/b41d86fa69bed9586f7e0d07ff72a865712c0e2a))
* **topology:** use usize::BITS for processor-count limits, fix CacheKind::Other's signed round-trip, reject reserved-key collisions, and fix legacy GroupCount==0 decoding ([63aac30](https://github.com/MikeGrier/windows-threadpool-sys/commit/63aac305704296d456e0a8e700a8247d9e5dc9fd))


### Miscellaneous Chores

* reset all crate versions to the lowest available pre-1.0 numbers ([3299724](https://github.com/MikeGrier/windows-threadpool-sys/commit/329972496de015ca3dfb73b5f8547b2db1250e89))

## Changelog
