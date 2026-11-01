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

### Server permissions

A new server allows browsing, logs, metrics, events and start, stop, restart, pause, unpause and kill.
Everything else is off until you turn it on in the setup command on the **server**:

| Option | Allows | Holder of the key can also |
| --- | --- | --- |
| `--allow-delete` | Deleting containers, images and volumes, and pruning | Remove data that cannot be recovered |
| `--allow-exec` | Container and server shells, browsing container files, network tests | Run commands, including a shell as the SSH login user |
| `--allow-images` | Pulling public images, checking for updates | Make the server contact registries and use disk |
| `--allow-download` | Downloading files and folders from containers | Read anything inside any container |
| `--allow-server-download` | Browsing and downloading the server's own files | Read whatever the SSH login user can read |
| `--allow-upload` | Copying files into containers | Change files inside running workloads |
| `--allow-compose` | Compose actions on projects whose files are on the server | Recreate projects from those files |
| `--allow-compose-edit` | Editing Compose and `.env` files | Change them, which together with `--allow-compose` amounts to root |
| `--allow-all` | All of the above | All of the above |

Options add up, and rerunning setup keeps what is already on. To allow everything:

```sh
curl -fsSL https://raw.githubusercontent.com/achainarong/tha/main/scripts/install.sh | sudo sh -s -- agent --key 'ssh-ed25519 AAAA...' --allow-all
```

**`--allow-all` gives every holder of that key full control of the server's containers and, in practice,
root on the server.** With a root login, it also exposes every file on it. Use it only on machines where
that is what you want, and prefer the dedicated `tha` user. `--no-security` and `--alert-webhook URL` are
separate. `:capabilities` in Tha shows what each server allows. To turn one off, set it to `false` in
`/etc/tha/agent.toml` and run `sudo systemctl restart tha-agent`, for example:

```toml
allow_upload = false
```

Keep these at the top of the file, before any table headers. See the [user guide](docs/guide.md) for details on each.

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
