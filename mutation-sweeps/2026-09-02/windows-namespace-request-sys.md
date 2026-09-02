# Mutation survivors -- windows-namespace-request-sys

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 120
- survived: 49
- timeout: 1

**Partly addressed already** in commit(s) `9a9163c, a07b50c` on branch
`mikegrier/deferred-namespace-ops`. The entries below are as the sweep
found them and have NOT been pruned -- re-run before treating any single
line as outstanding.

## Survived

### src/path.rs (9)

```
66:42: replace - with +
66:42: replace - with /
104:9: replace PathFailure::description -> &'static str with "xyzzy"
152:9: replace PathError::raw_os_error -> Option<i32> with None
167:9: replace <impl std::error::Error for PathError>::source -> Option<&(dyn std::error::Error +'static)> with None
298:20: replace > with == in prepare_units
298:20: replace > with >= in prepare_units
355:21: replace && with || in is_drive_designator
393:16: replace > with >= in resolve
```

### src/final_path.rs (8)

```
91:52: replace | with ^
91:52: replace | with &
110:21: replace | with ^ in <impl std::ops::BitOr for FinalPathFlags>::bitor
134:9: replace <impl fmt::Display for FinalPathError>::fmt -> fmt::Result with Ok(Default::default())
146:9: replace <impl std::error::Error for FinalPathError>::source -> Option<&(dyn std::error::Error +'static)> with None
217:9: replace QueryFinalPath::flags -> FinalPathFlags with Default::default()
261:24: replace < with <= in QueryFinalPath::perform
287:9: replace <impl crate::request::Request for QueryFinalPath>::perform -> Result<Wtf16String, FinalPathError> with Ok(Default::default())
```

### src/security.rs (6)

```
84:9: replace SecurityCaptureError::raw_os_error -> Option<i32> with None
84:9: replace SecurityCaptureError::raw_os_error -> Option<i32> with Some(0)
84:9: replace SecurityCaptureError::raw_os_error -> Option<i32> with Some(1)
84:9: replace SecurityCaptureError::raw_os_error -> Option<i32> with Some(-1)
105:9: replace <impl std::error::Error for SecurityCaptureError>::source -> Option<&(dyn std::error::Error +'static)> with None
308:9: replace SecurityDescriptor::is_empty -> bool with false
```

### src/handle.rs (5)

```
35:33: delete -
37:46: delete -
39:45: delete -
41:55: delete -
126:9: replace <impl std::error::Error for HandleCaptureError>::source -> Option<&(dyn std::error::Error +'static)> with None
```

### src/watch.rs (5)

```
122:9: replace <impl Drop for ChangeNotification>::drop with ()
207:21: replace | with ^ in NotifyFilter::union
289:9: replace WatchDirectory::subtree -> bool with false
295:9: replace WatchDirectory::filter -> NotifyFilter with Default::default()
324:9: replace <impl fmt::Display for NotifyFilter>::fmt -> fmt::Result with Ok(Default::default())
```

### src/open_by_id.rs (4)

```
223:9: replace OpenFileByIdentifier::desired_access -> u32 with 0
229:9: replace OpenFileByIdentifier::share_mode -> FILE_SHARE_MODE with Default::default()
235:9: replace OpenFileByIdentifier::security -> Option<&SecurityAttributes> with None
241:9: replace OpenFileByIdentifier::flags_and_attributes -> FILE_FLAGS_AND_ATTRIBUTES with Default::default()
```

### src/volume.rs (4)

```
54:9: replace VolumeInformation::label -> &Wtf16String with Box::leak(Box::new(Default::default()))
63:9: replace VolumeInformation::serial_number -> u32 with 1
69:9: replace VolumeInformation::maximum_component_length -> u32 with 1
78:9: replace VolumeInformation::flags -> u32 with 1
```

### src/open.rs (3)

```
172:9: replace OpenFile::desired_access -> u32 with 0
178:9: replace OpenFile::share_mode -> FILE_SHARE_MODE with Default::default()
190:9: replace OpenFile::creation_disposition -> FILE_CREATION_DISPOSITION with Default::default()
```

### src/full_path.rs (2)

```
189:24: replace < with <= in ResolveFullPath::perform
222:9: replace <impl crate::request::Request for ResolveFullPath>::perform -> Result<Wtf16String, FullPathError> with Ok(Default::default())
```

### src/query.rs (2)

```
219:44: replace * with +
219:44: replace * with /
```

### src/buffer.rs (1)

```
158:9: replace <impl Drop for AlignedBuffer>::drop with ()
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/buffer.rs (1)

```
80:16: replace == with != in AlignedBuffer::zeroed
```
