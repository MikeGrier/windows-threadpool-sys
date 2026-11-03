# Changelog

## [0.0.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/fixed-hash-table-v0.0.1...fixed-hash-table-v0.0.2) (2026-11-03)


### Features

* **fixed-hash-table:** add FixedHashMap, a map whose capacity is fixed when made and which refuses when full ([684605a](https://github.com/MikeGrier/windows-threadpool-sys/commit/684605a53a75b0e1cec661cdcf178f1b69010018))
* **fixed-hash-table:** add FixedHashSet, a fixed-capacity set over the map ([ec15628](https://github.com/MikeGrier/windows-threadpool-sys/commit/ec1562888db333e9038063cde92fe2cbd53cff9e))
* **fixed-hash-table:** add the entry API, iterators, retain, drain, try_extend and standard traits ([19b7964](https://github.com/MikeGrier/windows-threadpool-sys/commit/19b7964cc294b31eae6496adaee5fd4d274a102d))


### Bug Fixes

* **fixed-hash-table:** keep the table consistent after every item a drain yields ([ec228e2](https://github.com/MikeGrier/windows-threadpool-sys/commit/ec228e21bfc90cad8e471596a848326ad3d4be76))
