# restoric: plan

restoric is a terminal UI for browsing and restoring files from a [restic](https://restic.net) repository, in the spirit of macOS Time Machine. It's written in Rust on top of [rustic_core](https://crates.io/crates/rustic_core).

The one feature no existing tool has:

> **A view anchored on a folder, where you scrub through time and only see the snapshots in which that folder actually changed.**

This document holds everything decided so far: the research, the UX (with screenshots from the mockup), the change-detection design, the architecture, the milestones and the open questions. The clickable mockup is in [`docs/mockup.html`](docs/mockup.html); open it in a browser. The published copy is at https://claude.ai/artifact/UsxXeCem2TFQGTsNzuowSg.

---

## 1. Why this exists

### What's out there (researched 2026-10-05)

| Tool | What it does | Missing |
|---|---|---|
| [httm](https://github.com/kimono-koans/httm) | "Time Machine-like" file history for ZFS/btrfs/restic. Shows unique versions of a path, deduplicated by modification time and size. Has an interactive restore mode. | Needs `restic mount` (FUSE). Terminal fuzzy-finder UI, no timeline. Folder versions follow the folder's own modification time, so edits deeper down are missed. |
| [Backrest](https://github.com/garethgeorge/backrest), Zerobyte, Pluton | Web UIs: scheduling, browsing one snapshot, restore | Show one snapshot at a time. No per-path history. |
| [restic-browser](https://github.com/emuell/restic-browser) (emuell) | Desktop GUI: browse one snapshot, restore | Same |
| [Resty Desktop](https://github.com/nraboy/resty-desktop) | Desktop GUI with snapshot diff and search across snapshots | No per-path timeline |
| [omarchy-time-machine](https://github.com/JaxonWright/omarchy-time-machine) | Panel plugin: pick a day, stay in the same folder | Steps through every day, not only the days with changes. Omarchy only. |
| [noctalia-restic-snapshots](https://github.com/CarloCamacho/noctalia-restic-snapshots) | History strip, "diff vs previous" per snapshot | History covers the whole repo, not one path |
| [jrudolph/restic-browser](https://github.com/jrudolph/restic-browser) | One tree merged across snapshots, shows deleted files | No timeline. Looks unmaintained. |
| [Packrat Backup #30](https://github.com/setzor/Packrat-Backup/issues/30) | Planned "Previous versions" in Dolphin | Not built, low priority |
| restic itself | [restic#3073 "Show history of file"](https://github.com/restic/restic/issues/3073) | Still open, no PR |
| [dolphin-btrfs-snapshots](https://github.com/tomhense/dolphin-btrfs-snapshots), [KIO Snapshot](https://blog.guilgo.es/en/post/2026/08/kio-snapshot-btrfs-dolphin-plasma/) | Exactly this UX, for btrfs | Not restic |

### Design principles
1. **Anchored on a folder.** You stay in one place and time moves around you. Opening a subfolder keeps you at the same point in time.
2. **Only stop where something changed.** Snapshots where nothing changed are folded away and counted.
3. **Never destroy anything by default.** Restore puts the file next to the original unless you choose to overwrite, and overwriting asks for confirmation and can be undone.
4. **The UI never waits on the network.** All repo access runs in the background, and the screen draws from caches.
5. **Works with your existing restic setup.** It uses `RESTIC_REPOSITORY`, `RESTIC_PASSWORD_COMMAND` and so on, with no new config needed.
6. **Vim first, arrows always.** Every action has a vim key and a plain key. The bottom row always shows the keys you can use.
7. **No FUSE, no mount.** It reads the repository directly through rustic_core.

---

## 2. Core concept: detecting change

In a restic repository, each snapshot points to a root **tree**. A tree lists **nodes**, which are files, folders and links. A folder node has a `subtree` id, and a file node has a `content` list of blob ids. Trees are content-addressed and never change.

### 2.1 Fast path: subtree id
If folder `P` has the same subtree id in snapshot *n* and snapshot *n−1*, nothing under `P` changed. This needs a walk of only `depth(P)` trees per snapshot, and unchanged subtrees share ids, so their trees are loaded once.

### 2.2 Metadata churn and the fingerprint
Restic stores metadata such as **atime, ctime, inode and device id** in each node. These can change without any content changing, for example when a file is read (atime) or its permissions change. When they do, the subtree id changes too. If change points relied on tree ids alone, the timeline would show too many `●`.

**Decision:** "changed" means the **content fingerprint** changed:

```
fingerprint(file)    = H(type, size, content blob ids)          # mode optional, see open questions
fingerprint(symlink) = H(type, link target)
fingerprint(folder)  = H(sorted [(name, fingerprint(child))])
```

- Fingerprints are memoised **by tree id** in the on-disk cache. Tree ids never change, so the cache never needs invalidating.
- Fast path first: same subtree id means same fingerprint, with no recursion.
- `--strict` (or `:set strict`) switches to raw tree ids, for people who care about metadata changes.
- modification time is **excluded** by default: touching a file without changing it is not a change. This is open for discussion.

> **Check in M0:** how often do tree ids differ when fingerprints match, on a real repo? If almost never, fingerprints can be computed lazily.

### 2.3 What gets computed
| Name | Definition | Used for |
|---|---|---|
| Timeline set | Snapshots of the selected host (default: this machine) whose backup paths include the browsed folder, sorted by time | Every view |
| Change points of folder P | Snapshots where `fingerprint(P)` differs from the previous snapshot in the timeline set (or P first appears or disappears) | Row 1 of the timeline, the Versions pane, `H`/`L` |
| Item track | The same as change points, for the selected entry | Row 2 of the timeline, `{`/`}` |
| Entry change marker | The entry's node at snapshot n compared with n−1: `+` added, `~` changed, `−` deleted | Δ column |
| Folder counts | Number of files added, changed and deleted under P between n−1 and n | `+1 ~3 −1` labels |
| Versions | Consecutive runs of snapshots with an identical file fingerprint | Versions view |
| Deleted items | Names seen in P's trees in earlier snapshots that are missing now | `zh` |

Folder counts need a tree diff. Only walk subtrees whose fingerprints differ.

---

## 3. UX specification

Captures are 100×34 unless noted. The selected row is highlighted in the real UI; in these captures it's marked with `◂` at the end of the row. The sample data is invented.

### 3.1 Folder view (main screen)
Starting `restoric` with no arguments opens the **current folder**, at the newest snapshot that changed it (the mockup starts mid-history to show more). This is the Time Machine move of "enter from this Finder window".

<sub>`docs/screens/01-folder-view.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 ~/dev/project/src                                                            ◀ version 12 of 16 ▶
────────────────────────────────────────────────────────────────────────────────────────────────────
  Jul 2026           Aug                               Sep                                   now
  ● · ·   ·●·● ●  ●  ·· ● · ● · ●·  · ·●  · ● · ·  · · · ●·  ●· ● ·  ● · ·  ● · ·  ●   · ·  ┊ ●
  • · ·   ···· ·  •  ·· • · · · ··  · ··  · • · ·  · · · ··  •· • ·  · · ·  · · ·  ·   · ·  ┊ •
                                                             ▲                             − 1× +
  Sep 06 18:03   snapshot 9c3a7b4f   src/  +1 ~3 −1                              ● src/  • main.go
─ Versions ───────────────────────┬ src/ @ Sep 06 18:03 ────────────────────────────────────────────
  now         ~1  not backed up   │ NAME                             SIZE  MODIFIED        Δ
    ┄ 2 unchanged ┄               │ ..
   Sep 27 08:52  ~1               │ api/                                –  Sep 06 17:37    ~1
    ┄ 2 unchanged ┄               │ models/                             –  Aug 17 14:53
   Sep 20 20:35  ~1               │ config.go                       737 B  Sep 06 17:04    ~
    ┄ 2 unchanged ┄               │ legacy.go                       673 B  (deleted)       −
   Sep 13 20:51  ~2               │ main.go                          1.3K  Sep 06 16:34    ~          ◂
    ┄ 1 unchanged ┄               │ server.go                       845 B  Sep 06 15:17    +
   Sep 09 19:23  ~1               │ util.go                         823 B  Jul 14 06:49
    ┄ 1 unchanged ┄               │
 ▶ Sep 06 18:03  +1 ~3 −1         │                                                                   ◂
    ┄ 2 unchanged ┄               │
   Sep 02 20:16  ~1               │
    ┄ 5 unchanged ┄               │
   Aug 22 11:59  ~2               │
    ┄ 1 unchanged ┄               │
   Aug 17 16:04  +1 ~1            │
    ┄ 3 unchanged ┄               │
   Aug 10 15:44  ~1               │
    ┄ 1 unchanged ┄               │
   Aug 07 08:15  +1               │
    ┄ 1 unchanged ┄               │
   Aug 03 16:19  +1 ~1            │
──────────────────────────────────┴─────────────────────────────────────────────────────────────────
 H L change  [ ] every snapshot  { } item change  j k move  h l up/open  v versions  d diff
```

Layout, row by row:

| Rows | Content |
|---|---|
| 0 | Title bar: name, repo, host, snapshot count, cache status (`cache warm` / `indexing 120/430`) |
| 1 | Breadcrumb (each part clickable) · `◀ version 12 of 16 ▶`, or `between versions (16 total)` when viewing an unchanged snapshot |
| 3 | Timeline labels: months, or dates and day numbers when zoomed, plus `now` |
| 4 | **Row 1, `●`: changes in the current folder.** `·` = snapshot without changes. Highlighted cell = the snapshot being viewed. |
| 5 | **Row 2, `•`: changes in the selected entry.** `·` = exists but unchanged, blank = didn't exist. |
| 6 | `▲` under the viewed snapshot · zoom control `− 1× +` |
| 7 | Info: date, snapshot id, folder counts (or "no changes in src/ since …"), "N snapshots in this column", legend `● src/  • main.go` |
| 8 | Pane headers |
| 9–31 | Left: **Versions pane** (34 columns). Right: **listing**. |
| 33 | Key hints, messages, or the `:`/`/` input line. Pending count or prefix at the far right. |

**Versions pane:** the top row is `now`, showing what changed on disk since the last backup ("~1 not backed up", in purple). After that come the change points, newest first, each with its counts. Unchanged runs fold into `┄ 5 unchanged ┄`. `▶` marks the current version, or the folded run when you're viewing an unchanged snapshot.

**Listing columns:** NAME (folders end in `/`, deleted names are crossed out), SIZE, MODIFIED (the file's modification time inside that snapshot), Δ. A deleted entry shows `(deleted)`. Items deleted earlier (shown with `zh`) are in italics with `gone Sep 06`.

**"now" column:** after `┊`, a dot in purple means there are changes on disk that haven't been backed up.

> **Proposed (from review):** the legend on row 7 was too subtle. Put short labels at the left edge of rows 4 and 5 instead, e.g. `src/` and `main.go`.

### 3.2 The second timeline row
After `}`, the view jumps to the next change of `main.go`:

<sub>`docs/screens/02-file-track.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 ~/dev/project/src                                                            ◀ version 13 of 16 ▶
────────────────────────────────────────────────────────────────────────────────────────────────────
  Jul 2026           Aug                               Sep                                   now
  ● · ·   ·●·● ●  ●  ·· ● · ● · ●·  · ·●  · ● · ·  · · · ●·  ●· ● ·  ● · ·  ● · ·  ●   · ·  ┊ ●
  • · ·   ···· ·  •  ·· • · · · ··  · ··  · • · ·  · · · ··  •· • ·  · · ·  · · ·  ·   · ·  ┊ •
                                                                ▲                          − 1× +
  Sep 09 19:23   snapshot 62c0e9c7   src/  ~1                                    ● src/  • main.go
─ Versions ───────────────────────┬ src/ @ Sep 09 19:23 ────────────────────────────────────────────
  now         ~1  not backed up   │ NAME                             SIZE  MODIFIED        Δ
    ┄ 2 unchanged ┄               │ ..
   Sep 27 08:52  ~1               │ api/                                –  Sep 06 17:37
    ┄ 2 unchanged ┄               │ models/                             –  Aug 17 14:53
   Sep 20 20:35  ~1               │ config.go                       737 B  Sep 06 17:04
    ┄ 2 unchanged ┄               │ main.go                          1.3K  Sep 09 18:25    ~          ◂
   Sep 13 20:51  ~2               │ server.go                       845 B  Sep 06 15:17
    ┄ 1 unchanged ┄               │ util.go                         823 B  Jul 14 06:49
 ▶ Sep 09 19:23  ~1               │                                                                   ◂
    ┄ 1 unchanged ┄               │
   Sep 06 18:03  +1 ~3 −1         │
    ┄ 2 unchanged ┄               │
   Sep 02 20:16  ~1               │
    ┄ 5 unchanged ┄               │
   Aug 22 11:59  ~2               │
    ┄ 1 unchanged ┄               │
   Aug 17 16:04  +1 ~1            │
    ┄ 3 unchanged ┄               │
   Aug 10 15:44  ~1               │
    ┄ 1 unchanged ┄               │
   Aug 07 08:15  +1               │
    ┄ 1 unchanged ┄               │
   Aug 03 16:19  +1 ~1            │ 1 deleted earlier · zh to show
──────────────────────────────────┴─────────────────────────────────────────────────────────────────
 H L change  [ ] every snapshot  { } item change  j k move  h l up/open  v versions  d diff
```

Row 2 can only have a dot where row 1 has one, because a file changing means its folder changed too.

### 3.3 Zoom
`zi` / `zo` (or click `−`/`+`) zoom between 1×, 2×, 4× and 8×. The view centres on the selected snapshot, and `‹` `›` show there's more history to either side. When zoomed in, day labels (Mondays) appear.

<sub>`docs/screens/03-zoomed.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 ~/dev/project/src                                                            ◀ version 13 of 16 ▶
────────────────────────────────────────────────────────────────────────────────────────────────────
  Aug 30                         7                              14                           now
‹         ·    ●  ·   ·         ●  ·          ●      ·         ●        ·       ·         › ┊ ●
‹         ·    ·  ·   ·         •  ·          •      ·         ·        ·       ·         › ┊ •
                                              ▲                                            − 4× +
  Sep 09 19:23   snapshot 62c0e9c7   src/  ~1                                    ● src/  • main.go
─ Versions ───────────────────────┬ src/ @ Sep 09 19:23 ────────────────────────────────────────────
  now         ~1  not backed up   │ NAME                             SIZE  MODIFIED        Δ
    ┄ 2 unchanged ┄               │ ..
   Sep 27 08:52  ~1               │ api/                                –  Sep 06 17:37
    ┄ 2 unchanged ┄               │ models/                             –  Aug 17 14:53
   Sep 20 20:35  ~1               │ config.go                       737 B  Sep 06 17:04
    ┄ 2 unchanged ┄               │ main.go                          1.3K  Sep 09 18:25    ~          ◂
   Sep 13 20:51  ~2               │ server.go                       845 B  Sep 06 15:17
    ┄ 1 unchanged ┄               │ util.go                         823 B  Jul 14 06:49
 ▶ Sep 09 19:23  ~1               │                                                                   ◂
    ┄ 1 unchanged ┄               │
   Sep 06 18:03  +1 ~3 −1         │
    ┄ 2 unchanged ┄               │
   Sep 02 20:16  ~1               │
    ┄ 5 unchanged ┄               │
   Aug 22 11:59  ~2               │
    ┄ 1 unchanged ┄               │
   Aug 17 16:04  +1 ~1            │
    ┄ 3 unchanged ┄               │
   Aug 10 15:44  ~1               │
    ┄ 1 unchanged ┄               │
   Aug 07 08:15  +1               │
    ┄ 1 unchanged ┄               │
   Aug 03 16:19  +1 ~1            │ 1 deleted earlier · zh to show
──────────────────────────────────┴─────────────────────────────────────────────────────────────────
 H L change  [ ] every snapshot  { } item change  j k move  h l up/open  v versions  d diff
```

### 3.4 Command line
`:` opens the command line. Hints show at the right.

<sub>`docs/screens/04-command-line.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 ~/dev/project/src                                                            ◀ version 10 of 16 ▶
────────────────────────────────────────────────────────────────────────────────────────────────────
  Jul 2026           Aug                               Sep                                   now
  ● · ·   ·●·● ●  ●  ·· ● · ● · ●·  · ·●  · ● · ·  · · · ●·  ●· ● ·  ● · ·  ● · ·  ●   · ·  ┊ ●
  • · ·   ···· ·  •  ·· • · · · ··  · ··  · • · ·  · · · ··  •· • ·  · · ·  · · ·  ·   · ·  ┊ •
                                            ▲                                              − 1× +
  Aug 22 11:59   snapshot 6b0f6953   src/  ~2                                    ● src/  • main.go
─ Versions ───────────────────────┬ src/ @ Aug 22 11:59 ────────────────────────────────────────────
    ┄ 2 unchanged ┄               │ NAME                             SIZE  MODIFIED        Δ
   Sep 20 20:35  ~1               │ ..
    ┄ 2 unchanged ┄               │ api/                                –  Aug 17 13:17
   Sep 13 20:51  ~2               │ models/                             –  Aug 17 14:53
    ┄ 1 unchanged ┄               │ config.go                       634 B  Aug 03 14:07
   Sep 09 19:23  ~1               │ legacy.go                       673 B  Aug 22 09:20    ~
    ┄ 1 unchanged ┄               │ main.go                          1.2K  Aug 22 10:42    ~          ◂
   Sep 06 18:03  +1 ~3 −1         │ util.go                         823 B  Jul 14 06:49
    ┄ 2 unchanged ┄               │
   Sep 02 20:16  ~1               │
    ┄ 5 unchanged ┄               │
 ▶ Aug 22 11:59  ~2               │                                                                   ◂
    ┄ 1 unchanged ┄               │
   Aug 17 16:04  +1 ~1            │
    ┄ 3 unchanged ┄               │
   Aug 10 15:44  ~1               │
    ┄ 1 unchanged ┄               │
   Aug 07 08:15  +1               │
    ┄ 1 unchanged ┄               │
   Aug 03 16:19  +1 ~1            │
    ┄ 2 unchanged ┄               │
   Jul 28 15:59  ~1               │
   Jul 26 11:57  ~1               │
──────────────────────────────────┴─────────────────────────────────────────────────────────────────
 :find legacy█                            2026-09-01 · sep 1 · yesterday · 3d · find NAME · deleted
```

### 3.5 Find in all snapshots
`:find NAME` searches every snapshot by path. It answers "where did this file go?"

<sub>`docs/screens/05-find.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 Find  "legacy"   1 match across all 43 snapshots
────────────────────────────────────────────────────────────────────────────────────────────────────
  PATH                                                  FIRST SEEN    LAST SEEN     NOW
 ▶src/legacy.go                                         Jul 14 09:00  Sep 04 08:08  gone Sep 06       ◂



























────────────────────────────────────────────────────────────────────────────────────────────────────
 j k move  ⏎ go to last snapshot that has it  q back
```

`⏎` jumps to the **last snapshot that still had it**, in its folder, with it selected:

<sub>`docs/screens/06-find-jump.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 ~/dev/project/src                                                 ◀ between versions (16 total) ▶
────────────────────────────────────────────────────────────────────────────────────────────────────
  Jul 2026           Aug                               Sep                                   now
  ● · ·   ·●·● ●  ●  ·· ● · ● · ●·  · ·●  · ● · ·  · · · ●·  ●· ● ·  ● · ·  ● · ·  ●   · ·  ┊ ●
  • · ·   ···· •  ·  ·· · · · · ··  · ··  · • · ·  · · · ··  •                              ┊ ·
                                                          ▲                                − 1× +
  Sep 04 08:08   snapshot c005272e   no changes in src/ since Sep 02 20:16   2 snapshots in this col
─ Versions ───────────────────────┬ src/ @ Sep 04 08:08 ────────────────────────────────────────────
  now         ~1  not backed up   │ NAME                             SIZE  MODIFIED        Δ
    ┄ 2 unchanged ┄               │ ..
   Sep 27 08:52  ~1               │ api/                                –  Sep 02 18:05
    ┄ 2 unchanged ┄               │ models/                             –  Aug 17 14:53
   Sep 20 20:35  ~1               │ config.go                       634 B  Aug 03 14:07
    ┄ 2 unchanged ┄               │ legacy.go                       673 B  Aug 22 09:20               ◂
   Sep 13 20:51  ~2               │ main.go                          1.2K  Aug 22 10:42
    ┄ 1 unchanged ┄               │ util.go                         823 B  Jul 14 06:49
   Sep 09 19:23  ~1               │
    ┄ 1 unchanged ┄               │
   Sep 06 18:03  +1 ~3 −1         │
 ▶  ┄ 2 unchanged ┄               │
   Sep 02 20:16  ~1               │
    ┄ 5 unchanged ┄               │
   Aug 22 11:59  ~2               │
    ┄ 1 unchanged ┄               │
   Aug 17 16:04  +1 ~1            │
    ┄ 3 unchanged ┄               │
   Aug 10 15:44  ~1               │
    ┄ 1 unchanged ┄               │
   Aug 07 08:15  +1               │
    ┄ 1 unchanged ┄               │
   Aug 03 16:19  +1 ~1            │
──────────────────────────────────┴─────────────────────────────────────────────────────────────────
 Jumped to Sep 04 08:08, the last snapshot that has legacy.go
```

### 3.6 Deleted items
`zh` (or `.`) shows items deleted earlier. Selecting one shows its history on row 2: dots up to the deletion, then blank. `⏎`/`l` on a deleted folder jumps to the last snapshot that had it.

<sub>`docs/screens/07-deleted-shown.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 ~/dev/project/src                                                            ◀ version 14 of 16 ▶
────────────────────────────────────────────────────────────────────────────────────────────────────
  Jul 2026           Aug                               Sep                                   now
  ● · ·   ·●·● ●  ●  ·· ● · ● · ●·  · ·●  · ● · ·  · · · ●·  ●· ● ·  ● · ·  ● · ·  ●   · ·  ┊ ●
  • · ·   ···· •  ·  ·· · · · · ··  · ··  · • · ·  · · · ··  •                              ┊ ·
                                                                     ▲                     − 1× +
  Sep 13 20:51   snapshot 25e5c7c8   src/  ~2                                  ● src/  • legacy.go
─ Versions ───────────────────────┬ src/ @ Sep 13 20:51 ────────────────────────────────────────────
  now         ~1  not backed up   │ NAME                             SIZE  MODIFIED        Δ
    ┄ 2 unchanged ┄               │ ..
   Sep 27 08:52  ~1               │ api/                                –  Sep 13 20:33    ~1
    ┄ 2 unchanged ┄               │ models/                             –  Aug 17 14:53
   Sep 20 20:35  ~1               │ config.go                       789 B  Sep 13 20:13    ~
    ┄ 2 unchanged ┄               │ legacy.go                       673 B  gone Sep 06                ◂
 ▶ Sep 13 20:51  ~2               │ main.go                          1.3K  Sep 09 18:25               ◂
    ┄ 1 unchanged ┄               │ server.go                       845 B  Sep 06 15:17
   Sep 09 19:23  ~1               │ util.go                         823 B  Jul 14 06:49
    ┄ 1 unchanged ┄               │
   Sep 06 18:03  +1 ~3 −1         │
    ┄ 2 unchanged ┄               │
   Sep 02 20:16  ~1               │
    ┄ 5 unchanged ┄               │
   Aug 22 11:59  ~2               │
    ┄ 1 unchanged ┄               │
   Aug 17 16:04  +1 ~1            │
    ┄ 3 unchanged ┄               │
   Aug 10 15:44  ~1               │
    ┄ 1 unchanged ┄               │
   Aug 07 08:15  +1               │
    ┄ 1 unchanged ┄               │
   Aug 03 16:19  +1 ~1            │ showing deleted items · zh to hide
──────────────────────────────────┴─────────────────────────────────────────────────────────────────
 H L change  [ ] every snapshot  { } item change  j k move  h l up/open  v versions  d diff
```

### 3.7 Versions of a file (`v`)
One row per **distinct version**, newest first. Deleted periods appear as their own rows. The `on disk` row is always first. "COMPARED TO DISK" shows lines added and removed, or `identical`.

<sub>`docs/screens/08-versions.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop               6 versions in 43 snapshots
 Versions  ~/dev/project/src/main.go
────────────────────────────────────────────────────────────────────────────────────────────────────
  Jul 2026           Aug                               Sep                                   now
  ● · ·   ···· ·  ●  ·· ● · · · ··  · ··  · ● · ·  · · · ··  ●· ● ·  · · ·  · · ·  ·   · ·  ┊ ●
                                                                ▲                          − 1× +
  Sep 09 19:23 → Oct 02 12:21   in 11 snapshots  62c0e9c7 … b41050ad
────────────────────────────────────────────────────────────────────────────────────────────────────
  VERSION               SIZE   MODIFIED        SNAPSHOTS      COMPARED TO DISK
  on disk               1.3K   Oct 05 09:58
 ▶Sep 09 19:23          1.3K   Sep 09 18:25    11 snapshots   +1 −1                                   ◂
  Sep 06 18:03          1.3K   Sep 06 16:34    2 snapshots    +1 −2
  Aug 22 11:59          1.2K   Aug 22 10:42    9 snapshots    +3 −2
  Aug 03 16:19          1.3K   Aug 03 13:55    10 snapshots   +4 −4
  Jul 28 15:59          1.3K   Jul 28 15:13    3 snapshots    +4 −5
  Jul 14 09:00          1.3K   Jul 14 08:03    8 snapshots    +4 −4
















────────────────────────────────────────────────────────────────────────────────────────────────────
 j k version  ⏎ d diff vs disk  p diff vs previous  r restore  y yank  q back
```

### 3.8 Diff (`d`)
Two modes. `c` (the default): selected version → on disk, "what changed since this version". `p`: previous distinct version → selected version, "what this version changed". Unified diff with 3 lines of context, both line numbers, and `┄┄ around line N ┄┄` between changes.

<sub>`docs/screens/09-diff.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 main.go  Aug 22 11:59  →  on disk   (what changed since this version)                       +3 −2
────────────────────────────────────────────────────────────────────────────────────────────────────
 ┄┄ around line 18 ┄┄
   18   18        if err := validate(req); err != nil { return err }
   19   19        if err := validate(req); err != nil { return err }
   20   20        if err := validate(req); err != nil { return err }
   21       -     return nil
        21  +     signal.Notify(stop, os.Interrupt, syscall.SIGTERM)
   22   22        return nil
   23   23    }
   24   24
 ┄┄ around line 42 ┄┄
   42   42        ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
   43   43        w.Header().Set("Content-Type", "application/json")
   44   44        log.Printf("starting %s", name)
   45       -     user, err := store.GetUser(ctx, id)
        45  +     if len(args) == 0 { return errNoArgs }
   46   46        defer cancel()
        47  +     for i := 0; i < retries; i++ { time.Sleep(backoff(i)) }
   47   48    }
   48   49










────────────────────────────────────────────────────────────────────────────────────────────────────
 j k scroll  ]c [c next/prev change  H L older/newer  c vs disk  p vs previous  r restore
```

Binary files: show "binary file, 12.4K → 13.0K" instead of a diff. Files over 2 MB (configurable) are diffed only on request.

### 3.9 Restore (`r`)
Works on files and folders, from the folder view, the versions view and the diff.

<sub>`docs/screens/10-restore-dialog.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 main.go  Aug 22 11:59  →  on disk   (what changed since this version)                       +3 −2
────────────────────────────────────────────────────────────────────────────────────────────────────
 ┄┄ around line 18 ┄┄
   18   18        if err := validate(req); err != nil { return err }
   19   19        if err := validate(req); err != nil { return err }
   20   20        if err := validate(req); err != nil { return err }
   21       -     return nil
        21  +     signal.Notify(stop, os.Interrupt, syscall.SIGTERM)
   22   22 ┌─ Restore ──────────────────────────────────────────────────────────────────┐
   23   23 │                                                                            │
   24   24 │  main.go  @ Aug 22 11:59  6b0f6953                                         │
 ┄┄ around │  from /home/bege/dev/project/src/main.go                                   │
   42   42 │                                                                            │
   43   43 │  1 ( ) Overwrite original             ~/dev/project/src/main.go            │
   44   44 │  2 (•) Restore next to it             → main.go.2026-08-22_1159            │
   45      │  3 ( ) Restore to ~/Restored/         → ~/Restored/2026-08-22_1159/main.…  │
        45 │  4 ( ) Show in $PAGER                 read only, writes nothing            │
   46   46 │                                                                            │
        47 │                                                                            │
   47   48 │                                                                            │
   48   49 │  [ Restore ]   [ Cancel ]                      j k choose  ⏎ restore  esc  │
           └────────────────────────────────────────────────────────────────────────────┘









────────────────────────────────────────────────────────────────────────────────────────────────────
 j k scroll  ]c [c next/prev change  H L older/newer  c vs disk  p vs previous  r restore
```

| # | Option | Behaviour |
|---|---|---|
| 1 | Overwrite original, or "Restore to original location" if it's missing on disk | **Asks for confirmation** when something exists on disk. First moves the current file to `~/.local/share/restoric/undo/<timestamp>/`, so `:undo` can reverse it. |
| 2 | Restore next to it (**default**) | `name.2026-09-06_1803` or `dir.2026-09-06_1803/` |
| 3 | Restore to `~/Restored/` | `~/Restored/<stamp>/<name>` |
| 4 | File: show in `$PAGER` · folder: write a tar archive | Read only / `name-<stamp>.tar` |

The default is option 1 when nothing exists on disk. Keys: `j`/`k` or `1`–`4` to choose, `⏎` to restore, `esc` to cancel. Afterwards the bottom row confirms what happened, e.g. "Restored as server.go.2026-09-06_1803".

### 3.10 Help (`?`)
<sub>`docs/screens/11-help.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic   host bege-laptop                43 snapshots · cache warm
 ~/dev/project/src                                                 ◀ between versions (16 total) ▶
────────────────────────────────────────────────────────────────────────────────────────────────────
  Jul 2026           Aug                               Sep                                   now
  ● · ·   ·●·● ●  ●  ·· ● · ● · ●·  · ·●  · ● · ·  · · · ●·  ●· ● ·  ● · ·  ● · ·  ●   · ·  ┊ ●
  • · ·   ···┌─ Help ─────────────────────────────────────────────────────────────────┐· ·  ┊ •
             │                                                                        │  ▲ − 1× +
  Oct 02 12:2│  Folder view                                                           │  • main.go
─ Versions ──│  j k  ↓ ↑               move                                           │─────────────
  now        │  gg G  C-d C-u          top, bottom, half page                         │    Δ
 ▶  ┄ 2 uncha│  h l  - ⏎               parent folder / open                           │
   Sep 27 08:│  H L  ← →               older / newer change in this folder            │
    ┄ 2 uncha│  [ ]                    every snapshot, changed or not                 │
   Sep 20 20:│  { }                    older / newer change of the selected item      │
    ┄ 2 uncha│  v  d                   versions of the file / diff against disk       │
   Sep 13 20:│  r  y                   restore / yank snapshot:path                   │
    ┄ 1 uncha│  /  n N                 search this folder / next, previous match      │
   Sep 09 19:│  zh  .                  show deleted items                             │
    ┄ 1 uncha│  zi zo                  zoom the timeline                              │
   Sep 06 18:│  3H  5j                 counts work with motions                       │
    ┄ 2 uncha│                                                                        │
   Sep 02 20:│  Commands                                                              │
    ┄ 5 uncha│  :sep 1  :2026-09-01    jump to a date (also :yesterday :3d :2w)       │
   Aug 22 11:│  :find NAME             search every snapshot for a name               │
    ┄ 1 uncha│  :latest  :oldest                                                      │
   Aug 17 16:│                                                                        │
    ┄ 3 uncha│  Diff   ]c [c or n N changes, c vs disk, p vs previous                 │
   Aug 10 15:│  Press any key to close                                                │
    ┄ 1 uncha└────────────────────────────────────────────────────────────────────────┘
   Aug 07 08:15  +1               │
    ┄ 1 unchanged ┄               │
   Aug 03 16:19  +1 ~1            │ 1 deleted earlier · zh to show
──────────────────────────────────┴─────────────────────────────────────────────────────────────────
 H L change  [ ] every snapshot  { } item change  j k move  h l up/open  v versions  d diff
```

### 3.11 Narrow terminals (80 columns)
Below 100 columns the Versions pane folds away and the listing takes the full width. The timeline and other screens scale to the width. Below 80 columns: drop the MODIFIED column.

<sub>`docs/screens/12-80-columns.txt`</sub>

```text
 restoric  repo sftp:nas:/backup/restic               43 snapshots · cache warm
 ~/dev/project/src                             ◀ between versions (16 total) ▶
────────────────────────────────────────────────────────────────────────────────
  Jul 2026       Aug                       Sep                           now
  ● ··  ·●● ● ●  · ● ·●· ●· · ·● · ●··  ·· ·●· ●· ●· ● · · ● ··  ● · ·  ┊ ●
  • ··  ··· · •  · • ··· ·· · ·· · •··  ·· ··· •· •· · · · · ··  · · ·  ┊ •
                                                                     ▲ − 1× +
  Oct 02 12:21   snapshot b41050ad   no changes in src/ since Sep 27 08:52
─ src/ @ Oct 02 12:21 ──────────────────────────────────────────────────────────
  NAME                                           SIZE  MODIFIED        Δ
  ..
  api/                                              –  Sep 27 06:41
  models/                                           –  Aug 17 14:53
  config.go                                     789 B  Sep 13 20:13
  main.go                                        1.3K  Sep 09 18:25               ◂
  server.go                                     845 B  Sep 06 15:17
  util.go                                       823 B  Jul 14 06:49














  1 deleted earlier · zh to show
────────────────────────────────────────────────────────────────────────────────
 H L change  [ ] every snapshot  { } item change  j k move  h l up/open
```

### 3.12 Keymap

**Folder view**
| Keys | Action |
|---|---|
| `j` `k` / `↓` `↑` | Move selection |
| `gg` `G`, `Ctrl-d` `Ctrl-u`, `PgDn` `PgUp` | Top, bottom, half page |
| `h` `-` `⌫` | Parent folder (selects the folder you came from) |
| `l` `⏎` | Open folder / versions of the file. On a deleted item, jump to its last snapshot first. |
| `H` `L` / `←` `→` | Older / newer **change in this folder**. `L` past the last change goes to the newest snapshot. |
| `[` `]` / `Shift-←` `Shift-→` | Every snapshot, changed or not |
| `{` `}` | Older / newer change of the **selected item** |
| `Home` `End` | Oldest / newest change |
| `v` | Versions of the selected file |
| `d` | Diff the selected file against disk |
| `r` | Restore |
| `y` | Copy `snapshotid:/abs/path` to the clipboard |
| `zh` `.` | Show / hide deleted items |
| `zi` `zo` | Zoom the timeline |
| `/` then `n` `N` | Search this folder as you type (jumps to the first match, highlights all), next / previous match |
| `:` | Command line |
| `?` | Help (any key closes it) |
| `esc` | Clear the search highlight |
| `q` | Quit |
| *count* | `3H`, `5j`, … repeat a motion. Stops at the first boundary message. |

**Versions view:** `j` `k` / `H` `L` / `{` `}` move older and newer · `gg` `G` · `⏎` `d` `l` diff against disk · `p` diff against previous · `r` restore · `y` yank · `zi` `zo` · `q` `h` `esc` back.

**Diff view:** `j` `k` scroll · `Ctrl-d` `Ctrl-u` `space` · `gg` `G` · `]c` `[c` and `n` `N` next / previous change · `H` `L` older / newer version · `c` against disk · `p` against previous · `r` · `y` · `q` `h` `esc` back.

**Find view:** `j` `k` · `gg` `G` · `⏎` `l` go to the last snapshot with it · `q` `h` `esc` back.

**Restore dialog:** `j` `k` / `1`–`4` · `⏎` (twice for overwrite) · `esc` `q`.

Mouse (crossterm mouse events): click timeline dots, rows, breadcrumb parts, `◀ ▶`, `− +`, dialog options and key hints. Clicking a selected row opens it. Wheel scrolls lists.

### 3.13 Commands
| Command | Effect |
|---|---|
| `:2026-09-01`, `:09-01`, `:sep 1`, `:september 1` | Jump to the last snapshot on or before that day |
| `:today` `:yesterday` `:3d` `:2w` | Relative dates |
| `:latest` `:now` / `:oldest` `:first` | Ends of the timeline |
| `:find NAME` / `:f NAME` | Search every snapshot (path contains NAME, case-insensitive) |
| `:deleted` | Same as `zh` |
| `:host NAME`, `:tag T` | Change the snapshot filter *(new, not in the mockup)* |
| `:set strict` / `:set nostrict` | Tree-id vs fingerprint change detection *(new)* |
| `:undo` | Undo the last overwrite restore *(new)* |
| `:q` `:quit` | Quit |
| `:help` | Help |

An unknown command shows: `Unknown command ":x". Try :sep 1, :yesterday, :3d, :find NAME, :deleted`.

### 3.14 Messages (copy the mockup's wording)
- "This is the oldest version of this folder." / "Newest snapshot. Newer changes exist only on disk." / "Newest snapshot. Nothing changed on disk since."
- "No older snapshot of this folder." · "No older change to main.go." · "Select a file or folder first."
- "Jumped to Sep 04 08:08, the last snapshot that has legacy.go"
- "No match for "x" in this folder. :find x searches every snapshot."
- "Yanked 3e01f5b8:/home/bege/dev/project/src/util.go"
- "The file did not exist in these snapshots, so there is nothing to restore."
- "No snapshots that early. The oldest is from Jul 14 09:00."
- "Already at the top of the backup."

### 3.15 Colours
Use the **16 ANSI colours** so the terminal's own theme applies. Respect `NO_COLOR`.
- `+` added: green · `~` changed: blue · `−` deleted: red · not backed up: magenta · accent (current dot, `▶`, keys): yellow
- Selection: reverse video, or a dim background when the terminal supports it
- Folders: bold blue · deleted: red + strikethrough (where supported) · gone earlier: dim italic

---

## 4. Architecture

### 4.1 Crates
| Need | Crate | Notes |
|---|---|---|
| Repository | `rustic_core` | Pin an exact version and wrap it behind our own trait (§4.3) |
| Backends | `rustic_backend` | local, sftp, S3, rclone, REST… check the backend you actually use in M0 |
| TUI | `ratatui` + `crossterm` | Mouse capture on |
| Cache | `redb` | One file per repo id |
| Diff | `imara-diff` | Histogram algorithm. Our own hunk and context formatting. |
| Fuzzy / search | `nucleo` | For `/` and `:find` (substring at first, fuzzy later) |
| Dates | `jiff` | `:sep 1`, `:3d`, display in local time |
| CLI | `clap` (derive) | |
| Clipboard | `arboard` | Fall back to the OSC 52 escape sequence over SSH |
| Paths | `directories` | Config, cache, undo folder |
| Threads | `crossbeam-channel` | No async runtime |
| Errors / logs | `anyhow`, `thiserror`, `tracing` + `tracing-appender` (log to a file, never to the TUI) | |
| Tests | `insta` (snapshot tests), ratatui `TestBackend`, `tempfile` | |
| Later | `syntect` for diff highlighting, `trash` for undo via the system trash | |

### 4.2 Module layout
```
restoric/
├── Cargo.toml
├── PLAN.md
├── docs/            mockup.html, screens/*.txt
├── src/
│   ├── main.rs        clap args, env (RESTIC_*), start-up, terminal setup/teardown, panic hook
│   ├── config.rs      ~/.config/restoric/config.toml
│   ├── repo/
│   │   ├── mod.rs     trait Repo + our own types (SnapshotInfo, TreeId, Node, NodeKind)
│   │   ├── rustic.rs  RusticRepo: rustic_core implementation
│   │   └── fake.rs    FakeRepo: in-memory, built from a small DSL (tests, UI work, demo mode)
│   ├── index/
│   │   ├── fingerprint.rs   memoised by tree id
│   │   ├── timeline.rs      timeline set, change points per path, item tracks
│   │   ├── folder.rs        listing at snapshot n, Δ markers, counts, deleted items
│   │   └── versions.rs      runs per file
│   ├── cache.rs       redb tables (§4.4)
│   ├── worker.rs      background pool, Request/Response enums, generation ids
│   ├── restore.rs     restore/dump/tar, undo log
│   ├── diff.rs        load both sides (size limit, binary detection), imara-diff, hunks
│   ├── app/
│   │   ├── mod.rs     App state, view stack (Folder, Versions, Diff, Find), overlays (Dialog, Help)
│   │   ├── keys.rs    key parser: counts, prefixes (g, z, ], [), modes (normal, command, search, dialog)
│   │   ├── cmdline.rs `:` parser, dates via jiff
│   │   └── actions.rs one function per action, shared by keys and mouse
│   └── ui/
│       ├── timeline.rs  labels, tracks, zoom, ‹ ›, caret, clickable areas
│       ├── folder.rs    versions pane + listing
│       ├── versions.rs  diff.rs  find.rs  dialog.rs  help.rs  statusline.rs
│       └── theme.rs
└── tests/
    ├── fixtures/make_repo.sh   builds a real restic repo with a known history
    ├── index_*.rs              change points against the fixture
    └── ui_*.rs                 insta snapshots of every screen (the screens in this plan)
```

### 4.3 The `Repo` trait
Our own trait wraps rustic_core: it isolates API changes and lets the UI run against `FakeRepo`.

```rust
pub trait Repo: Send + Sync {
    fn snapshots(&self) -> Result<Vec<SnapshotInfo>>;          // id, time, host, paths, tags, root tree
    fn tree(&self, id: &TreeId) -> Result<Arc<Tree>>;           // nodes: name, kind, size, mtime, content ids, subtree id
    fn read_file(&self, snap: &SnapshotId, path: &Path, limit: u64) -> Result<FileBytes>;
    fn restore(&self, snap: &SnapshotId, path: &Path, dest: &Path, opts: RestoreOpts) -> Result<RestoreReport>;
    fn dump_tar(&self, snap: &SnapshotId, path: &Path, out: &mut dyn Write) -> Result<()>;
}
```
Everything in `index/` is written against this trait and holds no rustic types.

### 4.4 Cache (redb, `~/.cache/restoric/<repo-id>.redb`)
| Table | Key → value | Notes |
|---|---|---|
| `snapshots` | snapshot id → time, host, paths, tags, root tree | Refreshed at start-up; only new snapshots are read |
| `fingerprint` | tree id → fingerprint | Never needs invalidating |
| `path_tree` | (snapshot id, path) → subtree id | Makes later walks O(1) |
| `change_points` | (filter hash, path) → list of snapshot ids + counts | Recomputed when a new snapshot arrives (only the new tail) |
| `meta` | schema version, rustic_core version | Wipe if they don't match |

rustic_core has its own cache for index and tree packs. Check in M0 that tree packs are cached locally, so walks are fast after the first run.

### 4.5 Threads
- **UI thread:** the event loop over `crossbeam::select!` between crossterm events, worker responses and a tick for spinners. It draws only from `App` state and never calls `Repo`.
- **Worker pool** (N = number of CPUs, minimum 2): handles `Request` messages (`ChangePoints{path}`, `Listing{snap,path}`, `ItemTrack{snap_range,path}`, `Versions{path}`, `LoadFile{…}`, `Restore{…}`, `Find{q}`). Each request carries a **generation id**, and results from older generations are dropped, so fast scrolling stays snappy.
- **Prefetch:** after a listing loads, prefetch the neighbouring change points (n±1) and the item track for the selection.
- **Progress:** the title bar shows `indexing 120/430` while change points are computed. Partial results draw as they arrive, newest first.

### 4.6 Start-up
1. Read the repository and password from `--repo` / `RESTIC_REPOSITORY` / `RESTIC_REPOSITORY_FILE` and `RESTIC_PASSWORD` / `_FILE` / `_COMMAND`, the same as restic. Then the config file.
2. Open the repo (read only). Load the snapshot list from the cache, then fetch new ones in the background.
3. Pick the path: the argument or the current folder, made absolute. The timeline set is snapshots of the host (`--host`, default this machine's hostname) whose `paths` contain that path.
4. If the path isn't in any snapshot: show a clear message listing the backed-up paths for this host.

### 4.7 CLI
```
restoric [PATH]                       open the TUI at PATH (default: current folder)
  -r, --repo REPO       --password-command CMD     (plus all RESTIC_* env vars)
  --host HOST           --tag TAG                  --strict
  --at DATE             start at a date (:sep 1 syntax)
restoric log PATH [--json]            print the change points of PATH (no TUI)   (M1)
restoric versions FILE [--json]       print the distinct versions of FILE        (M3)
restoric demo                         TUI against FakeRepo with the mockup's sample data
```

---

## 5. Milestones

Each milestone ends in something usable and tested.

### M0: rustic_core test run (1–2 days)
Throwaway binary `spike/`:
- Open **your real repo** (and backend) with rustic_core. List snapshots.
- Walk to one folder in every snapshot, timed cold and warm.
- Count snapshots where the tree id differs vs where the fingerprint differs.
- Read one file from an old snapshot. Restore one file into a temporary folder.

**Go/no-go:** a warm walk over all snapshots under ~2 s for a typical folder, a cold walk acceptable with a progress indicator, the backend works, and the restored file matches. If rustic_core fails on something essential, record what and decide between fixing it upstream and a `restic`-CLI implementation of `Repo` (the trait makes that possible).

### M1: index and `restoric log`
- `Repo` trait, `RusticRepo`, `FakeRepo` + DSL.
- Fingerprints, timeline set, change points, folder counts, cache.
- `restoric log PATH` prints change points with counts.
- `tests/fixtures/make_repo.sh`: uses the real `restic` binary with `backup --time` to build a repo with known history: added, changed, deleted, re-created, metadata-only (`touch`, `chmod`) and renamed files.
- **Done when:** change points match the fixture's expected list, and metadata-only snapshots are skipped by default but shown with `--strict`.

### M2: read-only folder view
- Terminal setup and teardown (panic hook restores the terminal), event loop, worker, generation ids.
- Title bar, breadcrumb, timeline row 1 (no zoom), info row, Versions pane with folding, listing with Δ, key hints, help.
- Keys: `j k gg G C-d C-u h l - ⌫ ⏎ H L [ ] Home End q ?` and counts. Mouse clicks on rows, dots and breadcrumb.
- 80-column layout.
- **Done when:** insta snapshots of screens 01 and 12 (from FakeRepo) match, and it's usable on your real repo.

### M3: item track, versions, deleted items
- Row 2, `{` `}`, legend or labels (§3.1 proposal).
- Versions view (`v`) with on-disk row and compared-to-disk stats (computed lazily for visible rows).
- Deleted items (`zh` `.`), jump on deleted items.
- `restoric versions FILE`.
- **Done when:** screens 02, 07 and 08 match.

### M4: diff
- Load both sides with a size limit and binary detection, imara-diff, hunks, `c`/`p` modes, `]c` `[c` `n` `N`, `H` `L` across versions.
- **Done when:** screen 09 matches, and binary and large files are handled.

### M5: restore
- Dialog with 4 options, confirmation, undo log, `:undo`, `y` yank (arboard, then OSC 52).
- Files and folders, existing and deleted on disk.
- **Done when:** screen 10 matches. Integration tests restore from the fixture into a temporary folder for every option, and overwrite then `:undo` gives back the original bytes.

### M6: navigation extras
- `/` search with highlighting, `n` `N`. `:` command line with dates, `:find`, `:latest`, `:oldest`, `:deleted`, `:host`, `:tag`, `:set strict`.
- Find view and jump to the last snapshot with the match.
- Zoom (`zi` `zo`, clickable), "N snapshots in this column".
- **Done when:** screens 03, 04, 05, 06 and 11 match.

### M7: polish and release
- Config file (keymap overrides, colours, diff size limit, default host). `NO_COLOR`.
- `restoric demo`.
- Performance pass on a large repo (thousands of snapshots, deep trees).
- README with screenshots. `cargo install`, GitHub release binaries (Linux x86_64/aarch64, macOS), AUR/deb later.
- Optional: syntax highlighting in diffs, restore to the system trash.

---

## 6. Testing
- **Unit:** fingerprints (metadata churn ignored, content changes caught), key parser (counts, prefixes, modes), `:` parser (all date forms, errors), hunk building.
- **Index integration:** against the restic-built fixture repo (§M1), so we test against the real format restic writes, not only rustic's.
- **UI snapshots:** `insta` + ratatui `TestBackend` at 100×34 and 80×34, driven by FakeRepo with the same sample data as the mockup. The captures in this plan are the spec.
- **Restore integration:** every option, files and folders, deleted on disk, overwrite + undo, permissions kept.
- **Manual:** your real repo over the real backend, before each milestone is called done.

---

## 7. Risks
| Risk | Mitigation |
|---|---|
| rustic_core API changes or missing pieces | Pin the version, wrap it in `Repo`, test in M0. A restic-CLI `Repo` is possible as a fallback. |
| Metadata changes make every snapshot look changed | Content fingerprints (§2.2), measured in M0 |
| Slow first run on big or remote repos | Persistent cache, background indexing with progress, newest-first partial results |
| `restic prune` running while restoric reads (rustic doesn't take locks) | Treat missing packs as recoverable: reload the index, retry once, show an error in the status line without crashing |
| Restore overwrites something important | Default is "next to it", confirmation, undo folder |
| Snapshots with different backup roots or hosts | Timeline set filter (§2.3), `:host`, `:tag`, clear message when the path isn't backed up |
| Huge folders (100k entries) | Virtualised listing, counts computed lazily |

---

## 8. Open questions
1. **Which backend** does your repo use (local, sftp, S3, rclone, REST)? This decides what M0 tests.
2. **How big** is the repo: number of snapshots, files per snapshot? Sets the performance targets.
3. **Should a permission (mode) change count as a change?** The proposal is no by default, yes with `--strict`.
4. **Labels on the timeline rows** instead of the legend (§3.1): yes?
5. **License:** MIT/Apache-2.0 (like rustic)?
6. **Is the name `restoric` free** on crates.io and GitHub? Check before publishing.
7. **Snapshots from several hosts** of the same folder (e.g. a laptop and a desktop syncing a project): merge them into one timeline, or keep one host at a time (current plan)?

---

## 9. Out of scope (for now)
- Making backups, scheduling, prune/forget. restic/rustic and Backrest already do that.
- Writing to the repository in any way. restoric is read-only towards the repo.
- A graphical (non-terminal) app or file manager plugins. That's a possible future, with the index reusable as a library.
