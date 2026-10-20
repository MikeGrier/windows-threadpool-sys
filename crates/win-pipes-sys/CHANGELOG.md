# Changelog

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-pipes-sys-v0.0.1...win-pipes-sys-v0.1.0) (2026-10-20)


### Features

* **win-pipes-sys:** add ConnectNamedPipe in its two forms and DisconnectNamedPipe ([7a213ec](https://github.com/MikeGrier/windows-threadpool-sys/commit/7a213ecbd78a1779a10543f051084711bf8d2d39))
* **win-pipes-sys:** add CreatePipe, the anonymous pipe ([7a213ec](https://github.com/MikeGrier/windows-threadpool-sys/commit/7a213ecbd78a1779a10543f051084711bf8d2d39))
* **win-pipes-sys:** add GetNamedPipeInfo, GetNamedPipeHandleStateW and SetNamedPipeHandleState ([7a213ec](https://github.com/MikeGrier/windows-threadpool-sys/commit/7a213ecbd78a1779a10543f051084711bf8d2d39))
* **win-pipes-sys:** add PeekNamedPipe, TransactNamedPipe, CallNamedPipeW and WaitNamedPipeW ([7a213ec](https://github.com/MikeGrier/windows-threadpool-sys/commit/7a213ecbd78a1779a10543f051084711bf8d2d39))
* **win-pipes-sys:** add SynchronousPipe, so the waiting connect and transaction are safe only on a pipe not opened overlapped ([7a213ec](https://github.com/MikeGrier/windows-threadpool-sys/commit/7a213ecbd78a1779a10543f051084711bf8d2d39))
* **win-pipes-sys:** add the named-pipe peer queries ([7a213ec](https://github.com/MikeGrier/windows-threadpool-sys/commit/7a213ecbd78a1779a10543f051084711bf8d2d39))
* **win-pipes-sys:** new crate, create_named_pipe over CreateNamedPipeW ([7a213ec](https://github.com/MikeGrier/windows-threadpool-sys/commit/7a213ecbd78a1779a10543f051084711bf8d2d39))
