# Changelog

## [0.1.3](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-watcher-v0.1.2...windows-file-watcher-v0.1.3) (2026-08-29)


### Bug Fixes

* **file-watcher:** assert teardown on the queue, not on a drained count ([140cf3d](https://github.com/MikeGrier/windows-threadpool-sys/commit/140cf3d0a5261899eaaa4341db3bc5cc090d8192))

## [0.1.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-watcher-v0.1.1...windows-file-watcher-v0.1.2) (2026-08-27)


### Features

* **file-watcher:** add ContractChecker, the sequencing rules made executable (M3.2) ([38b4be2](https://github.com/MikeGrier/windows-threadpool-sys/commit/38b4be2c80315ea41a7628233c98836dc26e593d))
* **file-watcher:** add DesyncCause::is_reachable_in and make the generator bind to it (M2.3) ([f95a74f](https://github.com/MikeGrier/windows-threadpool-sys/commit/f95a74f9f78f2262779231493394b16203a108a5))
* **file-watcher:** add DesyncCause::is_terminal and adopt it in every example (M2.2) ([f990575](https://github.com/MikeGrier/windows-threadpool-sys/commit/f9905758028d26b208355e8b1ffe334611f96edf))
* **file-watcher:** add Receiver::has_pending, the predicate the doorbell rings on ([622196b](https://github.com/MikeGrier/windows-threadpool-sys/commit/622196b5461c2b6f1a2e95f9dc797bbb2a8c9b0b))
* **file-watcher:** add test_your_handler consumer example ([9ce426b](https://github.com/MikeGrier/windows-threadpool-sys/commit/9ce426b0c26ca2f14a7269ad5ae5c66f9400310f))
* **file-watcher:** add test-util RelativeName constructors ([d80d2f8](https://github.com/MikeGrier/windows-threadpool-sys/commit/d80d2f81bb18fb370a4d4264ffe270f8c653115e))
* **file-watcher:** add test-util VolumeIdentity constructor ([e760768](https://github.com/MikeGrier/windows-threadpool-sys/commit/e7607685abb86eda2654ac54b1fb8c4795a7dc60))
* **file-watcher:** expose and document the consumer test seam ([83e2986](https://github.com/MikeGrier/windows-threadpool-sys/commit/83e2986c25531432ae71a8d9335c75662f0258eb))


### Bug Fixes

* correct rename fidelity per windows-file-watcher D-9 ([3056907](https://github.com/MikeGrier/windows-threadpool-sys/commit/3056907a728ffe148ae19e4809d9dc371b112d49))
* **file-watcher:** a Batch can legally appear inside a fault bracket ([3b299a5](https://github.com/MikeGrier/windows-threadpool-sys/commit/3b299a536a5013b85d0950150a8e2703b4c0fca5))
* **file-watcher:** a clamped-down stall replay is unverifiable, not reproduced ([2fcd3dd](https://github.com/MikeGrier/windows-threadpool-sys/commit/2fcd3dd630f0d63b8d3552532fb4bb625b826803))
* **file-watcher:** accept an owed loss report after a terminator ([1cfd72a](https://github.com/MikeGrier/windows-threadpool-sys/commit/1cfd72a8c108e6bd142865d19275948dcf409532))
* **file-watcher:** advance ContractChecker state before reporting a violation ([d408efc](https://github.com/MikeGrier/windows-threadpool-sys/commit/d408efc454bb40b3fa38990ac763b0e50e78fa1b))
* **file-watcher:** classify check() panics as handler pathologies, floor the replay deadline ([aea9f14](https://github.com/MikeGrier/windows-threadpool-sys/commit/aea9f1480e00aac3615407e96075cda62acb04a1))
* **file-watcher:** derive the resume wake edge from the same quantity has_room tests ([8855e1d](https://github.com/MikeGrier/windows-threadpool-sys/commit/8855e1d4237aa34284f1752f17f1aa937265ed3a))
* **file-watcher:** stop teaching that every Desync is a rescan ([af0a2bc](https://github.com/MikeGrier/windows-threadpool-sys/commit/af0a2bc0eff6ff17eb2db601be4234850148661f))
* **harness:** route both bins' non-Windows path through the output seam ([172acc9](https://github.com/MikeGrier/windows-threadpool-sys/commit/172acc91ed02777c64826109d6911cb428482b5e))
* narrow the post-terminal loss exception, and bound replay's untrusted input ([82a9168](https://github.com/MikeGrier/windows-threadpool-sys/commit/82a91684c61f2f24cd5906df6cd54d9ed4e82c54))
* route tool/example output through one writer ([13c86d1](https://github.com/MikeGrier/windows-threadpool-sys/commit/13c86d1f2ced8908323b4dfbe7391e5dda7dfe9b))
* unbreak rustdoc CI, and extend contract checking to every real-watcher drain ([b83da57](https://github.com/MikeGrier/windows-threadpool-sys/commit/b83da57aa9f63ba5f822eb7df9ed484ca03a50c3))
* use lossless OsString as change-name identity, not a lossy String ([7498a3c](https://github.com/MikeGrier/windows-threadpool-sys/commit/7498a3c8e695e05174db1c4c4898f1c4f9bf1828))
* **windows-file-watcher-example-test-harness:** pin sibling dep version and enable docs.rs test-util feature ([ef43669](https://github.com/MikeGrier/windows-threadpool-sys/commit/ef43669952ae12442f4269914c8311a77bbb2ac1))
* **windows-file-watcher:** make has_room account for a pending latched loss ([700e0eb](https://github.com/MikeGrier/windows-threadpool-sys/commit/700e0eb66ddbd061c5078136ad808d885d0427cf))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-overlapped-io-sys bumped from 0.1.2 to 0.1.3
    * windows-threadpool-sys bumped from 0.1.2 to 0.1.3

## [0.1.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-watcher-v0.1.0...windows-file-watcher-v0.1.1) (2026-08-24)


### Bug Fixes

* **build:** use table headers for windows-sys so release-please can parse the manifests ([76a0d53](https://github.com/MikeGrier/windows-threadpool-sys/commit/76a0d53f3b06db73d6a2567933f66bcd4edac260))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-overlapped-io-sys bumped from 0.1.1 to 0.1.2
    * windows-threadpool-sys bumped from 0.1.1 to 0.1.2

## Changelog
