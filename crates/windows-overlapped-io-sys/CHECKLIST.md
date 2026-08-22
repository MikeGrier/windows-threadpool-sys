# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions
are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## M13: the notification-mode setter belongs to the core, not to the `fs` family

- [x] **M13.1** -- Move `Win32_Storage_FileSystem` into the always-on `windows-sys` feature set, ungate
  [`UnassociatedEndpoint::set_notification_modes`](src/endpoint.rs) and the `notification_flags` module, and
  drop the now-redundant `Win32_Storage_FileSystem` entry (and its explanatory comment) from the `socket`
  feature.

  **Gap:** every part of the notification-mode mechanism is core *except the one method that establishes it*.
  `NotificationModes`, `UnassociatedEndpoint::notification_modes`, the mode's carriage through
  `CompletionPort::associate`, and `AssociatedEndpoint::notification_modes` are all ungated; only
  `set_notification_modes` is `#[cfg(feature = "fs")]`. So a consumer enabling only `device` has a mode that
  is permanently `false` and no way to change it, while [`device.rs`](src/device.rs) reads
  `skip_completion_port_on_success` at every submission and `classify_issued` branches on it -- the device
  family's skip-on-success support is unreachable by the exact consumer it was built for.

  **The soundness edge, which is the deciding argument:** `assume_overlapped` is an ungated core `unsafe fn`
  whose documented safety obligation is that a caller who set a notification mode on the raw handle must
  re-declare it, because "a handle silently in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode would have its
  synchronous successes reported as pending, leaving operations outstanding forever." A `device`-only
  consumer is handed that obligation and no way to discharge it. A core contract must not be dischargeable
  only by whoever happened to enable an unrelated operation family.

  **Target:** the gate keyed on an operation family disappears rather than being widened to
  `any(fs, device)`. Widening would fix the reported symptom and leave the next family to rediscover it;
  the capability is a property of an endpoint, and an endpoint does not know its family. Records that this
  supersedes the `default = []` comment's claim that "the core completion machinery needs no operation-family
  `windows-sys` bindings at all" -- true when written, false since `assume_overlapped` took on that
  obligation. `windows-sys` is a bindings-only crate, so the cost is compile-time surface, not emitted code.

- [ ] **M13.2** -- Give the device family its own skip-on-success integration test, gated on `device` alone,
  rather than leaving it nested inside a file that requires `fs`.

  **Gap:** [`tests/skip_on_success_adapters.rs`](tests/skip_on_success_adapters.rs) is
  `#![cfg(all(windows, feature = "fs"))]` and carries the device coverage as an inner
  `#[cfg(feature = "device")] mod device`, so the device skip path is only ever exercised when `fs` is *also*
  enabled -- the same coupling as the defect in M13.1, in the tests that were supposed to catch it. Audit
  [`tests/device_ioctl.rs`](tests/device_ioctl.rs) for the same shape.

  **Target:** a `device`-only build sets skip-on-success on an endpoint, issues an ioctl, and observes
  `Started::Completed` with no packet queued and nothing outstanding, paired against a default endpoint that
  is always `Pending` -- the shape M12.3 used for sockets. As there, assert the synchronous arm is actually
  taken by at least one submission, so the test cannot pass vacuously if the mode stops being applied.

- [ ] **M13.3** -- Add a CI job that builds, lints, and tests each operation family on its own
  (`--no-default-features --features <family>`) for every family, plus the bare `--no-default-features` core.

  **Gap:** [ci.yml](../../.github/workflows/ci.yml) only ever runs `--all-features` and default (empty)
  features. No individual family combination is built anywhere, which is why M13.1's gap shipped, why M12.2's
  socket-only build had to be checked by hand, and why nothing would stop the next family repeating it.
  `--all-features` is structurally blind to this class of defect: it hides every missing gate.

  **Target:** the check is mechanical and additive -- adding a family to the matrix is a one-line change --
  so the guarantee survives the next family without anyone having to remember it.

- [ ] **M13.4** -- Record in [DESIGN-NOTES.md](DESIGN-NOTES.md): that the notification-mode mechanism is core
  rather than per-family, and why (the endpoint owns the capability, the submission seam depends on it, and
  `assume_overlapped`'s contract requires it); and supersede M12.2's note explaining the `socket` feature's
  widening, which M13.1 makes obsolete.
