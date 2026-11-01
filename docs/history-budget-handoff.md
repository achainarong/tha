# History disk budget handoff

Written 2026-11-01, when v0.29.0 was released. Implemented for v0.30.0.

The sections below preserve the original handoff. The implementation uses bounded
oldest-first eviction, a full-database write retry, and cleanup after budget reductions.
`tha history status` reports the age of the oldest retained reading. Normal eviction
does not count as dropped readings; actual write failures still do. Validation passed:
`make check`, `scripts/daemon_smoke.py`, and `scripts/remote_smoke.py`.

## Decision

**When history reaches its disk budget, delete the oldest readings and keep saving new ones.** This was
the user's decision, made on 2026-11-01.

It reverses the current behaviour, which is documented as deliberate: new readings stop being saved "rather
than old ones being lost". Those docs must change with the code (see below).

Why: recent readings are usually what someone needs while looking into a problem, and today a full budget
leaves a growing gap at the newest end of the chart until old readings expire.

## How it works today

All in `src/history.rs` unless noted.

- `capacity_pages` caps the database at the budget minus 4 MiB through SQLite's `max_page_count`.
- `Store::append_many` writes nothing once `size_bytes` is within 1 MiB of the budget. In practice the page
  cap usually comes first, and the insert fails with `SQLITE_FULL`.
- `Pending::flush` in `src/daemon.rs` counts failed writes as dropped readings and shows a warning.
- `Store::prune_batch` only deletes readings older than the retention period.
- `Store::maintenance` runs about once a minute. It removes up to 2000 containers with no readings left, and
  calls `Store::reclaim`.
- `Store::reclaim` gives empty space back with `VACUUM` when the file is over budget. It never deletes
  readings. This fix for a lowered budget should stay.

Measured with a 16 MiB budget: deleting rows and running maintenance still work while the database is full,
and space from deleted rows is reused without shrinking the file.

The agent shares this code through `src/agent/history.rs`, with its own `DISK_BYTES`, so the change applies
to servers too.

## Suggested design

These are proposals for whoever implements this, not decisions.

1. Add an eviction step to `Store` that deletes the oldest samples in small batches, using the `samples_expiry`
   index: `... ORDER BY at LIMIT 2000`. Small transactions let readers and ingestion run in between.
2. Measure used space as `(page_count - freelist_count) * page_size`, not file size, because deleting rows
   does not shrink the file.
3. Start evicting at about 90% of capacity and stop at about 80%, so it doesn't run on every write. Limit the
   batches per call to keep the writer responsive.
4. Run it from `maintenance`, so it normally happens before the budget is reached. In `append_many`, on
   `SQLITE_FULL`, evict and retry the write once. The transaction has already rolled back by then.
5. After a lowered budget, evict until the data fits, then let `reclaim` shrink the file.

Check that a single flush can't outrun eviction, for example a large fleet on the minimum 16 MiB budget. If a
write still fails after the retry, keep today's dropped count and warning.

## Things to decide while implementing

- Should the status mention eviction? The effective history is now shorter than `history_retention_hours`, and
  users will otherwise wonder why. Suggested: say how far back history really goes, without a warning, since
  this is now normal operation.
- Containers whose readings were all evicted are cleaned up by `maintenance` at 2000 per pass. Check this keeps
  up, because those rows count towards used space.

## What has to change

- Code: `Store::append_many` and `Store::maintenance` in `src/history.rs`, and possibly the warnings in
  `Pending` in `src/daemon.rs`.
- Tests in `src/history.rs`:
  - The test that sets `store.budget` just above the current size asserts "1 dropped" and "Disk budget
    reached". It needs rewriting.
  - `lowering_the_budget_resumes_once_old_readings_have_expired` asserts that readings within retention are
    kept when over budget. That will no longer be true.
  - Add tests: at the budget, the newest reading is saved and the oldest is gone; a chart query still works;
    the file stays within budget over many write and evict cycles.
- Comments: the doc comment on `Store::reclaim` says readings within retention are never removed.
- Docs:
  - `docs/guide.md`, the paragraph on `history_retention_hours` and `history_disk_mb`.
  - `docs/remote-history.md`, the Retention bullet.
  - The `history_disk_mb` comment in the guide's config example, if its meaning changes.

## Before finishing

Run `make check`, `scripts/daemon_smoke.py` and `scripts/remote_smoke.py`. Any notice text shown in the TUI
needs a natural German translation in `src/i18n/de.rs`; the collector's status and log text is English only.
