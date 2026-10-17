# Changelog

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-multi-file-tail-watcher-v0.0.1...win-multi-file-tail-watcher-v0.1.0) (2026-10-17)


### Features

* **tail-watcher:** a diagnostic sink the caller supplies ([4ffafce](https://github.com/MikeGrier/windows-threadpool-sys/commit/4ffafce0f15d29be08cdf85ca700a8b2f0a7c469))
* **tail-watcher:** a poll stopped on a full queue resumes when the caller rings the resume doorbell ([54b014f](https://github.com/MikeGrier/windows-threadpool-sys/commit/54b014f81b8f7bb2f3cb06f486cfca8aec472c58))
* **tail-watcher:** a terminal entry kind in the SQ ([f9bc33a](https://github.com/MikeGrier/windows-threadpool-sys/commit/f9bc33a1202196d1a5a51671eeeacf26b5d3581d))
* **tail-watcher:** a terminal read failure is delivered on the client's SQ ([468ebad](https://github.com/MikeGrier/windows-threadpool-sys/commit/468ebada6cb2908b051880c169c06e9c774d5816))
* **tail-watcher:** poll runs the engine on the thread pool, one poll at a time ([89f777d](https://github.com/MikeGrier/windows-threadpool-sys/commit/89f777d66e5d12fe9e56badc60269d12e0dcfd3a))
* **tail-watcher:** shutdown stops the watcher for good and poll is refused after it ([2b55128](https://github.com/MikeGrier/windows-threadpool-sys/commit/2b551286977fb33540109740b3a25189da124873))


### Bug Fixes

* **tail-watcher:** answer a refused read, document the callback hazards, and release as 0.1.0 ([ba9bbd8](https://github.com/MikeGrier/windows-threadpool-sys/commit/ba9bbd8671fa162a8940f00ddcfa99a01f877e18))
