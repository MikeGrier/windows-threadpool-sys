# Changelog

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
