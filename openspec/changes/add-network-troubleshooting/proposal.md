# Proposal

## Why

Tha shows network membership and published ports, but users still have to piece together whether containers share a network, how they are addressed, and what a published port actually exposes. A read-only troubleshooting workflow can explain configuration evidence without pretending it has tested connectivity.

## What Changes

- Expand existing network details into a keyboard-accessible troubleshooting workflow for a selected container and peer on the same runtime.
- Show runtime-reported network membership, addresses/aliases where available, and exposed versus published ports with bind scope.
- Explain shared-network evidence, missing data, and possible conflicting publication metadata without claiming reachability or DNS success.
- Preserve workspace privacy: outside-project attachments remain counted but unnamed.
- First version is passive/read-only. Exclude DNS lookups, ping, TCP/HTTP probes, container exec, helper containers, firewall inspection, automatic repairs, and cross-runtime connectivity claims.

## Capabilities

### New Capabilities

- `network-troubleshooting`: Identity-scoped passive inspection of container network relationships, addresses, and publication evidence.

### Modified Capabilities

None; there is no existing OpenSpec baseline.

## Impact

Touches typed network metadata, bounded backend inspection, network UI/navigation and peer selection, workspace scoping, localization, fixtures, and tests. No network scanner or external CLI dependency is required. Future active probes require a separate reviewed change and explicit user action.
