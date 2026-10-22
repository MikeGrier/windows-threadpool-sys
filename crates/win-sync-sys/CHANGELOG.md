# Changelog

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-sync-sys-v0.0.2...win-sync-sys-v0.1.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* **win-sync:** Event and WaitOnlyEvent are Clone and their clones share one handle; try_clone is removed, try_clone_wait_only is renamed duplicate_wait_only, and the conversions into OwnedHandle are replaced by conversions into SharedHandle, with try_into_owned recovering an OwnedHandle from the last holder.

### Features

* **win-sync:** an Event's clones share one handle, and the wait-only operation is duplicate_wait_only ([516466b](https://github.com/MikeGrier/windows-threadpool-sys/commit/516466b0152b32ae860555c306cf0c16cdb400bb))
* **win-sync:** delete try_clone and the conversions into OwnedHandle, now that clones share ([d306b91](https://github.com/MikeGrier/windows-threadpool-sys/commit/d306b910b5d6c78711411b77dda850a3cb3f15cf))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * win-handles-sys bumped from 0.1.0 to 0.1.1

## [0.0.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-sync-sys-v0.0.1...win-sync-sys-v0.0.2) (2026-10-15)


### Features

* **win-sync:** add duplicate_handle_with_access, Access and a wait-only event ([0d5249d](https://github.com/MikeGrier/windows-threadpool-sys/commit/0d5249dd68e4e516dcf6057fc5455702e9e5c324))
* **win-sync:** add the alertable, multiple-object and signal-and-wait waits, and QueueUserAPC ([5ae5e29](https://github.com/MikeGrier/windows-threadpool-sys/commit/5ae5e290f630e5c1588c64ce3c73b56e4a73b463))
* **win-sync:** add wait_for_single_object, a safe WaitForSingleObject over any handle ([d4fd350](https://github.com/MikeGrier/windows-threadpool-sys/commit/d4fd350a9b47bc8008b8b4303ab0e4eea8fffa7b))


### Bug Fixes

* **win-sync-sys:** queue_user_apc refuses a thread that is not this process's ([6c3d5de](https://github.com/MikeGrier/windows-threadpool-sys/commit/6c3d5de3ebd0722e99fd5d104a215aea7c81aa4e))
