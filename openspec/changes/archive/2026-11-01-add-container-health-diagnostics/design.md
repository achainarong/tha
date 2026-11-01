# Design

## Context

See `proposal.md`. `Container` already records health status, exit code, OOM flag, runtime error, and state. It does not expose detailed health-check attempts or restart count. Docker-compatible inspect is already used in `src/backend/docker.rs`; inventory health alone is insufficient. Events and metrics have different retained coverage in standalone and service modes.

## Goals / Non-Goals

**Goals:** An evidence-led single-container explanation; no polling of detailed inspection across every container; useful partial results; safe links to existing evidence.

**Non-Goals:** Root-cause certainty, automatic fixes, executing health checks, AI, cross-container causal inference, new persistence, broad group diagnosis.

## Decisions

1. **On-demand diagnostic state.** Propose `:diagnose` plus a contextual “Why is this unhealthy?” action, available for a selected container regardless of current health. Use a scrollable detail overlay rather than a ninth numbered tab. Confirm naming during review; do not steal a global key. Show a useful “no current failure evidence” state for healthy containers.
2. **Dedicated read-only inspect result.** Prefer a focused typed diagnostic model over inflating every inventory snapshot. Include optional reported restart count, health configuration/status/failing streak, recent attempt timestamps/exit/output, and state timestamps/error/OOM/exit facts. Distinguish absent fields from zero/false when the backend does not report them. Do not execute configured checks or display their command by default, since commands can contain credentials.
3. **Bounded asynchronous load.** Proposed initial limits: one active diagnostic load per selection, a five-second request deadline, ten latest reported health attempts, and 4 KiB displayed output per attempt with explicit truncation. Cap response bodies using existing transport limits or add a transport cap before deserialization if necessary. Selection/endpoint changes cancel or invalidate results. Manual refresh is explicit; do not add global detailed-inspect polling.
4. **Identity and freshness.** Bind to endpoint generation, runtime identity, namespace, immutable container ID, and request generation. Recreated containers with the same name must not inherit evidence. Show observation time and stale/offline status; reject late responses for replaced targets.
5. **Evidence, not diagnosis by assertion.** Structure the UI as Current state, Health checks, Exit/OOM/restarts, and Related evidence. A nonzero exit is not automatically the cause of current unhealthy status; historical OOM must be labeled historical. Missing health configuration is “no health check configured” only when inspect confirms it; unavailable inspection is “unknown.” Restart totals do not prove a restart loop. A possible-loop hint requires multiple timestamped restart events in a documented window and is labeled a possible explanation.
6. **Reuse evidence sources.** Read existing scoped event history and retained logs without clearing the user's log buffer or replacing subscriptions just to populate the summary. Include only timestamp-correlated evidence when a failure time is known; otherwise label it “recent available logs,” not “logs at failure.” Proposed preview: twenty events and twenty retained log entries. Deep links select the same immutable target; preserve independent log filters and warn when they hide evidence. Metrics links do not claim unavailable history exists.
7. **Backend capability boundaries.** Docker-compatible inspect is the first path. Handle omitted Podman fields explicitly. Native containerd can show already-available state and clear unsupported notices, not fabricated Docker health details. One failing request cannot block inventory, other runtimes, or navigation.

## Risks / Trade-offs

- [Health output contains secrets or terminal escapes] → Reuse sanitization, cap output, keep it local, and do not add automatic export or persistence.
- [Historical facts look like current causes] → Show timestamps, provenance, and separate observation from possible explanation.
- [Removed/recreated targets and endpoint races] → Reject generation/identity mismatches and offer refresh rather than redirecting by name.
- [Standalone has less history] → State available coverage and link to existing views without promising missing evidence.

## Migration Plan

No database migration or module installation. Introduce the action and diagnostic state incrementally behind normal capability checks. Rollback removes the view without modifying workload or historical data.
