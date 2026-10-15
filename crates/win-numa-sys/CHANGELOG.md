# Changelog

## [0.1.1](https://github.com/MikeGrier/windows-threadpool-sys/compare/win-numa-sys-v0.1.0...win-numa-sys-v0.1.1) (2026-10-15)


### Features

* **numa:** implement io-buffers' IoBuf and IoBufMut for NumaBuffer ([e4d9061](https://github.com/MikeGrier/windows-threadpool-sys/commit/e4d906126efdcaed52dabee9a234de36f753e50b))

## 0.1.0 (2026-09-25)


### ⚠ BREAKING CHANGES

* NumaBuffer::new takes Option<NumaNode> rather than Option<u32>. windows_ioring_sys::NumaBuffer still resolves -- the type is re-exported, so a path binding is unaffected -- but a call site passing a bare integer must wrap it. That is the newtype doing its job at the one boundary where a wrong u32 silently allocates on the wrong node, which windows-placement-probe documents as a real defect it had.

### Features

* create win-numa-sys and move NumaBuffer into it ([947b252](https://github.com/MikeGrier/windows-threadpool-sys/commit/947b252b065629352b7eeaa1a2000209112ccbac))
