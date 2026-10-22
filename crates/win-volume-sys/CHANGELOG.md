# Changelog

## [0.1.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-volume-sys-v0.1.1...win-volume-sys-v0.1.2) (2026-10-22)

## [0.1.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-volume-sys-v0.1.0...win-volume-sys-v0.1.1) (2026-10-20)


### Features

* **volume:** add storage_device_number, the device number a volume or disk reports ([02b4d8a](https://github.com/MikeGrier/windows-threadpool-sys/commit/02b4d8a18385b520ddf76d7714a17cdc944a53a2))

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-volume-sys-v0.0.1...win-volume-sys-v0.1.0) (2026-10-16)


### Features

* **volume:** storage property queries, unique id, disk NUMA and the generic one ([4575d63](https://github.com/MikeGrier/windows-threadpool-sys/commit/4575d637812e0aff666d04f66e55aa0beb5af3cd))
* **volume:** storage_dependency_information, the virtdisk dependency query ([f6382bf](https://github.com/MikeGrier/windows-threadpool-sys/commit/f6382bf8d138b8303ae2d175bf293263a9e9b6c3))
* **volume:** volume_numa_node, the NUMA node a volume resides on ([ccb5c6e](https://github.com/MikeGrier/windows-threadpool-sys/commit/ccb5c6e6b5e27617672066e246dd9d2714a2d784))
* **win-volume-sys:** add win-volume-sys, the disks a volume sits on, its sector size and its lock ([9d74bb5](https://github.com/MikeGrier/windows-threadpool-sys/commit/9d74bb5f230f6312cd909db29d2fe00b7decace2))


### Bug Fixes

* **volume:** one overlapped-safe DeviceIoControl helper, with exact error codes ([3b5a1fb](https://github.com/MikeGrier/windows-threadpool-sys/commit/3b5a1fb83860aaef2db2f39b9af257cd325d8be4))
* **volume:** query the direction the storage_dependency_information example names ([03580ff](https://github.com/MikeGrier/windows-threadpool-sys/commit/03580ff860951b7413f3dcae6464751709b69881))
* **volume:** volume_disk_extents takes the stricter checks of the namespace entry ([7790bae](https://github.com/MikeGrier/windows-threadpool-sys/commit/7790bae8d8dd6b1199b7958fcb543ede646de21b))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * win-error-codes bumped from 0.1.0 to 0.0.2
