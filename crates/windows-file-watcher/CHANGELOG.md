# Changelog

## [1.0.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-file-watcher-v0.1.0...windows-file-watcher-v1.0.0) (2026-08-21)


### ⚠ BREAKING CHANGES

* **threadpool:** `WaitableHandle::into_handle` returns `Result<OwnedHandle, Self>` instead of `OwnedHandle`. An `OwnedHandle` closes what it holds with `CloseHandle`, which is the wrong destructor for a custom-close target, so there is no correct value to hand back for one; it now returns `Err(self)`, giving the wrapper back intact. Callers on the default path add `?`, `.unwrap()` or equivalent. Panicking and closing with the wrong routine were both rejected as alternatives.

### Features

* **threadpool:** let a wait target own its close routine ([04a667c](https://github.com/MikeGrier/windows-threadpool-sys/commit/04a667ce63997089fa6499230fd1ca10da2ae8c7))
