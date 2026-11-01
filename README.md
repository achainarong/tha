# Tha · ท่า

A terminal UI for Docker, Podman and containerd. Browse containers, Compose projects,
images and volumes; follow logs, inspect metrics, and manage remote servers over SSH.

## Install

Linux amd64/arm64 or macOS Apple Silicon:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sh
tha
```

Add `~/.local/bin` to your `PATH` if needed. Try `tha --demo` without a runtime.

For background metrics, install in **service** mode:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sh -s -- service
```

For **image vulnerability scanning**, add the optional Grype module. Local scans
require the Docker CLI and a running Docker-compatible engine:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | MODULES=security sh
```

Rerun the installer to update. [Releases](https://github.com/achainarong/tha/releases)
include checksums; configuration and saved data are preserved.

## Basics

| Key | Action |
| --- | --- |
| `j/k`, arrows | Navigate |
| `Enter` / `Backspace` | Open / go back |
| `Tab` | Change focused pane |
| `Ctrl+Left/Right` | Switch Containers, Images, Volumes, Security |
| `/` | Search |
| `:` / `?` | Commands / help |
| `.` / `,` | Servers / settings |
| `q` | Back / quit |

## Remote servers

Create a dedicated SSH key on your computer, then install on the **Linux server**:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --key 'ssh-ed25519 AAAA...'
```

In Tha, open `:servers`, add the host with user `tha` and your key path, then test
the connection. Setup creates a restricted user and a persistent systemd agent.
It opens no additional network port. Deletion is opt-in with `--allow-delete`.

Alternatively, use an existing **root SSH key**:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --user root
```

Set the server user to `root` in Tha. Setup leaves root's keys unchanged;
anyone holding that key retains full root access.

### Remote container and server shells (server opt-in)

Remote exec is **disabled by default**, independently of deletion. Choose either:

- **Disabled:** use the setup command above without `--allow-exec`. Browsing,
  logs and the existing allowed lifecycle actions still work; container and server shells are refused.
- **Enabled:** add `--allow-exec` on the **server**, for example:

  ```sh
  curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --key 'ssh-ed25519 AAAA...' --allow-exec
  ```

  For an existing root login, use `agent --user root --allow-exec` instead.
  Select a running remote container in Tha and press `e` to open its shell in
  the Exec tab. Input, output, terminal resizing and exit status travel through
  the existing SSH agent channel; no Docker CLI or extra network port is needed.
  Update both Tha and tha-agent to a version supporting remote exec.

  To open a **shell on the server itself**, select its remote server row (for
  example, `container-lab (remote)`) and press `e`. The shell runs as the
  configured SSH login user: `tha` for the dedicated-user setup, or `root` for
  a root login. It uses a separate SSH channel with the same restricted key;
  Tha does not relax that key's forced-command or forwarding restrictions.

For an existing installation, root can also edit `/etc/tha/agent.toml`:

```toml
allow_exec = false # Default: refuse container exec and server shells
# allow_exec = true # Opt in: allow both container exec and server shells
```

Keep this setting at the top level, before any TOML table headers, and run
`sudo systemctl restart tha-agent` after changing it. Setup reruns and updates
preserve an existing opt-in; omitting `--allow-exec` does not revoke it. To revoke,
set `allow_exec = false` and restart the service. Active container sessions
disconnect; existing host-shell SSH sessions must be closed separately.
Full shell access can modify account-owned files, including `authorized_keys`.
Disabling the flag does not undo those changes or revoke keys added from a shell;
review the account and its keys when revoking access.

**Security:** enabling exec lets every holder of an authorized agent key run
commands with the container's configured user permissions **and open a host shell
as the SSH login user**. This deliberately grants full shell access as that user;
use a dedicated non-root user unless root access is intended. Privileged containers
or host mounts can turn this into host access. The agent does not enable
privileged exec or unrestricted Docker API access. Kubernetes-managed
containers remain excluded. Closing a session disconnects its streams; it does
not guarantee termination of commands or background processes inside the container.

### Background agent features

Agent setup enables resource-limited, load-aware security scans. The agent verifies
the official Grype image automatically—no manual SHA setup. Only reports are sent
back and saved centrally. Use `--no-security` to opt out. See [remote security](docs/remote-security.md).

The agent also records [performance history](docs/remote-history.md), even while
Tha is closed: 1-second samples, SQLite flushes every 15 seconds, and 24-hour retention.

## Configuration and development

Configuration: `~/.config/tha/config.toml` (or `$XDG_CONFIG_HOME/tha/config.toml`).
See [example configuration](tha.example.toml), [user guide](docs/guide.md), and
[keybindings](docs/keybindings.md).

Build with Rust 1.88+; current stable is recommended:

```sh
make demo
make check
make install
```
