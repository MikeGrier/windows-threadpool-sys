# Mutation survivors -- windows-platform-probes

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 102
- survived: 511
- timeout: 5

## Survived

### src/ioring.rs (81)

```
98:9: replace IoRingSupport<T>::measured -> Option<T> with None
141:9: replace Ring::submit_and_wait -> (i32, u32) with (0, 0)
141:9: replace Ring::submit_and_wait -> (i32, u32) with (0, 1)
141:9: replace Ring::submit_and_wait -> (i32, u32) with (1, 0)
141:9: replace Ring::submit_and_wait -> (i32, u32) with (1, 1)
141:9: replace Ring::submit_and_wait -> (i32, u32) with (-1, 0)
141:9: replace Ring::submit_and_wait -> (i32, u32) with (-1, 1)
164:9: replace Ring::collect -> Option<IORING_CQE> with None
164:9: replace Ring::collect -> Option<IORING_CQE> with Some(Default::default())
167:25: replace > with == in Ring::collect
167:25: replace > with < in Ring::collect
167:25: replace > with >= in Ring::collect
169:27: replace -= with += in Ring::collect
169:27: replace -= with /= in Ring::collect
201:9: replace Ring::pop -> Option<IORING_CQE> with Some(Default::default())
201:9: replace Ring::pop -> Option<IORING_CQE> with None
205:17: replace == with != in Ring::pop
212:9: replace <impl Drop for Ring>::drop with ()
219:5: replace is_available -> bool with true
219:5: replace is_available -> bool with false
268:9: replace <impl Drop for Fixture>::drop with ()
340:9: replace PipePair::server_handle -> *mut c_void with Default::default()
345:9: replace PipePair::fill with ()
377:9: replace RegistrationObservation::replaces -> bool with true
377:9: replace RegistrationObservation::replaces -> bool with false
377:45: replace && with || in RegistrationObservation::replaces
377:48: delete ! in RegistrationObservation::replaces
383:9: replace RegistrationObservation::appends -> bool with true
383:9: replace RegistrationObservation::appends -> bool with false
383:45: replace && with || in RegistrationObservation::appends
391:5: replace register -> i32 with 0
391:5: replace register -> i32 with 1
391:5: replace register -> i32 with -1
399:14: replace < with == in register
399:14: replace < with > in register
399:14: replace < with <= in register
404:18: replace == with != in register
406:22: replace < with == in register
406:22: replace < with > in register
406:22: replace < with <= in register
415:5: replace read_through_index -> i32 with 0
415:5: replace read_through_index -> i32 with 1
415:5: replace read_through_index -> i32 with -1
431:14: replace < with == in read_through_index
431:14: replace < with > in read_through_index
431:14: replace < with <= in read_through_index
436:18: replace == with != in read_through_index
438:22: replace < with == in read_through_index
438:22: replace < with > in read_through_index
438:22: replace < with <= in read_through_index
453:5: replace read_raw_handle -> (i32, usize, u8) with (0, 0, 0)
453:5: replace read_raw_handle -> (i32, usize, u8) with (0, 0, 1)
453:5: replace read_raw_handle -> (i32, usize, u8) with (0, 1, 0)
453:5: replace read_raw_handle -> (i32, usize, u8) with (0, 1, 1)
453:5: replace read_raw_handle -> (i32, usize, u8) with (1, 0, 0)
453:5: replace read_raw_handle -> (i32, usize, u8) with (1, 0, 1)
453:5: replace read_raw_handle -> (i32, usize, u8) with (1, 1, 0)
453:5: replace read_raw_handle -> (i32, usize, u8) with (1, 1, 1)
453:5: replace read_raw_handle -> (i32, usize, u8) with (-1, 0, 0)
453:5: replace read_raw_handle -> (i32, usize, u8) with (-1, 0, 1)
453:5: replace read_raw_handle -> (i32, usize, u8) with (-1, 1, 0)
453:5: replace read_raw_handle -> (i32, usize, u8) with (-1, 1, 1)
482:14: replace < with == in read_raw_handle
482:14: replace < with > in read_raw_handle
482:14: replace < with <= in read_raw_handle
487:18: replace == with != in read_raw_handle
489:23: replace < with == in read_raw_handle
489:23: replace < with > in read_raw_handle
489:23: replace < with <= in read_raw_handle
539:70: replace >= with < in measure_registration
540:69: replace >= with < in measure_registration
579:9: replace ThreadAgnosticism::survives_submitter_exit -> bool with true
579:9: replace ThreadAgnosticism::survives_submitter_exit -> bool with false
579:65: replace && with || in ThreadAgnosticism::survives_submitter_exit
579:40: replace && with || in ThreadAgnosticism::survives_submitter_exit
579:60: replace >= with < in ThreadAgnosticism::survives_submitter_exit
683:34: replace && with || in measure_thread_agnosticism
683:30: replace > with == in measure_thread_agnosticism
683:30: replace > with < in measure_thread_agnosticism
683:30: replace > with >= in measure_thread_agnosticism
683:83: replace == with != in measure_thread_agnosticism
```

### src/queue_contention.rs (55)

```
125:9: replace Observation::find -> Option<Run> with None
127:44: replace && with || in Observation::find
127:35: replace == with != in Observation::find
127:61: replace == with != in Observation::find
138:9: replace Observation::scaling -> Option<f64> with None
138:9: replace Observation::scaling -> Option<f64> with Some(0.0)
138:9: replace Observation::scaling -> Option<f64> with Some(1.0)
138:9: replace Observation::scaling -> Option<f64> with Some(-1.0)
140:37: replace / with % in Observation::scaling
140:37: replace / with * in Observation::scaling
195:57: replace / with % in median_run
195:57: replace / with * in median_run
197:29: replace * with + in median_run
197:29: replace * with / in median_run
201:39: replace / with % in median_run
201:39: replace / with * in median_run
202:35: replace / with * in median_run
202:35: replace / with % in median_run
202:52: replace / with * in median_run
202:52: replace / with % in median_run
214:5: replace time_contended_atomic -> Repetition with Default::default()
219:48: replace + with - in time_contended_atomic
219:48: replace + with * in time_contended_atomic
239:5: replace capacity_for -> usize with 0
239:5: replace capacity_for -> usize with 1
239:16: replace * with + in capacity_for
239:16: replace * with / in capacity_for
256:40: replace + with - in start_barrier
256:40: replace + with * in start_barrier
260:5: replace time_isolated_mpsc -> Repetition with Default::default()
270:61: replace + with - in time_isolated_mpsc
270:61: replace + with * in time_isolated_mpsc
270:39: replace * with + in time_isolated_mpsc
270:39: replace * with / in time_isolated_mpsc
287:5: replace time_isolated_reserving -> Repetition with Default::default()
297:61: replace + with - in time_isolated_reserving
297:61: replace + with * in time_isolated_reserving
297:39: replace * with + in time_isolated_reserving
297:39: replace * with / in time_isolated_reserving
316:5: replace time_drained_mpsc -> Repetition with Default::default()
323:40: replace + with - in time_drained_mpsc
323:40: replace + with * in time_drained_mpsc
330:15: delete ! in time_drained_mpsc
345:68: replace + with - in time_drained_mpsc
345:68: replace + with * in time_drained_mpsc
345:46: replace * with + in time_drained_mpsc
345:46: replace * with / in time_drained_mpsc
380:5: replace time_drained_reserving -> Repetition with Default::default()
385:40: replace + with - in time_drained_reserving
385:40: replace + with * in time_drained_reserving
390:15: delete ! in time_drained_reserving
405:68: replace + with - in time_drained_reserving
405:68: replace + with * in time_drained_reserving
405:46: replace * with + in time_drained_reserving
405:46: replace * with / in time_drained_reserving
```

### src/device_map.rs (52)

```
77:9: replace MapObservation::is_found -> bool with true
77:9: replace MapObservation::is_found -> bool with false
88:9: replace MapObservation::entries -> Vec<&str> with vec![]
88:9: replace MapObservation::entries -> Vec<&str> with vec![""]
88:9: replace MapObservation::entries -> Vec<&str> with vec!["xyzzy"]
89:45: delete ! in MapObservation::entries
96:9: replace MapObservation::is_exactly -> bool with true
96:24: replace == with != in MapObservation::is_exactly
96:9: replace MapObservation::is_exactly -> bool with false
121:9: replace DeviceMapFinding::impersonation_changes_the_map -> bool with true
121:9: replace DeviceMapFinding::impersonation_changes_the_map -> bool with false
121:37: replace && with || in DeviceMapFinding::impersonation_changes_the_map
121:40: delete ! in DeviceMapFinding::impersonation_changes_the_map
131:9: replace DeviceMapFinding::claim_is_exclusive -> bool with true
131:9: replace DeviceMapFinding::claim_is_exclusive -> bool with false
140:9: replace DeviceMapFinding::sessions_differ -> bool with true
140:9: replace DeviceMapFinding::sessions_differ -> bool with false
144:13: delete match arm (Some(own), Some(anonymous)) in DeviceMapFinding::sessions_differ
144:49: replace != with == in DeviceMapFinding::sessions_differ
158:5: replace effective_logon_session -> Option<(u32, i32)> with None
158:5: replace effective_logon_session -> Option<(u32, i32)> with Some((0, 0))
158:5: replace effective_logon_session -> Option<(u32, i32)> with Some((0, 1))
158:5: replace effective_logon_session -> Option<(u32, i32)> with Some((0, -1))
158:5: replace effective_logon_session -> Option<(u32, i32)> with Some((1, 0))
158:5: replace effective_logon_session -> Option<(u32, i32)> with Some((1, 1))
158:5: replace effective_logon_session -> Option<(u32, i32)> with Some((1, -1))
162:15: replace == with != in effective_logon_session
165:19: replace == with != in effective_logon_session
185:13: replace == with != in effective_logon_session
207:29: replace == with != in query
261:9: replace SubstDrive::claim -> Option<Self> with None
282:53: replace != with == in SubstDrive::claim
282:21: replace & with | in SubstDrive::claim
282:21: replace & with ^ in SubstDrive::claim
282:26: replace << with >> in SubstDrive::claim
282:44: replace - with / in SubstDrive::claim
282:44: replace - with + in SubstDrive::claim
319:9: replace SubstDrive::letter -> &str with ""
292:24: replace == with != in SubstDrive::claim
325:9: replace SubstDrive::target -> &str with ""
319:9: replace SubstDrive::letter -> &str with "xyzzy"
331:9: replace <impl Drop for SubstDrive>::drop with ()
325:9: replace SubstDrive::target -> &str with "xyzzy"
342:5: replace remove with ()
344:63: replace | with & in remove
344:35: replace | with & in remove
344:63: replace | with ^ in remove
344:35: replace | with ^ in remove
353:5: replace wide -> Vec<u16> with vec![]
353:5: replace wide -> Vec<u16> with vec![0]
353:5: replace wide -> Vec<u16> with vec![1]
368:45: replace == with != in measure_with_subst
```

### src/bin/core_affinity.rs (48)

```
14:5: replace main -> std::io::Result<()> with Ok(())
20:5: replace render -> String with String::new()
20:5: replace render -> String with "xyzzy".into()
82:8: delete ! in render
103:64: replace && with || in render
103:55: replace == with != in render
103:78: replace == with != in render
107:64: replace && with || in render
107:55: replace == with != in render
107:78: replace == with != in render
198:26: replace < with == in render
198:26: replace < with > in render
198:26: replace < with <= in render
220:9: replace && with || in render
219:22: delete ! in render
220:12: delete ! in render
280:50: replace / with % in render
280:50: replace / with * in render
312:18: replace > with < in render
312:18: replace > with == in render
312:18: replace > with >= in render
312:25: replace * with + in render
312:25: replace * with / in render
329:24: replace > with == in render
329:24: replace > with < in render
329:24: replace > with >= in render
329:32: replace * with + in render
329:32: replace * with / in render
426:43: replace / with % in render
426:43: replace / with * in render
427:34: replace >= with < in render
429:27: replace <= with > in render
445:23: replace > with == in render
445:23: replace > with < in render
445:23: replace > with >= in render
478:5: replace render_node_distances with ()
546:55: replace > with == in render_node_distances
546:55: replace > with < in render_node_distances
546:55: replace > with >= in render_node_distances
549:54: replace < with == in render_node_distances
549:54: replace < with > in render_node_distances
549:54: replace < with <= in render_node_distances
555:20: replace == with != in render_node_distances
581:21: replace < with == in render_node_distances
581:21: replace < with > in render_node_distances
581:21: replace < with <= in render_node_distances
581:14: replace / with % in render_node_distances
581:14: replace / with * in render_node_distances
```

### src/pool_growth.rs (46)

```
69:9: replace Gate::wait with ()
75:9: replace Gate::open with ()
82:9: replace <impl Drop for Gate>::drop with ()
108:9: replace GrowthObservation::saturated -> bool with true
108:9: replace GrowthObservation::saturated -> bool with false
108:36: replace >= with < in GrowthObservation::saturated
117:9: replace GrowthObservation::one_thread_each -> bool with false
117:9: replace GrowthObservation::one_thread_each -> bool with true
117:31: replace == with != in GrowthObservation::one_thread_each
124:9: replace GrowthObservation::slowest_arrival -> Duration with Default::default()
136:9: replace GrowthObservation::largest_gap -> Duration with Default::default()
159:9: replace GrowthObservation::throttles_after -> Option<usize> with None
159:9: replace GrowthObservation::throttles_after -> Option<usize> with Some(0)
159:9: replace GrowthObservation::throttles_after -> Option<usize> with Some(1)
163:62: replace >= with < in GrowthObservation::throttles_after
164:32: replace + with - in GrowthObservation::throttles_after
164:32: replace + with * in GrowthObservation::throttles_after
190:9: replace <impl Drop for GateGuard>::drop with ()
284:35: replace + with - in measure_growth
285:37: replace && with || in measure_growth
285:26: replace < with == in measure_growth
285:26: replace < with > in measure_growth
285:26: replace < with <= in measure_growth
285:75: replace < with == in measure_growth
285:75: replace < with > in measure_growth
285:75: replace < with <= in measure_growth
344:9: replace RaiseObservation::saturated_before_raise -> bool with true
344:35: replace == with != in RaiseObservation::saturated_before_raise
344:9: replace RaiseObservation::saturated_before_raise -> bool with false
401:35: replace + with - in measure_raise_while_saturated
402:37: replace && with || in measure_raise_while_saturated
402:26: replace < with == in measure_raise_while_saturated
402:26: replace < with > in measure_raise_while_saturated
402:26: replace < with <= in measure_raise_while_saturated
402:75: replace < with == in measure_raise_while_saturated
402:75: replace < with > in measure_raise_while_saturated
402:75: replace < with <= in measure_raise_while_saturated
414:35: replace + with - in measure_raise_while_saturated
415:37: replace && with || in measure_raise_while_saturated
415:26: replace < with == in measure_raise_while_saturated
415:26: replace < with > in measure_raise_while_saturated
415:26: replace < with <= in measure_raise_while_saturated
415:75: replace <= with > in measure_raise_while_saturated
418:58: replace > with == in measure_raise_while_saturated
418:58: replace > with < in measure_raise_while_saturated
418:58: replace > with >= in measure_raise_while_saturated
```

### src/bin/peer_index_cache.rs (34)

```
12:5: replace main with ()
49:9: replace / with % in main
48:42: replace - with + in main
49:9: replace / with * in main
58:14: replace > with == in main
48:42: replace - with / in main
58:14: replace > with < in main
58:14: replace > with >= in main
75:47: replace / with * in main
75:47: replace / with % in main
107:39: replace / with % in main
107:39: replace / with * in main
108:39: replace / with % in main
108:39: replace / with * in main
110:44: replace / with % in main
110:44: replace / with * in main
112:44: replace / with % in main
112:44: replace / with * in main
113:43: replace / with % in main
113:43: replace / with * in main
125:38: replace > with == in main
125:38: replace > with < in main
125:38: replace > with >= in main
126:8: delete ! in main
130:23: replace >= with < in main
135:23: replace <= with > in main
141:31: replace < with == in main
141:31: replace < with > in main
141:31: replace < with <= in main
165:44: replace / with % in main
165:44: replace / with * in main
172:23: replace < with == in main
172:23: replace < with > in main
172:23: replace < with <= in main
```

### src/completion_port.rs (33)

```
91:9: replace ReadAttempt::succeeded -> bool with true
91:9: replace ReadAttempt::succeeded -> bool with false
91:60: replace && with || in ReadAttempt::succeeded
91:31: replace && with || in ReadAttempt::succeeded
91:26: replace >= with < in ReadAttempt::succeeded
91:45: replace == with != in ReadAttempt::succeeded
91:79: replace == with != in ReadAttempt::succeeded
125:9: replace CompletionPortFinding::is_valid -> bool with true
125:9: replace CompletionPortFinding::is_valid -> bool with false
127:13: replace && with || in CompletionPortFinding::is_valid
126:13: replace && with || in CompletionPortFinding::is_valid
135:9: replace CompletionPortFinding::association_forecloses_ioring -> bool with true
135:9: replace CompletionPortFinding::association_forecloses_ioring -> bool with false
137:13: replace && with || in CompletionPortFinding::association_forecloses_ioring
136:13: replace && with || in CompletionPortFinding::association_forecloses_ioring
136:16: delete ! in CompletionPortFinding::association_forecloses_ioring
137:16: delete ! in CompletionPortFinding::association_forecloses_ioring
145:9: replace CompletionPortFinding::threadpool_io_forecloses_ioring -> bool with true
145:9: replace CompletionPortFinding::threadpool_io_forecloses_ioring -> bool with false
145:25: replace && with || in CompletionPortFinding::threadpool_io_forecloses_ioring
145:28: delete ! in CompletionPortFinding::threadpool_io_forecloses_ioring
186:9: replace <impl Drop for PoolIo>::drop with ()
295:5: replace read_through_port -> bool with true
295:5: replace read_through_port -> bool with false
314:16: replace == with != in read_through_port
317:18: replace != with == in read_through_port
363:77: replace && with || in read_through_port
363:52: replace && with || in read_through_port
363:19: replace && with || in read_through_port
363:14: replace != with == in read_through_port
363:59: replace == with != in read_through_port
363:37: replace == with != in read_through_port
363:90: replace == with != in read_through_port
```

### src/doorbell_cost.rs (26)

```
95:9: replace Observation::get -> Option<f64> with None
95:9: replace Observation::get -> Option<f64> with Some(0.0)
95:9: replace Observation::get -> Option<f64> with Some(1.0)
95:9: replace Observation::get -> Option<f64> with Some(-1.0)
97:31: replace == with != in Observation::get
111:9: replace Observation::doorbell_share_of_submit -> Option<f64> with None
111:9: replace Observation::doorbell_share_of_submit -> Option<f64> with Some(0.0)
111:9: replace Observation::doorbell_share_of_submit -> Option<f64> with Some(1.0)
111:9: replace Observation::doorbell_share_of_submit -> Option<f64> with Some(-1.0)
113:17: replace > with == in Observation::doorbell_share_of_submit
113:17: replace > with < in Observation::doorbell_share_of_submit
113:17: replace > with >= in Observation::doorbell_share_of_submit
113:43: replace / with % in Observation::doorbell_share_of_submit
113:43: replace / with * in Observation::doorbell_share_of_submit
131:49: replace / with % in time_loop
131:49: replace / with * in time_loop
222:5: replace measure_park_and_wake -> Option<f64> with None
222:5: replace measure_park_and_wake -> Option<f64> with Some(0.0)
222:5: replace measure_park_and_wake -> Option<f64> with Some(1.0)
222:5: replace measure_park_and_wake -> Option<f64> with Some(-1.0)
244:23: replace != with == in measure_park_and_wake
229:15: replace == with != in measure_park_and_wake
257:66: replace != with == in measure_park_and_wake
271:9: replace && with || in measure_park_and_wake
271:55: replace / with % in measure_park_and_wake
271:55: replace / with * in measure_park_and_wake
```

### src/error_mode.rs (25)

```
58:21: replace && with || in BitOutcome::is_settable
58:39: replace & with | in BitOutcome::is_settable
64:9: replace BitOutcome::is_silently_dropped -> bool with false
64:50: replace != with == in BitOutcome::is_silently_dropped
64:39: replace & with | in BitOutcome::is_silently_dropped
64:39: replace & with ^ in BitOutcome::is_silently_dropped
120:9: replace <impl Drop for ThreadErrorMode>::drop with ()
141:28: replace == with != in with_thread_mode
149:11: replace != with == in with_thread_mode
185:31: replace | with ^ in settable_bits
196:5: replace combined_invalid_installs_nothing -> (bool, u32) with (true, 0)
196:5: replace combined_invalid_installs_nothing -> (bool, u32) with (true, 1)
196:44: replace | with & in combined_invalid_installs_nothing
196:44: replace | with ^ in combined_invalid_installs_nothing
197:26: replace | with & in combined_invalid_installs_nothing
197:26: replace | with ^ in combined_invalid_installs_nothing
226:9: replace ProcessVersusThread::is_independent -> bool with true
227:13: replace && with || in ProcessVersusThread::is_independent
226:27: replace & with | in ProcessVersusThread::is_independent
282:5: replace alignment_bit_is_sticky_at_process_scope -> (u32, u32) with (0, 0)
282:5: replace alignment_bit_is_sticky_at_process_scope -> (u32, u32) with (0, 1)
282:5: replace alignment_bit_is_sticky_at_process_scope -> (u32, u32) with (1, 0)
282:5: replace alignment_bit_is_sticky_at_process_scope -> (u32, u32) with (1, 1)
283:34: replace | with ^ in alignment_bit_is_sticky_at_process_scope
283:34: replace | with & in alignment_bit_is_sticky_at_process_scope
```

### src/cancel_io.rs (14)

```
86:9: replace CancelOutcome::returned -> bool with true
86:9: replace CancelOutcome::returned -> bool with false
136:9: replace <impl Drop for ThreadHandle>::drop with ()
86:9: delete ! in CancelOutcome::returned
170:35: replace + with - in cancel_under_watchdog
156:40: replace != with == in cancel_under_watchdog
171:37: replace && with || in cancel_under_watchdog
171:26: replace < with == in cancel_under_watchdog
171:26: replace < with > in cancel_under_watchdog
171:26: replace < with <= in cancel_under_watchdog
171:40: delete ! in cancel_under_watchdog
226:19: delete ! in cancel_against_busy_thread
206:5: replace cancel_against_busy_thread -> Vec<CancelOutcome> with vec![]
235:35: replace + with - in cancel_against_busy_thread
```

### src/bin/doorbell_cost.rs (11)

```
31:5: replace render -> String with String::new()
23:5: replace main with ()
59:19: replace > with == in render
31:5: replace render -> String with "xyzzy".into()
59:19: replace > with < in render
59:19: replace > with >= in render
115:19: replace > with == in render
115:19: replace > with < in render
115:19: replace > with >= in render
130:36: replace / with % in render
130:36: replace / with * in render
```

### src/bin/queue_contention.rs (11)

```
16:5: replace main with ()
68:47: replace match guard plain.nanos_per_push > 0.0 with false in main
68:47: replace match guard plain.nanos_per_push > 0.0 with true in main
68:68: replace > with == in main
68:68: replace > with < in main
68:68: replace > with >= in main
93:5: replace print_table with ()
106:5: replace format_scaling -> String with String::new()
106:5: replace format_scaling -> String with "xyzzy".into()
110:5: replace format_nanos -> String with String::new()
110:5: replace format_nanos -> String with "xyzzy".into()
```

### src/handle_state.rs (11)

```
81:9: replace <impl Drop for Fixture>::drop with ()
107:52: replace | with & in DirHandle::open
107:52: replace | with ^ in DirHandle::open
107:33: replace | with ^ in DirHandle::open
184:56: replace / with % in DirHandle::enumerate
212:9: replace <impl Drop for DirHandle>::drop with ()
230:9: replace SingleShot::run -> bool with true
299:9: replace CursorObservation::continued -> bool with true
321:9: replace CursorObservation::restarted -> bool with true
377:5: replace closing_duplicate_preserves_source -> bool with true
397:5: replace query_disturbs_cursor -> (bool, bool) with (true, false)
```

### src/request_cost.rs (11)

```
101:9: replace Observation::get -> Option<f64> with None
101:9: replace Observation::get -> Option<f64> with Some(0.0)
101:9: replace Observation::get -> Option<f64> with Some(1.0)
101:9: replace Observation::get -> Option<f64> with Some(-1.0)
103:31: replace == with != in Observation::get
122:49: replace / with % in time_loop
122:49: replace / with * in time_loop
197:5: replace system_directory -> std::path::PathBuf with Default::default()
207:21: replace || with && in system_directory
207:16: replace == with != in system_directory
207:32: replace >= with < in system_directory
```

### src/worker_context.rs (8)

```
60:9: replace WorkerContext::is_unimpersonated -> bool with true
60:32: replace && with || in WorkerContext::is_unimpersonated
70:9: replace WorkerContext::critical_error_handler_enabled -> bool with true
95:9: replace IdentityAsymmetry::disagree -> bool with true
95:41: replace && with || in IdentityAsymmetry::disagree
127:20: replace && with || in observe_here
127:15: replace != with == in observe_here
127:23: delete ! in observe_here
```

### src/bin/error_mode.rs (7)

```
22:5: replace name -> &'static str with ""
22:5: replace name -> &'static str with "xyzzy"
23:9: delete match arm bits::FAIL_CRITICAL_ERRORS in name
24:9: delete match arm bits::NO_GP_FAULT_ERROR_BOX in name
25:9: delete match arm bits::NO_ALIGNMENT_FAULT_EXCEPT in name
26:9: delete match arm bits::NO_OPEN_FILE_ERROR_BOX in name
32:5: replace main with ()
```

### src/bin/request_cost.rs (7)

```
22:5: replace main with ()
90:24: replace > with == in main
90:24: replace > with < in main
90:24: replace > with >= in main
111:18: replace > with == in main
111:18: replace > with < in main
111:18: replace > with >= in main
```

### src/topology.rs (7)

```
137:9: replace Observation::domain_counts -> Vec<(&'static str, usize)> with vec![]
137:9: replace Observation::domain_counts -> Vec<(&'static str, usize)> with vec![("", 1)]
137:9: replace Observation::domain_counts -> Vec<(&'static str, usize)> with vec![("xyzzy", 1)]
226:30: replace += with *= in measure
230:45: replace += with -= in measure
230:45: replace += with *= in measure
279:90: replace != with == in measure
```

### src/bin/topology.rs (6)

```
19:5: replace main with ()
54:22: replace > with == in main
54:22: replace > with < in main
54:22: replace > with >= in main
77:8: delete ! in main
77:51: replace == with != in main
```

### src/bin/cancel_io.rs (4)

```
19:5: replace describe -> String with String::new()
19:5: replace describe -> String with "xyzzy".into()
45:44: delete ! in main
29:5: replace main with ()
```

### src/bin/completion_port.rs (4)

```
20:5: replace describe with ()
30:5: replace report with ()
67:8: delete ! in report
99:5: replace main with ()
```

### src/bin/ioring.rs (3)

```
19:5: replace main with ()
59:16: delete ! in main
21:8: delete ! in main
```

### src/bin/pool_growth.rs (3)

```
37:5: replace main with ()
17:5: replace report with ()
61:8: delete ! in main
```

### src/bin/device_map.rs (1)

```
19:5: replace main with ()
```

### src/bin/handle_state.rs (1)

```
20:5: replace main with ()
```

### src/bin/worker_context.rs (1)

```
20:5: replace main with ()
```

### src/report.rs (1)

```
50:9: replace <impl Report for Stdout>::line with ()
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/handle_state.rs (5)

```
154:9: replace DirHandle::enumerate -> Result<Vec<String>, u32> with Ok(vec![])
154:9: replace DirHandle::enumerate -> Result<Vec<String>, u32> with Ok(vec![String::new()])
154:9: replace DirHandle::enumerate -> Result<Vec<String>, u32> with Ok(vec!["xyzzy".into()])
190:21: replace == with != in DirHandle::enumerate
193:20: replace += with *= in DirHandle::enumerate
```
