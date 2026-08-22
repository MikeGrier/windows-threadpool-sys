# Checklist: windows-overlapped-io-sys

All planned milestones are complete; there are no pending work items. Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

This file reopens when new work (a new operation family, a new backend, or hardening) is planned.

One thing M10 deliberately left unbuilt, recorded here rather than in a design note so it is not orphaned
a second time: there is no socket-side notification-mode setter, because sockets have no unassociated
endpoint type to carry the provenance attribute and Win32 restricts skip-on-success on a socket to Layered
Service Providers returning IFS handles. `socket::classify_socket` is correct only while that stays true,
and says so. Adding the setter means adding the capability probe and updating `classify_socket` in the
same change.
