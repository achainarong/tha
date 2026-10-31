# Tha feature handoff

## Status and intent

Prepared 2026-10-31 as an implementation handoff, not an approval to run workload actions. All three changes have since been implemented; each `tasks.md` has a verification record and lists what remains open. None are archived, pending review.

The user requested three features:

1. Better log filtering: entering text hides nonmatching entries; multiple words use AND, not OR. Their example is `backend1 500`: both must occur in the same log message.
2. A “Why is this unhealthy?” view joining health-check failures, exits, restarts, OOM evidence, events, and relevant logs.
3. Network troubleshooting.

Recommended delivery order: log filtering, health diagnostics, network troubleshooting. This is a recommendation, not a user-mandated priority. Each change can ship independently.

## OpenSpec artifacts

| Change | Capability | Location |
| --- | --- | --- |
| `improve-log-text-filter` | `log-text-filtering` | `openspec/changes/improve-log-text-filter/` |
| `add-container-health-diagnostics` | `container-health-diagnostics` | `openspec/changes/add-container-health-diagnostics/` |
| `add-network-troubleshooting` | `network-troubleshooting` | `openspec/changes/add-network-troubleshooting/` |

Each directory contains `proposal.md`, `design.md`, `tasks.md`, and a delta specification under `specs/`. OpenSpec was initialized with the `spec-driven` schema and no generated agent integrations. The repository had no existing OpenSpec specifications; these are new capability contracts, not a complete baseline of Tha's existing behavior. Do not archive them before implementation and review.

## Decision gate before implementation

**Resolved 2026-10-31: message text only.** A source named `backend1` with the payload `HTTP 500` does not match `backend1 500`.

Other proposed defaults, not separately confirmed by the user: case-insensitive literal substrings, whitespace-separated terms, no quote/operator syntax, a simple text field separate from advanced structured JSON filters, and preserving `/` as highlight search. Health and network commands, bounds, and first-release scope in the designs are engineering proposals for review.

## Starting instructions for the next agent

1. Read this handoff and all artifacts for the chosen change; inspect current source before editing. No application source was changed by this handoff.
2. Resolve the log scope decision above with the user before implementing that change. Review the proposed read-only network scope with them before expanding it.
3. Run `openspec status --change <change-name>` and `openspec instructions apply --change <change-name>` for the current checklist and workflow.
4. Implement in small slices; add tests with each slice and check tasks only after their verification passes. Keep unrelated user work intact.
5. Run `openspec validate --changes --strict --no-interactive` and `make check`. For this documentation-only handoff, OpenSpec validation is the relevant check; application checks are required when implementation changes land.
6. Review behavior in demo mode and use opt-in real-runtime checks only where needed. Never start engines, workloads, probes, or helper containers merely to inspect these features.
7. After implementation and review, archive each completed change with `openspec archive <change-name>` and inspect the resulting main specification.

## Shared implementation constraints

- Keep the terminal responsive: bounded asynchronous requests, capped output, stale-result rejection, and isolated endpoint failures.
- Keep runtime and immutable container identity attached to diagnostic results. Never redirect evidence to a recreated container or a different engine.
- Missing/unsupported data is unknown, never zero, healthy, connected, or a successful test.
- Reuse existing navigation, theme, localization, keymap, sanitization, logs, events, and metrics behavior. Avoid adding numbered detail tabs that conflict with the existing eight tabs.
- Standalone must remain useful without a collector. Historical coverage differs from service mode and must be explained.
- Health output and logs can contain secrets. Sanitize terminal controls and do not add automatic uploads, exports, or new persistent diagnostic stores.

## Source map verified during handoff

- `src/logs/filter.rs`: structured source/level/JSON-field/time predicates; currently no plain-text term filter.
- `src/logs/view.rs`: retained-row projection, search, wrapping, layout/search caches, and anchors.
- `src/logs/export.rs`: filtered retained-entry export snapshot; ensure text filtering uses the same predicate.
- `src/keyboard.rs`, `src/ui/mod.rs`: log filter interactions, dialogs, log rendering, overview/network rendering, and UI tests.
- `src/model.rs`: `Container` already includes health status, exit code, OOM flag, error, port bindings, and network names; detailed health-check attempts and restart count are not fields on this model.
- `src/backend/docker.rs`: Docker-compatible inventory and existing container inspect usage; inspect reuse needs verification rather than assuming detailed health data is already collected.
- `src/backend/mod.rs`, `src/backend/containerd.rs`: backend dispatch and capability differences.
- `src/events.rs`, `src/ui/events.rs`, `src/history.rs`, `src/ui/history.rs`: existing event/metrics evidence and history integration.
- `src/app.rs`, `src/demo.rs`, `src/i18n/`, `src/keymap.rs`, `src/shortcuts.rs`: state, fixtures, translation, and discoverability integration.
- `tests/runtime_contracts.rs`, `benches/performance.rs`: runtime fixtures and performance coverage to extend where applicable.

The network model currently keeps network membership and generic resource details, not a complete typed per-endpoint address/alias model. Network metadata work must account for this rather than promising existing DNS or connectivity-test support.

## Handoff verification

- `openspec validate --changes --strict --no-interactive`: all three changes passed.
- `openspec status --change improve-log-text-filter`: all four planning artifacts present. Artifact completion does not mean implementation completion or resolution of the product decision gate.
- No application code was changed and application tests were not run for this documentation-only handoff.
