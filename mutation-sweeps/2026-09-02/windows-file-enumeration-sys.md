# Mutation survivors -- windows-file-enumeration-sys

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 410
- survived: 50
- timeout: 8

**Partly addressed already** in commit(s) `07882f0, 49019f2` on branch
`mikegrier/deferred-namespace-ops`. The entries below are as the sweep
found them and have NOT been pruned -- re-run before treating any single
line as outstanding.

## Survived

### src/error.rs (16)

```
113:9: replace RequestFailure::describe -> &'static str with "xyzzy"
209:9: replace BeginFailure::describe -> &'static str with ""
209:9: replace BeginFailure::describe -> &'static str with "xyzzy"
279:9: replace BeginError::capture_error -> Option<&CaptureError> with None
285:9: replace <impl fmt::Display for BeginError>::fmt -> fmt::Result with Ok(Default::default())
294:9: replace <impl std::error::Error for BeginError>::source -> Option<&(dyn std::error::Error +'static)> with None
320:9: replace SessionFailure::describe -> &'static str with ""
320:9: replace SessionFailure::describe -> &'static str with "xyzzy"
363:9: replace SessionError::os_error -> Option<&io::Error> with None
369:9: replace <impl fmt::Display for SessionError>::fmt -> fmt::Result with Ok(Default::default())
378:9: replace <impl std::error::Error for SessionError>::source -> Option<&(dyn std::error::Error +'static)> with None
402:9: replace PredicateFailure::describe -> &'static str with "xyzzy"
460:9: replace MalformedRecord::describe -> &'static str with ""
460:9: replace MalformedRecord::describe -> &'static str with "xyzzy"
583:9: replace <impl std::error::Error for EnumerationError>::source -> Option<&(dyn std::error::Error +'static)> with None
584:13: delete match arm EnumerationError::Impersonation(error) in <impl std::error::Error for EnumerationError>::source
```

### src/path.rs (7)

```
43:42: replace - with +
43:42: replace - with /
77:20: replace > with == in prepare
77:20: replace > with >= in prepare
131:5: replace is_drive_designator -> bool with true
134:21: replace && with || in is_drive_designator
175:16: replace > with >= in resolve
```

### src/native.rs (5)

```
86:56: replace | with & in open_directory
86:56: replace | with ^ in open_directory
86:37: replace | with & in open_directory
86:37: replace | with ^ in open_directory
160:5: replace volume_serial -> Result<u64, Win32Error> with Ok(1)
```

### src/session.rs (4)

```
212:9: replace SessionShared::acquire_handle with ()
222:9: replace SessionShared::release_handle with ()
222:56: replace != with == in SessionShared::release_handle
797:9: replace Receiver::is_empty -> bool with true
```

### src/completion_ring.rs (3)

```
96:26: replace > with >= in RingState::can_reserve
336:26: replace -= with += in CompletionRing::release_reservation
336:26: replace -= with /= in CompletionRing::release_reservation
```

### src/pattern.rs (3)

```
80:9: replace NamePattern::empty -> Self with Default::default()
204:21: replace == with != in ordinal_equal_ignoring_case
207:21: replace == with != in ordinal_equal_ignoring_case
```

### src/submission_ring.rs (3)

```
58:9: replace <impl std::fmt::Debug for BeginMessage>::fmt -> std::fmt::Result with Ok(Default::default())
250:24: replace -= with += in SubmissionRing::push_abandon
250:24: replace -= with /= in SubmissionRing::push_abandon
```

### src/admission.rs (2)

```
101:9: replace EnumerationHandle::cancel with ()
235:17: delete match arm ControlMessage::Begin(begin) in try_begin_with_token
```

### src/engine.rs (2)

```
214:13: delete match arm Phase::Opened in advance
304:25: replace == with != in start
```

### src/registry.rs (2)

```
106:9: replace Registry::is_accepting -> bool with true
111:9: replace Registry::stop_accepting with ()
```

### src/record.rs (1)

```
177:58: replace + with - in parse_record
```

### src/request.rs (1)

```
23:47: replace * with +
```

### src/timestamp.rs (1)

```
52:58: replace | with ^ in WindowsFileTimestamp::from_filetime
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/completion_ring.rs (6)

```
101:9: replace RingState::closed -> bool with false
101:23: replace == with != in RingState::closed
101:43: replace == with != in RingState::closed
190:9: replace CompletionRing::remove_session with ()
192:28: replace -= with += in CompletionRing::remove_session
192:28: replace -= with /= in CompletionRing::remove_session
```

### src/pattern.rs (1)

```
224:5: replace code_point_width -> Option<usize> with Some(0)
```

### src/session.rs (1)

```
124:9: replace SessionWork::is_suppressed -> bool with true
```
