# Design session 2026-10-29: common practice in database and file-system logs

Component: [crates/durable-ioring](../crates/durable-ioring/COMPONENT.md). Work item:
[DI-21.1](../crates/durable-ioring/CHECKLIST.md), the survey the sample log of `DI-M21` is built from. A research record, not
a decision record: it decides nothing, and no work is queued by it. It extends
[the 2026-10-24 prior-art session](DESIGN-SESSION-2026-10-24-durable-ioring-prior-art.md) and does not repeat it: sources
numbered 1 to 24 are that session's, and this one continues at 25. A bracketed number below 25 is a source of that session.

**Method.** Three research agents searched and read sources, one each for database logs, file-system journals and what every
log faces. The assistant then re-fetched the cited pages on 2026-10-29 and matched each quotation below mechanically against
the fetched text, with whitespace and hyphenation normalised. **VERIFIED** means that match succeeded. A passage that could
not be re-fetched, or whose words did not match, is not quoted as verified: it is described as **UNVERIFIED**, or listed
under "Not found". Two agent claims were corrected by the re-check, both recorded where they arise (ARIES and group commit,
and a RocksDB changelog entry). A quotation is at most a clause; the sources are linked for the rest.

## Summary

Every log here does the same three things. It writes a record of a change before the change is allowed to reach the
structure it protects (the write-ahead rule); it gives each record a position or identity that recovery can order; and it
can tell, after a crash, where the valid records end. They differ in what they log, in how a record is identified, in how
the end is found, and in when space is reclaimed. Four problems recur in every system read: amortising the cost of forcing
the log (group commit), recognising a record that only partly reached the media, keeping a record's identity from being
confused with older data in reused space, and reclaiming space without losing what recovery needs.

What `durable-ioring` offers a log is narrow and exact: it reports when a sealed set of writes is on stable media and what a
failure puts in doubt, and it orders one lineage's writes after another's. The contract is explicit that it gives **no
atomicity beyond the device's** and **nothing across a process lifetime**
([CONTRACT.md](../crates/durable-ioring/CONTRACT.md)), so record framing, torn-record detection, record identity across a
crash, end-of-log detection, recovery and space reclamation are all the log's. That division is what a sample log has to
show. The facility mapping is under E, and what it raises for the engineer under F.

## A. Database logs

### A1. ARIES [5]

- **Page LSN, and redo that can be repeated.** VERIFIED, section 4.2: "contains the LSN of the log record that describes the
  latest update to the page". Section 10.1 compares it with the record's LSN: "If the page LSN is less than the log
  record's LSN" the update is redone, and the page's LSN is then set to the record's.
- **Fuzzy checkpoints.** VERIFIED, section 5.4: "Such a fuzzy checkpoint is initiated by writing a begin-chkpt record." The
  end record carries the transaction table and the dirty-pages table, and the redo pass starts where the dirty-pages
  table's smallest RecLSN says: "gives the starting point for the redo recovery" (section 4.4; the sentence is split by a
  page break in the extracted text).
- **Compensation records.** VERIFIED, section 4.1: "UndoNxtLSN is the value of PrevLSN of the log record that the current
  log record is compensating". A rollback is logged, so a crash during rollback does not undo twice.
- **The write-ahead rule.** VERIFIED, section 1.1: "must already be on stable storage before the changed data is allowed to
  replace the previous version".
- **A record's identity.** VERIFIED, section 4.1: the LSN is the "Address of the first byte of the log record in the
  ever-growing log address space".
- **Reserved space.** VERIFIED: "we can keep in reserve enough log space to be able to roll back all currently running
  transactions". Already cited in [the earlier session](DESIGN-SESSION-2026-10-24-durable-ioring-prior-art.md).
- **Group commit.** CORRECTED. The research agent reported the paper never mentions it. It does, once, in passing: of IMS
  Fast Path, "group commit logic is used", citing another paper. ARIES itself does not describe it.
- **Truncating the log below what recovery needs: still not found.** The paper's text has no "truncat" at all; its one
  mention of "archiving" is about splitting the log across files. The minimum RecLSN is the prerequisite for reclaiming, but
  no passage says the log is cut there. This is the passage [the earlier session](DESIGN-SESSION-2026-10-24-durable-ioring-prior-art.md)
  also failed to find, and it is not read in any source of this one either.

### A2. PostgreSQL [25, 26, 27]

- **Per-record check.** VERIFIED: "Each individual record in a WAL file is protected by a CRC-32C (32-bit) check".
- **LSN is a byte position.** VERIFIED: "representing a byte position in the write-ahead log stream".
- **Torn pages, and why full pages are written.** VERIFIED: "PostgreSQL periodically writes full page images to permanent
  WAL storage before modifying the actual page on disk." The reason it gives is a power loss in the middle of a page write,
  leaving some 512-byte sectors written and others not.
- **Reclaiming.** VERIFIED: "WAL segments preceding the one containing the redo record are no longer needed and can be
  recycled or removed". A checkpoint's redo record is the line.
- **Group commit.** VERIFIED: "group commit leader process will sleep after acquiring a lock within XLogFlush". The delay is
  a policy: "a delay is only performed if at least commit_siblings other transactions are active".
- **Where the log ends.** Read in the source, not prose: the reader validates a record's header and length before trusting
  it, and replay stops at the first record that does not validate. UNVERIFIED as quoted text here; the research agent read
  `xlogreader.c`, and this pass did not re-fetch it.

### A3. SQLite WAL [28, 29, 30]

- **A generation stamp.** VERIFIED: the WAL header's first salt is a "random integer incremented with each checkpoint".
- **What makes a frame valid.** VERIFIED: it is valid only if "The salt-1 and salt-2 values in the frame-header match salt
  values in the wal-header", and its checksum matches one computed consecutively over what precedes it. A frame left over
  from an earlier use of the same file offsets fails the salt test.
- **The commit marker.** VERIFIED: "For commit records, the size of the database file in pages after the commit".
- **Reset, not truncate.** VERIFIED: "the writer will rewind the WAL back to the beginning and start putting new
  transactions at the beginning of the WAL"; the changed salts "invalidate old frames in the WAL that have already been
  checkpointed but not yet overwritten"; and "The WAL file can optionally be truncated on a reset, but it need not be."
- **Atomicity.** VERIFIED: "SQLite has traditionally assumed that a sector write is not atomic. However, SQLite does always
  assume that a sector write is linear." And: "SQLite never assumes that database page writes are atomic, regardless of the
  PSOW setting."

### A4. LevelDB and RocksDB [31, 32, 33]

- **Framing.** VERIFIED: "The log file contents are a sequence of 32KB blocks", each record carrying a checksum, "crc32c of
  type and data", a length and a type of FULL, FIRST, MIDDLE or LAST, so a record may span blocks. "A record never starts
  within the last six bytes of a block."
- **What recovery does with a bad tail is a setting, not a rule.** VERIFIED, RocksDB's four modes: tolerate corrupted tail
  records ("the WAL replay ignores any error discovered at the tail of the log"); absolute consistency ("any IO error during
  WAL replay is considered as data corruption"); point-in-time ("the WAL replay is stopped after encountering an IO error",
  recovering to "a point-in-time where it is consistent"); and skip any corrupted record ("any IO error while reading the log
  is ignored").
- **Reclaiming.** VERIFIED: "the corresponding WAL log(s) become obsolete and are archived", once their memtable is flushed.

### A5. InnoDB redo log [34]

VERIFIED, from an archived copy of the 8.0 manual: it writes its files "in a circular fashion", identified by an
ever-increasing LSN. What a full circle does to writers is UNVERIFIED here: the manual page that says it returned 403 and
was read only by an agent through a proxy.

## B. File-system journals

### B1. ext4 and jbd2 [35, 36, 37]

- **Three modes.** VERIFIED, from the admin guide: in `data=ordered`, "All data are forced directly out to the main file
  system prior to its metadata being committed to the journal"; in `data=writeback`, "Data ordering is not preserved, data
  may be written into the main file system after its metadata has been committed to the journal"; and in `data=journal` all
  data goes through the journal first.
- **A transaction ends with a sentinel.** VERIFIED: "The commit block is a sentry that indicates that a transaction has been
  completely written to the journal", and "If there is no commit record (or the checksums don't match), the transaction will
  be discarded during replay."
- **The log is flushed before it is relied on.** VERIFIED, from the same document: the journal record is written once "the
  important data transaction is fully written to the disk and flushed from the disk write cache".
- **Recovery is three passes.** VERIFIED, `fs/jbd2/recovery.c`: "In the first pass, we look for the end of the log. In the
  second, we assemble the list of revoke blocks. In the third and final pass, we replay any un-revoked blocks". A revoke
  block exists "to prevent replay of a block in an earlier transaction".
- **Reclaiming is checkpointing.** VERIFIED, `fs/jbd2/checkpoint.c`: "Checkpointing is the process of ensuring that a
  section of the log is committed fully to disk", so that the portion of the log can be reused; and a routine exists to wait
  until there is space in the journal.

### B2. XFS [38, 39]

- **A position is a cycle and an offset.** VERIFIED: "A LSN carries the cycle in the upper 32 bits and the offset in the
  lower 32 bits." The log is circular, so the cycle says which lap a block belongs to.
- **It is write-ahead logging.** VERIFIED: "XFS uses Write Ahead Logging for ensuring changes to the filesystem metadata are
  atomic and recoverable."
- **A torn write at the head is detected and cut off.** VERIFIED, `xfs_log_recover.c`: a warning reads "Torn write (CRC
  failure) detected at log block", and a comment notes that the search for the tail "clears the blocks at the new head".

### B3. NTFS [40, 41]

What Microsoft publishes is a name. VERIFIED: the reserved-file table lists `$LogFile` as a "Transactional logging file", and
the pages read give nothing of its layout. Everything else comes from a third-party reverse-engineering project, read from
an archived copy, which says so itself: "Little is known about the LogFile's structure." From it, VERIFIED: "Effectively,
the log file is viewed as a circular entity", and "Individual log records are identified by logical sequence numbers". What
NTFS's log covers of a file's data is not in any source read: that it is metadata only is UNVERIFIED.

### B4. ZFS and Btrfs [42]

VERIFIED, OpenZFS: "The ZFS Intent Log (ZIL) satisfies POSIX requirements for synchronous transactions", allocated from the
pool by default, with a separate log device an option. A log of synchronous intent beside a transactional main structure, not
a log the main structure is rebuilt from. Btrfs's log tree was read only in source comments and is not quoted.

### B5. What the journaling literature says about ordering [43, 44]

VERIFIED, Prabhakaran et al.: "writeback mode has the weakest consistency semantics of the three modes", and recovery is "a
basic form of redo logging". VERIFIED, Seltzer et al.: "The system enforces write-ahead logging", and "journaling alone is not
sufficient to" solve the metadata update problem.

## C. What every log faces

### C1. Group commit [45, 46, 47]

- **The idea.** VERIFIED, DeWitt et al. 1984: "The transactions with commit records on the same log page are committed as a
  group". Hagmann 1987: "a set of updates are grouped together in one log write to amortize the cost of the log write disk
  I/O over several updates". Helland et al. 1988: "many transactions are committed with a single disk I/O to the log".
- **The price is latency, and a timer is a policy.** VERIFIED, Hagmann: "fsd forces its log twice a second", which "induces a
  certain degree of uncertainty about when some modifications to the file system are permanent". PostgreSQL's
  `commit_delay` is the same idea as a setting (A2).
- **It needs concurrency to pay.** VERIFIED, MariaDB: "The idea with group commit is to amortize the costs of each flush to
  disk over multiple commits from multiple parallel transactions."

### C2. A record that only partly reached the media [28, 29, 14, 48, 49]

- **Systems assume little about sector atomicity.** SQLite (A3) assumes a sector write is linear but not atomic. VERIFIED,
  Pillai et al. [14]: "all tested file systems seemingly provide atomic single-sector overwrites", an observation they say in
  some cases arises because the disk provides atomic sector writes, and which would not hold on a medium with only
  byte-level atomicity.
- **A check on every record is the common answer.** CRC-32C in PostgreSQL (A2), a checksum per frame in SQLite (A3), a
  checksum per record in LevelDB (A4), a CRC on the log in XFS (B2), and a checksum on the commit in ext4 (B1). VERIFIED,
  IRON file systems [48]: "utilize end-to-end checksums to detect when block corruption occurs", a technique the authors say
  commodity file systems lacked.
- **A cheaper test for the tail.** VERIFIED, etcd's WAL decoder [49]: a comment "isTornEntry determines whether the last
  entry of the WAL was partially written and corrupted because of a torn write", splitting "on sector boundaries" of a
  512-byte minimum sector, and "if any data for a sector chunk is all 0, it's a torn write".
- **What to do with it differs.** RocksDB makes it a mode (A4); SQLite, PostgreSQL, ext4 and XFS stop at the first bad
  record and treat it as the end.

### C3. A record's identity across a crash

| Log | What identifies a record, and keeps it from older data in reused space |
|---|---|
| ARIES | The LSN, an address in an ever-growing space (A1). |
| PostgreSQL | The LSN, a byte position (A2). |
| SQLite WAL | Position plus a per-checkpoint salt pair a frame must match (A3). |
| InnoDB | An ever-increasing LSN over a circular file (A5). |
| XFS | Cycle and offset: the lap, so a stale block from the last lap is known (B2). |
| NTFS | A logical sequence number over a circular file, per the third-party page (B3). |
| Raft | VERIFIED [23]: "if two logs contain an entry with the same index and term, then the logs are identical in all entries up through the given index." |

A reused circular space is the case that forces the identity to be more than a position, which is why SQLite stamps a
generation, XFS counts laps, and Raft pairs an index with a term.

### C4. Reclaiming space, and a full log

- **Reclaim at a line recovery no longer needs.** PostgreSQL recycles segments before the checkpoint's redo record (A2);
  jbd2 checkpoints so the section "can be reused" (B1); RocksDB archives a WAL when its memtable is flushed (A4); SQLite
  rewinds and re-salts (A3); InnoDB advances a checkpoint around a circle (A5).
- **A bound is reserved ahead.** ARIES keeps log space in reserve to roll back every running transaction (A1).
- **Compaction is a second answer.** VERIFIED, Raft [23]: its log, "in a practical system, it cannot grow without bound",
  which is why a snapshot replaces a prefix.
- **What a full log does to writers** is in the sources only for jbd2, which waits for a checkpoint to free space (B1). For
  InnoDB it is UNVERIFIED (A5).

### C5. Ordering a log and the data it protects

- **The rule.** ARIES (A1) and Seltzer et al. (B5) state it; ext4's ordered mode implements it for file data (B1): data
  forced out before the metadata record commits.
- **What a flush promises, and what it does not.** VERIFIED, `fsync(2)`: "Calling fsync() does not necessarily ensure that
  the entry in the directory containing the file has also reached disk", so a newly created log file's directory entry needs
  its own flush. VERIFIED, Microsoft, File Buffering: "Discussion of caching does not consider any hardware caching on the
  physical disk itself". VERIFIED, Microsoft, File Caching: "File system metadata is always cached". FlushFileBuffers and
  the write-through and no-buffering flags are cited in [the earlier session](DESIGN-SESSION-2026-10-24-durable-ioring-prior-art.md)
  as [9].

### C6. After a failed flush

- **PostgreSQL now stops and recovers from the log.** VERIFIED: it "will raise a PANIC-level error on failure to flush
  modified data files to the file system", and "the only way to avoid data loss is to recover from the WAL after any failure
  is reported". It rejects a retry because a second attempt "may be reported as successful, when in fact the data has been
  lost". This is the current form of the finding in [12] and [13].
- **RocksDB's changelog entry about a retryable error stopping writes could not be matched** in the file as fetched, and is
  not quoted; see "Not found".

## D. What is common, and what differs

| | What it logs | Identity | Valid end found by | Space reclaimed by |
|---|---|---|---|---|
| ARIES | Page changes, redo and undo | LSN, an address | End of log reached in analysis | Not found in the paper |
| PostgreSQL | Page changes, full pages after a checkpoint | LSN, a byte position | First record failing its length or CRC | Recycling segments before the redo record |
| SQLite WAL | Pages | Position plus a generation salt | First frame failing salt or checksum | Reset to the start with new salts |
| LevelDB, RocksDB | Writes | Position in 32 KB blocks, a CRC each | A recovery mode (stop, tolerate, skip) | Archive once the memtable is flushed |
| InnoDB | Page changes | Ever-increasing LSN | Not read | A checkpoint advancing around a circle |
| ext4 jbd2 | Metadata; data too in `data=journal` | Position in a transaction between descriptor and commit blocks | A pass looking for the end of the log | Checkpointing |
| XFS | Metadata, logical and physical | Cycle and offset | A CRC pass that cuts off a torn head | Not read |
| NTFS | Not documented by Microsoft | LSN, per a third party | Not read | Not read |

Common to both kinds: the write-ahead rule, an identity recovery can order, a check that finds the end, and a checkpoint
that sets how much log recovery needs. Different: what is logged (file-system journals log metadata by default and say
so, databases log page or record changes), whether the end is found by a sentinel (ext4's commit block) or by the first
record that fails a check (the rest), and whether a bad tail is an error or a mode (RocksDB alone makes it a setting).

## E. Which durable-ioring facility a log built on each practice would use

The right-hand column cites the contract; "none" means the crate provides nothing and the log supplies it.

| Practice | Facility | Note |
|---|---|---|
| A record's identity (an LSN) | **Epoch id** | The contract says "a journal transaction ID or a log sequence number can be the id directly". The id orders epochs; it is not stored in the log by the crate. |
| Group commit | **Seal** | `make_durable_through(n)` is the group boundary: the log chooses how many records an epoch holds and when to seal. A commit timer (C1) is the log's policy; the crate has no timer. |
| The write-ahead rule across the log and the data it protects | **Gate across lineages** | A write gated on another lineage's epoch is released only once that epoch is durable. A cycle is refused, unnamed. The log and the data are two lineages. |
| A commit sentinel written after a transaction's records are durable | **Gate** | The sentinel (ext4's commit block) is a write gated on its transaction's epoch: a candidate mapping, not read as a mechanism in any source. |
| A log on its own device | **Flush domain, and a provider** | A target declares the domains it depends on; a provider serves them and answers each once. A provider's request is lent for its call. |
| Several logs or a log and data files | **Lineages** | An independent durability sequence each, with its own epoch space and high-water mark. |
| What to do after a failed flush (C6) | **Failures, suspects, heal, abandon, the observer** | The crate reports a failure and what it puts in doubt; heal means the consumer re-issued the writes, and only a later seal covering them releases it. A log that stops and recovers abandons. |
| A failure holding submit room | **Capacity** | An unresolved failure holds the capacity of the writes it puts at risk until resolved, so a log must heal or abandon or it is refused at submit. |
| Torn or partial record detection (C2) | **None** | "No atomicity beyond the device's"; the crate does not query the device's atomic unit. Checksums and the tail test are the log's. |
| End of the log, record framing, recovery passes, checkpoints | **None** | "Nothing across a process lifetime ... recovery after a crash is the consumer's own." |
| Reclaiming log space (C4) | **None** | The crate bounds what is in flight and what a failure holds; how much log recovery needs is the log's. |
| Ordering within an epoch, or overlapping writes | **None** | The contract gives none; the consumer orders them. |

## F. Findings that bear on DI-21.2

These are findings, not decisions; no work is queued by this record.

1. **The sample is the other half of the division.** Every practice in C2 to C4 that is not group commit or the write-ahead
   rule is the log's, by the contract's own words. A sample therefore has to supply framing, a check on each record, an
   identity, an end-of-log test and recovery, and shows where `durable-ioring` stops. How much of that the sample builds is
   the scope question DI-21.2 brings to the engineer.
2. **Two designs for finding the end of the log are on offer.** A check on each record, stopping at the first that fails
   (PostgreSQL, SQLite, LevelDB, etcd, XFS), or a sentinel that a transaction is complete (ext4's commit block). The second
   maps onto a gate; the first needs no facility. Which the sample shows is open.
3. **Both responses to a failed flush are in use.** PostgreSQL stops and recovers from the log (C6); `durable-ioring` offers
   heal, which needs the consumer to have kept the writes, and abandon. A sample can show either, and the first is the one
   that needs no retention. That bears on the retention question already in DI-21.2.
4. **Group commit's timer sits outside the crate in every source.** The sample is where one would live if it shows one.
5. **No new gap in `durable-ioring` was found, and that is weak evidence.** The survey read practice and built nothing
   against the API, so a gap is more likely to appear while the sample is designed. The one already recorded, ordering
   without durability, is finding 1 of [the earlier session](DESIGN-SESSION-2026-10-24-durable-ioring-prior-art.md).
6. **Windows adds little that is documented.** Microsoft's pages read say nothing about the drive's own cache and name
   `$LogFile` without describing it (C5, B3), so a log on a Windows file cannot cite Microsoft for what its file system
   journals of file data.

## Sources

Read 2026-10-29. Sources 1 to 24 are in [the earlier session](DESIGN-SESSION-2026-10-24-durable-ioring-prior-art.md).

25. PostgreSQL documentation, "28.1. Reliability". https://www.postgresql.org/docs/current/wal-reliability.html
26. PostgreSQL documentation, "28.5. WAL Configuration" and "8.20. pg_lsn Type". https://www.postgresql.org/docs/current/wal-configuration.html
27. PostgreSQL documentation, "19.5. Write Ahead Log" (https://www.postgresql.org/docs/current/runtime-config-wal.html) and "19.14. Error Handling". https://www.postgresql.org/docs/current/runtime-config-error-handling.html
28. SQLite, "Database File Format", the write-ahead log section. https://www.sqlite.org/fileformat2.html
29. SQLite, "Powersafe Overwrite" and "Atomic Commit In SQLite". https://www.sqlite.org/psow.html
30. SQLite, "Write-Ahead Logging". https://www.sqlite.org/wal.html
31. LevelDB, "leveldb Log format". https://github.com/google/leveldb/blob/main/doc/log_format.md
32. RocksDB wiki, "WAL Recovery Modes". https://github.com/facebook/rocksdb/wiki/WAL-Recovery-Modes
33. RocksDB wiki, "Write-Ahead-Log-File-Format". https://github.com/facebook/rocksdb/wiki/Write-Ahead-Log-File-Format
34. MySQL 8.0 Reference Manual, "Redo Log", archived copy. https://web.archive.org/web/20231208180428/https://dev.mysql.com/doc/refman/8.0/en/innodb-redo-log.html
35. Linux kernel documentation, "Journal (jbd2)". https://www.kernel.org/doc/html/latest/filesystems/ext4/journal.html
36. Linux kernel documentation, "ext4 General Information". https://www.kernel.org/doc/html/latest/admin-guide/ext4.html
37. Linux kernel source, `fs/jbd2/checkpoint.c` and `fs/jbd2/recovery.c`. https://raw.githubusercontent.com/torvalds/linux/master/fs/jbd2/recovery.c
38. Linux kernel documentation, "XFS Logging Design". https://www.kernel.org/doc/html/latest/filesystems/xfs/xfs-delayed-logging-design.html
39. Linux kernel source, `fs/xfs/xfs_log_recover.c`. https://raw.githubusercontent.com/torvalds/linux/master/fs/xfs/xfs_log_recover.c
40. Microsoft, [MS-FSCC] "NTFS Reserved File Names". https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/b04c3bd0-79dc-4e58-b8ed-74f19fc2ea0a
41. Linux-NTFS project, "File - $LogFile (2)", archived copy. https://web.archive.org/web/20190603063251/https://flatcap.org/linux-ntfs/ntfs/files/logfile.html
42. OpenZFS documentation, zpoolconcepts(7), "Intent Log". https://openzfs.github.io/openzfs-docs/man/v2.3/7/zpoolconcepts.7.html
43. Prabhakaran, Arpaci-Dusseau, Arpaci-Dusseau, "Analysis and Evolution of Journaling File Systems", USENIX ATC 2005. https://www.usenix.org/legacy/publications/library/proceedings/usenix05/tech/general/full_papers/prabhakaran/prabhakaran_html/index.html
44. Seltzer, Ganger, McKusick, Smith, Soules, Stein, "Journaling Versus Soft Updates", USENIX ATC 2000. https://www.usenix.org/legacy/event/usenix2000/general/full_papers/seltzer/seltzer_html/
45. DeWitt, Katz, Olken, Shapiro, Stonebraker, Wood, "Implementation Techniques for Main Memory Database Systems", SIGMOD 1984. https://github.com/tpn/pdfs/blob/master/Implementation%20Techniques%20for%20Main%20Memory%20Database%20Systems%20-%201984%20(971697.602261).pdf
46. Hagmann, "Reimplementing the Cedar File System Using Logging and Group Commit", Xerox PARC CSL-87-7 (SOSP 1987). https://www.mirrorservice.org/sites/www.bitsavers.org/pdf/xerox/parc/techReports/CSL-87-7_Reimplementing_the_Cedar_File_System_Using_Logging_and_Group_Commit.pdf
47. Helland, Sammer, Lyon, Carr, Garrett, Reuter, "Group Commit Timers and High Volume Transaction Systems", Tandem TR 88.1, read from an OCR transcription. https://archive.org/stream/bitsavers_tandemtechroupCommitTimersandHighVolumeTransaction_1079622/TR88-01_Group_Commit_Timers_and_High-Volume_Transaction_Systems_Mar88_djvu.txt
48. Prabhakaran et al., "IRON File Systems", SOSP 2005. https://github.com/tpn/pdfs/raw/master/IRON%20File%20Systems%20-%202005%20(iron-sosp05).pdf
49. etcd, `server/storage/wal/decoder.go`. https://raw.githubusercontent.com/etcd-io/etcd/main/server/storage/wal/decoder.go
50. MariaDB Knowledge Base, "Group Commit for the Binary Log", archived copy. https://web.archive.org/web/20231230153606/https://mariadb.com/kb/en/group-commit-for-the-binary-log/
51. Linux man-pages, fsync(2). https://man7.org/linux/man-pages/man2/fsync.2.html
52. Microsoft Learn, "File Buffering" and "File Caching". https://learn.microsoft.com/en-us/windows/win32/fileio/file-buffering

## Not found

- **ARIES' passage on cutting the log below what recovery needs**: absent from the paper's text, as in the earlier session.
- **The MySQL pages on a full redo log and on checkpoints**: returned 403 to a direct fetch, so what a full circle does to
  writers is not verified here.
- **A RocksDB changelog entry that a flush error is now a hard error until `DB::Resume()`**: reported by a research agent, and
  its words are not in `HISTORY.md` as fetched. Not quoted.
- **PostgreSQL's reader code on where the log ends**: read by an agent in `xlogreader.c`, not re-fetched.
- **The NVMe specification's atomic write unit for power failure**: not read; a kernel structure's documentation was cited
  by an agent and is not used.
- **Any Microsoft description of `$LogFile`'s contents**, and of what NTFS journals of file data.
- **Btrfs's log tree and ZFS's intent-log record format** beyond the one OpenZFS sentence.
- **Ordering without durability on Windows**: not examined, as in the earlier session.

## DI-21.2: the sample's scope (continued 2026-10-29)

The engineer set the purpose, 2026-10-29: "What I would like to implement is some common easy to use log facility which is
perhaps overly conservative but like I said is easy to use. Ideally with variable length log records, a simple to
understand paradigm regarding log reuse, whether having two log files that we alternate between, or whatever scheme is
commonly used." And: "The intention is to provide an example of the use of the API set over a simple to use log file to
provide a log that is production qiuality, if not necessarily having the utmost performance characteristics we might wish
for. If/when we lift the example to be a separate crate, we would aim for optimal performance characteristics, this is
meant as an example with high utility."

**The assistant's proposal.** A ring of N fixed-size segment files, two by default, of which alternating between two files
is the N = 2 case; records of variable length, each with a header carrying the segment's generation, its LSN, its length
and a CRC-32C, never spanning a segment; the end of the log found by the first record that fails a check, with the
segments ordered by the generation in their headers and no control file; `release_before(lsn)` making whole segments
reusable, and an append refused when every segment is full and none released. Three deliberately conservative rules: pad
every commit to a block boundary, so no block holding a record reported durable is ever rewritten and correctness does not
rest on sector atomicity (C2); zero-fill each segment and make it durable when the log is created, so appends change no
file metadata (C5); and fail-stop on a failed commit, as PostgreSQL does (C6). On the ring: one lineage, the epoch id is the
LSN, and a commit is a seal, which is group commit (E). A second step showing the write-ahead rule enforced by a gate: data
writes gated on the log's epoch.

**The engineer's answers**, the same day:

- "1: ring of N segments."
- "2: pad commit to block boundary and zero fill at creation is smart, good I approve."
- "3: fail-stop on a failed flush, but we do need to explain how to recover"
- "4: I would include it, as you say this is showcasing the ring"
- "5: yes definitely the example. repeat what the blocking API is?"
- "I would like to make this a reality over the memory base dioring, so let's schedule whatever is needed for that so we
  can do a great deal of fault injection testing/modeling"
- "I want the example built so that it can run on all of the above if possible (windows dioring, memory, linux if/when we
  implement)"

**The blocking API, restated** (the assistant's; asked about, not yet answered). The log has one owner and is driven by
`&mut self`; it runs no threads of its own. `append(payload)` copies a record into the current commit group and returns its
LSN; it never blocks, and is refused for a record too large, a log whose segments are all full and unreleased, or a log that
has failed. `commit()` pads the group to a block boundary, writes it, seals through its last LSN and returns that LSN; it
does not block either. `wait_durable(lsn)` blocks the calling thread until the log is durable through `lsn` or has failed,
doing the ring's work on that thread as it waits (pumped delivery), so a simple caller sets nothing up.
`commit_and_wait()` is the two together. `durable_through()` asks without waiting, and `release_before(lsn)` says what the
caller no longer needs. Opening an existing log returns the records it recovered, in order. It must not be called from inside
a provider's callback, which is the same rule as the `blocking_push` example's.

**What follows for the platforms.** A log that runs on the Windows driver, on a dioring over the in-memory backend, and on a
later Linux driver can use only the `DurableRing` contract, plus a small seam each platform supplies for creating or opening
a segment's target; the zero-fill at creation is done through the ring itself, so it needs nothing else. No dioring over the
in-memory backend exists: the only implementations of `DurableRing` are the Windows driver and a test fake. Building one, and
giving the in-memory backend a model of what a crash leaves, is crate work, scheduled as `DI-M24`.

The decision is recorded as [DI-D-116](../crates/durable-ioring/DESIGN-NOTES.md#di-d-116).

## Measured against two-phase commit (continued 2026-10-29)

The engineer: "the time I've seen a common log facility in the past was in support of a two-phase commit facility, where the
2PC commit facility needed the RMs to be able to implement Prepare / Commit well and the common log was the mechanism for
this. Is this at least a beginning foundation for this sort of functionality? I'm not specifically looking to develop a 2pc
but it's the one yardstick I have history with."

**The assistant's answer.** The log half, partly. A forced record is `commit()` then `wait_durable(lsn)`; a record's identity
is its LSN; recovery returns every record reported durable, so in-doubt work is found; the write-ahead rule for a resource
manager's own data is the gate step; and fail-stop then recovery is what a participant does on an I/O failure. Four gaps:

1. **No reservation.** A participant that has voted yes must always be able to log its outcome; an append refused because
   the log is full breaks that. ARIES reserves log space for the same reason (A1); the assistant recalled, without checking
   in this session, that Windows' CLFS offers `ReserveAndAppendLog`.
2. **Tail pinning.** One old, unreleased record holds every segment after it; logs move the tail by re-logging live state
   forward, which only the client can describe.
3. **One client per log.** A common log usually carries several clients' streams, sharing its forces and its space.
4. **No read by LSN while running**, which undo, and resending an outcome, need.

**The engineer:** "Gap 1 seems very worthwhile. Is it very challenging?"; "Gap 2 is not something that I want to pursue I
think. This is a hazard of the 2pc that I would call out of scope. unless you felt this was easily achievable?"; "gap 3: is
the issue here more about identifying the records and their provenance or concurrent access?"; "gap 4: this interests me,
because to the third point, I can imagine a single log that covers multiple concurrent transactions".

**The assistant's answers.** Reservations are accounting, with the conservative rules setting their cost: a reservation for
a record of up to `s` bytes costs its header and `s` rounded up to whole blocks, and each segment counts as its size less one
maximum record, since a record never spans a segment. Unreserved appends are refused before they eat reserved room;
reservations are not durable, and a client re-reserves after reopening, before taking new work. A reservation promises log
space, not the ring's submission capacity, which clears as I/O completes. With reservations, a pinned log refuses new work
while prepared work finishes. Gap 2 is not easy: unpinning needs re-logging only the client can do, so the contract states
the hazard. Gap 3 is provenance and space more than concurrency: records tagged by client, a release point per client with
the log reclaiming only to the lowest, and reservations per client; all clients share one lineage, so they share each force.
For gap 4, an LSN defined as a byte position in an ever-growing log address space, as ARIES and PostgreSQL define it (A1,
A2), makes reading by LSN arithmetic: the generation names the segment and the remainder the offset, and the record's own
header confirms it. Many concurrent transactions need no help from the log beyond that: the client puts a transaction id and
the previous record's LSN in the payload (ARIES' `PrevLSN`) and walks the chain with `read(lsn)`.

**The engineer's answers:** "gap 1: yes. gap 4: yes. gap 3: yes in the sample, implement a simple mutex based
synchronization I guess". Gap 2 stays out of scope. The decision is added to
[DI-D-116](../crates/durable-ioring/DESIGN-NOTES.md#di-d-116).
