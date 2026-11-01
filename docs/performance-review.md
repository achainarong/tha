# Rust performance review

Reviewed 2026-11-01 at `e2be6f7` (Tha 0.20.4). The initial audit and baseline are preserved below; implementation results are recorded in the follow-up section.

The requested `rust-performance-optimizer` skill was unavailable. Review used the installed `rust-best-practices` guidance, source inspection, Clippy, and the existing offline release benchmarks. The initial audit made no application changes.

## Summary

Yes, there are useful opportunities. Start by measuring and removing repeated container cloning in log-scope checks, then consider Explorer rebuilds and log viewport traversal. Existing incremental log synchronization is already extremely cheap and should be preserved.

| Priority | Opportunity | Evidence | Next step |
| --- | --- | --- | --- |
| 1 | Avoid cloning selected containers before unchanged log-scope checks | Confirmed source work; not timed by current benchmarks | Benchmark `update_logs` with unchanged large selections |
| 2 | Reuse Explorer structure between snapshot changes | Measured refresh cost of 6.17–7.56 ms at 5,000 containers | Separate construction from expansion/filtering and measure both |
| 3 | Read the log viewport without traversing all preceding wrapped rows | Measured follow viewport cost of 104 µs for long retained lines | Add a reverse-tail fast path and cached row count |
| 4 | Index network/volume attachments and project membership | Confirmed repeated scans; no resource-heavy benchmark | Add resource-rich fixtures before changing lookup strategy |
| 5 | Overlap independent inventory requests | Sequential network calls visible in source; runtime latency unmeasured | Measure a delayed mock runtime, preserving partial failures |

Priority reflects likely value and frequency, not a measured ranking of end-to-end bottlenecks.

## Measurements and limitations

Environment: Linux x86_64, AMD Ryzen 9 5950X (16 cores / 32 threads), rustc 1.99.0, LLVM 23.1.1. Existing release configuration uses thin LTO and strips symbols. One benchmark invocation on this machine; no isolated CPU, allocation profile, flamegraph, or live runtime/SSH measurement.

Commands run:

```sh
cargo bench --locked --offline --bench performance
cargo clippy --all-targets --all-features --locked -- -D clippy::perf
cargo clippy --all-targets --all-features --locked -- -D clippy::perf -W clippy::redundant_clone -W clippy::needless_collect
```

All commands succeeded. Default performance lints passed. Additional lints found only a redundant clone in test code at `src/discovery.rs:225`; this is not an application optimization priority.

Selected benchmark results, converted from µs to ms where helpful:

| Workload | Median | Batch p95 |
| --- | ---: | ---: |
| 500 retained short lines: visible follow viewport | 1.315 µs | 1.710 µs |
| 5,000 retained short lines: visible follow viewport | 7.715 µs | 7.758 µs |
| 5,000 retained long lines: visible follow viewport | 104.067 µs | 121.512 µs |
| 5,000 retained long lines: unchanged layout sync | 0.051 µs | 0.052 µs |
| 5,000 retained long lines: cold layout | 11.109 ms | 11.926 ms |
| 5,000 retained long lines: push + search sync | 22.173 µs | 23.000 µs |
| 5,000 retained long lines: full search frame | 0.665 ms | 0.695 ms |
| Long-line stream frame, 32 new entries | 1.370 ms | 1.440 ms |
| Long-line stream frame, 512 new entries | 12.465 ms | 14.613 ms |
| Explorer refresh, 100 containers / 1 project | 0.115 ms | 0.137 ms |
| Explorer refresh, 1,000 containers / 1 project | 1.192 ms | 1.208 ms |
| Explorer refresh, 5,000 containers / 1 project | 6.166 ms | 6.930 ms |
| Explorer refresh, 5,000 containers / 1,000 projects | 7.563 ms | 8.168 ms |

The harness takes 21 batch samples after three warm-up actions. Its p95 is the p95 of batch-average operation times, **not** individual input-to-display latency. Frame tests use a 160 × 50 TestBackend; they exclude real terminal transport and event-loop queueing. Log fixture labels use nominal lengths (100/4,000); fixture text is built by repeating a string, not truncating to those exact lengths.

The largest measured stream frame remains below the event loop's 70 ms log-batching interval, so these results do not establish a frame-budget failure. Conversely, the full-frame benchmark calls `ui::draw` directly and does not measure `update_logs`, backend requests, or other main-loop preparation.

## 1. Stop deep-cloning containers on unchanged log selections

**Locations:** `src/main.rs:937`, `src/main.rs:1160–1179`, `src/main.rs:1263–1279`, `src/app.rs:2931–2952`.

Before each redraw, `update_logs` obtains `selected_containers()`. That filters the snapshot and clones every selected `Container`, including its owned strings and collections. It then constructs a formatted scope key, including a temporary vector of container keys, before discovering that the subscription scope is unchanged and returning.

**Proposal:** Introduce a borrowed selection iterator for read-only work and a structured subscription identity. Eventually gate scope recomputation on selection/snapshot generations. Only create owned subscription targets when a subscription actually changes. Read-only UI callers such as `src/ui/mod.rs:2323` can also benefit from borrowed selection access.

**Validation:** Benchmark unchanged scopes containing 1/100/1,000/5,000 metadata-rich containers, including allocations. Exercise the real pre-draw path, not just `ui::draw`. Verify reconnects, membership changes under the same selected row, `since` changes, alias/offline transitions, and export scope validation. Do not replace the identity with only the row key: container membership can change without selection changing.

## 2. Avoid rebuilding the entire Explorer for view-only changes

**Locations:** `src/app.rs:2750–2773`, `src/app.rs:3484–3683`, `src/app.rs:3719–3757`.

`refresh_tree` constructs a fresh forest of owned nodes, groups projects/services, formats titles, and then clones rows into the flattened result. Expansion, collapse, and search use the same rebuild path as new snapshots. At 5,000 containers this costs 6.17–7.56 ms even in the synthetic fixture without networks or volumes.

**Proposal:** First benchmark construction and flattening separately. Consider caching each engine's structural tree until its snapshot/context presentation changes, then flattening that structure for expansion/filter changes. Compute the normalized query once per refresh, not once per root. Search can also precompute subtree matches rather than recursively rechecking descendants at successive levels; tree depth is limited, so do not label this universally quadratic.

**Validation:** Add expanded/collapsed, filtered/no-match, multi-engine, namespace, and resource-rich workloads. Current tree timings cover the empty-query refresh; the later search assertion is not timed. Preserve matching-parent subtree exposure, selection retention, ordering, machine focus, aliases, localization, and shared resource placement. Start with narrow invalidation rules rather than a global cache that can silently go stale.

## 3. Make follow-mode viewport lookup proportional to visible rows

**Locations:** `src/logs/view.rs:297–359`.

`start` sums every layout's row count in follow mode. `visible` then flattens layouts from the beginning and skips to the tail. Despite incremental layout/search synchronization, retrieving the same 30 visible rows becomes more expensive as retained wrapped content grows: 1.315 µs for 500 short entries versus 104.067 µs for 5,000 long entries.

**Proposal:** Maintain a total visual-row count as layouts are appended, evicted, or invalidated. In follow mode, walk backward from the last layout until the viewport is filled, then restore display order. Keep the existing anchored scrolling path initially; add prefix indexes only if paused scrolling measurements justify them.

**Validation:** Compare viewport contents against the existing traversal for eviction, partial first/last entries, resize, Unicode, wrapping, filtering, and pretty JSON. Re-run `visible_follow`, full-frame, and streaming benchmarks. The isolated viewport cost is only part of the 0.665 ms long-line frame, so expect a smaller whole-frame gain than microbenchmark gain.

## 4. Build attachment indexes once per snapshot

**Locations:** `src/backend/docker.rs:441–499`, `src/app.rs:3607–3636`.

For each network and volume, snapshot construction scans containers twice: once for owners and once for attached names. Each membership check also searches the container's attachment vector. Explorer resource grouping additionally loops over resources for each project and can scan containers again.

**Proposal:** Build network/volume-to-container and owner-project indexes in a single pass over container attachments. Reuse equivalent project membership information when constructing resource nodes. Preserve stable display ordering rather than exposing hash iteration order.

**Validation:** Current Explorer fixtures contain no resources and cannot quantify this opportunity. Add many-project fixtures with shared/external networks and volumes, varying attachments per container. Test unattached resources, cross-project sharing, duplicate names across namespaces, missing labels, and failed inventory calls. Measure before adding persistent indexes: a temporary per-snapshot index may be sufficient.

## 5. Measure sequential inventory request latency

**Locations:** `src/backend/docker.rs:274–295`, `src/backend/docker.rs:409–505`.

Snapshot collection negotiates a client and fetches runtime information and containers, then awaits images, networks, and volumes sequentially. Independent resource request latencies therefore accumulate; a slow images request delays starting the later resource requests.

**Proposal:** After confirming dependencies, overlap the independent resource fetches while retaining their individual result handling. Separately measure whether reusing a negotiated client per worker is worthwhile; reconnects and runtime replacement must invalidate it. Do not parallelize dependent work or introduce unbounded request concurrency across runtimes.

**Validation:** Use delayed mock endpoints to compare total snapshot latency and verify that one failed inventory does not hide containers or successful inventories. Preserve existing timeouts and the worker's cancellation/replacement behavior. No live-runtime speedup was measured in this review.

## Implementation order and guardrails

1. Add missing pre-draw/log-scope and resource-heavy benchmarks; retain this baseline.
2. Optimize unchanged log-scope checks and follow-mode viewport lookup as small, separate changes.
3. Measure Explorer construction versus flattening before introducing caching.
4. Tackle attachment indexes and runtime request overlap only with representative fixtures.
5. Compare multiple release runs under similar machine load and report both operation and end-to-end results, plus allocation counts where relevant.

Keep existing incremental log layout/search caches, bounded channels, log redraw batching, and skipped missed timer ticks. Do not replace them with full rescans. There is no evidence here justifying unsafe code, custom allocators, blanket inlining, a new async runtime, or release-profile changes. Cold-layout and 512-entry long-line burst costs also deserve profiling before selecting a parsing/layout optimization; the current aggregate measurements do not identify which sub-operation dominates.

The ignored history-storage benchmark was not run, and history/database, metrics sampling, security scanning, remote transport, and real terminal output are not performance-certified by this review.

## Follow-up: first two low-risk optimizations

Implemented the agreed log-scope and viewport changes without changing subscription identity semantics or paused navigation:

- Added `App::selected_container_refs()` for borrowed selection access. Log-scope checks and export validation use it; owned subscription targets are cloned only after detecting a scope change. The formatted identity and its membership ordering remain unchanged, avoiding a broader cache invalidation change. Key strings and the temporary key vector still allocate; generation-based identity caching remains deferred.
- Cached the retained visual-row total, updating it on layout append, eviction, mode invalidation, and content reset. Follow-mode viewport traversal now reads backward from the tail and reverses only the returned visible rows. Paused traversal remains unchanged.
- Added regression coverage for borrowed membership/project focus, unchanged subscription content, changed membership under the same selected row, `since` changes, disconnect/reconnect, Unicode/partial wrapped rows, continuation flags, eviction, resize, empty filtering, pretty JSON, and content reset. Existing export-pinning tests also pass.
- Added an explicitly ignored `benchmark_unchanged_log_scope` test covering 1/100/1,000/5,000 metadata-rich containers. It times the real subscription check, not only UI rendering.

Verification:

```sh
cargo test --all-targets --all-features --locked --offline
cargo clippy --all-targets --all-features --locked --offline -- -D warnings
cargo bench --locked --offline --bench performance
cargo test --release --locked --offline --bin tha benchmark_unchanged_log_scope -- --ignored --nocapture
```

The full offline suite passed: 579 tests, 14 intentionally ignored tests (including opt-in benchmarks and live runtime/SSH checks). Clippy passed with warnings denied. These checks are evidence against regressions, not a guarantee; no live runtime or interactive terminal smoke test was performed.

Selected release median comparison (same machine, separate invocations):

| Operation | Before | After |
| --- | ---: | ---: |
| Unchanged log-scope check: 1 container | 0.529 µs | 0.306 µs |
| Unchanged log-scope check: 100 containers | 34.532 µs | 7.337 µs |
| Unchanged log-scope check: 1,000 containers | 352.218 µs | 65.300 µs |
| Unchanged log-scope check: 5,000 containers | 1.863 ms | 0.315 ms |
| Follow viewport: 500 short entries | 1.315 µs | 0.151 µs |
| Follow viewport: 5,000 short entries | 7.715 µs | 0.173 µs |
| Follow viewport: 5,000 long entries | 104.067 µs | 0.089 µs |
| Long-entry unchanged layout sync | 0.051 µs | 0.051 µs |
| Long-entry full search frame | 665.299 µs | 564.158 µs |
| Long-entry 512-arrival stream frame | 12.465 ms | 11.153 ms |

These are single-invocation comparisons, not controlled statistical claims. Unchanged Explorer code also measured faster in the follow-up, demonstrating machine/run variance; do not attribute all full-frame or streaming differences to these changes. The viewport result directly supports removal of retained-history traversal. Remaining work: Explorer structural caching, attachment indexes, and overlapping inventory requests still require their own benchmarks and implementation passes.
