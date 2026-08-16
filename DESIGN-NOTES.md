# Design notes

This is the Windows Threadpool System crate. (windows-threadpool-sys).

It provides a Rust memory safe API over the Windows operating system's threadpool APIs. The Windows threadpool
is uniquely valuable in that it gives the ability to interact with the Windows operating system's
blocking facilities (in general) without the application dedicating any threads to the waits.

Many mechanisms have been added to Windows over the decades to allow for aggregation of waits
(WaitForMultipleObjectsEx, IO Completion Ports), but these all have limits and all end up requiring the
application to multiplex threads to deal with those limits.

The Windows thread pool works with the Windows kernel to, when no work is scheduled, have no extra threads
allocated towards the work at all. This is a unique value of it, and allows services on Windows to use the
thread pool and quiesce down to extremely low cost, low power states.

Current (August 2026) Rust work dispatch systems are uniform with Linux with typically means having one thread
created permanently per available processor per "reactor", and on Windows, with component boundaries being
DLLs not processes, this can lead to processes having (n * P) idle threads' stacks consuming memory for no
good reason, where 'n' is the number of reactor instances in the process and 'P' is the number of processors
on the machine.

This crate does not attempt to solve this problem, but it does provide a useful building block for code that
wants to avoid contributing to it. The Windows threadpool types are inherently memory unsafe and leave many
choices up to the developer. The `windows-sys` crate published by Microsoft helps with the basics of the FFI
to the APIs, but does little to help turn the alphabet and phrasebook into a useful programming model.
