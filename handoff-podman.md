# Handoff: Native Podman engine discovery

## Goal

Add native Podman discovery to `tha` so running Podman machines and their containers, images, networks, and volumes appear alongside existing Docker engines on macOS.

Repository: `/Users/a.chainarong/Development/tinkering/tha`

Discovery must not require a manually created Docker context, Podman Desktop, or the privileged `podman-mac-helper`.

## Workflow

Read applicable repository instructions first. Inspect existing engine discovery, Docker client setup, engine selection, refresh behavior, and tests. Confirm scope and propose concrete file changes before implementing. Do not commit changes.

The implementation details below are an approach, not verified repository-specific code paths.

## Implementation approach

### 1. Discover machines

Use structured command output:

```sh
podman machine list --format json
```

- Treat a missing Podman executable as an optional provider being unavailable, not an application failure.
- Handle no machines and stopped machines without attempting a connection.
- Support multiple machines rather than assuming the default machine name.
- Use bounded subprocess execution and avoid blocking the UI.

### 2. Resolve running-machine sockets

Inspect each running machine:

```sh
podman machine inspect <machine-name>
```

Discover the Unix socket from `ConnectionInfo.PodmanSocket.Path`. Verify the actual JSON structure against supported Podman versions, including whether inspect returns an array.

Do not hardcode socket paths. Paths can vary by machine and environment and may change across restarts. Handle missing fields, malformed JSON, failed commands, and unavailable sockets gracefully.

For context only, this installation previously reported:

```text
unix:///var/folders/5_/9rz0zpy53w10nwn2zyg55_5m0000gp/T/podman/podman-machine-default-api.sock
```

That is an example, not a configuration value to embed in code.

### 3. Connect through the compatible API

Podman exposes a Docker-compatible HTTP API over its forwarded Unix socket.

- Reuse the existing Docker client where compatible.
- Verify API version negotiation and the specific endpoints the app uses.
- Test listing containers, images, networks, and volumes.
- Bound connection attempts with timeouts.
- Report inaccessible sockets and unsupported operations clearly without breaking other engines.
- Do not assume full Docker API compatibility merely because connecting succeeds.

### 4. Represent Podman as an engine

Example display name:

```text
Podman — podman-machine-default
```

Use a stable identity based on provider and machine name, not a temporary socket path. Integrate with existing engine selection and refresh behavior.

- Refresh socket information after machine restarts.
- Handle stopped machines and stale connections.
- Deduplicate when the same Podman endpoint is also exposed through a Docker context, following the repository's existing identity conventions where possible.
- Preserve existing OrbStack, Docker Desktop, and Colima behavior.

### 5. Verify Compose grouping

Inspect labels on the existing Podman Compose fixture. Verify whether the app's current Compose grouping recognizes the labels emitted by `podman-compose`; adapt only if needed. Do not assume they exactly match Docker Compose labels.

## Non-goals and safeguards

- Do not create or start VMs automatically.
- Do not change the default Docker context.
- Do not change `DOCKER_HOST` globally.
- Do not install privileged helpers or require Podman Desktop.
- Do not change rootless/rootful machine configuration.
- Do not modify or delete existing containers, networks, or volumes.
- No unrelated refactors or commits.

## Acceptance criteria

- A running Podman machine appears without a Docker context.
- Its containers, images, networks, and volumes are visible.
- Compose-created containers are grouped correctly using verified labels.
- Missing Podman, no machines, and stopped machines are handled gracefully.
- Invalid JSON, command failures, timeouts, missing socket fields, and inaccessible sockets do not crash the app or break discovery of other providers.
- Multiple machines are supported.
- Refresh handles machine stops and restarts without stale connections.
- Duplicate discovery through a Docker context is handled.
- Existing Docker engine discovery continues working.
- Automated tests cover discovery parsing, failure cases, and relevant integration behavior using mocked commands/API responses rather than requiring a local VM in CI.

## Existing manual-test fixtures

Podman was installed with:

```sh
brew install podman podman-compose
```

The machine and resources below were created and verified running during this session. Recheck their current state rather than assuming they remain running.

Machine:

```text
podman-machine-default
```

| Type | Name | URL |
| --- | --- | --- |
| Compose container | `hello-podman_hello_1` | http://localhost:8080 |
| Standalone container | `hello-podman-standalone` | http://localhost:8081 |
| Compose network | `hello-podman_hello` | |
| Standalone network | `hello-podman-standalone-net` | |
| Compose volume | `hello-podman_hello-data` | |
| Standalone volume | `hello-podman-standalone-data` | |

Both containers use `docker.io/library/nginx:alpine` and previously returned HTTP 200 with the nginx welcome page. Each uses its own named network and volume. Ports bind to `127.0.0.1`.

Read-only checks:

```sh
podman machine list
podman machine inspect podman-machine-default
podman ps
podman network ls
podman volume ls
podman inspect hello-podman_hello_1
curl http://localhost:8080
curl http://localhost:8081
```

The shared Compose fixture is outside the repository:

```text
/private/var/folders/5_/9rz0zpy53w10nwn2zyg55_5m0000gp/T/opencode/container-visibility.q7SYFg/compose.yaml
```

It may not persist indefinitely. No need to recreate or modify it for discovery work.

Other engines also have fixtures for regression checks:

| Engine / Docker context | Compose container | Standalone container | HTTP ports |
| --- | --- | --- | --- |
| OrbStack / `orbstack` | `hello-orbstack-hello-1` | `hello-orbstack-standalone` | 8082, 8083 |
| Docker Desktop / `desktop-linux` | `hello-desktop-hello-1` | `hello-desktop-standalone` | 8084, 8085 |

Each of these containers also has a dedicated named network and volume. The default Docker context was restored to `colima` after fixture setup. Leave existing runtime configuration and fixtures intact.

## Validation and delivery

Run the repository's relevant lint, tests, and build checks. Manually verify Podman discovery against the fixtures and check existing engine discovery for regressions. Report pass/fail per acceptance criterion, maintainability and edge-case findings, rollback considerations, remaining compatibility limitations, and a PR-readiness verdict. Do not commit.
