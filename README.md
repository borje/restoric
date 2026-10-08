# restoric

A terminal UI for browsing and restoring files from a [restic](https://restic.net) repository, in the spirit of Time Machine. It looks like the [yazi](https://github.com/sxyazi/yazi) file manager and is built on [rustic_core](https://crates.io/crates/rustic_core).

You open a folder and scrub through time. restoric only stops at the snapshots where that folder actually changed, and folds the rest away:

![restoric browsing a folder's history, diffing a version and restoring it](docs/demo.gif)

- The row of dots marks the snapshots where the selected file changed (`●`) and where only its folder did (`○`).
- A change means the content, permissions or owner changed. A file that was only touched, or whose access time moved, doesn't count (`--strict` counts everything restic stored).
- Only this machine's snapshots are shown, picked by hostname as `restic snapshots --host` does. `--browse` (or a folder that isn't in your backups) opens a picker of the other hosts and backup paths in the repository, for a renamed machine or a server's backups.
- Restoring works like copy and paste: `y` yanks, `p` puts the old version next to the original, `P` overwrites it after asking. `:undo` puts back what was overwritten.
- restoric never writes to the repository, and needs no FUSE or mount.

## Install

From a checkout (Rust 1.91 or newer):

```sh
cargo install --locked --path .
```

Release binaries for Linux (x86_64, aarch64) and macOS are built for each tag. A [Nerd Font](https://www.nerdfonts.com) gives file-type icons; without one, use `--no-icons` or `icons = false`.

Try it without a repository:

```sh
restoric demo
```

## Use

restoric reads the same environment as restic:

```sh
export RESTIC_REPOSITORY=sftp:nas:/backup/restic
export RESTIC_PASSWORD_COMMAND="pass show restic"
restoric                 # the current folder
restoric ~/dev/project   # or any folder in your backups
```

Every backend rustic supports works: local, sftp, REST, rclone, S3, B2, Azure and the rest.

| | |
|---|---|
| `-r`, `--repo`, `--repository-file` | The repository (or `RESTIC_REPOSITORY`, `RESTIC_REPOSITORY_FILE`) |
| `--password-file`, `--password-command` | The password (or `RESTIC_PASSWORD`, `_FILE`, `_COMMAND`) |
| `--insecure-no-password` | A repository made with `restic init --insecure-no-password` |
| `--host NAME`, `--tag TAG` | Whose snapshots to show (default: this machine's hostname) |
| `--strict` | Count any metadata change |
| `--select NAME`, `--at DATE` | Start with NAME selected, or at a date (`sep 1`, `2026-09-01`, `3d`) |
| `--browse` | Start in the repository picker: other hosts and backup paths (restores from another host ask for a directory) |
| `restoric log PATH [--json]` | Print when PATH changed, with counts |
| `restoric versions FILE [--json]` | Print the distinct versions of FILE |

## Keys

Vim keys and arrows, meaning what they mean in yazi where they can. Press `?` for the full list.

| | |
|---|---|
| `j` `k` `gg` `G` `C-d` `C-u` | Move |
| `h` `l` `⏎` `-` | Parent folder / open (a file opens its versions) |
| `H` `L` | Older / newer change in this folder |
| `[` `]` | Every snapshot, changed or not |
| `{` `}` | Older / newer change of the selected item |
| `⇥` `J` `K` | Preview: content or diff against disk, scroll |
| `d` | Full-screen diff against disk (`p` in it: against the previous version) |
| `Space` `v` | Select / visual select |
| `y` `p` `P` | Yank, restore next to it, overwrite |
| `r` | Restore options: overwrite, next to it, `~/Restored/`, `$PAGER` or tar |
| `cc` `cd` `cf` | Copy `snapshot:path`, the folder, the name |
| `.` | Show items deleted earlier |
| `/` `n` `N` `f` | Search this folder, filter it |
| `s`, `:find NAME` | Search every snapshot |
| `:sep 1` `:yesterday` `:3d` | Jump to a date |
| `zi` `zo` | Zoom the timeline |
| `:undo` | Undo the last overwrite |
| `q` | Back one level: from the folder view, to the picker if restoric started there, else quit (`Ctrl-c` and `:q` always quit) |

The mouse works too: click dots, rows, the breadcrumb and the arrows.

## Config

`~/.config/restoric/config.toml`. Every key is optional:

```toml
host = ["laptop", "old-laptop-name"]   # this machine's hostnames, if it was renamed
tag = ""                               # only snapshots with this tag
icons = true                           # Nerd Font icons
preview_max_kb = 64
diff_max_mb = 2                        # bigger files are diffed when you ask
memory_cache_mb = 256
disk_cache_mb = 2048
restore_dir = "~/Restored"

[keys]                                 # action = "key"
versions = "i"
half_down = "C-f"

[colors]                               # ANSI names, "#rrggbb" or 0–255
accent = "magenta"
```

Colours follow your terminal's 16-colour theme, and `NO_COLOR` turns them off. restoric keeps a cache in `~/.cache/restoric/` and a log in `~/.local/state/restoric/restoric.log` (`RESTORIC_LOG=debug` for more).

## Status

Work in progress; see [PLAN.md](PLAN.md) for the design and what's done. Licensed under the [GNU AGPL v3](LICENSE) or later.
