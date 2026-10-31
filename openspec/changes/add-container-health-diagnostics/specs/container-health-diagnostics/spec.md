# Container Health Diagnostics

## Purpose

Help users investigate unhealthy and failing containers through a bounded read-only summary of runtime observations and related available evidence.

## ADDED Requirements

### Requirement: Single-container diagnostic entry point
The system SHALL provide a discoverable read-only diagnostic view for a selected container, including healthy, unhealthy, restarting, and stopped containers. Loading diagnostics SHALL NOT run health checks, start workloads, or alter container configuration.

#### Scenario: Investigate an unhealthy container
- **WHEN** the user opens diagnostics for an unhealthy container
- **THEN** the view identifies the runtime and immutable container target and begins loading available evidence without changing the workload

#### Scenario: Investigate a healthy container
- **WHEN** diagnostics find no current failure evidence for a healthy container
- **THEN** the view reports that observation rather than inventing a failure

### Requirement: Report health-check evidence and state facts
The system SHALL show available health status, recent reported health-check attempts with timestamps, exit results and output, restart count, exit/OOM facts, and runtime errors. Output and history SHALL be bounded and truncation SHALL be visible. Missing fields SHALL remain unknown rather than being replaced by healthy values or zero counts.

#### Scenario: Health check fails with output
- **WHEN** the runtime reports a failing check with a timestamp, exit code, and output
- **THEN** the view shows that evidence and visibly labels any truncated output

#### Scenario: No check is configured
- **WHEN** successful inspection confirms that no health check is configured
- **THEN** the view says no health check is configured rather than claiming checks passed

#### Scenario: Backend omits restart count
- **WHEN** the backend does not supply a restart count
- **THEN** the view labels it unavailable rather than showing zero

### Requirement: Separate observations from possible explanations
The system SHALL distinguish current state, timestamped historical facts, and possible explanations. A reported restart total alone SHALL NOT prove a restart loop, and historical OOM or exit evidence SHALL NOT be asserted as the cause of a current health failure.

#### Scenario: Historical OOM precedes current health failure
- **WHEN** an earlier OOM event and a later unhealthy check are both available
- **THEN** each is labeled with its time and the OOM is not presented as a proven cause of the check failure

#### Scenario: Only cumulative restart count is available
- **WHEN** the runtime reports restarts but no timed sequence of repeated failures is available
- **THEN** the view reports the count without asserting a restart loop

### Requirement: Link available related evidence
The system SHALL offer bounded scoped event/log evidence and navigation to existing Logs, Events, and Metrics views. It SHALL explain coverage gaps, preserve existing user log filters, and label evidence as time-correlated only when timestamps support that association.

#### Scenario: Failure time is unknown
- **WHEN** recent retained logs are available but a failure timestamp is unknown
- **THEN** the view labels them recent available logs rather than logs at the failure

#### Scenario: Follow an evidence link with filters active
- **WHEN** the user opens Logs from diagnostics and the existing filter hides related entries
- **THEN** the same target and filter are retained and the UI explains the filtering constraint

### Requirement: Keep asynchronous evidence identity-safe
The system SHALL load diagnostics asynchronously with bounded requests and isolate errors. Evidence SHALL remain tied to its runtime, endpoint generation, namespace, immutable container ID, and request. Replaced-target results SHALL be discarded, and stale evidence SHALL be labeled.

#### Scenario: A container is recreated with the same name
- **WHEN** a diagnostic response arrives for the removed container after a new container with its name appears
- **THEN** the old response is not attached to the replacement container

#### Scenario: One endpoint times out
- **WHEN** a diagnostic request times out
- **THEN** the view reports unavailable or partial evidence while navigation and other runtime refreshes remain usable

### Requirement: Unsupported and unsafe output is handled explicitly
The system SHALL label unsupported diagnostic fields or backends, sanitize displayed runtime text, and keep diagnostic evidence local without automatic uploads or new diagnostic persistence.

#### Scenario: Backend lacks detailed health-check support
- **WHEN** diagnostics are opened on a backend without detailed health-check evidence
- **THEN** available state facts remain visible with an explicit unsupported notice for missing capabilities

#### Scenario: Health output includes terminal controls
- **WHEN** a reported health-check output contains terminal escape sequences
- **THEN** the view renders sanitized text without executing terminal controls
