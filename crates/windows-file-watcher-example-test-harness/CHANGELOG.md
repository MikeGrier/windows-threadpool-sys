# Changelog

## [0.1.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-watcher-example-test-harness-v0.1.1...windows-file-watcher-example-test-harness-v0.1.2) (2026-08-29)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-file-watcher bumped from 0.1.2 to 0.1.3

## [0.1.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-watcher-example-test-harness-v0.1.0...windows-file-watcher-example-test-harness-v0.1.1) (2026-08-27)


### Features

* **file-watcher:** add DesyncCause::is_reachable_in and make the generator bind to it (M2.3) ([f95a74f](https://github.com/MikeGrier/windows-threadpool-sys/commit/f95a74f9f78f2262779231493394b16203a108a5))
* **file-watcher:** add DesyncCause::is_terminal and adopt it in every example (M2.2) ([f990575](https://github.com/MikeGrier/windows-threadpool-sys/commit/f9905758028d26b208355e8b1ffe334611f96edf))
* **harness:** capture/replay bins (M5) ([7d89555](https://github.com/MikeGrier/windows-threadpool-sys/commit/7d89555dc0339033ef41691bac1b8f4246b71c11))
* **harness:** contract-legal seeded schedule generator (M2) ([8f089f5](https://github.com/MikeGrier/windows-threadpool-sys/commit/8f089f527dff1a1b0f814ff20d378297e6a4df62))
* **harness:** examples, exposition, and full-arc test (M6) ([2a7f3ff](https://github.com/MikeGrier/windows-threadpool-sys/commit/2a7f3ff8d1ea98b5507c3274c3b64641d84d2040))
* **harness:** JSON record/replay of a captured pathology (M4) ([4587103](https://github.com/MikeGrier/windows-threadpool-sys/commit/458710381717463ecf93ffea9d8c72c81db0757e))
* **harness:** oracles for panic, invariant, and wedge (M3) ([9a7ed23](https://github.com/MikeGrier/windows-threadpool-sys/commit/9a7ed23715d89bd39b2840b599f74b99a7ddf07b))
* **harness:** scaffold example test harness crate (M1 spine) ([92f95ee](https://github.com/MikeGrier/windows-threadpool-sys/commit/92f95eec809924abe6322d0a9ac7d1c0e7d1e35a))


### Bug Fixes

* correct rename fidelity per windows-file-watcher D-9 ([3056907](https://github.com/MikeGrier/windows-threadpool-sys/commit/3056907a728ffe148ae19e4809d9dc371b112d49))
* **file-watcher:** a Batch can legally appear inside a fault bracket ([3b299a5](https://github.com/MikeGrier/windows-threadpool-sys/commit/3b299a536a5013b85d0950150a8e2703b4c0fca5))
* **file-watcher:** a clamped-down stall replay is unverifiable, not reproduced ([2fcd3dd](https://github.com/MikeGrier/windows-threadpool-sys/commit/2fcd3dd630f0d63b8d3552532fb4bb625b826803))
* **file-watcher:** advance ContractChecker state before reporting a violation ([d408efc](https://github.com/MikeGrier/windows-threadpool-sys/commit/d408efc454bb40b3fa38990ac763b0e50e78fa1b))
* **file-watcher:** classify check() panics as handler pathologies, floor the replay deadline ([aea9f14](https://github.com/MikeGrier/windows-threadpool-sys/commit/aea9f1480e00aac3615407e96075cda62acb04a1))
* **file-watcher:** derive the resume wake edge from the same quantity has_room tests ([8855e1d](https://github.com/MikeGrier/windows-threadpool-sys/commit/8855e1d4237aa34284f1752f17f1aa937265ed3a))
* **file-watcher:** stop teaching that every Desync is a rescan ([af0a2bc](https://github.com/MikeGrier/windows-threadpool-sys/commit/af0a2bc0eff6ff17eb2db601be4234850148661f))
* guarantee VolumeChanged serials differ by construction ([7e5b85c](https://github.com/MikeGrier/windows-threadpool-sys/commit/7e5b85cdc59f3f7f2972f56ace9cdadf124f3172))
* **harness:** distinguish a harness panic from a genuine stall ([0f7f078](https://github.com/MikeGrier/windows-threadpool-sys/commit/0f7f078168ecfe5054035bf8e7948695fe1183a3))
* **harness:** generator legality overhaul per PR [#42](https://github.com/MikeGrier/windows-threadpool-sys/issues/42) review ([f8b0dc5](https://github.com/MikeGrier/windows-threadpool-sys/commit/f8b0dc5237a79030fb2b83c84a406030fb4cc623))
* **harness:** Resumed and Established are attempted together, not delivered together ([974a3d1](https://github.com/MikeGrier/windows-threadpool-sys/commit/974a3d13aefa795d540ba1fbbb42a3b16d4faffd))
* **harness:** route both bins' non-Windows path through the output seam ([172acc9](https://github.com/MikeGrier/windows-threadpool-sys/commit/172acc91ed02777c64826109d6911cb428482b5e))
* narrow the post-terminal loss exception, and bound replay's untrusted input ([82a9168](https://github.com/MikeGrier/windows-threadpool-sys/commit/82a91684c61f2f24cd5906df6cd54d9ed4e82c54))
* route tool/example output through one writer ([13c86d1](https://github.com/MikeGrier/windows-threadpool-sys/commit/13c86d1f2ced8908323b4dfbe7391e5dda7dfe9b))
* unbreak rustdoc CI, and extend contract checking to every real-watcher drain ([b83da57](https://github.com/MikeGrier/windows-threadpool-sys/commit/b83da57aa9f63ba5f822eb7df9ed484ca03a50c3))
* use lossless OsString as change-name identity, not a lossy String ([7498a3c](https://github.com/MikeGrier/windows-threadpool-sys/commit/7498a3c8e695e05174db1c4c4898f1c4f9bf1828))
* **windows-file-watcher-example-test-harness:** a live watch's fault recovery always enters as Arm, never a standalone Open ([867f419](https://github.com/MikeGrier/windows-threadpool-sys/commit/867f4192a024fcef8874d5fe0c338366bac162f8))
* **windows-file-watcher-example-test-harness:** capture returns ExitCode::FAILURE on non-Windows ([a572b5b](https://github.com/MikeGrier/windows-threadpool-sys/commit/a572b5b8f3307591ea4db9e3abde5e93e60aea51))
* **windows-file-watcher-example-test-harness:** carry a change name losslessly, not only as UTF-8 ([a4e5c96](https://github.com/MikeGrier/windows-threadpool-sys/commit/a4e5c96466db919a86ccb854b0fc015e9bf3f0d8))
* **windows-file-watcher-example-test-harness:** clamp a recorded Stalled deadline instead of trusting it verbatim ([abbb639](https://github.com/MikeGrier/windows-threadpool-sys/commit/abbb639a0974f50dc9a6aa77141bedbc3f8a36fc))
* **windows-file-watcher-example-test-harness:** constrain a Coarse-tier watch's live loop to what a coarse endpoint can actually report ([4a5a744](https://github.com/MikeGrier/windows-threadpool-sys/commit/4a5a744427458f23ef9632a5f3e660acc4cc7853))
* **windows-file-watcher-example-test-harness:** fix a stale doc count/missing stability note, a broken intra-doc link, and name test-data protocol identities ([ca08ccf](https://github.com/MikeGrier/windows-threadpool-sys/commit/ca08ccf7f2d761dc77aa3f7b630955990f57632c))
* **windows-file-watcher-example-test-harness:** key PresenceTracker's presence set by WatchId, not name alone ([1be97b1](https://github.com/MikeGrier/windows-threadpool-sys/commit/1be97b10c8c645d83007055d681ded56999d06e4))
* **windows-file-watcher-example-test-harness:** pin sibling dep version and enable docs.rs test-util feature ([ef43669](https://github.com/MikeGrier/windows-threadpool-sys/commit/ef43669952ae12442f4269914c8311a77bbb2ac1))
* **windows-file-watcher-example-test-harness:** replay a recording under its own deadline, not a plain run ([c23d1e2](https://github.com/MikeGrier/windows-threadpool-sys/commit/c23d1e214b2f6f46493ce8fa38c73fea5407383c))
* **windows-file-watcher-example-test-harness:** stop conflating RetryMode and VolumeChangePolicy, guard zero-weight configs ([3e78dca](https://github.com/MikeGrier/windows-threadpool-sys/commit/3e78dca933526287944a2b92d90944580b80db41))
* **windows-file-watcher-example-test-harness:** track volume-identity continuity across a watch's VolumeChanged events ([dc5cf30](https://github.com/MikeGrier/windows-threadpool-sys/commit/dc5cf3012c5d7c968baed54c0a6cb8292c5c6493))
* **windows-file-watcher-example-test-harness:** unconditional RetryQuestion for interactive watches, O(1) live-queue interleave ([a4d0a9b](https://github.com/MikeGrier/windows-threadpool-sys/commit/a4d0a9b7d35a64507858e6bf974c6cb75364c418))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-file-watcher bumped from 0.1.1 to 0.1.2
