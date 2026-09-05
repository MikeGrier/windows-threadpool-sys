# Mutation survivors -- windows-threadpool-sys

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 102
- survived: 44
- timeout: 18

## Survived

### src/cleanup_group.rs (16)

```
433:9: replace <impl std::fmt::Debug for CleanupGroup>::fmt -> std::fmt::Result with Ok(Default::default())
460:9: replace WorkMember<'_>::wait with ()
466:9: replace WorkMember<'_>::cancel_pending with ()
490:9: replace TimerMember<'_>::set_at with ()
496:9: replace TimerMember<'_>::disarm with ()
506:9: replace TimerMember<'_>::is_set -> bool with true
512:9: replace TimerMember<'_>::wait with ()
518:9: replace TimerMember<'_>::cancel_pending with ()
543:9: replace PeriodicTimerMember<'_>::start with ()
562:9: replace PeriodicTimerMember<'_>::stop with ()
569:9: replace PeriodicTimerMember<'_>::is_running -> bool with true
575:9: replace PeriodicTimerMember<'_>::wait with ()
585:9: replace PeriodicTimerMember<'_>::stop_and_drain with ()
625:9: replace WaitMember<'_>::disarm with ()
631:9: replace WaitMember<'_>::wait with ()
637:9: replace WaitMember<'_>::cancel_pending with ()
```

### src/wait.rs (10)

```
50:5: replace relative_filetime -> FILETIME with Default::default()
53:58: replace / with % in relative_filetime
55:17: delete - in relative_filetime
58:31: replace >> with << in relative_filetime
133:9: replace <impl std::fmt::Debug for WaitTarget>::fmt -> std::fmt::Result with Ok(Default::default())
357:9: replace WaitContext::release_suppression with ()
348:17: replace != with == in WaitContext::suppress_and_disarm
380:9: replace <impl std::fmt::Debug for WaitActivation<'_>>::fmt -> std::fmt::Result with Ok(Default::default())
396:9: replace WaitActivation<'_>::is_signalled -> bool with false
735:9: replace ThreadpoolWait::wait with ()
```

### src/io.rs (6)

```
458:9: replace ThreadpoolIo::wait with ()
468:9: replace <impl fmt::Debug for ThreadpoolIo>::fmt -> fmt::Result with Ok(Default::default())
531:9: replace <impl fmt::Debug for IoCompletion>::fmt -> fmt::Result with Ok(Default::default())
477:18: replace > with >= in <impl Drop for ThreadpoolIo>::drop
558:9: replace IoCompletion::error -> Option<io::Error> with None
558:27: replace == with != in IoCompletion::error
```

### src/timer.rs (6)

```
100:39: replace / with % in absolute_filetime
119:5: replace millis_u32 -> u32 with 1
199:9: replace TimerContext::release_suppression with ()
190:18: replace != with == in TimerContext::suppress_and_disarm
232:9: replace <impl std::fmt::Debug for TimerFiring<'_>>::fmt -> std::fmt::Result with Ok(Default::default())
680:9: replace <impl std::fmt::Debug for ThreadpoolTimer>::fmt -> std::fmt::Result with Ok(Default::default())
```

### src/timer/periodic.rs (3)

```
37:9: replace <impl std::fmt::Debug for PeriodicTick<'_>>::fmt -> std::fmt::Result with Ok(Default::default())
378:9: replace ThreadpoolPeriodicTimer::wait with ()
442:9: replace <impl std::fmt::Debug for ThreadpoolPeriodicTimer>::fmt -> std::fmt::Result with Ok(Default::default())
```

### src/callback_env.rs (2)

```
62:45: replace << with >>
239:9: replace CallbackEnviron<'pool>::from_inner -> Self with Default::default()
```

### src/pool.rs (1)

```
239:9: replace <impl Drop for ThreadpoolPool>::drop with ()
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/io.rs (6)

```
275:9: replace ThreadpoolIo::outstanding -> usize with 0
398:9: replace ThreadpoolIo::cancel -> io::Result<()> with Ok(())
418:9: replace ThreadpoolIo::cancel_all -> io::Result<()> with Ok(())
462:9: replace ThreadpoolIo::raw_handle -> HANDLE with Default::default()
477:18: replace > with < in <impl Drop for ThreadpoolIo>::drop
477:18: replace > with == in <impl Drop for ThreadpoolIo>::drop
```

### src/timer.rs (4)

```
83:56: replace / with * in relative_filetime
478:9: replace ThreadpoolTimer::set_after with ()
557:9: replace ThreadpoolTimer::cancel_pending with ()
613:9: replace ThreadpoolTimer::into_parts -> (PTP_TIMER, *mut core::ffi::c_void) with (Default::default(), Default::default())
```

### src/wait.rs (3)

```
113:9: replace WaitTarget::raw -> HANDLE with Default::default()
477:9: replace WaitActivation<'_>::rearm_reporting -> bool with true
714:9: replace ThreadpoolWait::arm with ()
```

### src/cleanup_group.rs (2)

```
484:9: replace TimerMember<'_>::set_after with ()
619:9: replace WaitMember<'_>::arm with ()
```

### src/timer/periodic.rs (2)

```
358:9: replace ThreadpoolPeriodicTimer::stop with ()
409:9: replace ThreadpoolPeriodicTimer::into_parts -> (PTP_TIMER, *mut core::ffi::c_void, Duration) with (Default::default(), Default::default(), Default::default())
```

### src/work.rs (1)

```
139:9: replace ThreadpoolWork::into_parts -> (PTP_WORK, *mut c_void) with (Default::default(), Default::default())
```
