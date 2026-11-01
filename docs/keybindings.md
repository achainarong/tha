# Keybindings

Edit `$XDG_CONFIG_HOME/tha/config.toml` (default `~/.config/tha/config.toml`),
or the file selected with `tha --config PATH`. Changes apply within a few seconds.
The checked-in `tha.example.toml` includes a ready-to-edit `[bindings]` section.

```toml
[bindings]
"ctrl-left" = "previous-view"
"ctrl-right" = "next-view"
"x" = "zoom"
"z" = "none"
"alt-j" = "down"
"alt-k" = "up"
"alt-n" = "focus-next"
"alt-p" = "focus-previous"
"alt-s" = "stop"
```

The left side is a key in crokey syntax, such as `x`, `shift-x`, `ctrl-left`,
`alt-j`, `F6`, `space`, or `pagedown`. The right side is an action name below.
Keys are global overrides, but actions still require their normal view/focus.
An action unavailable in the current view does not fall through to the overridden
default action. Security actions require the optional Security module.

New keys add aliases; existing default keys remain active. To fully move an action,
bind the new key and set each unwanted old key to `none`. For example, the config
above moves fullscreen from `z` to `x`. A key can have only one configured action;
duplicate normalized keys and unknown actions are rejected.

**Ctrl+Left / Ctrl+Right** cycle the top-level Containers / Images / Volumes /
Security overviews by default. Shift+arrows and F7/F8 remain compatibility aliases.
To disable those aliases, bind `shift-left`, `shift-right`, `F7`, and `F8` to `none`.

## Keyboard layouts

Press `,` to choose a layout, or set `keyboard = "german"` in `config.toml`. A layout
follows how that keyboard is normally used instead of copying US key positions, and only
adds keys, so every default keeps working. `:`, `/` and `?` stay where German keyboards
have them, and `z` stays Zoom.

| Action | US | German adds | Why |
| --- | --- | --- | --- |
| Metrics range shorter / longer | `[` `]` | `-` `+` | `[` and `]` need AltGr on QWERTZ |
| Leave shell input | `Ctrl-]` | `F12` | `Ctrl-]` needs AltGr and is unreliable; in this layout the shell no longer receives `F12` |

Your own `[bindings]` take priority over a layout's keys.

## Input and safety

Custom bindings apply only in navigation, not while typing, using pickers or
confirmations, or entering commands in an embedded shell. Confirmation keys
(`y`, `n`, Enter, Esc, typed container names) retain their original behavior.
Lifecycle and cleanup shortcuts still open confirmations and revalidate targets;
rebinding them cannot skip those safeguards. Lifecycle key-repeat events are ignored.

Esc, Enter, Tab, Shift-Tab, `/`, `:`, Ctrl-c, Ctrl-], and its legacy Ctrl-5 encoding
cannot be overridden or disabled. They remain reliable input/cancel/safety keys.
You can bind additional keys to their actions (`escape`, `open`, `focus-next`,
`focus-previous`, `search`, `commands`, `quit`) in navigation.

`?` help lists configured overrides, all action names, and built-in defaults.
The overview hints and introductory cheatsheet describe built-in defaults; your
configured keys take precedence.

## Complete action reference

Inside `:file-explorer`, `/` finds in the selected folder (or a selected file's parent), and
`Ctrl-f` finds across the source filesystem. Enter starts the search, `n`/`N` jump between matches,
and `x` cancels a running search. `d` opens a prefilled download dialog for the selected match.
These modal keys do not trigger global navigation or workload actions while typing a search.

### General navigation

| Action names | Behavior |
| --- | --- |
| `none` | Disable this key in navigation |
| `help`, `views`, `commands` | Open help, view picker, or command palette |
| `back`, `escape`, `quit` | Back/quit from home; cancel/restore/back; quit immediately |
| `up`, `down`, `left`, `right` | Move/scroll; left/right adapt to tree, detail tabs, logs, metrics, or runtime selection |
| `previous-tab`, `next-tab` | Previous/next detail tab from any focused detail tab |
| `page-up`, `page-down`, `half-page-up`, `half-page-down` | Page or half-page navigation |
| `first`, `last` | First/last item; last also resumes log following |
| `open`, `search` | Open/inspect the current item; search the current view |
| `focus-next`, `focus-previous` | Cycle panes; also switch images/findings in the Security overview |
| `previous-view`, `next-view` | Cycle top-level overviews |
| `containers-view`, `images-view`, `volumes-view`, `security-view` | Open a top-level overview |
| `overview`, `logs`, `networks`, `exec`, `metrics`, `layers`, `security`, `events` | Select an inner workload tab; `exec` selects navigation, not shell input |
| `settings` | Open the settings pane |
| `servers` | Open the Servers modal to add, edit or remove servers |
| `toggle-connection` | Connect or disconnect the selected server |
| `zoom` | Expand/restore the focused Explorer, detail tab, Performance, or inventory view |
| `toggle-branch` | Toggle an Explorer branch |
| `next-match`, `previous-match` | Navigate tree/log search matches |
| `workspace-back`, `all-runtimes` | Return one workspace or all the way to runtime home |
| `refresh`, `monochrome` | Refresh engines; toggle terminal colors |

### Performance and metrics

| Action names | Behavior |
| --- | --- |
| `performance`, `performance-close`, `performance-scope` | Toggle Performance; close focused Performance; switch runtime/container scope |
| `sort-contexts` | Open runtime sorting picker |
| `file-explorer`, `download` | Open the file explorer, or prompt for a path to download |
| `timeline` | Show incidents for the selected workload |
| `alerts` | Show alerts |
| `fleet` | Show what needs attention across all connections |
| `copy-id`, `copy-name`, `copy-image`, `copy-log-line` | Copy to the clipboard, if the terminal allows it |
| `upload`, `network-test` | Prompt for an upload or a connection test |
| `capabilities` | Show what the selected connection can do, and why not |
| `saved-views`, `save-view`, `open-view`, `delete-view` | Open the saved views picker, or prompt for a view name to save, open or delete |
| `previous-range`, `next-range` | Shorter/longer metrics time range |
| `metrics-left`, `metrics-right` | Pan/inspect earlier or later metrics |
| `metrics-cursor`, `metrics-live` | Toggle metrics cursor; return to live metrics |

### Logs and shells

| Action names | Behavior |
| --- | --- |
| `next-issue`, `previous-issue` | Move between possible issues in the logs |
| `wrap`, `follow`, `json` | Toggle log wrapping, following, or pretty-printed JSON |
| `diagnose`, `network-diagnose` | Open read-only health evidence or network troubleshooting for the selected container |
| `filter-logs`, `filter-logs-advanced`, `clear-log-filter` | Edit the text filter, edit the advanced JSON filter, or clear both |
| `export-logs` | Open the reviewed export dialog; never exports immediately |
| `logs-since` | Prompt for a log start time (`15m`, `2h`, `1d`, RFC 3339, or `off`) |
| `reconnect-logs` | Reload the current logs now; streams also reconnect automatically |
| `open-exec`, `exec-sessions`, `close-exec` | Open container or remote-server shell (`e`; remote requires server `allow_exec`), choose retained session, close active session |
| `reload-layers` | Retry the read-only image layer lookup |

### Lifecycle and inventory

| Action names | Behavior |
| --- | --- |
| `actions`, `action-results` | Open contextual actions or background lifecycle results |
| `start`, `stop`, `restart`, `delete-container`, `pause`, `unpause`, `kill` | Request a confirmed workload operation (Explorer/Performance only) |
| `compose-update`, `compose-pull`, `compose-up`, `compose-recreate`, `compose-down` | Request a confirmed Compose operation on the selected project or service |
| `compose-edit` | Open the selected project's Compose file in your editor |
| `compose-output` | Show the last Compose operation's output |
| `previous-runtime`, `next-runtime` | Select an inventory runtime |
| `mark`, `mark-unused`, `clear-marks` | Manage inventory cleanup marks |
| `remove-resources` | Open unused-resource removal confirmation |
| `prune-resources` | Confirm pruning all unreferenced images or volumes in the selected runtime (`p` / `:prune`; ignores search/filter) |
| `inventory-filter`, `inventory-sort`, `refresh-inventory` | Filter, sort, or refresh inventory |
| `removal-results` | Review the last cleanup results |
| `check-updates`, `pull-image` | Compare visible images with their registries; pull the selected image's tags |

### Security

| Action names | Behavior |
| --- | --- |
| `security-focus-next` | Switch images/findings panes in the Security overview |
| `security-previous-runtime`, `security-next-runtime` | Select the Security image-list runtime |
| `check-image`, `check-images` | Check one/all images for scanning |
| `check-security-image`, `clear-scan-checks` | Toggle a Security image-list check or clear scan checks |
| `scan-image`, `scan-images`, `cancel-scan` | Scan selected/checked images or cancel the scan queue |
| `update-security-db` | Update scanner/database and rescan |
