# Mutation survivors -- windows-file-watcher-example-test-harness

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 77
- survived: 69
- timeout: 3

## Survived

### src/generator.rs (26)

```
106:16: replace ^ with | in Rng::next_u64
106:16: replace ^ with & in Rng::next_u64
106:21: replace >> with << in Rng::next_u64
107:16: replace ^ with & in Rng::next_u64
107:16: replace ^ with | in Rng::next_u64
107:21: replace >> with << in Rng::next_u64
108:16: replace >> with << in Rng::next_u64
118:21: replace < with <= in Rng::below
158:23: replace - with + in Rng::weighted
158:23: replace - with / in Rng::weighted
247:9: replace Generator::config -> &GeneratorConfig with Box::leak(Box::new(Default::default()))
393:39: replace && with || in Generator::generate_watch
441:9: replace Generator::pick_index -> usize with 0
493:9: delete match arm 0 in gen_change_kind
494:9: delete match arm 1 in gen_change_kind
496:9: delete match arm 3 in gen_change_kind
495:9: delete match arm 2 in gen_change_kind
503:5: replace gen_name -> String with String::new()
503:5: replace gen_name -> String with "xyzzy".into()
570:9: delete match arm 0 in gen_detail
578:9: delete match arm 1 in gen_detail
604:33: replace & with | in gen_volume
604:33: replace & with ^ in gen_volume
623:20: replace + with * in gen_changed_volume
623:52: replace - with + in gen_changed_volume
623:52: replace - with / in gen_changed_volume
```

### src/bin/replay.rs (17)

```
34:9: replace Output<E, O>::report with ()
46:5: replace main -> std::process::ExitCode with Default::default()
110:48: replace * with +
110:48: replace * with /
110:41: replace * with +
110:41: replace * with /
122:9: replace imp::load_bounded -> Option<Recording> with None
135:59: replace + with - in imp::load_bounded
135:59: replace + with * in imp::load_bounded
139:30: replace > with == in imp::load_bounded
139:30: replace > with >= in imp::load_bounded
157:37: replace > with == in imp::load_bounded
157:37: replace > with < in imp::load_bounded
157:37: replace > with >= in imp::load_bounded
168:9: replace imp::main -> std::process::ExitCode with Default::default()
197:9: replace imp::replay -> bool with false
197:9: replace imp::replay -> bool with true
```

### src/bin/capture.rs (10)

```
39:9: replace Output<E, O>::report with ()
51:5: replace main -> std::process::ExitCode with Default::default()
73:9: replace imp::main -> std::process::ExitCode with Default::default()
89:9: replace imp::capture -> bool with true
89:9: replace imp::capture -> bool with false
98:30: replace + with - in imp::capture
98:30: replace + with * in imp::capture
118:23: replace += with *= in imp::capture
146:13: replace imp::Args::parse -> Option<Self> with None
118:23: replace += with -= in imp::capture
```

### src/example_handler.rs (10)

```
54:9: replace PresenceTracker::rescans -> u32 with 1
65:9: replace PresenceTracker::stopped -> &BTreeSet<WatchId> with Box::leak(Box::new(BTreeSet::new()))
71:9: replace PresenceTracker::is_subscribed -> bool with true
77:9: replace PresenceTracker::volume_changes -> u32 with 0
77:9: replace PresenceTracker::volume_changes -> u32 with 1
106:13: delete match arm Notification::VolumeChanged{..} in <impl Handler for PresenceTracker>::on
97:54: replace match guard cause.is_terminal() with false in <impl Handler for PresenceTracker>::on
106:71: replace += with -= in <impl Handler for PresenceTracker>::on
106:71: replace += with *= in <impl Handler for PresenceTracker>::on
149:31: replace += with -= in <impl Handler for BuggyHandler>::on
```

### src/oracle.rs (3)

```
34:9: replace Outcome::is_healthy -> bool with true
201:5: replace panic_message -> String with String::new()
201:5: replace panic_message -> String with "xyzzy".into()
```

### src/schedule.rs (3)

```
229:9: replace Schedule::len -> usize with 1
229:9: replace Schedule::len -> usize with 0
235:9: replace Schedule::is_empty -> bool with false
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/generator.rs (3)

```
115:42: replace % with / in Rng::below
118:21: replace < with == in Rng::below
118:21: replace < with > in Rng::below
```
