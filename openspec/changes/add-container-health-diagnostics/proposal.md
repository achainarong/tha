# Proposal

## Why

Tha already exposes health, events, logs, and metrics in separate places, but users must assemble the explanation for an unhealthy or repeatedly failing container themselves. A read-only evidence summary can make this workflow faster without guessing a root cause or requiring AI.

## What Changes

- Add a discoverable “Why is this unhealthy?” diagnostic view for a selected container, also useful for failed exits and restarts.
- Join reported state, bounded recent health-check attempts, restart count where available, exit/OOM/error evidence, and relevant available events/logs.
- Distinguish current observations, historical evidence, possible explanations, and missing/unsupported data.
- Link directly to existing Logs, Events, and Metrics views while retaining container scope.
- Start with Docker-compatible inspection; show explicit limits for Podman differences and native containerd.
- Exclude automatic remediation, AI analysis, scheduled alerts, extra persistence, and group-wide diagnosis in the first version.

## Capabilities

### New Capabilities

- `container-health-diagnostics`: Read-only, identity-scoped evidence explaining reported unhealthy and failing container states.

### Modified Capabilities

None; there is no existing OpenSpec baseline.

## Impact

Touches backend inspection, typed diagnostic state, async request handling, contextual actions/commands, diagnostic UI, existing evidence navigation, fixtures, localization, and tests. No new service module or external tool should be necessary.
