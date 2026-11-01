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

Agent setup enables resource-limited, load-aware security scans. The agent verifies
the official Grype image automatically—no manual SHA setup. Only reports are sent
back and saved centrally. Use `--no-security` to opt out. See [remote security](docs/remote-security.md).

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
