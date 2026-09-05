# Mutation survivors -- windows-waitable-queues

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 400
- survived: 4
- timeout: 120

**Partly addressed already** in commit(s) `0599a5d, 40bc19d` on branch
`mikegrier/deferred-namespace-ops`. The entries below are as the sweep
found them and have NOT been pruned -- re-run before treating any single
line as outstanding.

## Survived

### src/reserving_mpsc.rs (2)

```
139:61: replace - with /
221:42: replace | with ^ in claim_word
```

### src/metrics.rs (1)

```
118:18: replace > with >= in Metrics::record_depth
```

### src/slotwise_mpsc.rs (1)

```
475:27: replace > with < in Producer<T>::push
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/reserving_mpsc.rs (48)

```
115:49: replace - with +
115:49: replace - with /
204:5: replace position_of -> u32 with 0
204:5: replace position_of -> u32 with 1
204:11: replace & with | in position_of
204:11: replace & with ^ in position_of
412:9: replace Shared<T>::capacity_u32 -> u32 with 0
433:9: replace Shared<T>::has_room_beyond_reservations -> bool with true
433:9: replace Shared<T>::has_room_beyond_reservations -> bool with false
439:18: replace < with == in Shared<T>::has_room_beyond_reservations
439:18: replace < with > in Shared<T>::has_room_beyond_reservations
439:18: replace < with <= in Shared<T>::has_room_beyond_reservations
454:9: replace Shared<T>::len -> usize with 0
454:9: replace Shared<T>::len -> usize with 1
475:9: replace Shared<T>::remaining -> usize with 1
490:9: replace Shared<T>::has_ready_item -> bool with true
490:9: replace Shared<T>::has_ready_item -> bool with false
491:50: replace & with ^ in Shared<T>::has_ready_item
492:47: replace == with != in Shared<T>::has_ready_item
508:9: replace Shared<T>::release_producer with ()
508:58: replace != with == in Shared<T>::release_producer
649:9: replace Producer<T>::push -> Result<(), PushError<T>> with Ok(())
656:16: delete ! in Producer<T>::push
665:28: replace != with == in Producer<T>::push
735:28: replace != with == in Producer<T>::reserve
772:9: replace Producer<T>::capacity -> usize with 0
772:9: replace Producer<T>::capacity -> usize with 1
778:9: replace Producer<T>::len -> usize with 0
778:9: replace Producer<T>::len -> usize with 1
801:9: replace Producer<T>::is_full -> bool with false
801:26: replace == with != in Producer<T>::is_full
813:9: replace Producer<T>::remaining -> usize with 1
852:9: replace <impl Drop for Producer<T>>::drop with ()
969:9: replace <impl Drop for Reservation<T>>::drop with ()
1010:9: replace Consumer<T>::pop -> Option<T> with None
1011:57: replace & with | in Consumer<T>::pop
1011:57: replace & with ^ in Consumer<T>::pop
1014:50: replace != with == in Consumer<T>::pop
1088:9: replace Consumer<T>::is_disconnected -> bool with true
1088:9: replace Consumer<T>::is_disconnected -> bool with false
1088:55: replace == with != in Consumer<T>::is_disconnected
1142:9: replace Consumer<T>::arm -> io::Result<bool> with Ok(true)
1142:9: replace Consumer<T>::arm -> io::Result<bool> with Ok(false)
1149:12: delete ! in Consumer<T>::arm
1188:9: replace <impl Parked for Consumer<T>>::pop -> Option<T> with None
1196:9: replace <impl Parked for Consumer<T>>::arm -> io::Result<bool> with Ok(false)
1200:9: replace <impl Parked for Consumer<T>>::is_disconnected -> bool with true
1200:9: replace <impl Parked for Consumer<T>>::is_disconnected -> bool with false
```

### src/slotwise_mpsc.rs (38)

```
226:24: replace - with + in build
226:24: replace - with / in build
331:9: replace Shared<T>::slot_index -> usize with 1
331:9: replace Shared<T>::slot_index -> usize with 0
331:29: replace & with | in Shared<T>::slot_index
331:29: replace & with ^ in Shared<T>::slot_index
356:9: replace Shared<T>::len -> usize with 0
356:9: replace Shared<T>::len -> usize with 1
378:9: replace Shared<T>::has_ready_item -> bool with false
378:9: replace Shared<T>::has_ready_item -> bool with true
380:47: replace == with != in Shared<T>::has_ready_item
449:9: replace Producer<T>::push -> Result<(), PushError<T>> with Ok(())
460:27: replace < with == in Producer<T>::push
460:27: replace < with <= in Producer<T>::push
460:27: replace < with > in Producer<T>::push
467:20: delete ! in Producer<T>::push
475:27: replace > with == in Producer<T>::push
475:27: replace > with >= in Producer<T>::push
606:9: replace Producer<T>::capacity -> usize with 0
498:16: delete ! in Producer<T>::push
606:9: replace Producer<T>::capacity -> usize with 1
615:9: replace Producer<T>::len -> usize with 0
615:9: replace Producer<T>::len -> usize with 1
681:9: replace <impl Drop for Producer<T>>::drop with ()
681:65: replace != with == in <impl Drop for Producer<T>>::drop
722:9: replace Consumer<T>::pop -> Option<T> with None
732:21: replace != with == in Consumer<T>::pop
796:9: replace Consumer<T>::is_disconnected -> bool with true
796:9: replace Consumer<T>::is_disconnected -> bool with false
796:55: replace == with != in Consumer<T>::is_disconnected
917:9: replace Consumer<T>::arm -> io::Result<bool> with Ok(false)
924:12: delete ! in Consumer<T>::arm
978:9: replace <impl Parked for Consumer<T>>::pop -> Option<T> with None
986:9: replace <impl Parked for Consumer<T>>::arm -> io::Result<bool> with Ok(false)
990:9: replace <impl Parked for Consumer<T>>::is_disconnected -> bool with true
990:9: replace <impl Parked for Consumer<T>>::is_disconnected -> bool with false
1046:9: replace <impl crate::Bounded for Producer<T>>::len -> usize with 0
1046:9: replace <impl crate::Bounded for Producer<T>>::len -> usize with 1
```

### src/spsc.rs (26)

```
269:9: replace Shared<T>::len -> usize with 0
269:9: replace Shared<T>::len -> usize with 1
282:9: replace Shared<T>::remaining -> usize with 1
384:9: replace Producer<T>::push -> Result<(), PushError<T>> with Ok(())
395:47: replace >= with < in Producer<T>::push
399:16: delete ! in Producer<T>::push
408:12: delete ! in Producer<T>::push
423:9: replace Producer<T>::capacity -> usize with 0
423:9: replace Producer<T>::capacity -> usize with 1
429:9: replace Producer<T>::len -> usize with 0
429:9: replace Producer<T>::len -> usize with 1
460:9: replace Producer<T>::remaining -> usize with 1
547:9: replace <impl Drop for Producer<T>>::drop with ()
582:9: replace Reservation<'_, T>::send -> Result<(), Disconnected<T>> with Ok(())
663:9: replace Consumer<T>::pop -> Option<T> with None
697:9: replace Consumer<T>::len -> usize with 0
697:9: replace Consumer<T>::len -> usize with 1
703:9: replace Consumer<T>::is_empty -> bool with true
703:9: replace Consumer<T>::is_empty -> bool with false
703:20: replace == with != in Consumer<T>::is_empty
716:9: replace Consumer<T>::is_disconnected -> bool with false
716:9: delete ! in Consumer<T>::is_disconnected
830:9: replace Consumer<T>::arm -> io::Result<bool> with Ok(false)
887:9: replace <impl Parked for Consumer<T>>::pop -> Option<T> with None
895:9: replace <impl Parked for Consumer<T>>::arm -> io::Result<bool> with Ok(false)
899:9: replace <impl Parked for Consumer<T>>::is_disconnected -> bool with false
```

### src/blocking.rs (3)

```
90:12: delete ! in recv
128:12: delete ! in recv_timeout
204:9: delete match arm WAIT_OBJECT_0 | WAIT_TIMEOUT in wait
```

### src/capacity.rs (3)

```
79:5: replace validate_capacity -> Result<(), CapacityError> with Ok(())
102:17: replace > with == in validate_capacity
102:17: replace > with < in validate_capacity
```

### src/doorbell.rs (1)

```
278:9: replace Doorbell::signal with ()
```

### src/traits.rs (1)

```
156:9: replace Bounded::remaining -> usize with 1
```
