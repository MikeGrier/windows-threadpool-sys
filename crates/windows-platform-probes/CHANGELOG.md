# Changelog

## [2026.1022.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1022.0...windows-platform-probes-v2026.1022.1) (2026-10-22)

## [2026.1022.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1020.0...windows-platform-probes-v2026.1022.0) (2026-10-22)


### Features

* **platform-probes:** make the probes' sampling settable from the command line, and the queue probe's smallest run no longer panics ([19dbdf3](https://github.com/MikeGrier/windows-threadpool-sys/commit/19dbdf3bb8a6d7c556ab02529737842f3342f221))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-waitable-queues bumped from 0.1.2 to 0.1.3

## [2026.1020.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1018.0...windows-platform-probes-v2026.1020.0) (2026-10-20)


### Features

* **platform-probes:** time a shared handle in request_cost, not a captured one ([98edaa0](https://github.com/MikeGrier/windows-threadpool-sys/commit/98edaa0643e7207f29383e01c3b599812e252133))

## [2026.1018.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1017.1...windows-platform-probes-v2026.1018.0) (2026-10-18)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-waitable-queues bumped from 0.1.1 to 0.1.2

## [2026.1017.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1017.0...windows-platform-probes-v2026.1017.1) (2026-10-17)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-waitable-queues bumped from 0.1.0 to 0.1.1

## [2026.1017.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1016.0...windows-platform-probes-v2026.1017.0) (2026-10-17)

## [2026.1016.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1015.1...windows-platform-probes-v2026.1016.0) (2026-10-16)


### ⚠ BREAKING CHANGES

* **namespace-request:** OpenFile::new, WatchDirectory::new and ResolveFullPath::new take a Wtf16String and return a Result; prepare, PreparedPath, PathError, PathFailure and the path module are removed.

### Features

* **namespace-request:** open, watch and resolve by a Wtf16String, and retire prepare ([3083c05](https://github.com/MikeGrier/windows-threadpool-sys/commit/3083c050319c9bbee663c1f784ebbdd6084c2be2))
* **platform-probes:** rewrite the request_cost probe for the request crate's path design ([8e8327c](https://github.com/MikeGrier/windows-threadpool-sys/commit/8e8327ca2c6b875b6479f42aaa57692faa5732b9))

## [2026.1015.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1015.0...windows-platform-probes-v2026.1015.1) (2026-10-15)

## [2026.1015.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.1004.0...windows-platform-probes-v2026.1015.0) (2026-10-15)

## [2026.1004.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.925.0...windows-platform-probes-v2026.1004.0) (2026-10-04)

## [2026.925.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v2026.919.0...windows-platform-probes-v2026.925.0) (2026-09-25)

## [2026.919.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-platform-probes-v0.0.1...windows-platform-probes-v2026.919.0) (2026-09-19)


### Features

* automate calendar-versioned probe binary releases ([ce1ec46](https://github.com/MikeGrier/windows-threadpool-sys/commit/ce1ec46dea1becc478bd55f1e95e4769f1cef865))
* automate calendar-versioned probe binary releases ([fde97d6](https://github.com/MikeGrier/windows-threadpool-sys/commit/fde97d6b71886984e1d7ff6af53855d25f87a4f1))
* **platform-probes:** add the placement probes, and derive both cost renderings from one source ([cb45ed6](https://github.com/MikeGrier/windows-threadpool-sys/commit/cb45ed62323c890a7a14189812e9d887d4a685f0))


### Bug Fixes

* **platform-probes:** bracket the whole placement run, and keep redirected rows out of the verdict ([8a0b18f](https://github.com/MikeGrier/windows-threadpool-sys/commit/8a0b18f8f7f67639f9dfa62da5bb8722623c00f0))
* **platform-probes:** correct four claims their own cited sources contradict ([7b54bc5](https://github.com/MikeGrier/windows-threadpool-sys/commit/7b54bc50710274316d05da484c546b686dbafe5d))
* **platform-probes:** disclose when the banner and the rows describe different machines ([f839d58](https://github.com/MikeGrier/windows-threadpool-sys/commit/f839d58feee2693c161dbc51f9e09cf1e558d775))
* **platform-probes:** fail the source census loudly when it cannot read a source ([c2eebf8](https://github.com/MikeGrier/windows-threadpool-sys/commit/c2eebf8ea61fd218402ee9b6d7975dc8a6c945ef))
* **platform-probes:** report a refusal to pin instead of crashing the probe ([ba95bcd](https://github.com/MikeGrier/windows-threadpool-sys/commit/ba95bcd0d7534d6535cc4e261f0ed6676918612c))
* **platform-probes:** stop the disagree disclaimer pointing at a body that may not exist ([143c721](https://github.com/MikeGrier/windows-threadpool-sys/commit/143c72141bc4fba38b54081b12287e8c41a2602d))
* **platform-probes:** stop two verdicts claiming more than their figures support ([2ad67cc](https://github.com/MikeGrier/windows-threadpool-sys/commit/2ad67cc423eda6e2719229d512feac895a811379))
* **probes:** a renamed copy of a capture is not a second observation ([eba2007](https://github.com/MikeGrier/windows-threadpool-sys/commit/eba2007845b03f10609a44aff329cc0c302f08ac))
* **probes:** read capture tables to their terminator, and name the provenance gap ([cf85328](https://github.com/MikeGrier/windows-threadpool-sys/commit/cf853283edbd339a2869506cb916cbcc7df0ab21))
* **probes:** read corpus counts as integers, and reject a zero processor count ([5dc1058](https://github.com/MikeGrier/windows-threadpool-sys/commit/5dc1058efaf4c992731ee446f178ae27a5199b69))
* **probes:** refuse a capture whose runs were not taken together, and render Wide ([e7e0ebe](https://github.com/MikeGrier/windows-threadpool-sys/commit/e7e0ebe852dc6f2e65cfeba8a9b9bc08babef5dd))
* **probes:** reject a duplicated capture, and a spread built from a bad endpoint ([be45b42](https://github.com/MikeGrier/windows-threadpool-sys/commit/be45b426253d27a52409d76803d2786de904769a))
* **probes:** reject an unrecognised layout, and correct a note that retracted itself ([1fb482f](https://github.com/MikeGrier/windows-threadpool-sys/commit/1fb482f318f44ea5ef7339770b1a7bc2815c2b11))
* **probes:** reject duplicate capture rows and unordered intervals ([60fc576](https://github.com/MikeGrier/windows-threadpool-sys/commit/60fc576f79c33375771d79497447edef21d9ba17))
* **probes:** reject fixture and capture values that are not what they claim ([e7d0a28](https://github.com/MikeGrier/windows-threadpool-sys/commit/e7d0a2854c90e178b23c99c0f27a81e9bf541c68))
* **probes:** repair form-feed corruption, and widen the did-not-run sentinel ([24b2af8](https://github.com/MikeGrier/windows-threadpool-sys/commit/24b2af831cc871241eea596b1b223693eaa86a37))
* **probes:** require a release capture, and check the half of it nobody checked ([6c6551a](https://github.com/MikeGrier/windows-threadpool-sys/commit/6c6551ad7636261ceffc45eb292ffe81c522206d))
* **probes:** route the capture scripts through one writer ([0f6248d](https://github.com/MikeGrier/windows-threadpool-sys/commit/0f6248d0ecadef0a7f00745d621cb8828637c616))
* **probes:** the isolated control was computed in the wrong direction ([4cfe574](https://github.com/MikeGrier/windows-threadpool-sys/commit/4cfe574083c8192fca7eb2c2736bf3fb2f60e58b))
* **probes:** unlink a public field's doc from a private item ([5d44194](https://github.com/MikeGrier/windows-threadpool-sys/commit/5d441949c9884ecc4108d21e3e591ea9dbffb737))
* remove the release-please probe self-cycle ([e52f8aa](https://github.com/MikeGrier/windows-threadpool-sys/commit/e52f8aa50cff4dae267347e45e7dca204c2da8db))
* wire probe oracle feature in test wiring ([befe334](https://github.com/MikeGrier/windows-threadpool-sys/commit/befe334a2da80d8e252594e6c308ca9b93b743be))
