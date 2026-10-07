# Checklist: the sabotage harness kills a hung run through a job object

A sweep of `windows-waitable-queues` sat for its job's full hour on one entry (run
37656687292, shard 0/4), past every bound
[run-sabotage.ps1](tools/run-sabotage.ps1) applies. The one unbounded step on that
path was the kill: `Stop-Tree` walks descendants with a WMI query and stops each
with `Stop-Process`. Independently of what stalled, a parent-PID walk cannot find a
descendant whose parent has already exited. The engineer chose to replace the walk
with a Windows job object, launched by a tool written in Rust rather than a C#
helper, so the repository does not take on another language.

Feature-scoped: deleted when complete, its content moved to the root
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## JL-M1 -- A job-object launcher, and the harness on it

- [x] **JL-1.1** -- Narrate each kill under `-TraceKills`, and give the sweep step a
  timeout below the job's so a stall uploads its transcripts. Landed as e6cd07d3,
  before this checklist existed.

- [ ] **JL-1.2** -- **The launcher crate, `crates/win-job-launcher` (a binary of
  the same name, `publish = false`).** Runs one command inside a fresh job
  object under a wall-clock bound, redirecting its stdout and stderr to files, and
  writes the outcome (exited with a code, timed out and killed, or not started) to
  a result file. The child is created suspended, assigned to the job, then resumed,
  so no descendant can start outside it; the job is `KILL_ON_JOB_CLOSE`, so the
  tree also dies if the launcher does. On the bound it calls `TerminateJobObject`.
  `--trace` narrates each step to stderr. Unit tests for argument parsing and the
  result format; tests over real processes for exit, timeout, a grandchild, and a
  grandchild whose parent has exited.

- [ ] **JL-1.3** -- **The harness launches every phase through `win-job-launcher`.**
  `Invoke-Bounded` starts the launcher instead of cargo and reads its result file;
  `Stop-Tree` is deleted. The harness keeps a backstop deadline over the launcher
  itself, and stopping the launcher kills the tree through `KILL_ON_JOB_CLOSE`. The
  launcher is built once per harness run (or passed in), so test stubs still need
  no build of their own. A harness test with a detached grandchild whose parent
  exits shows the gap the tree walk had; `sabotage2.json`'s kill entry retargets
  to the new kill; `-TraceKills` forwards to `--trace`. Docs in
  [README-sabotage.md](tools/README-sabotage.md) and the script's help.
