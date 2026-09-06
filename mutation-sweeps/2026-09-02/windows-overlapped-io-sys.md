# Mutation survivors -- windows-overlapped-io-sys

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 135
- survived: 43
- timeout: 23

## Survived

### src/iocp.rs (11)

```
315:9: replace CompletionPort::live_operations -> &OperationRegistry with Box::leak(Box::new(Default::default()))
353:9: replace CompletionPort::run_down -> io::Result<()> with Ok(())
353:34: replace > with < in CompletionPort::run_down
445:9: replace CompletionPort::report_outstanding_at_drop with ()
473:9: replace <impl fmt::Debug for CompletionPort>::fmt -> fmt::Result with Ok(Default::default())
481:9: replace <impl Drop for CompletionPort>::drop with ()
482:18: replace == with != in <impl Drop for CompletionPort>::drop
653:15: replace == with != in AssociatedEndpoint<'port>::cancel_all
672:31: replace > with >= in <impl Drop for AssociatedEndpoint<'_>>::drop
799:9: replace <impl fmt::Debug for Completion>::fmt -> fmt::Result with Ok(Default::default())
813:9: replace <impl Drop for Completion>::drop with ()
```

### src/fs.rs (10)

```
133:11: replace != with == in classify
133:5: replace classify -> io::Result<()> with Ok(())
464:9: replace PageBuffers::pages -> usize with 0
464:9: replace PageBuffers::pages -> usize with 1
476:9: replace PageBuffers::is_empty -> bool with true
490:9: replace PageBuffers::as_bytes_mut -> &mut[u8] with Vec::leak(Vec::new())
490:9: replace PageBuffers::as_bytes_mut -> &mut[u8] with Vec::leak(vec![0])
490:9: replace PageBuffers::as_bytes_mut -> &mut[u8] with Vec::leak(vec![1])
512:9: replace <impl fmt::Debug for PageBuffers>::fmt -> fmt::Result with Ok(Default::default())
520:9: replace <impl Drop for PageBuffers>::drop with ()
```

### src/socket.rs (7)

```
96:9: replace AssociatedSocket<'port>::key -> usize with 0
96:9: replace AssociatedSocket<'port>::key -> usize with 1
160:19: replace |= with &= in AssociatedSocket<'port>::set_notification_modes
307:9: replace AssociatedSocket<'port>::cancel -> io::Result<()> with Ok(())
312:19: replace == with != in AssociatedSocket<'port>::cancel
326:9: replace AssociatedSocket<'port>::cancel_all -> io::Result<()> with Ok(())
327:15: replace == with != in AssociatedSocket<'port>::cancel_all
```

### src/config.rs (6)

```
22:9: replace <impl std::fmt::Display for SourceTrackingAlreadySet>::fmt -> std::fmt::Result with Ok(Default::default())
40:5: replace set_source_tracking -> Result<(), SourceTrackingAlreadySet> with Ok(())
51:5: replace source_tracking_enabled -> bool with true
55:5: replace default_from_env -> bool with true
51:5: replace source_tracking_enabled -> bool with false
55:5: replace default_from_env -> bool with false
```

### src/device.rs (4)

```
99:5: replace in_ptr -> *const c_void with Default::default()
99:12: replace == with != in in_ptr
118:5: replace classify -> io::Result<()> with Ok(())
122:29: replace == with != in classify
```

### src/buf.rs (2)

```
197:9: replace <impl IoBuf for OversizedBuffer>::stable_ptr -> *const u8 with Default::default()
209:9: replace <impl IoBufMut for OversizedBuffer>::stable_mut_ptr -> *mut u8 with Default::default()
```

### src/endpoint.rs (2)

```
110:48: replace | with ^ in UnassociatedEndpoint::open
214:19: replace |= with &= in UnassociatedEndpoint::set_notification_modes
```

### src/blocking.rs (1)

```
141:9: replace <impl std::fmt::Display for TryFromEndpointError>::fmt -> std::fmt::Result with Ok(Default::default())
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/iocp.rs (13)

```
222:9: replace CompletionPort::get -> io::Result<Option<Completion>> with Ok(None)
291:9: replace CompletionPort::deregister_dequeued -> Option<OperationId> with None
304:9: replace CompletionPort::raw -> HANDLE with Default::default()
327:9: replace CompletionPort::outstanding -> usize with 1
353:34: replace > with == in CompletionPort::run_down
533:9: replace AssociatedEndpoint<'port>::notification_modes -> crate::NotificationModes with Default::default()
549:9: replace AssociatedEndpoint<'port>::outstanding -> usize with 1
652:9: replace AssociatedEndpoint<'port>::cancel_all -> io::Result<()> with Ok(())
660:9: replace AssociatedEndpoint<'port>::raw_handle -> HANDLE with Default::default()
672:31: replace > with == in <impl Drop for AssociatedEndpoint<'_>>::drop
672:31: replace > with < in <impl Drop for AssociatedEndpoint<'_>>::drop
682:34: replace > with == in <impl Drop for AssociatedEndpoint<'_>>::drop
682:34: replace > with >= in <impl Drop for AssociatedEndpoint<'_>>::drop
```

### src/identity.rs (8)

```
51:5: replace try_next_generation -> Option<u64> with None
55:22: replace != with == in try_next_generation
55:51: replace + with - in try_next_generation
196:9: replace OperationId::as_ptr -> *mut OVERLAPPED with Default::default()
293:9: replace OperationRegistry::insert with ()
322:9: replace OperationRegistry::remove -> Option<OperationId> with None
399:9: replace OperationRegistry::len -> usize with 1
421:15: delete ! in OperationRegistry::wait_until_empty
```

### src/endpoint.rs (1)

```
230:52: replace |= with &= in UnassociatedEndpoint::set_notification_modes
```

### src/operation.rs (1)

```
299:9: replace Operation<P>::overlapped_ptr -> *mut OVERLAPPED with Default::default()
```
