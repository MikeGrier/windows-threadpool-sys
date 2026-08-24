# Changelog

## [1.0.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-ioring-sys-v0.1.0...windows-ioring-sys-v1.0.0) (2026-08-24)


### ⚠ BREAKING CHANGES

* **ioring:** cross-ring identity checks and registration-drop safety (PR #20 review)
* **ioring:** give the safe SharedFile-based push methods the plain names, suffix the unsafe raw ones with _raw
* **ioring:** close FileRef::Raw(HANDLE)'s lifetime gap with unsafe raw entry points and a safe SharedFile wrapper

### Features

* **ioring:** add the windows-ioring-sys crate skeleton ([8bd10a5](https://github.com/MikeGrier/windows-threadpool-sys/commit/8bd10a5b934997cc8c3c5fba4d81a6b91ed57085))
* **ioring:** close FileRef::Raw(HANDLE)'s lifetime gap with unsafe raw entry points and a safe SharedFile wrapper ([1aeebd8](https://github.com/MikeGrier/windows-threadpool-sys/commit/1aeebd84ac67dd8b3c9ab25c8144ebfff0567a78))
* **ioring:** give the safe SharedFile-based push methods the plain names, suffix the unsafe raw ones with _raw ([fe66cf3](https://github.com/MikeGrier/windows-threadpool-sys/commit/fe66cf32efed2ed8f19826cc3ecf4728f393e007))
* **ioring:** implement M1, ring lifecycle and capability negotiation ([1f1a801](https://github.com/MikeGrier/windows-threadpool-sys/commit/1f1a801e36ce36c1b36b95977be7629b90870cad))
* **ioring:** M7 -- ring-copy, a topology-aligned sample ([7618c88](https://github.com/MikeGrier/windows-threadpool-sys/commit/7618c8893fec1990ad370814b3170a5118a4e451))
* **ioring:** Model A delivery -- completion event wired to the thread pool ([c6f52b3](https://github.com/MikeGrier/windows-threadpool-sys/commit/c6f52b34e55e23a6ccd9fc9824bf735bb68194b5))
* **ioring:** operation identity, buffer ownership, and rundown ([ca0ba44](https://github.com/MikeGrier/windows-threadpool-sys/commit/ca0ba442cb2a22c510dcf619e29a8e7d9f06ad03))
* **ioring:** registration -- registered file handles and buffers ([26af335](https://github.com/MikeGrier/windows-threadpool-sys/commit/26af33505c6eb18aa44d2320995ad650ad775d4b))
* **ioring:** the submission builder (Batch, per-op push, backpressure) ([06e0dfa](https://github.com/MikeGrier/windows-threadpool-sys/commit/06e0dfa21e968e700badfaa140fdfd2be2f85b4d))
* **topology:** implement M4 and fix release-please registration ([79cf8d1](https://github.com/MikeGrier/windows-threadpool-sys/commit/79cf8d1b8a1cf1c0b0ca1fd614b842d531cc5a88))


### Bug Fixes

* **ioring:** cross-ring identity checks and registration-drop safety (PR [#20](https://github.com/MikeGrier/windows-threadpool-sys/issues/20) review) ([ba228ad](https://github.com/MikeGrier/windows-threadpool-sys/commit/ba228ad114eb9fa1abd44ff93b52d5f247cc3d7c))
* **ioring:** refuse a second file/buffer registration and release the ring mutex before invoking completion callbacks ([8c2060c](https://github.com/MikeGrier/windows-threadpool-sys/commit/8c2060c1076a659fa77c27f72849a0984430d2b7))
* **ioring:** require an unforgeable Completion to claim a Token, validate registered span bounds, and fix ring-copy's short-read handling ([ce846f1](https://github.com/MikeGrier/windows-threadpool-sys/commit/ce846f1163325d4c48aff714ef961c88d77f4e56))
* **ioring:** stop ring-copy from destroying the source when it names the destination ([91f0c9b](https://github.com/MikeGrier/windows-threadpool-sys/commit/91f0c9b5083b48dc5c5dc453146836725d8712b9))
