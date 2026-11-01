# Design

## Context

See `proposal.md` for motivation. `src/logs/filter.rs` parses a JSON object containing source, level, field, and time predicates. `LogView` in `src/logs/view.rs` separately manages highlighted literal/regex search and caches keyed partly by the filter. Export snapshots already accept the structured filter.

## Goals / Non-Goals

**Goals:** One shared effective predicate for rendering and export; no retained-buffer mutation; understandable input; stable scrolling and bounded work.

**Non-Goals:** Replacing highlight search, fetching additional history, a query language, multiline grouping, collector-side filtering, saved queries.

## Decisions

1. **Matching scope: message text only (user decision, 2026-10-31).** Source names do not match, so a source named `backend1` with the message `HTTP 500` does not match `backend1 500`. The advanced `source` key narrows by source. The timestamp prefix the engine adds is also excluded.
2. **Separate simple and advanced controls.** Proposed interaction: F3 / `:filter-logs` opens a clearly labeled plain-text filter field with an Advanced route retaining structured JSON editing. Keep `/` as highlight search and Ctrl-R as search-only regex mode. Do not auto-detect whether arbitrary text is JSON: log text can legitimately begin with `{`. Verify existing keyboard customization and modal conventions before wiring this flow.
3. **Whitespace AND tokens.** Split on whitespace, ignore empty terms, lowercase once per query, and use literal substring tests against message text. Order is irrelevant. No quote stripping or wildcard/operator interpretation in v1; quotes are literal characters. Alternatives such as OR-by-default or a full grammar do not satisfy the requested simplicity.
4. **One effective filter.** Add text terms to the effective filter state used by `Filter::matches`, while preserving backward-compatible structured JSON parsing. Keep simple text editing from erasing advanced predicates and vice versa. This avoids mismatches between rendered rows and exports; do not implement a renderer-only hide pass.
5. **Original entry semantics.** Match the original retained payload, not the source label, inserted JSON indentation, or separate wrapped screen rows. Retain full matching entries. AND terms cannot be satisfied by different entries.
6. **Explicit edit/apply/cancel.** Proposed flow applies on Enter and cancels on Esc without changing the active filter. Clearing text preserves advanced predicates; the existing clear-all command clears both. Display active terms and matching/retained entry counts without confusing these with wrapped-row counts. Export confirmation includes the active text constraint.
7. **Invalidate derived state only.** Ensure filter equality/cache keys include text terms. Recompute visible layouts and search counts on filter changes, follow incoming matching entries when following, and clamp paused anchors when their entry is hidden or evicted. Do not refetch logs or reset the retained buffer on filter edits.

## Risks / Trade-offs

- [Source names might be expected to match] → The dialog says source names do not count, and a discriminating source-only test covers it.
- [Large buffers and fast live streams] → Compile terms on edit, reuse the existing bounded view pipeline, and extend performance tests to catch repeated full-buffer work on idle frames.
- [Two filter controls become confusing] → Label Text and Advanced, show a combined summary, and document each clearing action.
- [Unicode case folding changes substring behavior] → Follow existing lowercase conventions; cover non-ASCII input without promising locale-aware linguistic matching.

## Migration Plan

No database or configuration migration. Preserve existing advanced JSON filter documents and search bindings. Add backward-compatibility tests before changing input wiring. Rollback is code-only and must not discard retained data.
