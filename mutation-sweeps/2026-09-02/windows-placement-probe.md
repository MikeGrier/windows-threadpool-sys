# Mutation survivors -- windows-placement-probe

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 315
- survived: 199
- timeout: 4

**Partly addressed already** in commit(s) `47dfa18` on branch
`mikegrier/deferred-namespace-ops`. The entries below are as the sweep
found them and have NOT been pruned -- re-run before treating any single
line as outstanding.

## Survived

### src/peer_index_cache.rs (70)

```
106:9: replace Strategy::label -> &'static str with ""
106:9: replace Strategy::label -> &'static str with "xyzzy"
122:9: replace Strategy::name -> &'static str with ""
122:9: replace Strategy::name -> &'static str with "xyzzy"
166:9: replace Observation::get -> Option<Run> with None
168:35: replace == with != in Observation::get
209:38: replace / with % in median
209:38: replace / with * in median
213:38: replace / with % in median
213:38: replace / with * in median
214:40: replace / with % in median
214:40: replace / with * in median
214:56: replace / with % in median
214:56: replace / with * in median
238:17: replace < with == in time_real_spsc
238:17: replace < with > in time_real_spsc
238:17: replace < with <= in time_real_spsc
240:19: replace += with -= in time_real_spsc
240:19: replace += with *= in time_real_spsc
288:28: replace - with / in Ring::new_on
288:28: replace - with + in Ring::new_on
334:48: replace - with +
349:51: replace - with +
349:51: replace - with /
463:29: replace | with ^ in Slots::on_numa_node
504:9: replace <impl Drop for Slots>::drop with ()
544:5: replace observed_node_of_region -> Option<u32> with Some(0)
552:28: replace match guard first == page_node with true in observed_node_of_region
570:21: replace match guard size > 0 with true in page_size
565:5: replace page_size -> usize with 1
570:21: replace match guard size > 0 with false in page_size
570:26: replace > with == in page_size
570:26: replace > with >= in page_size
570:26: replace > with < in page_size
586:26: replace >> with << in observed_node
696:52: replace == with != in time_model_placed
776:23: replace << with >> in pin_current_thread
887:5: replace produce -> u64 with 0
887:5: replace produce -> u64 with 1
897:31: replace += with -= in produce
897:31: replace += with *= in produce
898:76: replace == with != in produce
901:31: replace += with -= in produce
901:31: replace += with *= in produce
906:76: replace == with != in produce
911:55: replace == with != in produce
912:35: replace += with -= in produce
912:35: replace += with *= in produce
915:52: replace == with != in produce
928:34: replace & with | in produce
928:34: replace & with ^ in produce
941:5: replace consume -> u64 with 0
941:5: replace consume -> u64 with 1
945:17: replace < with == in consume
945:17: replace < with > in consume
945:17: replace < with <= in consume
950:27: replace += with -= in consume
950:27: replace += with *= in consume
951:22: replace == with != in consume
954:27: replace += with -= in consume
954:27: replace += with *= in consume
956:22: replace == with != in consume
962:25: replace == with != in consume
963:31: replace += with -= in consume
963:31: replace += with *= in consume
966:22: replace == with != in consume
977:46: replace & with | in consume
977:46: replace & with ^ in consume
980:15: replace += with -= in consume
980:15: replace += with *= in consume
```

### src/core_affinity.rs (54)

```
108:5: replace within_class_pair -> Option<(ProcessorPlace, ProcessorPlace)> with None
110:48: replace == with != in within_class_pair
116:41: replace && with || in within_class_pair
116:31: replace != with == in within_class_pair
116:59: replace == with != in within_class_pair
122:5: replace efficiency_classes -> Vec<u8> with vec![]
122:5: replace efficiency_classes -> Vec<u8> with vec![0]
122:5: replace efficiency_classes -> Vec<u8> with vec![1]
197:61: replace * with + in RunPlan::timed_runs
216:9: replace RunPlan::estimated_seconds -> f64 with 0.0
216:9: replace RunPlan::estimated_seconds -> f64 with 1.0
216:44: replace * with + in RunPlan::estimated_seconds
216:44: replace * with / in RunPlan::estimated_seconds
216:28: replace * with + in RunPlan::estimated_seconds
216:28: replace * with / in RunPlan::estimated_seconds
520:5: replace assert_group_support with ()
645:48: replace / with % in measure
645:48: replace / with * in measure
653:46: replace / with % in measure
653:46: replace / with * in measure
654:46: replace / with % in measure
654:46: replace / with * in measure
655:46: replace / with % in measure
655:46: replace / with * in measure
679:48: replace / with % in measure
679:48: replace / with * in measure
686:46: replace / with % in measure
686:46: replace / with * in measure
687:46: replace / with % in measure
687:46: replace / with * in measure
688:46: replace / with % in measure
688:46: replace / with * in measure
715:52: replace / with % in measure
715:52: replace / with * in measure
722:50: replace / with % in measure
723:50: replace / with % in measure
722:50: replace / with * in measure
723:50: replace / with * in measure
724:50: replace / with % in measure
724:50: replace / with * in measure
749:9: replace Observation::get -> Option<Measurement> with None
751:48: replace && with || in Observation::get
751:35: replace == with != in Observation::get
751:62: replace == with != in Observation::get
763:9: replace Observation::node_pairs_measured -> Vec<(u32, u32)> with vec![]
763:9: replace Observation::node_pairs_measured -> Vec<(u32, u32)> with vec![(0, 1)]
763:9: replace Observation::node_pairs_measured -> Vec<(u32, u32)> with vec![(0, 0)]
763:9: replace Observation::node_pairs_measured -> Vec<(u32, u32)> with vec![(1, 0)]
763:9: replace Observation::node_pairs_measured -> Vec<(u32, u32)> with vec![(1, 1)]
786:9: replace Observation::node_pair_rows -> Vec<Measurement> with vec![]
789:70: replace && with || in Observation::node_pair_rows
789:62: replace == with != in Observation::node_pair_rows
789:84: replace == with != in Observation::node_pair_rows
829:9: replace Observation::placements -> Vec<Placement> with vec![]
```

### src/fingerprint.rs (26)

```
207:9: replace Slice::same_cache_domain -> Option<bool> with None
207:9: replace Slice::same_cache_domain -> Option<bool> with Some(true)
207:9: replace Slice::same_cache_domain -> Option<bool> with Some(false)
212:42: replace == with != in Slice::same_cache_domain
220:9: replace Slice::same_efficiency_class -> Option<bool> with None
220:9: replace Slice::same_efficiency_class -> Option<bool> with Some(true)
220:9: replace Slice::same_efficiency_class -> Option<bool> with Some(false)
225:40: replace == with != in Slice::same_efficiency_class
231:9: replace <impl fmt::Display for Slice>::fmt -> fmt::Result with Ok(Default::default())
367:65: replace > with == in Fingerprint::from_topology
367:65: replace > with < in Fingerprint::from_topology
367:65: replace > with >= in Fingerprint::from_topology
380:43: replace == with != in Fingerprint::from_topology
382:44: replace += with *= in Fingerprint::from_topology
412:34: replace > with == in Fingerprint::from_topology
412:34: replace > with < in Fingerprint::from_topology
412:34: replace > with >= in Fingerprint::from_topology
482:22: replace > with < in <impl fmt::Display for Fingerprint>::fmt
502:5: replace discover_places -> std::io::Result<Vec<ProcessorPlace>> with Ok(vec![])
543:9: replace MissingPlacement::what -> &'static str with ""
543:9: replace MissingPlacement::what -> &'static str with "xyzzy"
694:25: replace match guard !any_core_domain with true in places_from_topology
694:67: replace | with ^ in places_from_topology
708:25: replace match guard !any_core_domain with true in places_from_topology
749:5: replace print_banner with ()
772:5: replace print_banner_with with ()
```

### src/machine.rs (14)

```
131:5: replace read_cpu_model -> Option<String> with None
131:5: replace read_cpu_model -> Option<String> with Some("xyzzy".into())
136:21: delete ! in read_cpu_model
212:5: replace detect_virtualisation -> (VirtualisationHint, Option<String>) with (Default::default(), None)
268:35: replace * with + in read_registry_string
268:35: replace * with / in read_registry_string
286:15: replace == with != in read_registry_string
288:31: replace * with + in read_registry_string
288:31: replace * with / in read_registry_string
308:32: replace / with * in read_registry_string
324:5: replace read_registry_u32 -> Option<u32> with None
324:5: replace read_registry_u32 -> Option<u32> with Some(0)
324:5: replace read_registry_u32 -> Option<u32> with Some(1)
346:13: replace == with != in read_registry_u32
```

### src/paste_json.rs (13)

```
89:9: replace <impl Visitor<'de> for NodeVisitor>::expecting -> fmt::Result with Ok(Default::default())
245:31: replace match guard !items.is_empty() with true in write_value
236:66: replace + with - in write_value
236:66: replace + with * in write_value
236:49: replace + with * in write_value
247:32: replace + with * in write_value
284:63: replace + with - in write_filled
284:71: replace > with >= in write_filled
284:63: replace + with * in write_filled
284:49: replace + with * in write_filled
284:37: replace + with - in write_filled
284:37: replace + with * in write_filled
290:12: delete ! in write_filled
```

### src/bin/placement_probe/main.rs (10)

```
45:5: replace main -> ExitCode with Default::default()
53:5: replace run -> ExitCode with Default::default()
190:8: delete ! in run
162:25: replace != with == in run
339:5: replace write_backup with ()
435:27: replace match guard error.kind() == std::io::ErrorKind::AlreadyExists with true in write_backup_with
478:36: replace == with != in write_temporary
491:27: replace match guard error.kind() == std::io::ErrorKind::AlreadyExists with true in write_temporary
564:5: replace help -> String with String::new()
564:5: replace help -> String with "xyzzy".into()
```

### src/build_identity.rs (8)

```
30:9: replace BuildSource::label -> &'static str with ""
30:9: replace BuildSource::label -> &'static str with "xyzzy"
43:9: replace <impl fmt::Display for BuildSource>::fmt -> fmt::Result with Ok(Default::default())
91:17: delete match arm "0" in BuildIdentity::current
90:17: delete match arm "1" in BuildIdentity::current
95:17: delete match arm "ci" in BuildIdentity::current
96:17: delete match arm "local" in BuildIdentity::current
140:5: replace non_empty -> Option<&'static str> with None
```

### src/bin/placement_probe/sink.rs (2)

```
51:9: replace <impl Sink for Stdio>::line with ()
55:9: replace <impl Sink for Stdio>::problem with ()
```

### src/record.rs (1)

```
336:9: replace <impl fmt::Display for SubmissionRecord>::fmt -> fmt::Result with Ok(Default::default())
```

### src/submission.rs (1)

```
54:14: replace ^= with |= in checksum
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/peer_index_cache.rs (4)

```
557:16: replace += with *= in observed_node_of_region
565:5: replace page_size -> usize with 0
703:74: replace == with != in time_model_placed
738:9: replace <impl Drop for PinSignal<'_>>::drop with ()
```
