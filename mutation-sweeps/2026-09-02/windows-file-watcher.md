# Mutation survivors -- windows-file-watcher

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 415
- survived: 113
- timeout: 10

## Survived

### src/scenario.rs (71)

```
60:9: replace Rng::next_u64 -> u64 with 0
60:9: replace Rng::next_u64 -> u64 with 1
62:16: replace ^ with | in Rng::next_u64
62:16: replace ^ with & in Rng::next_u64
62:21: replace >> with << in Rng::next_u64
63:16: replace ^ with | in Rng::next_u64
63:16: replace ^ with & in Rng::next_u64
63:21: replace >> with << in Rng::next_u64
64:11: replace ^ with | in Rng::next_u64
64:11: replace ^ with & in Rng::next_u64
64:16: replace >> with << in Rng::next_u64
89:21: replace < with <= in Rng::range
118:5: replace seed -> u64 with 0
118:5: replace seed -> u64 with 1
128:5: replace env_u64 -> u64 with 0
128:5: replace env_u64 -> u64 with 1
161:9: replace millis::deserialize -> Result<Duration, D::Error> with Ok(Default::default())
378:21: delete match arm Operation::Concurrent{branches} in Scenario::operation_count::count
437:9: replace TempDir::cleanup with ()
472:9: replace HarnessParams::for_operation_count -> Self with Default::default()
473:64: replace + with - in HarnessParams::for_operation_count
473:13: delete field timeout from struct Self expression in HarnessParams::for_operation_count
473:58: replace / with % in HarnessParams::for_operation_count
473:64: replace + with * in HarnessParams::for_operation_count
473:58: replace / with * in HarnessParams::for_operation_count
638:9: replace Fleet<'m>::open_session_bounded with ()
651:9: replace Fleet<'m>::close_session with ()
654:45: replace == with != in Fleet<'m>::close_session
669:9: replace Fleet<'m>::subscribe with ()
687:9: replace Fleet<'m>::cancel_watch with ()
699:9: replace Fleet<'m>::drain_available with ()
737:9: replace HarnessOutcome::record with ()
739:30: replace += with -= in HarnessOutcome::record
739:30: replace += with *= in HarnessOutcome::record
740:30: replace += with *= in HarnessOutcome::record
742:57: replace += with -= in HarnessOutcome::record
742:57: replace += with *= in HarnessOutcome::record
743:64: replace += with -= in HarnessOutcome::record
743:64: replace += with *= in HarnessOutcome::record
744:62: replace += with -= in HarnessOutcome::record
744:62: replace += with *= in HarnessOutcome::record
745:69: replace += with -= in HarnessOutcome::record
745:69: replace += with *= in HarnessOutcome::record
746:65: replace += with *= in HarnessOutcome::record
747:72: replace += with -= in HarnessOutcome::record
748:71: replace += with -= in HarnessOutcome::record
747:72: replace += with *= in HarnessOutcome::record
748:71: replace += with *= in HarnessOutcome::record
763:13: replace + with - in HarnessOutcome::total
763:13: replace + with * in HarnessOutcome::total
762:13: replace + with * in HarnessOutcome::total
761:13: replace + with - in HarnessOutcome::total
761:13: replace + with * in HarnessOutcome::total
760:13: replace + with - in HarnessOutcome::total
760:13: replace + with * in HarnessOutcome::total
759:13: replace + with - in HarnessOutcome::total
759:13: replace + with * in HarnessOutcome::total
758:13: replace + with - in HarnessOutcome::total
758:13: replace + with * in HarnessOutcome::total
757:13: replace + with - in HarnessOutcome::total
757:13: replace + with * in HarnessOutcome::total
773:62: replace | with &
773:62: replace | with ^
814:9: replace apply_operation::check_bounded_sleep with ()
997:13: delete match arm Operation::Repeat{count, pattern} in count_barrier_uses
1004:13: delete match arm Operation::Concurrent{branches} in count_barrier_uses
996:59: replace += with -= in count_barrier_uses
1001:54: replace += with *= in count_barrier_uses
1130:35: replace < with == in run_scenario_keep_dir
1130:35: replace < with > in run_scenario_keep_dir
1130:35: replace < with <= in run_scenario_keep_dir
```

### src/watcher.rs (21)

```
86:5: replace | with ^
85:5: replace | with ^
84:5: replace | with ^
83:5: replace | with ^
82:5: replace | with ^
81:5: replace | with ^
562:47: replace match guard changes.is_empty() with false in WatcherInner::publish
629:28: replace == with != in WatcherInner::enter_fault
673:26: replace < with == in WatcherInner::answer
673:26: replace < with > in WatcherInner::answer
673:26: replace < with <= in WatcherInner::answer
759:71: replace && with || in WatcherInner::on_path_based_reopen
851:9: replace WatcherInner::remove_route_from_volume_change -> Option<(usize, Vec<WatchId>)> with None
854:16: delete ! in WatcherInner::remove_route_from_volume_change
1086:31: replace match guard classify(&error) == OpenFailure::Unsupported with true in WatcherInner::install
1086:31: replace match guard classify(&error) == OpenFailure::Unsupported with false in WatcherInner::install
1086:48: replace == with != in WatcherInner::install
1389:54: replace && with || in DirectoryWatcher::remove_route
1519:9: replace DirectoryWatcher::take_routes -> Vec<Route> with vec![]
1578:9: replace <impl std::fmt::Debug for DirectoryWatcher>::fmt -> std::fmt::Result with Ok(Default::default())
1589:9: replace <impl Drop for DirectoryWatcher>::drop with ()
```

### src/directory.rs (8)

```
325:60: replace | with ^ in identify
325:53: replace << with >> in identify
388:52: replace | with ^ in DirectoryHandle::open
388:33: replace | with ^ in DirectoryHandle::open
391:44: replace | with ^ in DirectoryHandle::open
635:33: replace | with ^ in canonical_path
635:33: replace | with & in canonical_path
651:20: replace < with <= in canonical_path
```

### src/bin/run_scenario.rs (4)

```
24:9: replace Output<E, O>::diagnostic with ()
42:5: replace main -> std::process::ExitCode with Default::default()
77:5: replace main -> std::process::ExitCode with Default::default()
29:9: replace Output<E, O>::result with ()
```

### src/coarse.rs (2)

```
94:9: replace <impl Drop for CoarseHandle>::drop with ()
83:9: replace <impl std::fmt::Debug for CoarseHandle>::fmt -> std::fmt::Result with Ok(Default::default())
```

### src/notify.rs (2)

```
274:74: replace - with + in <impl Iterator for Records<'_>>::next
274:74: replace - with / in <impl Iterator for Records<'_>>::next
```

### src/servicing.rs (2)

```
233:9: replace <impl Drop for Servicer<T>>::drop with ()
239:9: replace <impl std::fmt::Debug for Servicer<T>>::fmt -> std::fmt::Result with Ok(Default::default())
```

### src/route.rs (1)

```
167:9: replace <impl std::fmt::Debug for Route>::fmt -> std::fmt::Result with Ok(Default::default())
```

### src/testing.rs (1)

```
34:9: replace TempDir::cleanup with ()
```

### src/watch.rs (1)

```
208:9: replace Watch::cancel with ()
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/scenario.rs (3)

```
86:42: replace % with / in Rng::range
89:21: replace < with == in Rng::range
89:21: replace < with > in Rng::range
```

### src/servicing.rs (3)

```
141:9: replace Servicer<T>::submit -> Result<(), Rejected<T>> with Ok(())
147:36: replace == with != in Servicer<T>::submit
258:5: replace drain with ()
```

### src/queue.rs (2)

```
942:9: replace Receiver::recv_timeout -> Option<Notification> with None
1125:5: replace take -> Option<Entry> with None
```

### src/directory.rs (1)

```
651:20: replace < with == in canonical_path
```

### src/watcher.rs (1)

```
1629:17: replace != with == in classify_submission
```
