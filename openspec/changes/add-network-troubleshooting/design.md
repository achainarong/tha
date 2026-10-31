# Design

## Context

See `proposal.md`. `Container` exposes network names and typed port bindings; `Resource` carries generic network details and attachment IDs. `PortBinding::overview` already distinguishes exposed-only ports, loopback, wildcard, specific addresses, and unknown host addresses. The current workspace network view deliberately hides outside-project workload names. Address/alias details need richer evidence, not string parsing from display text.

## Goals / Non-Goals

**Goals:** Reuse the existing Networks tab, clarify passive evidence, compare a selected same-runtime peer, and keep runtime/workspace boundaries intact.

**Non-Goals:** Active probes, testing actual DNS or reachability, inspecting host processes/firewalls/routes, modifying networks, cross-runtime inference, topology graph rendering, full network management.

## Decisions

1. **Deepen the existing Networks workflow.** Add a contextual “Troubleshoot network” route from a selected container into a focused Networks detail/overlay, with a proposed `:network-diagnose` command for discoverability. Offer a same-runtime peer picker within permitted workspace scope, rather than creating a top-level view or conflicting numbered tab. Show an accessible single-container summary even without a peer.
2. **Use typed optional endpoint evidence.** Model stable network ID, reported name, driver/scope/mode, endpoint addresses and aliases where available. Extend a focused network diagnostic model rather than parsing `Resource.details`. Unknown fields stay optional; host/none/container-sharing modes need explicit descriptions and may not fit ordinary bridge membership. Backends lacking detailed metadata show partial results.
3. **Bound on-demand metadata loading.** Reuse inventory membership/ports first; load additional selected-target inspect evidence asynchronously. Proposed limits: five-second deadline, at most four concurrent inspect requests, and a capped visible peer/attachment preview with a truncation count. Avoid inspecting every container on every refresh. Apply response limits, sanitization, and generation-based stale-result rejection.
4. **Distinguish facts from inferences.** Shared exact network identity is a configuration fact, not proof that packets flow or aliases resolve. No shared bridge is not proof of disconnection: published ports, host networking, routing, or external networks may provide paths. Label DNS aliases “reported aliases; resolution not tested.” Do not call the workstation's localhost a remote runtime's host.
5. **Explain existing port evidence consistently.** Reuse or extend `PortBinding::overview`; retain protocol, IPv4/IPv6 address family, bind scope, and unknown host address semantics. Exposed-only ports are not published host endpoints. Potential overlaps require the same runtime/host scope and protocol, with stopped-container configurations distinguished from active bindings. Wildcard/specific overlap and dual-stack uncertainty are candidates for investigation, not confirmed bind failures. No host-process conflict detection is promised.
6. **Enforce workspace visibility everywhere.** Peer pickers, summaries, warnings, addresses, aliases, and details must not leak outside-project names or metadata. Shared resources can show visible members and the count of hidden attachments, reusing existing workspace filtering. A user can intentionally leave the workspace to inspect broader runtime data.
7. **No active test behind a passive label.** Opening or refreshing this view performs read-only runtime API requests only. Any future DNS/TCP/HTTP test requires a separate spec defining execution location, consent, endpoint identity, deadlines, cancellation, dependencies, and traffic/security implications. It is not a hidden task in this change.

## Risks / Trade-offs

- [Users mistake configuration for connectivity] → Keep “not tested” labels beside shared network and alias evidence; never show a green reachability result without a test.
- [Host, none, overlay, rootless, or Podman behavior differs] → Present reported modes and explicit limits; fixture-test partial metadata rather than infer universal networking rules.
- [Names match across runtimes or networks] → Compare runtime identity plus network ID, not display names; unknown IDs mean incomplete comparison.
- [Publication warnings are false positives] → Label possible overlaps, account for protocol/bind scope/state, and avoid host-wide claims.
- [Shared-resource details leak workspace data] → Filter at the data projection boundary and test names, addresses, and aliases, not only row labels.

## Migration Plan

No database or config migration. Add typed optional data and a read-only UI incrementally, retaining existing network selection links. Rollback removes diagnostic presentation without changing runtime resources.
