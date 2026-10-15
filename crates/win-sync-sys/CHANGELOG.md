# Changelog

## [0.0.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-sync-sys-v0.0.1...win-sync-sys-v0.0.2) (2026-10-15)


### Features

* **win-sync:** add duplicate_handle_with_access, Access and a wait-only event ([0d5249d](https://github.com/MikeGrier/windows-threadpool-sys/commit/0d5249dd68e4e516dcf6057fc5455702e9e5c324))
* **win-sync:** add the alertable, multiple-object and signal-and-wait waits, and QueueUserAPC ([5ae5e29](https://github.com/MikeGrier/windows-threadpool-sys/commit/5ae5e290f630e5c1588c64ce3c73b56e4a73b463))
* **win-sync:** add wait_for_single_object, a safe WaitForSingleObject over any handle ([d4fd350](https://github.com/MikeGrier/windows-threadpool-sys/commit/d4fd350a9b47bc8008b8b4303ab0e4eea8fffa7b))


### Bug Fixes

* **win-sync-sys:** queue_user_apc refuses a thread that is not this process's ([6c3d5de](https://github.com/MikeGrier/windows-threadpool-sys/commit/6c3d5de3ebd0722e99fd5d104a215aea7c81aa4e))
