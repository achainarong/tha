# network-troubleshooting Specification

## Purpose
Help users inspect container network relationships and published-port configuration while clearly distinguishing passive metadata from actual connectivity tests.

## Requirements

### Requirement: Passive single-container and peer inspection
The system SHALL provide a discoverable read-only network troubleshooting view for a selected container, with optional comparison to a permitted peer on the same runtime. Opening or refreshing it SHALL NOT execute commands in containers, start helpers, send connectivity probes, or mutate runtime resources.

#### Scenario: Open troubleshooting without a peer
- **WHEN** the user opens network troubleshooting for a container
- **THEN** available network and port evidence is shown without requiring a peer or generating probe traffic

#### Scenario: Choose a peer
- **WHEN** the user selects a visible same-runtime peer
- **THEN** the view compares the two immutable container targets without changing either container

### Requirement: Display reported network endpoint evidence
The system SHALL show available reported network identities, names, modes, endpoint addresses, aliases, and attachment relationships. Missing or unsupported fields SHALL be labeled unknown or unavailable, not inferred from names. Displayed runtime text SHALL be sanitized.

#### Scenario: Address and alias metadata is available
- **WHEN** the runtime reports an endpoint address and aliases
- **THEN** they are displayed as reported configuration with an explicit indication that resolution and reachability have not been tested

#### Scenario: Backend supplies membership only
- **WHEN** the backend reports network membership but no endpoint addresses
- **THEN** membership remains visible and endpoint addresses are marked unavailable

#### Scenario: Special network mode is reported
- **WHEN** the runtime reports host, none, or container-sharing network mode
- **THEN** that mode is identified without fabricating ordinary bridge membership

### Requirement: Relationship explanations respect evidence limits
The system SHALL compare network identities only within the same runtime and label shared membership as configuration evidence, not proof of connectivity. No shared network SHALL NOT be presented as proof of disconnection. Incomplete identities SHALL produce an incomplete comparison rather than a definitive result.

#### Scenario: Two containers share a network identity
- **WHEN** both selected endpoints report the same network ID on the same runtime
- **THEN** the view identifies the shared network and states that connectivity has not been tested

#### Scenario: No shared network is reported
- **WHEN** complete metadata reports no shared network
- **THEN** the view reports that fact without ruling out published-port, host-network, routed, or external paths

#### Scenario: Identical network names have different identities
- **WHEN** two reported networks share a display name but have different IDs
- **THEN** they are not treated as the same network

### Requirement: Explain exposed and published ports accurately
The system SHALL distinguish exposed-only ports from host-published ports and show protocol and reported host bind scope. Possible publication overlaps SHALL be labeled unconfirmed and scoped to the same runtime, accounting for protocol, addresses, and container state. Runtime-host bindings SHALL NOT be represented as verified workstation endpoints.

#### Scenario: Port is exposed but not published
- **WHEN** a container reports an exposed port without a host port
- **THEN** it is not displayed as a published host endpoint

#### Scenario: Loopback and wildcard bindings differ
- **WHEN** bindings use loopback, wildcard, or a specific host address
- **THEN** their scopes are explained separately, including address family where reported

#### Scenario: Potential overlap is visible in metadata
- **WHEN** reported same-runtime publication metadata suggests overlapping active bindings
- **THEN** the UI labels a possible overlap, not a proven port-bind failure or host-process conflict

### Requirement: Preserve workspace visibility boundaries
The system SHALL apply workspace visibility rules to peer choices, attachments, addresses, aliases, relationship explanations, warnings, and navigation. Shared resources SHALL retain hidden-attachment counts without disclosing outside-workspace workload metadata.

#### Scenario: Shared network contains outside-project containers
- **WHEN** troubleshooting runs inside a project workspace on a shared network
- **THEN** permitted project workloads are shown and outside-project attachments are represented only by counts without leaking their names, addresses, or aliases

### Requirement: Bound and identity-scope asynchronous inspection
The system SHALL keep metadata requests asynchronous and bounded, isolate failures, and bind results to runtime, endpoint generation, namespace, immutable target identities, and request generation. Stale or replaced-target results SHALL NOT populate a new selection.

#### Scenario: Peer changes during inspection
- **WHEN** the user selects another peer before the previous request finishes
- **THEN** the late result does not overwrite the new comparison

#### Scenario: One runtime lacks network inspection support
- **WHEN** detailed network inspection is unsupported or fails for a runtime
- **THEN** the view retains available passive evidence with a clear limitation and other runtimes and keyboard navigation continue working
