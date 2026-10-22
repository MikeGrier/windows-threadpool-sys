# Changelog

## [0.4.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.3.2...win-durable-ioring-v0.4.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring's target and submit traits gain a declared access and carry it in a typed Key<A>, Diagnostic and DiagnosticKind gain the begin/end pair variants, and a sink is given every pair's end; win-durable-ioring asks for file access as a typed set of named rights, takes data access as the type parameter of every handle it opens, takes a share mode as a typed set, takes OpenedFile::open's path as a Wtf16String, uses Win32's own disposition enum, and removes HandleKindQuery and HandleKind.

### Features

* **durable-ioring:** a typed Key&lt;A&gt; carries a target's access, every sink takes the begin/end pairs, and the ETW provider writes each pair as one activity ([341858e](https://github.com/MikeGrier/windows-threadpool-sys/commit/341858ed566c12a38bfde2dd06a7d9c1a9da1a09))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * durable-ioring bumped from 0.2.0 to 0.3.0
    * windows-ioring-sys bumped from 0.8.0 to 0.9.0
    * win-sync-sys bumped from 0.0.2 to 0.1.0
    * windows-threadpool-sys bumped from 0.3.0 to 0.4.0
    * win-volume-sys bumped from 0.1.1 to 0.1.2
    * windows-namespace-request-sys bumped from 0.5.0 to 0.5.1
    * win-handles-sys bumped from 0.1.0 to 0.1.1
    * win-debugging-sys bumped from 0.0.1 to 0.0.2
    * durable-ioring-events bumped from 0.0.1 to 0.1.0

## [0.3.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.3.1...win-durable-ioring-v0.3.2) (2026-10-20)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-namespace-request-sys bumped from 0.4.2 to 0.5.0

## [0.3.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.3.0...win-durable-ioring-v0.3.1) (2026-10-20)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * win-volume-sys bumped from 0.1.0 to 0.1.1

## [0.3.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.2.1...win-durable-ioring-v0.3.0) (2026-10-17)


### ⚠ BREAKING CHANGES

* **win-durable-ioring:** the CallbackEnviron accepted by DeliveryMode::Pool, DeliveryMode::Signalled and EntryDelivery::new now comes from windows-threadpool-sys 0.3.

### Features

* **win-durable-ioring:** release as 0.3.0, paired with windows-threadpool-sys 0.3 ([1bbea90](https://github.com/MikeGrier/windows-threadpool-sys/commit/1bbea905b3ded93d7e18a1595ceb2fb2d6d9e24b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-ioring-sys bumped from 0.7.2 to 0.8.0
    * windows-threadpool-sys bumped from 0.2.2 to 0.3.0
    * windows-namespace-request-sys bumped from 0.4.1 to 0.4.2

## [0.2.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.2.0...win-durable-ioring-v0.2.1) (2026-10-17)


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * windows-ioring-sys bumped from 0.7.1 to 0.7.2
    * windows-threadpool-sys bumped from 0.2.1 to 0.2.2
    * windows-namespace-request-sys bumped from 0.4.0 to 0.4.1

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.1.0...win-durable-ioring-v0.2.0) (2026-10-16)


### ⚠ BREAKING CHANGES

* **namespace-request:** OpenFile::new, WatchDirectory::new and ResolveFullPath::new take a Wtf16String and return a Result; prepare, PreparedPath, PathError, PathFailure and the path module are removed.
* **win-durable-ioring:** Win32FileTarget::new takes an OpenedFile, not a SharedHandle, and the warranty, geometry and capabilities setters are removed.

### Features

* **namespace-request:** open, watch and resolve by a Wtf16String, and retire prepare ([3083c05](https://github.com/MikeGrier/windows-threadpool-sys/commit/3083c050319c9bbee663c1f784ebbdd6084c2be2))
* **win-durable-ioring:** a file target is made from a file this crate opened ([b49d421](https://github.com/MikeGrier/windows-threadpool-sys/commit/b49d4212150c67d2fc67bd9da96b2e9511798557))
* **win-durable-ioring:** a raw volume is a file target, opened here and its disks found ([81dbdb7](https://github.com/MikeGrier/windows-threadpool-sys/commit/81dbdb78ca7a21927ae226de9db3794fa42656a5))
* **win-durable-ioring:** add a whole physical disk as a file target ([fd281df](https://github.com/MikeGrier/windows-threadpool-sys/commit/fd281df87aeed800348d406657497ddf659c3898))
* **win-durable-ioring:** add error_code_from_win32 and win32_code_of ([0758381](https://github.com/MikeGrier/windows-threadpool-sys/commit/075838108d3fe8765f6ba293b1375739411c0946))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * durable-ioring bumped from 0.1.0 to 0.2.0
    * windows-namespace-request-sys bumped from 0.3.0 to 0.4.0
    * win-error-codes bumped from 0.1.0 to 0.0.2

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-durable-ioring-v0.0.1...win-durable-ioring-v0.1.0) (2026-10-15)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring no longer contains Dioring, the Windows backend, delivery, the readiness check or the Windows re-exports; they are win-durable-ioring's. ErrorCode::of, the default DioringIds time type and the TimeBase bound on a win-time-sys clock are replaced as described above.

### Features

* **win-durable-ioring:** Dioring::completion_event, a wait-only handle on the ring's own event ([747e46f](https://github.com/MikeGrier/windows-threadpool-sys/commit/747e46f6eb619c388faf2ada9aab44085d1bfb54))


### Bug Fixes

* **win-durable-ioring:** a push only queues, and a failed doorbell or an unreadable ring reaches the consumer ([71f3562](https://github.com/MikeGrier/windows-threadpool-sys/commit/71f35626011941ad59e6a6e3efbfd49b5e86d794))
* **win-durable-ioring:** end and retire refuse a lineage handle the instance did not mint ([00fb4a4](https://github.com/MikeGrier/windows-threadpool-sys/commit/00fb4a4bde839651646eeb31c177f465076c9a73))
* **win-durable-ioring:** Model A's handler is told when the instance's pop fails ([622848e](https://github.com/MikeGrier/windows-threadpool-sys/commit/622848e92dc4b1adbfa1536ea0016f5a6854112a))
* **win-durable-ioring:** refuse a buffer beyond a u32 at check; no safe CompletionEvent from any handle ([f640c2c](https://github.com/MikeGrier/windows-threadpool-sys/commit/f640c2ccb2d883be0055d537e4e51239752bec3d))
* **win-durable-ioring:** report a lineage release the core finds not live ([c98ff3b](https://github.com/MikeGrier/windows-threadpool-sys/commit/c98ff3b56190b408c8935409e18168fed89da0cc))
* **win-durable-ioring:** the backend's ring is an owned one, so a completion it held nothing for is not a case ([ceb3b72](https://github.com/MikeGrier/windows-threadpool-sys/commit/ceb3b720e8054ef8ff711736da8f8d0e077df03d))


### Code Refactoring

* **durable-ioring:** split the Windows driver out into win-durable-ioring ([e8ff929](https://github.com/MikeGrier/windows-threadpool-sys/commit/e8ff92981e05304c897a20e0836778c5f209f8c5))
