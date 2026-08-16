# Plans

## Initial API foundation

1. Record the safety invariants shared by all Windows thread pool objects.
2. Add the minimal `windows-sys` feature set for pools, callback environments,
	cleanup groups, and work objects.
3. Implement and test one end-to-end work submission abstraction.
4. Use the work abstraction to validate the callback ownership model before
	extending it to timers, waits, and I/O.
