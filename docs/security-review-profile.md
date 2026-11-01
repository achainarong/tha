# Security review profile

The threat model for security reviews of Tha, read by the `rust-security-review` skill. It
overrides the skill's generic guidance. A change that weakens an invariant below is a finding,
and so is removing or loosening a test that pins one.

## What runs where

- **`tha`**: a TUI and optional user service on the operator's computer, as that user. It talks
  to local engines, and to servers through the system `ssh`.
- **`tha-agent`**: on servers, started as a forced command from `authorized_keys`, plus a
  root-owned systemd service. Policy is `/etc/tha/agent.toml`.
- **No listening network port.** Agent HTTP runs over SSH stdio and private Unix sockets. A new
  TCP listener or browser-facing surface is a new trust boundary and needs its own review. The
  planned web UI's access model is admin email invites plus required 2FA.

## Attackers

| Attacker | Controls | Must not gain |
|---|---|---|
| Anyone who can create a container or image on an engine | Names, labels (including Compose labels), env, logs, files and file names, image metadata | Code execution or commands on the operator's computer; control of the operator's terminal |
| Compromised server | Everything the agent returns: listings, tar streams, reports, history, SSH stderr | Local file writes outside the chosen destination, or code execution on the operator's computer. It can lie about its own contents (documented) |
| Holder of a restricted SSH key, **without the `tha` client** | `SSH_ORIGINAL_COMMAND`, every HTTP byte sent to the agent | Anything beyond the allowlist and enabled `allow_*` flags; root; a general shell; engine access through a side path |
| Other local users on either machine | Shared directories, attempts to reach sockets and files | Reading sockets, databases, exports, or edit copies; racing a privileged file operation |
| Network attacker | Traffic | Anything; SSH and HTTPS must be verified |

### Not attackers

The operator's own `tha` config and keys, and the root-owned agent policy. These are still
validated, so that a mistake cannot become an option injection, but bypassing validation
requires the access they already have. Engine data is **not** trusted config: it is attacker-
controlled even when it comes from the operator's own engine.

## Boundaries and invariants

### Agent (`src/agent.rs`, the main boundary)

- Refused unless a route allows it. A forwarded call is re-created, so client headers and bodies
  never reach the engine (`routes`, `check`, `query_allowed`, `screen_headers`,
  `screen_upgrades`, `forward`).
- One spelling per route: `%`, `\`, `#`, empty segments, and non-graphic characters are refused.
- `SSH_ORIGINAL_COMMAND` is only compared or strictly parsed and never reaches a shell
  (`parse_command`, `files::parse_host_command`, `compose_edit::parse_host_command`).
- Every capability is enforced on the server by its `allow_*` flag, and all default off.
  `listing` capabilities are advisory, so a client-side check alone protects nothing.
- Server-file requests run as the SSH login user, not the root service (`files::serve_host`,
  `remote_exec`).
- Agent policy is rejected if others can write it, and unknown fields are refused
  (`load_config_owned_by`, `deny_unknown_fields`). New `AgentConfig` fields must default closed.
- Exec and shell stay opt-in, independently of deletion. Forced delete stays refused.
- Security scanning: clients cannot choose scanner images, commands, mounts, privileges, or
  limits; the scanner receives no Docker socket. See `docs/remote-security.md`.

### SSH and processes (`src/remote.rs`, `src/remote_exec.rs`, `src/agent/setup.rs`)

- `Remote::validate` runs for typed and hand-edited entries; a host or user can never become an
  option, and `with_host` adds `--`.
- `common_args` keeps agent and X11 forwarding, port forwards, local commands, and passwords off.
  Host-key checking stays with the user's OpenSSH settings: never add
  `StrictHostKeyChecking=no` or a throwaway known-hosts file.
- A **root login has no forced command**, so the remote login shell parses the command string.
  Anything variable must be encoded (`files::encode`) or fixed text. Never add raw values to a
  remote command.
- `authorized_line` allows only plain characters in the `authorized_keys` command path, and
  `merge_authorized_keys` refuses other active keys that would bypass the agent.
- Compose paths come from labels, which are attacker-controlled. `compose.rs` refuses relative or
  unsafe paths. Before running, `check_server_files` refuses linked files, and anything Compose
  reads, or a folder above it, that someone without engine access could change. That covers the
  labelled files and the default `.env`, not files a Compose file itself includes.
- The editor is a closed enum (`src/editor.rs`), not a command string.

### Hostile data shown or saved (`src/model.rs`, `src/ui/`, `src/files.rs`, `src/downloads.rs`)

- `model::safe_text` cleans engine and remote text. Ratatui-core also drops control graphemes
  from its buffer, so a missing `safe_text` on a value only rendered through ratatui is not
  escape injection: at most, visible remnants. Report it as a finding when the value reaches
  `println!`, `eprintln!`, an export, a confirmation prompt it could disguise, or a check that
  compares it (as `compose.rs` does).
- Exec sessions render through `vt100` and `tui-term`, not the real terminal. Bytes reaching the
  real terminal directly would be a finding.
- Downloads go only to the destination the operator confirmed, never overwrite, and create files
  privately. A folder is saved as a tar and not extracted, so tar traversal does not apply to
  folder downloads unless that changes. The tar reader in `files.rs` still handles hostile input
  and must stay bounded.
- Reads from a remote are bounded (`MAX_*` limits, `.take`). A new unbounded read from a server
  is a candidate, but rate a client crash from a server the operator chose as Low.

### Local files and sockets (`src/live.rs`, `src/history.rs`, `src/events.rs`, `src/compose_edit.rs`)

- Sockets, databases, preferences, exports, and edit copies are `0600` in private folders, with
  owner, mode, and link checks that refuse foreign or unsafe files.
- Edits are atomic and refuse a file that changed meanwhile. Agent config edits are validated
  before saving.
- Deleting never recurses and never follows a link (`rm --` in a container, `delete_checked` on a
  server), and refuses the agent policy under any path or hard link. Creating on a server never
  replaces anything (`create_new`). Both need `allow_server_edit`, not `allow_compose_edit`.
- Deleting in a container runs the image's `rm`, which an upload can replace, so the `delete` route
  needs `allow_exec` as well as `allow_upload`. It must never be reachable with `allow_upload` alone.
- `src/compose_edit.rs` and `src/files.rs` act on server paths chosen by the key holder. Only the
  SSH login user's rights may apply, so check any path work running as root for symlink races.

### Updates and release (`src/update.rs`, `scripts/install.sh`, `.github/workflows/`)

- The installer verifies a release checksum and fails closed. `update.rs` accepts only a plain
  `vX.Y.Z` tag from the redirect.
- `release.yml` has read-only default permissions, with `contents: write` only on the publishing
  job.

## Accepted risks

Report these only if a change makes them worse.

- A root SSH key is not restricted to the agent (documented in the README and shown in the form).
- `curl | sh` installation and `tha update` running `install.sh` from `main`. The checksum comes
  from the same release as the binary, so this trusts GitHub and HTTPS, and it is not a signature.
- The scanner trusts the official Docker Hub repository and HTTPS (`docs/remote-security.md`).
- An authenticated compromised server can lie about its own contents and scan results.
- `--allow-*` flags grant exactly what they say, including whatever the SSH login user can read
  or write. A broad flag doing what it documents is not a finding.

## Severity examples

- **Critical**: a restricted key gains a shell, root, or engine calls off the allowlist; hostile
  labels or server data execute code on the operator's computer.
- **High**: a server writes outside the chosen download destination; a disabled `allow_*` action
  still works; host-key verification is turned off.
- **Medium**: a hostile name reaches the real terminal or disguises a confirmation; a remote
  client can crash the shared root agent service.
- **Low**: a chosen server crashes one client session.
