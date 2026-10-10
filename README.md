# restoric

A terminal UI for browsing and restoring files from a [restic](https://restic.net) repository, in the spirit of Time Machine. It looks like the [yazi](https://github.com/sxyazi/yazi) file manager and is built on [rustic_core](https://crates.io/crates/rustic_core).

You open a folder and scrub through time. restoric only stops at the snapshots where that folder actually changed, and folds the rest away:

![restoric browsing a folder's history, diffing a version and restoring it](docs/demo.gif)

- The row of dots marks the snapshots where the selected file changed (`●`) and where only its folder did (`○`).
- A change means the content, permissions or owner changed. A file that was only touched, or whose access time moved, doesn't count (`--strict` counts everything restic stored; `--bytes-only` only the content, for drives whose permissions or owner change wholesale, like a macOS external drive set to ignore ownership).
- Only this machine's snapshots are shown, picked by hostname as `restic snapshots --host` does. `--browse` (or a folder that isn't in your backups) opens a picker of the other hosts and backup paths in the repository, for a renamed machine or a server's backups.
- Restoring works like copy and paste: `y` yanks, `p` puts the old version next to the original, `P` overwrites it after asking, and says first what differs on disk and how much it writes. `:undo` puts back what was overwritten.
- restoric never writes to the repository, and needs no FUSE or mount.

## Install

With Homebrew (macOS or Linux):

```sh
brew install borje/tap/restoric
```

With Rust 1.91 or newer:

```sh
cargo install --locked restoric
```

Or download a binary for Linux (x86_64, aarch64; `-musl` builds are static and run on any distro, Alpine included) or macOS (Apple silicon, Intel) from [Releases](https://github.com/borje/restoric/releases), unpack it and put `restoric` on your `PATH`. On Debian or Ubuntu, take the `.deb` instead: `sudo apt install ./restoric_*.deb`. On macOS, if a binary downloaded in a browser won't open, run `xattr -d com.apple.quarantine restoric`.

A [Nerd Font](https://www.nerdfonts.com) gives file-type icons; without one, use `--no-icons` or `icons = false`.

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

With no password from any of these, restoric asks for it on the terminal, hidden. To skip the prompt, keep it in the system keychain and fetch it with a password command:

```sh
export RESTIC_PASSWORD_COMMAND="security find-generic-password -s restic -w"   # macOS
export RESTIC_PASSWORD_COMMAND="secret-tool lookup service restic"             # Linux
```

| | |
|---|---|
| `-r`, `--repo`, `--repository-file` | The repository (or `RESTIC_REPOSITORY`, `RESTIC_REPOSITORY_FILE`) |
| `--password-file`, `--password-command` | The password (or `RESTIC_PASSWORD`, `_FILE`, `_COMMAND`); asked for when none is given |
| `--insecure-no-password` | A repository made with `restic init --insecure-no-password` |
| `--host NAME`, `--tag TAG` | Whose snapshots to show (default: this machine's hostname) |
| `--strict` | Count any metadata change |
| `--bytes-only` | Count only content changes, not permissions or owner |
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
| `i` | Snapshot details: label, description, what the backup added and read (`H` `L` `[` `]` move it through time) |
| `S` | Folder sizes, and the bytes this snapshot added to the folder (walks each folder once; cached) |
| `b` | Pin this snapshot as the base: the listing and `d` compare with it instead of the snapshot before (`b` on it again to unpin) |
| `Space` `v` | Select / visual select |
| `y` `p` `P` | Yank, restore next to it, overwrite |
| `r` | Restore options: overwrite, next to it, `~/Restored/`, `$PAGER` or tar |
| `cc` `cd` `cf` | Copy `snapshot:path`, the folder, the name |
| `.` | Show items deleted earlier |
| `zn` | On disk: hide files and folders not in the backup |
| `zc` | Hide what didn't change |
| `/` `n` `N` `f` | Search this folder, filter it |
| `s`, `:find NAME` | Search every snapshot |
| `:sep 1` `:yesterday` `:3d` | Jump to a date |
| `zi` `zo` | Zoom the timeline |
| `:undo` | Undo the last overwrite |
| `:set strict` `:set bytes` | Count every change / only content (`:set nostrict`, `:set nobytes` go back) |
| `q` | Back one level: from the folder view, to the picker if restoric started there, else quit (`Ctrl-c` and `:q` always quit) |

The mouse works too: click dots, rows, the breadcrumb and the arrows.

## Config

`~/.config/restoric/config.toml` (or `$XDG_CONFIG_HOME/restoric/`), on macOS too. Every key is optional:

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

Keys are a character, `C-x`, `Enter`, `Tab`, `Space`, `Backspace`, the arrows, `Home`, `End`, `PageUp`, `PageDown` or `F1`–`F12`, and they win over the built-in ones in every view. The actions are `down` `up` `top` `bottom` `half_down` `half_up` `parent` `open` `versions` `root` `older_change` `newer_change` `older_snapshot` `newer_snapshot` `oldest_change` `newest_change` `older_item_change` `newer_item_change` `preview_mode` `scroll_down` `scroll_up` `deleted` `changes_only` `diff` `select` `visual` `yank` `paste` `overwrite` `restore` `show` `search` `filter` `find` `next_match` `prev_match` `command` `zoom_in` `zoom_out` `help` `info` `sizes` `base` `quit`. The colours are `accent` `added` `changed` `deleted` `live` `dim` `dir` `code` `selected` (`selected` sets the background). A mistake in the file stops restoric with a message naming it.

Colours follow your terminal's 16-colour theme, and `NO_COLOR` turns them off. restoric keeps a cache in `~/.cache/restoric/`, what `:undo` needs in `~/.local/share/restoric/undo/` and a log in `~/.local/state/restoric/restoric.log` (`RESTORIC_LOG=debug` for more). These are the same on Linux and macOS, and follow the `XDG_*_HOME` variables when they're set.

## Status

Early (0.x), expect changes. Report bugs in [issues](https://github.com/borje/restoric/issues). Licensed under the [GNU AGPL v3](LICENSE) or later.
