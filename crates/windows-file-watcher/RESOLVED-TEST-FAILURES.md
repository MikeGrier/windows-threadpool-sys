# Resolved test failures: windows-file-watcher

Entries moved here from [UNRESOLVED-TEST-FAILURES.md](UNRESOLVED-TEST-FAILURES.md)
once diagnosed, with what the cause turned out to be. Append-only: entries are
never deleted, so a failure that recurs can be matched against one already
understood.

## Resolved 2026-08-28 23:30:20 -04:00 -- `watcher::tests::a_dropped_watcher_stops_delivering` intermittent on CI

**First observed:** 2026-08-29, PR #46, run 33225948225 (job 99029623588),
asserting `a dropped watcher must deliver nothing further`.

**Cause: a defect in the test, not in rundown.** The test asserted on the wrong
side of a queue. Notifications are enqueued by the completion callback and
drained by a *separate* thread (the test's `Drained` pump); the test sampled the
drained count immediately after `drop(watcher)` and compared it 200 ms later.
Teardown guarantees only that nothing further is **enqueued** -- per D-38, the
crate owns when watching stops and the client owns when reading stops -- so a
notification enqueued before teardown but drained after the sample looked
exactly like a post-teardown delivery.

The window is not narrow, because one `std::fs::write` is a create *and* a
write and so produces two notifications, `Added` then `Modified`, which need not
share a completion. `wait_for_name` returns on the first change with a matching
name, so the second was still in flight at the moment the test dropped the
watcher. Measured over 10 runs, the count at that moment was 1 rather than 2 in
4 of them -- the race was live in roughly 40% of runs and lost only because the
pump thread happened to win. On a loaded CI box it does not.

**How it was established.** Starving the drain thread (a 150 ms sleep per item,
standing in for a CI box that will not schedule it promptly) reproduced the
recorded failure deterministically: `settled=1`, count `2` after 200 ms, the
count assertion failing with exactly the recorded message. The same run showed
both notifications naming `before-drop.txt`, and `after-drop.txt` **never**
delivered even after a full drain. Rundown was doing its job throughout.

Worth recording that the original symptom could not distinguish the two
hypotheses: the count assertion ran first, so its failure aborted the test
before the `after-drop.txt` assertion could say whether a post-teardown change
had actually been delivered. The ordering hid the evidence that would have
settled it.

**Fix.** The test now receives inline instead of through a background pump.
After `drop(watcher)` the last sender is released, so the queue is drained until
`recv_timeout` reports the end of the stream, and `is_disconnected` is asserted
so that a drain which stopped on a timeout instead is a failure rather than a
silent gap. The assertions then cover the whole delivered set, and no sleep or
count sampling is involved. Verified non-vacuous by sabotage: deferring teardown
until after the second file is created makes the test fail on the
`after-drop.txt` assertion. Stable over 200 consecutive runs, 100 of them under
2x CPU oversubscription.

The same investigation found the public `DirectoryWatcher::stop` doc comment
saying teardown means "nothing further is delivered", which is the wording that
invited the test's mistake and would invite the same one from a consumer. It now
states that the guarantee covers enqueueing only, and that draining to
disconnection is what completes a delivered set.
