# Tasks

## 1. Typed passive network evidence

- [x] 1.1 Add optional typed endpoint/network diagnostic metadata and capability states; verify fixtures for bridge, host, none, container-sharing mode, IPv4/IPv6, aliases, missing IDs, and unsupported backends, and document unknown semantics.
- [x] 1.2 Add bounded on-demand read-only inspection tied to runtime/endpoint/container generations; verify deadline, cancellation/stale response, removed target, partial data, sanitization, and response-cap tests without global inspect polling.
- [x] 1.3 Add pure relationship and port-publication explanations; verify tests for shared exact network IDs, same names with different IDs, no shared networks, exposed-only ports, loopback/wildcard/specific bindings, protocol differences, stopped containers, and uncertain dual-stack overlaps.

## 2. Scoped troubleshooting workflow

- [x] 2.1 Add the contextual route, proposed command, focused network summary, and same-runtime peer picker using existing navigation conventions; verify UI/input tests for target/peer changes, back/close, narrow terminals, unknown fields, and loading/partial failures.
- [x] 2.2 Apply workspace visibility to all projections and links; verify tests prevent outside-project names, addresses, aliases, and warning details leaking while preserving hidden attachment counts.
- [x] 2.3 Retain network-to-container navigation and add explicit not-tested labels; verify UI tests never claim shared membership proves connectivity or no shared membership proves disconnection.
- [x] 2.4 Add demo examples, localization, and README/help coverage explaining passive versus active tests and remote-host binding semantics; verify translation coverage and manually inspect fixtures in demo mode.

## 3. Integration verification

- [x] 3.1 Run `make check` and `openspec validate add-network-troubleshooting --strict --no-interactive`; record results and verify no added path invokes exec, probes, or runtime mutation.
- [x] 3.2 If a suitable engine is available and the user permits test workloads, compare displayed metadata with the engine's inspection output; record engine/version and explicitly list unverified modes/backends.

## Verification record (2026-10-31)

- 1.2 stays open for one gap only: results are bounded after parsing, but the response body is not capped before deserialization, because Bollard's `inspect_container` offers no limit. All other parts of 1.2 are done and tested.
- 3.1: the only runtime call added is `inspect_container`. No path adds exec, probes, helpers or mutation.
- 3.2 used read-only inspects of containers that were already running on Docker 29.5.2 (API 1.54, Colima). No workload was started or changed. Confirmed against it: bridge mode, network ID, IPv4 with prefix, and null aliases on the default bridge.
- Not verified on a real engine, fixture-tested only: host, none and container-sharing modes, IPv6, reported aliases, user-defined networks, Podman, and native containerd against a live runtime.
- 2.4 was reviewed by rendering demo fixtures to a test backend, not in a live terminal.
- Attachment lists match the inventory's network by name, because `Resource` has no network ID. Shared-network comparison uses IDs only.
- `make check` and `openspec validate --changes --strict --no-interactive` pass.

## Workflow follow-up

- Review the passive first-release scope before implementation and treat active testing as a separate future change.
- Archive only after implementation, checks, and user review.

## Response cap (2026-11-01)

- 1.2 is now closed. `diagnostics::inspect` reads the response through `backend::docker::get_json_capped`, which stops at `MAX_RESPONSE_BYTES` (8 MiB) before parsing and works over local sockets and the remote relay. Container IDs are validated before they enter the request path. Tests cover a normal response, an oversized response, and IDs that could change the path. The image comparison on the Overview tab still uses Bollard's `inspect_image`.
