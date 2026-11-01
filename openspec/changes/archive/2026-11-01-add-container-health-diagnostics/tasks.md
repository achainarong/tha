# Tasks

## 1. Gather typed bounded evidence

- [x] 1.1 Add the focused diagnostic result and capability states; verify tests distinguish reported zero/false from missing/unsupported fields and document these semantics.
- [x] 1.2 Add on-demand Docker-compatible inspection with deadline, response/output bounds, and bounded health history; verify fixtures for healthy, unhealthy, no configured check, restarting, OOM, nonzero exit, missing fields, and malformed/oversized output.
- [x] 1.3 Bind requests to runtime/endpoint/container generations and reject stale results; verify tests for selection changes, recreated names, removed targets, offline engines, and partial errors without stopping inventory.

## 2. Present evidence and navigation

- [x] 2.1 Add `:diagnose` and a contextual action with a scrollable diagnostic overlay; verify keyboard, close/back behavior, narrow layout, refresh, sanitization, and healthy/unsupported/partial states in UI tests.
- [x] 2.2 Integrate bounded available event/log previews with timestamps and coverage labels; verify tests do not merge evidence from different container IDs or claim timestamp-free logs correlate with a failure.
- [x] 2.3 Add links to Logs, Events, and Metrics preserving identity and existing filters; verify integration tests for navigation, missing history, hidden log evidence, and no implicit workloads or health-check execution.
- [x] 2.4 Add representative demo fixtures and localized help/README guidance; verify translation coverage and manually inspect the evidence distinctions in demo mode.

## 3. Integration verification

- [x] 3.1 Run `make check` and `openspec validate add-container-health-diagnostics --strict --no-interactive`; record results and verify loading never blocks keyboard navigation.
- [x] 3.2 If a suitable test engine is available and the user permits test workloads, verify supported inspect fields against it; record engine/version and leave unavailable runtime checks explicitly unverified.

## Verification record (2026-10-31)

- 1.2 stays open for one gap only: output and attempts are bounded after parsing, and the 5-second deadline covers the request, but the response body is not capped before deserialization. Bollard's `inspect_container` offers no limit, so closing this needs a custom transport call. All other parts of 1.2 are done and tested.
- 3.2 used read-only inspects of containers that were already running on Docker 29.5.2 (API 1.54, Colima). No workload was started or changed. Confirmed against it: unhealthy check with 5 attempts, no configured check, restart counts, start and finish times, and a 404 for an unknown ID.
- 3.2 found that Docker clears `ExitCode` and `OOMKilled` on start, so a container with 391 restarts reports exit 0 while running. The view now says so, and timed exits overrule its "no current failure" statement.
- Not verified on a real engine, fixture-tested only: Podman, native containerd against a live runtime, OOM flag set, and health output containing terminal controls.
- 2.4 was reviewed by rendering demo fixtures to a test backend, not in a live terminal.
- User request after review: the Overview tab now shows the health check as configured and any entrypoint or command that differs from the image, so commands are visible by default there. This also reads the image with `inspect_image`, for that tab only. This overrides design decision 2 for that tab; the `:diagnose` overlay still leaves the command out and points to the tab. It inspects only the selected container, every 10 seconds while the tab is open, not every container.
- `make check` and `openspec validate --changes --strict --no-interactive` pass.

## Workflow follow-up

- Review proposed commands, limits, and evidence wording before implementation is considered complete.
- Archive only after implementation, checks, and user review.

## Response cap (2026-11-01)

- 1.2 is now closed. `diagnostics::inspect` reads the response through `backend::docker::get_json_capped`, which stops at `MAX_RESPONSE_BYTES` (8 MiB) before parsing and works over local sockets and the remote relay. Container IDs are validated before they enter the request path. Tests cover a normal response, an oversized response, and IDs that could change the path. The image comparison on the Overview tab still uses Bollard's `inspect_image`.
