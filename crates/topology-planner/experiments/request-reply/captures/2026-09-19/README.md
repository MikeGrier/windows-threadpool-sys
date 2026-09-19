# EP-X2.2 request/reply demonstration

Recorded on 2026-09-19 UTC. The
[final report](request-reply-final.json) holds the exact trace/configuration, source/build
identity, trial order, individual outcomes, credit histories, worker assignments
and aggregate/per-lane response distributions. The checkout was dirty while this
experiment was implemented; its source/manifests fingerprint identifies that build.
Each capture used one release executable throughout. The
[first report](request-reply.json) is retained separately: final review found a race
between a worker's deadline cancellation and the coordinator's stop-reason update.
The update now follows reply draining; an active-service deadline regression covers
the repair. The final report is a new run of the same matrix, not an overwritten or
filtered version of the first report. Their build identities distinguish the sources.

This is an unpinned in-memory request service. It establishes neither kernel
completion scheduling nor physical NUMA or device behavior. The protocol is
[DESIGN-NOTES.md](../../DESIGN-NOTES.md); reproduction is in
[README.md](../../README.md). There are no timing acceptance thresholds.

## Behavioral observations

The empty, steady, burst, skewed and pressure scenarios drained their admitted
work. Each cancellation trial stopped at the configured admission count, returned
one terminal reply per admitted ID, and retained the remaining IDs as unadmitted.
The completed/cancelled split differed between candidates, as completion can race
the common cancellation boundary. All reports passed the common verifier.

In the skewed traces, assigned workers served their respective lanes and the hot
lane accounted for most requests. In the shared traces, both workers served hot-lane
requests and their assignment counts differed across same-code retakes. Assigned
captures recorded lane-full admission attempts; total outstanding credits stayed
within the configured ceilings for both candidates. These observations concern
ownership and admission behavior, not physical locality or a faster arrangement.

Response distributions retain scheduled arrival as their origin, including
first-offer lateness and pre-admission waiting. Cancelled outcomes and unadmitted
IDs are not removed to produce a completed-only success census. The report retains
the same-code retakes and their variation. Worker active time is service wall time,
not OS CPU accounting, and the host was not isolated from other activity.

## Review

The current retain/revise/merge/delete decision is
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#rr-d4-behavioral-findings-and-disposition).
No parameter or topology is promoted to a planner default. Parent
[CHECKLIST.md](../../../../CHECKLIST.md) -> `EP-R1.7` owns the contract synthesis;
later paused work is not implicitly resumed by this experiment.

Verification included the package unit/CLI/doc suite, workspace Clippy, and the
persistent sabotage manifest. Wrong routing, duplicate correlation, shifted
latency origin, bypassed verification and a disabled credit guard were caught;
the equivalent service-chunk control survived. Raw records were also checked for
outcome totals, budget and assigned-worker identity before being retained.