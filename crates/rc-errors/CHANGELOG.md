# Changelog

## [0.0.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/rc-errors-v0.0.1...rc-errors-v0.0.2) (2026-11-03)


### Features

* **rc-errors:** add Refusing, an allocator that refuses every allocation ([624d5b9](https://github.com/MikeGrier/windows-threadpool-sys/commit/624d5b9f260f27a12080206cc85ce250664a6bf5))
* **rc-errors:** add SharedError, a one-count shared error freed through the allocator that made it ([618b5bb](https://github.com/MikeGrier/windows-threadpool-sys/commit/618b5bbc73ddb7aeacd88f8615b9bf5393b20d22))
* **rc-errors:** add StaticError and ErrorReport, a static or a shared error compared by identity ([a7a118b](https://github.com/MikeGrier/windows-threadpool-sys/commit/a7a118b76d956a0991f7f8dd6efc0670d071c699))
* **rc-errors:** add the allocator interface, mirroring the standard library's unstable Allocator ([b076bb9](https://github.com/MikeGrier/windows-threadpool-sys/commit/b076bb9edef1bb2d25054fc8a60d9a815fa3d7c7))
