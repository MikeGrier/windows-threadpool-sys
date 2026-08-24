# Changelog

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-topology-sys-v0.1.0...windows-topology-sys-v0.2.0) (2026-08-24)


### Features

* **topology:** implement M1, safe enumeration of processor topology ([5da5ff2](https://github.com/MikeGrier/windows-threadpool-sys/commit/5da5ff23d57c1123148880d8fcd1f45af1bda7b2))
* **topology:** implement M2, the open-kinded topology description ([bb0c627](https://github.com/MikeGrier/windows-threadpool-sys/commit/bb0c627ffe840feb857b0cc44cad657dfabe3869))
* **topology:** implement M3, serde behind a default-off feature ([1e8fbd9](https://github.com/MikeGrier/windows-threadpool-sys/commit/1e8fbd95f3a1066867ea56abfab015b90d2a91ba))
* **topology:** implement M4 and fix release-please registration ([79cf8d1](https://github.com/MikeGrier/windows-threadpool-sys/commit/79cf8d1b8a1cf1c0b0ca1fd614b842d531cc5a88))


### Bug Fixes

* **topology:** preserve exact integer precision in AttributeValue ([d5c0464](https://github.com/MikeGrier/windows-threadpool-sys/commit/d5c04647b511e6fea2110aaa6fd10f41c97b5fd4))
* **topology:** use inline code instead of broken intra-doc links to a private constant ([b41d86f](https://github.com/MikeGrier/windows-threadpool-sys/commit/b41d86fa69bed9586f7e0d07ff72a865712c0e2a))
* **topology:** use usize::BITS for processor-count limits, fix CacheKind::Other's signed round-trip, reject reserved-key collisions, and fix legacy GroupCount==0 decoding ([63aac30](https://github.com/MikeGrier/windows-threadpool-sys/commit/63aac305704296d456e0a8e700a8247d9e5dc9fd))
