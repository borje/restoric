# restoric: plan

restoric is a terminal UI for browsing and restoring files from a [restic](https://restic.net) repository, in the spirit of macOS Time Machine, with a look borrowed from the [yazi](https://github.com/sxyazi/yazi) file manager. It's written in Rust on top of [rustic_core](https://crates.io/crates/rustic_core).

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
5. **Works with your existing restic setup.** It uses `RESTIC_REPOSITORY`, `RESTIC_PASSWORD_COMMAND` and so on, with no new config needed. When different folders go to different repositories, the config can list them all, and restoric finds the one that holds the folder (§4.6).
6. **Vim and yazi keys, arrows always.** Every action has a vim key and a plain key, and keys mean the same as in yazi where they can. Prefix keys pop up what can follow, and `?` lists everything.
7. **No FUSE, no mount.** It reads the repository directly through rustic_core.
8. **Looks at home next to yazi.** Three columns with a preview, a status bar with a mode badge, few lines, rounded popups, Nerd Font icons.

---

## 2. Core concept: detecting change

In a restic repository, each snapshot points to a root **tree**. A tree lists **nodes**, which are files, folders and links. A folder node has a `subtree` id, and a file node has a `content` list of blob ids. Trees are content-addressed and never change.

### 2.1 Fast path: subtree id
If folder `P` has the same subtree id in snapshot *n* and snapshot *n−1*, nothing under `P` changed. This needs a walk of only `depth(P)` trees per snapshot, and unchanged subtrees share ids, so their trees are loaded once.

### 2.2 Metadata churn and the fingerprint
Restic stores metadata such as **atime, ctime, inode and device id** in each node. These can change without any content changing, for example when a file is read (atime) or rewritten with the same content (ctime, inode). When they do, the subtree id changes too. If change points relied on tree ids alone, the timeline would show too many `●`.

**Decision:** "changed" means the **content fingerprint** changed:

```
fingerprint(file)    = H(type, size, mode, uid, gid, content blob ids)
fingerprint(symlink) = H(type, link target)
fingerprint(folder)  = H(sorted [(name, fingerprint(child))])
```

- **How it's computed (M1):** files, links and other entries get their fingerprint from their own node, with no lookups. Folder fingerprints are never stored. Instead, two versions of a folder are compared by **diffing their trees**: walk both side by side, go only into subtrees whose ids differ, and stop at the first real difference. This gives the same answer as comparing `fingerprint(folder)`, but the cost follows what changed, not the size of the folder (§4.8). Computing a folder fingerprint means reading every tree under it once. The results are cached by **pairs of tree ids**. Tree ids never change, so the cache never needs invalidating.
- Fast path first: the same subtree id means no change, with no recursion.
- **Permission changes count** (mode), and so do owner and group changes. They show as `~` like a content change.
- Modification time is **excluded**: touching a file without changing it is not a change.
- `--strict` (or `:set strict`) switches to raw tree ids, for people who care about every metadata change.

> **Check in M0:** how often do tree ids differ when fingerprints match, on a real repo? Results are in §11 (M0). The tree diff above is lazy either way.

### 2.3 What gets computed
| Name | Definition | Used for |
|---|---|---|
| Timeline set | Snapshots of **this machine** (§2.4) that hold the browsed folder, sorted by time. A snapshot holds it when one of its backup paths is the folder, above it, or below it (`restic backup dir/a.log dir/b.csv` holds `dir`, since the snapshot tree has the folders above each backup path) | Every view |
| Change points of folder P | Snapshots where `fingerprint(P)` differs from the previous snapshot in the timeline set (or P first appears or disappears) | The timeline's `○` (`●` when nothing is selected), the Versions pane, `H`/`L` |
| Item track | The same as change points, for the selected entry | The timeline's `●`, `{`/`}` |
| Entry change marker | The entry's node at snapshot n compared with n−1: `+` added, `~` changed, `−` deleted | Δ column |
| Folder counts | Number of files added, changed and deleted under P between n−1 and n | `+1 ~3 −1` labels |
| Versions | Consecutive runs of snapshots with an identical file fingerprint | Versions view |
| Deleted items | Names seen in P's trees in earlier snapshots that are missing now | `zh` |

Folder counts need a tree diff. Only walk subtrees whose ids differ. An **item** is anything but a folder, or an empty folder, so a change always counts as at least one.

### 2.4 Only this machine
restoric only shows snapshots made **on this machine**. Snapshots from other computers backing up to the same repository are ignored.

This is possible because every restic snapshot records the **hostname** of the machine that made it (the `hostname` field, shown by `restic snapshots`). restoric compares it with this machine's hostname, the same way `restic snapshots --host` filters.

It works for normal setups. Things that can break it, and what restoric does:

| Situation | What happens | What restoric does |
|---|---|---|
| The machine was **renamed** | Older snapshots carry the old hostname and would disappear | Config `host = ["new-name", "old-name"]` accepts several names. If old snapshots seem to be missing (the path exists under another hostname with the same backup paths), the status bar says so once. |
| Backups use `restic backup --host X` | Snapshots carry X, not the real hostname | Set `host = "X"` in the config, or pass `--host X` |
| **Two machines with the same hostname** back up to one repo | restic can't tell them apart either | Can't be separated by hostname. Separate them with tags (`restic backup --tag laptop`) and set `tag = "laptop"` in restoric's config. |
| Containers or VMs with random hostnames | Each run looks like a new machine | Use `--host` when backing up, as above |

To look at another machine's snapshots on purpose, use the repository picker (§3.18).

So: **yes, it can tell machines apart**, as reliably as restic itself does. The one case it can't handle on its own is two machines sharing a hostname, and tags solve that.

### 2.5 Changes on disk since the newest snapshot
The `on disk` row, the `on disk` marker at the right end of the timeline row, "vs disk" and the versions view compare with the files on disk (M3):

- A file **counts as changed on disk** when its kind, size, modification time or permissions differ from the newest snapshot. That's the same test restic uses to decide whether to read a file again. Content isn't hashed, so a file that's only touched counts as changed here, unlike between snapshots (§2.2).
- A folder's `on disk` counts walk the folder on disk and its tree in the newest snapshot. This runs in the background, once per folder, and is cached for the session. A large folder takes a while, and the row shows `…` until it's done.
- "vs disk" and the VS DISK column diff the first 64 KB of each side.
- Disk access goes through a `Disk` trait, so tests use a fake disk built from the same DSL as `FakeRepo` (a `disk` block after the snapshots). Symlinks are never followed.

---

## 3. UX specification

The look follows **[yazi](https://github.com/sxyazi/yazi)**: three columns, a preview on the right, a status bar with a mode badge, which-key popups for prefix keys, rounded popups, few lines, and Nerd Font icons. yazi is built on ratatui too. The **timeline** is restoric's own and stays at the top.

Captures are 100×34 unless noted. The selected row is highlighted in the real UI; in these captures it's marked with `◂` at the end of the row (a row can carry several highlights, one per column). The sample data is invented. Icons are stand-ins (`▸ ◇ ¶ $ ≡ ○`) for Nerd Font glyphs.

### 3.1 Folder view (main screen)
Starting `restoric` with no arguments opens the **current folder**, at the newest snapshot that changed it (the mockup starts mid-history to show more). This is the Time Machine move of "enter from this Finder window".

<sub>`docs/screens/01-folder-view.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ○·○ ○ ●  · ● · ○· ○·  ··○  · ●· · · · ·○·  ● ● · ○ · · ○ · · ○  · ·  ┊ ● main.go  ○ src/
                                                    ▲                                      − 1× +

 on disk  ~1          │   ..                              │ main.go · v5/6 · 1.3K        ⇥ content
   ┄ 2 unchanged ┄    │ ▸ api/                      ~1    │ changed here · + new line · − removed
 Sep 27 08:52 ~1      │ ▸ models/                         │
   ┄ 2 unchanged ┄    │ ◇ config.go          737 B  ~     │  27    var req CreateOrderRequest
 Sep 20 20:35 ~1      │ ◇ legacy.go          673 B  −     │  28    id := chi.URLParam(r, "id")
   ┄ 2 unchanged ┄    │ ◇ main.go             1.3K  ~     │  29    user, err := store.GetUser(ctx, …  ◂
 Sep 13 20:51 ~2      │ ◇ server.go          845 B  +     │  30    cfg.Port = envInt("PORT", 8080)
   ┄ 1 unchanged ┄    │ ◇ util.go            823 B        │  31  }
 Sep 09 19:23 ~1      │                                   │  32
   ┄ 1 unchanged ┄    │                                   │  33  func handleOrder(ctx context.Conte…
▶Sep 06 18:03 +1~3−1  │                                   │  34    var req CreateOrderRequest         ◂
   ┄ 2 unchanged ┄    │                                   │  35    return nil
 Sep 02 20:16 ~1      │                                   │  36+   user, err := store.GetUser(ctx, …
   ┄ 5 unchanged ┄    │                                   │  37    metrics.Requests.WithLabelValues…
 Aug 22 11:59 ~2      │                                   │  38    cfg.Port = envInt("PORT", 8080)
   ┄ 1 unchanged ┄    │                                   │  39  }
 Aug 17 16:04 +1~1    │                                   │  40
   ┄ 3 unchanged ┄    │                                   │  41  func handleUser(ctx context.Contex…
 Aug 10 15:44 ~1      │                                   │  42    if err := validate(req); err != …
   ┄ 1 unchanged ┄    │                                   │  43    ctx, cancel := context.WithTimeo…
 Aug 07 08:15 +1      │                                   │  44    w.Header().Set("Content-Type", "…
   ┄ 1 unchanged ┄    │                                   │  45    log.Printf("starting %s", name)
 Aug 03 16:19 +1~1    │                                   │  46+   if len(args) == 0 { return errNo…
   ┄ 2 unchanged ┄    │                                   │  47    defer cancel()
 Jul 28 15:59 ~1      │                                   │  48+   for i := 0; i < retries; i++ { t…
 Jul 26 11:57 ~1      │                                   │  49  }
 Jul 24 12:16 ~1      │                                   │  50
                      │                                   │
 NOR  Sep 06 18:03  9c3a7b4f  src/ +1 ~3 −1  2 in column · zi             Sep 06 16:34  5/7  ? help
```

| Rows | Content |
|---|---|
| 0 | Header: `restoric` and the clickable breadcrumb. While indexing, `indexing 3/16` on the right |
| 1 | Timeline labels: months, or dates and day numbers when zoomed, plus `on disk` at the right end |
| 2 | **The row of dots, one per snapshot column.** `●` = the selected entry changed. `○` = the current folder changed but the selected entry didn't. `·` = neither changed. Blank = the entry didn't exist (and the folder didn't change). With nothing selected (`..`), `●` marks the folder's changes. Highlighted cell = the snapshot being viewed. The cell under `now` uses the same symbols for changes on disk since the newest snapshot. **Labels at the right edge:** the entry, then `○` and the folder (`main.go  ○ src/`). |
| 3 | `▲` under the viewed snapshot · zoom control `− 1× +` |
| 4 | Blank |
| 5–32 | Three columns, separated by thin `│` lines: **Versions** (22 wide) · **listing** (40% of the rest, 40 to 50 wide) · **preview** (the rest) |
| 33 | **Status bar** (or the `:` / `/` / `f` input line) |

**Versions column:** the top row is `on disk`, showing what changed on disk since the last backup (`on disk  ~1`, in purple; `= latest` when nothing has). After that come the change points, newest first, with compact counts (`+1~3−1`). Unchanged runs fold into `┄ 5 unchanged ┄`. `▶` marks the current version, or the folded run when you're viewing an unchanged snapshot. Clicking a row jumps there.

**Listing:** mark bar (`┃` for selected items), icon, name (folders end in `/`, a deleted name is crossed out), size, Δ (`+` added, `~` changed, `−` deleted in this snapshot, compact counts for folders, `gone` for items deleted earlier; the Δ column is as wide as its widest visible entry). No column headers. With items deleted earlier hidden, the bottom says `1 deleted · . to show`.

**Preview** of the selected entry *as it was in this snapshot*:
- **File:** a heading (`main.go · v5/6 · 1.3K`) and a second line saying whether it changed here ("changed here · + new line · − removed", "new in this snapshot", or "unchanged since Sep 09 19:23"). Below that, the content with line numbers. The margin marks `+` for lines that are new in this version and `−` where lines were removed, compared with the previous version. It scrolls to the first change automatically.
- `⇥` (Tab) switches between **content** and **vs disk** (an inline diff against the file on disk). `J`/`K` scroll. The mode shows at the top right and is clickable.
- **Folder:** its contents in that snapshot, with icons and change markers.
- **Deleted item:** the last version, headed "deleted · last version Aug 22, gone Sep 06".
- `..`: the parent folder's path.

**Status bar:** mode badge (`NOR`; `SEL` when items are selected; `VIS` in visual mode; `DIFF`, `FIND`, `RST` in those screens), then date · snapshot id · folder counts (or "unchanged since Sep 02"), then extra notes ("2 in column · zi", `filter "go"`, "2 yanked"). On the right: the selected file's modification time, position (`5/7`), any pending count or prefix key, and `? help`. While a restore runs, the middle shows its progress instead, in every view: `restoring 2/5 src/  ━━━━━━──────  35%  1.2M / 3.4M  esc stop` (§3.11).

**Messages** appear as a small rounded popup at the top right of the panes, like yazi's notifications, and disappear at the next key press.

### 3.2 The selected entry on the timeline
After `}`, the view jumps to the next change of `main.go`. A file changing means its folder changed too, so one row holds both: `●` where the entry changed, `○` where only the folder did. This used to be two rows, the folder's and the entry's. The folder's row only repeated the Versions column, so they were merged (§8).

<sub>`docs/screens/02-file-track.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ○·○ ○ ●  · ● · ○· ○·  ··○  · ●· · · · ·○·  ● ● · ○ · · ○ · · ○  · ·  ┊ ● main.go  ○ src/
                                                      ▲                                    − 1× +

 on disk  ~1          │   ..                              │ main.go · v6/6 · 1.3K        ⇥ content
   ┄ 2 unchanged ┄    │ ▸ api/                            │ changed here · + new line · − removed
 Sep 27 08:52 ~1      │ ▸ models/                         │
   ┄ 2 unchanged ┄    │ ◇ config.go          737 B        │  26    return json.NewEncoder(w).Encode…
 Sep 20 20:35 ~1      │ ◇ main.go             1.3K  ~     │  27    var req CreateOrderRequest         ◂
   ┄ 2 unchanged ┄    │ ◇ server.go          845 B        │  28    id := chi.URLParam(r, "id")
 Sep 13 20:51 ~2      │ ◇ util.go            823 B        │  29    user, err := store.GetUser(ctx, …
   ┄ 1 unchanged ┄    │                                   │  30    cfg.Port = envInt("PORT", 8080)
▶Sep 09 19:23 ~1      │                                   │  31  }                                    ◂
   ┄ 1 unchanged ┄    │                                   │  32
 Sep 06 18:03 +1~3−1  │                                   │  33  func handleOrder(ctx context.Conte…
   ┄ 2 unchanged ┄    │                                   │  34    var req CreateOrderRequest
 Sep 02 20:16 ~1      │                                   │  35    return nil
   ┄ 5 unchanged ┄    │                                   │  36−   metrics.Requests.WithLabelValues…
 Aug 22 11:59 ~2      │                                   │  37    cfg.Port = envInt("PORT", 8080)
   ┄ 1 unchanged ┄    │                                   │  38  }
 Aug 17 16:04 +1~1    │                                   │  39
   ┄ 3 unchanged ┄    │                                   │  40  func handleUser(ctx context.Contex…
 Aug 10 15:44 ~1      │                                   │  41    if err := validate(req); err != …
   ┄ 1 unchanged ┄    │                                   │  42    ctx, cancel := context.WithTimeo…
 Aug 07 08:15 +1      │                                   │  43    w.Header().Set("Content-Type", "…
   ┄ 1 unchanged ┄    │                                   │  44    log.Printf("starting %s", name)
 Aug 03 16:19 +1~1    │                                   │  45    if len(args) == 0 { return errNo…
   ┄ 2 unchanged ┄    │                                   │  46    defer cancel()
 Jul 28 15:59 ~1      │                                   │  47    for i := 0; i < retries; i++ { t…
 Jul 26 11:57 ~1      │                                   │  48  }
 Jul 24 12:16 ~1      │   1 deleted · . to show           │  49
                      │                                   │
 NOR  Sep 09 19:23  62c0e9c7  src/ ~1                                     Sep 09 18:25  4/6  ? help
```

### 3.3 Preview: diff against disk
`⇥` switches the preview to an inline diff against the file on disk:

<sub>`docs/screens/03-preview-diff.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ○·○ ○ ●  · ● · ○· ○·  ··○  · ●· · · · ·○·  ● ● · ○ · · ○ · · ○  · ·  ┊ ● main.go  ○ src/
                                                      ▲                                    − 1× +

 on disk  ~1          │   ..                              │ main.go · v6/6 · 1.3K        ⇥ vs disk
   ┄ 2 unchanged ┄    │ ▸ api/                            │ changed here · + new line · − removed
 Sep 27 08:52 ~1      │ ▸ models/                         │
   ┄ 2 unchanged ┄    │ ◇ config.go          737 B        │ ┄ line 18
 Sep 20 20:35 ~1      │ ◇ main.go             1.3K  ~     │     if err := validate(req); err != nil…  ◂
   ┄ 2 unchanged ┄    │ ◇ server.go          845 B        │     if err := validate(req); err != nil…
 Sep 13 20:51 ~2      │ ◇ util.go            823 B        │     if err := validate(req); err != nil…
   ┄ 1 unchanged ┄    │                                   │ -   return nil
▶Sep 09 19:23 ~1      │                                   │ +   signal.Notify(stop, os.Interrupt, s…  ◂
   ┄ 1 unchanged ┄    │                                   │     return nil
 Sep 06 18:03 +1~3−1  │                                   │   }
   ┄ 2 unchanged ┄    │                                   │
 Sep 02 20:16 ~1      │                                   │
   ┄ 5 unchanged ┄    │                                   │
 Aug 22 11:59 ~2      │                                   │
   ┄ 1 unchanged ┄    │                                   │
 Aug 17 16:04 +1~1    │                                   │
   ┄ 3 unchanged ┄    │                                   │
 Aug 10 15:44 ~1      │                                   │
   ┄ 1 unchanged ┄    │                                   │
 Aug 07 08:15 +1      │                                   │
   ┄ 1 unchanged ┄    │                                   │
 Aug 03 16:19 +1~1    │                                   │
   ┄ 2 unchanged ┄    │                                   │
 Jul 28 15:59 ~1      │                                   │
 Jul 26 11:57 ~1      │                                   │
 Jul 24 12:16 ~1      │   1 deleted · . to show           │
                      │                                   │
 NOR  Sep 09 19:23  62c0e9c7  src/ ~1                                     Sep 09 18:25  4/6  ? help
```

### 3.4 Which-key popups
Pressing a prefix key (`g`, `z`, `c`, and `]` `[` in the diff) shows what can follow. Entries are clickable. The pending key also shows in the status bar.

<sub>`docs/screens/04-which-key.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ○·○ ○ ●  · ● · ○· ○·  ··○  · ●· · · · ·○·  ● ● · ○ · · ○ · · ○  · ·  ┊ ● main.go  ○ src/
                                                      ▲                                    − 1× +

 on disk  ~1          │   ..                              │ main.go · v6/6 · 1.3K        ⇥ content
   ┄ 2 unchanged ┄    │ ▸ api/                            │ changed here · + new line · − removed
 Sep 27 08:52 ~1      │ ▸ models/                         │
   ┄ 2 unchanged ┄    │ ◇ config.go          737 B        │  26    return json.NewEncoder(w).Encode…
 Sep 20 20:35 ~1      │ ◇ main.go             1.3K  ~     │  27    var req CreateOrderRequest         ◂
   ┄ 2 unchanged ┄    │ ◇ server.go          845 B        │  28    id := chi.URLParam(r, "id")
 Sep 13 20:51 ~2      │ ◇ util.go            823 B        │  29    user, err := store.GetUser(ctx, …
   ┄ 1 unchanged ┄    │                                   │  30    cfg.Port = envInt("PORT", 8080)
▶Sep 09 19:23 ~1      │                                   │  31  }                                    ◂
   ┄ 1 unchanged ┄    │                                   │  32
 Sep 06 18:03 +1~3−1  │                                   │  33  func handleOrder(ctx context.Conte…
   ┄ 2 unchanged ┄    │                                   │  34    var req CreateOrderRequest
 Sep 02 20:16 ~1      │                                   │  35    return nil
   ┄ 5 unchanged ┄    │                                   │  36−   metrics.Requests.WithLabelValues…
 Aug 22 11:59 ~2      │                                   │  37    cfg.Port = envInt("PORT", 8080)
   ┄ 1 unchanged ┄    │                                   │  38  }
 Aug 17 16:04 +1~1    │                                   │  39
   ┄ 3 unchanged ┄    │                                   │  40  func handleUser(ctx context.Contex…
 Aug 10 15:44 ~1      │                                   │  41    if err := validate(req); err != …
   ┄ 1 unchanged ┄    │                                   │  42    ctx, cancel := context.WithTimeo…
 Aug 07 08:15 +1      │                                   │  43    w.Header().Set("Content-Type", "…
   ┄ 1 unchanged ┄    │                                   │  44  ╭─ z ────────────────────────────╮
 Aug 03 16:19 +1~1    │                                   │  45  │ zh   show / hide deleted       │…
   ┄ 2 unchanged ┄    │                                   │  46  │ zi   zoom timeline in          │
 Jul 28 15:59 ~1      │                                   │  47  │ zo   zoom timeline out         │…
 Jul 26 11:57 ~1      │                                   │  48  ╰────────────────────────────────╯
 Jul 24 12:16 ~1      │   1 deleted · . to show           │  49
                      │                                   │
 NOR  Sep 09 19:23  62c0e9c7  src/ ~1                                  z  Sep 09 18:25  4/6  ? help
```

### 3.5 Select, yank, paste
Restoring works like yazi's copy and paste, out of the past.
- `Space` toggles selection and moves down. `v` starts visual mode (a range); pressing `v` again keeps the range selected. `esc` leaves visual mode, then clears the selection.
- `y` yanks the selection (or the item under the cursor) from **this snapshot**.
- `p` restores the yanked items **next to the originals** (`name.2026-09-09_1923`, then `-2`, `-3` if that exists), or to their original place if they're missing on disk.
- `P` **overwrites** the files on disk, after a confirmation. The current files are moved to the undo folder first, so `:undo` can put them back.

<sub>`docs/screens/05-selection.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ○      ○ ○ ○ ○    ○   ○  ○     ○    ○         ○   ● ○ · ○ · · ○ · · ○  · ·  ┊ ○ server.go  ○ src/
                                                      ▲                                    − 1× +

 on disk  ~1          │   ..                              │ server.go · v1/1 · 845 B     ⇥ content
   ┄ 2 unchanged ┄    │ ▸ api/                            │ unchanged since Sep 06 18:03
 Sep 27 08:52 ~1      │ ▸ models/                         │
   ┄ 2 unchanged ┄    │┃◇ config.go          737 B        │   1  package main
 Sep 20 20:35 ~1      │┃◇ main.go             1.3K  ~     │   2
   ┄ 2 unchanged ┄    │ ◇ server.go          845 B        │   3  import (                             ◂
 Sep 13 20:51 ~2      │ ◇ util.go            823 B        │   4    "context"
   ┄ 1 unchanged ┄    │                                   │   5    "fmt"
▶Sep 09 19:23 ~1      │                                   │   6    "net/http"                         ◂
   ┄ 1 unchanged ┄    │                                   │   7  )
 Sep 06 18:03 +1~3−1  │                                   │   8
   ┄ 2 unchanged ┄    │                                   │   9  func mustEnv(ctx context.Context) …
 Sep 02 20:16 ~1      │                                   │  10    if len(args) == 0 { return errNo…
   ┄ 5 unchanged ┄    │                                   │  11    id := chi.URLParam(r, "id")
 Aug 22 11:59 ~2      │                                   │  12    log.Printf("starting %s", name)
   ┄ 1 unchanged ┄    │                                   │  13  }
 Aug 17 16:04 +1~1    │                                   │  14
   ┄ 3 unchanged ┄    │                                   │  15  func readSecret(ctx context.Contex…
 Aug 10 15:44 ~1      │                                   │  16    if len(args) == 0 { return errNo…
   ┄ 1 unchanged ┄    │                                   │  17    if len(args) == 0 { return errNo…
 Aug 07 08:15 +1      │                                   │  18    ctx, cancel := context.WithTimeo…
   ┄ 1 unchanged ┄    │                                   │  19  }
 Aug 03 16:19 +1~1    │                                   │  20
   ┄ 2 unchanged ┄    │                                   │  21  func startServer(ctx context.Conte…
 Jul 28 15:59 ~1      │                                   │  22    srv := &http.Server{Addr: cfg.Ad…
 Jul 26 11:57 ~1      │                                   │  23    defer cancel()
 Jul 24 12:16 ~1      │   1 deleted · . to show           │  24    for i := 0; i < retries; i++ { t…
                      │                                   │
 SEL  Sep 09 19:23  62c0e9c7  src/ ~1                                     Sep 06 15:17  5/6  ? help
```

<sub>`docs/screens/06-yank-toast.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ○      ○ ○ ○ ○    ○   ○  ○     ○    ○         ○   ● ○ · ○ · · ○ · · ○  · ·  ┊ ○ server.go  ○ src/
                                                      ▲                                    − 1× +

 on disk  ~1          │   ..                     ╭────────────────────────────────────────────────╮
   ┄ 2 unchanged ┄    │ ▸ api/                   │ Yanked 2 items from Sep 09 19:23. p restores   │
 Sep 27 08:52 ~1      │ ▸ models/                │ next to the original, P overwrites.            │
   ┄ 2 unchanged ┄    │ ◇ config.go          737 ╰────────────────────────────────────────────────╯
 Sep 20 20:35 ~1      │ ◇ main.go             1.3K  ~     │   2
   ┄ 2 unchanged ┄    │ ◇ server.go          845 B        │   3  import (                             ◂
 Sep 13 20:51 ~2      │ ◇ util.go            823 B        │   4    "context"
   ┄ 1 unchanged ┄    │                                   │   5    "fmt"
▶Sep 09 19:23 ~1      │                                   │   6    "net/http"                         ◂
   ┄ 1 unchanged ┄    │                                   │   7  )
 Sep 06 18:03 +1~3−1  │                                   │   8
   ┄ 2 unchanged ┄    │                                   │   9  func mustEnv(ctx context.Context) …
 Sep 02 20:16 ~1      │                                   │  10    if len(args) == 0 { return errNo…
   ┄ 5 unchanged ┄    │                                   │  11    id := chi.URLParam(r, "id")
 Aug 22 11:59 ~2      │                                   │  12    log.Printf("starting %s", name)
   ┄ 1 unchanged ┄    │                                   │  13  }
 Aug 17 16:04 +1~1    │                                   │  14
   ┄ 3 unchanged ┄    │                                   │  15  func readSecret(ctx context.Contex…
 Aug 10 15:44 ~1      │                                   │  16    if len(args) == 0 { return errNo…
   ┄ 1 unchanged ┄    │                                   │  17    if len(args) == 0 { return errNo…
 Aug 07 08:15 +1      │                                   │  18    ctx, cancel := context.WithTimeo…
   ┄ 1 unchanged ┄    │                                   │  19  }
 Aug 03 16:19 +1~1    │                                   │  20
   ┄ 2 unchanged ┄    │                                   │  21  func startServer(ctx context.Conte…
 Jul 28 15:59 ~1      │                                   │  22    srv := &http.Server{Addr: cfg.Ad…
 Jul 26 11:57 ~1      │                                   │  23    defer cancel()
 Jul 24 12:16 ~1      │   1 deleted · . to show           │  24    for i := 0; i < retries; i++ { t…
                      │                                   │
 NOR  Sep 09 19:23  62c0e9c7  src/ ~1  2 yanked                           Sep 06 15:17  5/6  ? help
```

<sub>`docs/screens/07-confirm-overwrite.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ○      ○ ○ ○ ○    ○   ○  ○     ○    ○         ○   ● ○ · ○ · · ○ · · ○  · ·  ┊ ○ server.go  ○ src/
                                                      ▲                                    − 1× +

 on disk  ~1          │   ..                              │ server.go · v1/1 · 845 B     ⇥ content
   ┄ 2 unchanged ┄    │ ▸ api/                            │ unchanged since Sep 06 18:03
 Sep 27 08:52 ~1      │ ▸ models/                         │
   ┄ 2 unchanged ┄    │ ◇ config.go          737 B        │   1  package main
 Sep 20 20:35 ~1      │ ◇ main.go             1.3K  ~     │   2
   ┄ 2 unchanged ┄    ╭─ Overwrite? ─────────────────────────────────────────╮
 Sep 13 20:51 ~2      │                                                      │
   ┄ 1 unchanged ┄    │  Replace 2 items on disk with the version from Sep   │
▶Sep 09 19:23 ~1      │  09 19:23? The current version is kept for :undo.    │
   ┄ 1 unchanged ┄    │                                                      │
 Sep 06 18:03 +1~3−1  │  [y] Overwrite   [n] Cancel                          │
   ┄ 2 unchanged ┄    ╰──────────────────────────────────────────────────────╯ctx context.Context) …
 Sep 02 20:16 ~1      │                                   │  10    if len(args) == 0 { return errNo…
   ┄ 5 unchanged ┄    │                                   │  11    id := chi.URLParam(r, "id")
 Aug 22 11:59 ~2      │                                   │  12    log.Printf("starting %s", name)
   ┄ 1 unchanged ┄    │                                   │  13  }
 Aug 17 16:04 +1~1    │                                   │  14
   ┄ 3 unchanged ┄    │                                   │  15  func readSecret(ctx context.Contex…
 Aug 10 15:44 ~1      │                                   │  16    if len(args) == 0 { return errNo…
   ┄ 1 unchanged ┄    │                                   │  17    if len(args) == 0 { return errNo…
 Aug 07 08:15 +1      │                                   │  18    ctx, cancel := context.WithTimeo…
   ┄ 1 unchanged ┄    │                                   │  19  }
 Aug 03 16:19 +1~1    │                                   │  20
   ┄ 2 unchanged ┄    │                                   │  21  func startServer(ctx context.Conte…
 Jul 28 15:59 ~1      │                                   │  22    srv := &http.Server{Addr: cfg.Ad…
 Jul 26 11:57 ~1      │                                   │  23    defer cancel()
 Jul 24 12:16 ~1      │   1 deleted · . to show           │  24    for i := 0; i < retries; i++ { t…
                      │                                   │
 NOR  Sep 09 19:23  62c0e9c7  src/ ~1  2 yanked                           Sep 06 15:17  5/6  ? help
```

### 3.6 Command line and find
`:` opens the command line, and `s` opens it with `find ` already typed. Hints show at the right.

<sub>`docs/screens/08-command-line.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ○·○ ○ ○  · ○ · ○· ○·  ··○  · ○· · · · ·○·  ○ ○ · ○ · · ○ · · ○  · ·  ┊ ○ util.go  ○ src/
                                                ▲                                          − 1× +

 on disk  ~1          │   ..                              │ util.go · v1/1 · 823 B       ⇥ content
   ┄ 2 unchanged ┄    │ ▸ api/                      ~1    │ unchanged since Jul 14 09:00
 Sep 27 08:52 ~1      │ ▸ models/                         │
   ┄ 2 unchanged ┄    │ ◇ config.go          634 B        │   1  package main
 Sep 20 20:35 ~1      │ ◇ legacy.go          673 B        │   2
   ┄ 2 unchanged ┄    │ ◇ main.go             1.2K        │   3  import (
 Sep 13 20:51 ~2      │ ◇ util.go            823 B        │   4    "context"                          ◂
   ┄ 1 unchanged ┄    │                                   │   5    "fmt"
 Sep 09 19:23 ~1      │                                   │   6    "net/http"
   ┄ 1 unchanged ┄    │                                   │   7  )
 Sep 06 18:03 +1~3−1  │                                   │   8
   ┄ 2 unchanged ┄    │                                   │   9  func retry(ctx context.Context) er…
▶Sep 02 20:16 ~1      │                                   │  10    b, err := os.ReadFile(path)        ◂
   ┄ 5 unchanged ┄    │                                   │  11    for i := 0; i < retries; i++ { t…
 Aug 22 11:59 ~2      │                                   │  12    cache.Set(key, value, 10*time.Mi…
   ┄ 1 unchanged ┄    │                                   │  13    conn, err := sql.Open("postgres"…
 Aug 17 16:04 +1~1    │                                   │  14  }
   ┄ 3 unchanged ┄    │                                   │  15
 Aug 10 15:44 ~1      │                                   │  16  func loadConfig(ctx context.Contex…
   ┄ 1 unchanged ┄    │                                   │  17    if err := validate(req); err != …
 Aug 07 08:15 +1      │                                   │  18    for i := 0; i < retries; i++ { t…
   ┄ 1 unchanged ┄    │                                   │  19    srv := &http.Server{Addr: cfg.Ad…
 Aug 03 16:19 +1~1    │                                   │  20    return nil
   ┄ 2 unchanged ┄    │                                   │  21  }
 Jul 28 15:59 ~1      │                                   │  22
 Jul 26 11:57 ~1      │                                   │  23  func mustEnv(ctx context.Context) …
 Jul 24 12:16 ~1      │                                   │  24    total := order.Subtotal() + orde…
                      │                                   │
 :find legacy█                               sep 1 · 2026-09-01 · yesterday · 3d · find NAME · undo
```

`:find NAME` searches every snapshot by path. It answers "where did this file go?"

How it works (M6): the search starts at the backup root and matches paths relative to it, ignoring case and Unicode form (NFC/NFD). When a folder matches, its contents aren't listed again. Matches are memoised by (folder path, tree id), so an unchanged folder is read once, and the cost follows the number of distinct trees. It runs in the background; the header shows `searching 120/430` and partial results arrive as it goes.

<sub>`docs/screens/09-find.txt`</sub>

```text
 restoric  Find  "legacy"                                                   1 match in 43 snapshots

  PATH                                                  FIRST SEEN    LAST SEEN     NOW
▶ src/legacy.go                                         Jul 14 09:00  Sep 04 08:08  gone Sep 06       ◂





























 FIND  ⏎ jumps to the last snapshot that has it                                         1/1  ? help
```

`⏎` jumps to the **last snapshot that still had it**, in its folder, with it selected:

<sub>`docs/screens/10-find-jump.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ○·○ ● ○  · ○ · ○· ○·  ··○  · ●· · · · ·○·  ● ○   ○     ○     ○       ┊ ○ legacy.go  ○ src/
                                                 ▲                                         − 1× +

 on disk  ~1          │   ..                     ╭────────────────────────────────────────────────╮
   ┄ 2 unchanged ┄    │ ▸ api/                   │ Jumped to Sep 04 08:08, the last snapshot that │
 Sep 27 08:52 ~1      │ ▸ models/                │ has legacy.go                                  │
   ┄ 2 unchanged ┄    │ ◇ config.go          634 ╰────────────────────────────────────────────────╯
 Sep 20 20:35 ~1      │ ◇ legacy.go          673 B        │   5    "fmt"                              ◂
   ┄ 2 unchanged ┄    │ ◇ main.go             1.2K        │   6    "net/http"
 Sep 13 20:51 ~2      │ ◇ util.go            823 B        │   7  )
   ┄ 1 unchanged ┄    │                                   │   8
 Sep 09 19:23 ~1      │                                   │   9  func flushCache(ctx context.Contex…
   ┄ 1 unchanged ┄    │                                   │  10    ctx, cancel := context.WithTimeo…
 Sep 06 18:03 +1~3−1  │                                   │  11    user, err := store.GetUser(ctx, …
▶  ┄ 2 unchanged ┄    │                                   │  12    for i := 0; i < retries; i++ { t…
 Sep 02 20:16 ~1      │                                   │  13    user, err := store.GetUser(ctx, …
   ┄ 5 unchanged ┄    │                                   │  14  }
 Aug 22 11:59 ~2      │                                   │  15
   ┄ 1 unchanged ┄    │                                   │  16  func authMiddleware(ctx context.Co…
 Aug 17 16:04 +1~1    │                                   │  17    cache.Set(key, value, 10*time.Mi…
   ┄ 3 unchanged ┄    │                                   │  18    for i := 0; i < retries; i++ { t…
 Aug 10 15:44 ~1      │                                   │  19    b, err := os.ReadFile(path)
   ┄ 1 unchanged ┄    │                                   │  20  }
 Aug 07 08:15 +1      │                                   │  21
   ┄ 1 unchanged ┄    │                                   │  22  func migrate(ctx context.Context) …
 Aug 03 16:19 +1~1    │                                   │  23    ctx, cancel := context.WithTimeo…
   ┄ 2 unchanged ┄    │                                   │  24    if err := validate(req); err != …
 Jul 28 15:59 ~1      │                                   │  25+   signal.Notify(stop, os.Interrupt…
 Jul 26 11:57 ~1      │                                   │  26  }
 Jul 24 12:16 ~1      │                                   │  27
                      │                                   │
 NOR  Sep 04 08:08  c005272e  unchanged since Sep 02  2 in column · zi    Aug 22 09:20  4/6  ? help
```

### 3.7 Deleted items
`.` (or `zh`) shows items deleted earlier, marked `gone` in Δ and in italics. The timeline shows the item's history: dots up to the deletion, then only the folder's `○`. The preview shows the last version. `⏎`/`l` on a deleted folder jumps to the last snapshot that had it.

<sub>`docs/screens/11-deleted-shown.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ○·○ ● ○  · ○ · ○· ○·  ··○  · ●· · · · ·○·  ● ○   ○     ○     ○       ┊ ○ legacy.go  ○ src/
                                                          ▲                                − 1× +

 on disk  ~1          │   ..                              │ legacy.go · deleted          ⇥ content
   ┄ 2 unchanged ┄    │ ▸ api/                      ~1    │ last version Aug 22, gone Sep 06
 Sep 27 08:52 ~1      │ ▸ models/                         │
   ┄ 2 unchanged ┄    │ ◇ config.go          789 B  ~     │   4    "context"
 Sep 20 20:35 ~1      │ ◇ legacy.go          673 B  gone  │   5    "fmt"                              ◂
   ┄ 2 unchanged ┄    │ ◇ main.go             1.3K        │   6    "net/http"
▶Sep 13 20:51 ~2      │ ◇ server.go          845 B        │   7  )                                    ◂
   ┄ 1 unchanged ┄    │ ◇ util.go            823 B        │   8
 Sep 09 19:23 ~1      │                                   │   9  func flushCache(ctx context.Contex…
   ┄ 1 unchanged ┄    │                                   │  10    ctx, cancel := context.WithTimeo…
 Sep 06 18:03 +1~3−1  │                                   │  11    user, err := store.GetUser(ctx, …
   ┄ 2 unchanged ┄    │                                   │  12    for i := 0; i < retries; i++ { t…
 Sep 02 20:16 ~1      │                                   │  13    user, err := store.GetUser(ctx, …
   ┄ 5 unchanged ┄    │                                   │  14  }
 Aug 22 11:59 ~2      │                                   │  15
   ┄ 1 unchanged ┄    │                                   │  16  func authMiddleware(ctx context.Co…
 Aug 17 16:04 +1~1    │                                   │  17    cache.Set(key, value, 10*time.Mi…
   ┄ 3 unchanged ┄    │                                   │  18    for i := 0; i < retries; i++ { t…
 Aug 10 15:44 ~1      │                                   │  19    b, err := os.ReadFile(path)
   ┄ 1 unchanged ┄    │                                   │  20  }
 Aug 07 08:15 +1      │                                   │  21
   ┄ 1 unchanged ┄    │                                   │  22  func migrate(ctx context.Context) …
 Aug 03 16:19 +1~1    │                                   │  23    ctx, cancel := context.WithTimeo…
   ┄ 2 unchanged ┄    │                                   │  24    if err := validate(req); err != …
 Jul 28 15:59 ~1      │                                   │  25+   signal.Notify(stop, os.Interrupt…
 Jul 26 11:57 ~1      │                                   │  26  }
 Jul 24 12:16 ~1      │                                   │  27
                      │                                   │
 NOR  Sep 13 20:51  25e5c7c8  src/ ~2  2 yanked                                         4/7  ? help
```

### 3.8 Filter
`f` filters the listing as you type (yazi's `f`). `⏎` keeps the filter, and the status bar shows it. `esc` clears it. Changing folder clears it too.

<sub>`docs/screens/12-filter.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ○      ○ ○ ○ ○    ● · ○· ○·  ··○  · ○· · · · ·○·  ● ○ · ● · · ○ · · ○  · ·  ┊ ○ config.go  ○ src/
                                                                           ▲               − 1× +

 on disk  ~1          │   ..                              │ config.go · v3/3 · 789 B     ⇥ content
▶  ┄ 2 unchanged ┄    │ ◇ config.go          789 B        │ unchanged since Sep 13 20:51              ◂
 Sep 27 08:52 ~1      │ ◇ main.go             1.3K        │
   ┄ 2 unchanged ┄    │ ◇ server.go          845 B        │   9  func retry(ctx context.Context) er…
 Sep 20 20:35 ~1      │ ◇ util.go            823 B        │  10    total := order.Subtotal() + orde…
   ┄ 2 unchanged ┄    │                                   │  11    ctx, cancel := context.WithTimeo…
 Sep 13 20:51 ~2      │                                   │  12    log.Printf("starting %s", name)
   ┄ 1 unchanged ┄    │                                   │  13    if len(args) == 0 { return errNo…
 Sep 09 19:23 ~1      │                                   │  14  }
   ┄ 1 unchanged ┄    │                                   │  15
 Sep 06 18:03 +1~3−1  │                                   │  16  func migrate(ctx context.Context) …
   ┄ 2 unchanged ┄    │                                   │  17    b, err := os.ReadFile(path)
 Sep 02 20:16 ~1      │                                   │  18    id := chi.URLParam(r, "id")
   ┄ 5 unchanged ┄    │                                   │  19+   signal.Notify(stop, os.Interrupt…
 Aug 22 11:59 ~2      │                                   │  20    if err := validate(req); err != …
   ┄ 1 unchanged ┄    │                                   │  21    var req CreateOrderRequest
 Aug 17 16:04 +1~1    │                                   │  22  }
   ┄ 3 unchanged ┄    │                                   │  23
 Aug 10 15:44 ~1      │                                   │  24  func startServer(ctx context.Conte…
   ┄ 1 unchanged ┄    │                                   │  25    var req CreateOrderRequest
 Aug 07 08:15 +1      │                                   │  26    w.Header().Set("Content-Type", "…
   ┄ 1 unchanged ┄    │                                   │  27    id := chi.URLParam(r, "id")
 Aug 03 16:19 +1~1    │                                   │  28    slog.Info("request done", "statu…
   ┄ 2 unchanged ┄    │                                   │  29    cache.Set(key, value, 10*time.Mi…
 Jul 28 15:59 ~1      │                                   │  30    if len(args) == 0 { return errNo…
 Jul 26 11:57 ~1      │                                   │  31  }
 Jul 24 12:16 ~1      │   1 deleted · . to show           │  32
                      │                                   │
 filter: go█                                                    type to filter · ⏎ keep · esc clear
```

### 3.9 Versions of a file (`⏎` or `l` on a file)
One row per **distinct version**, newest first, with deleted periods as their own rows. The `on disk` row is always first. "VS DISK" shows lines added and removed, or `identical`. The preview on the right shows the selected version, with its changes marked.

<sub>`docs/screens/13-versions.txt`</sub>

```text
 restoric  Versions  ~/dev/project/src/main.go                           6 versions in 43 snapshots
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ··· · ●  · ● · ·· ··  ···  · ●· · · · ···  ● ● · · · · · · · ·  · ·  ┊ ● main.go
                                                      ▲                                    − 1× +

 VERSION         SIZE  SNAPSHOTS  VS DISK       │main.go · Sep 09 19:23                  ⇥ content
 on disk         1.3K                           │kept in 11 snapshots · 62c0e9c7
▶Sep 09 19:23    1.3K  11 snaps   +1 −1         │                                                     ◂
 Sep 06 18:03    1.3K  2 snaps    +1 −2         │ 25  func newRouter(ctx context.Context) error {
 Aug 22 11:59    1.2K  9 snaps    +3 −2         │ 26    return json.NewEncoder(w).Encode(resp)
 Aug 03 16:19    1.3K  10 snaps   +4 −4         │ 27    var req CreateOrderRequest
 Jul 28 15:59    1.3K  3 snaps    +4 −5         │ 28    id := chi.URLParam(r, "id")
 Jul 14 09:00    1.3K  8 snaps    +4 −4         │ 29    user, err := store.GetUser(ctx, id)
                                                │ 30    cfg.Port = envInt("PORT", 8080)
                                                │ 31  }
                                                │ 32
                                                │ 33  func handleOrder(ctx context.Context) error {
                                                │ 34    var req CreateOrderRequest
                                                │ 35    return nil
                                                │ 36−   metrics.Requests.WithLabelValues(r.Method).…
                                                │ 37    cfg.Port = envInt("PORT", 8080)
                                                │ 38  }
                                                │ 39
                                                │ 40  func handleUser(ctx context.Context) error {
                                                │ 41    if err := validate(req); err != nil { retur…
                                                │ 42    ctx, cancel := context.WithTimeout(ctx, 5*t…
                                                │ 43    w.Header().Set("Content-Type", "application…
                                                │ 44    log.Printf("starting %s", name)
                                                │ 45    if len(args) == 0 { return errNoArgs }
                                                │ 46    defer cancel()
                                                │ 47    for i := 0; i < retries; i++ { time.Sleep(b…
                                                │ 48  }
                                                │ 49
 NOR  ~/dev/project/src/main.go                                                         1/6  ? help
```

### 3.10 Full-screen diff (`d`)
Two modes. `c` (the default): selected version → on disk, "what changed since this version". `p`: previous distinct version → selected version, "what this version changed". Unified diff with 3 lines of context, both line numbers, and `┄┄ around line N ┄┄` between changes.

<sub>`docs/screens/14-diff.txt`</sub>

```text
 restoric  main.go  Aug 22 11:59  →  on disk                                                  +3 −2
 what changed since this version
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












 DIFF  vs disk (c)  ]c [c changes · H L versions                                       1/19  ? help
```

Binary files: show "binary file, 12.4K → 13.0K" instead of a diff. Files over 2 MB (configurable) are diffed only on request: the view says how big the file is, and `⏎` diffs it anyway (reading up to 256 MB). The preview reads at most 64 KB of each side. Both sides are read and diffed in the background. When every remaining change is already on screen, `]c` says "No more changes below." Line-number columns widen for files over 9 999 lines.

### 3.11 Restore options (`r`)
For targets other than "next to it" and "overwrite". Works on files and folders, from the folder view, the versions view and the diff.

<sub>`docs/screens/15-restore-dialog.txt`</sub>

```text
 restoric  main.go  Aug 22 11:59  →  on disk                                                  +3 −2
 what changed since this version
 ┄┄ around line 18 ┄┄
   18   18        if err := validate(req); err != nil { return err }
   19   19        if err := validate(req); err != nil { return err }
   20   20        if err := validate(req); err != nil { return err }
   21       -     return nil
        21  +     signal.Notify(stop, os.Interrupt, syscall.SIGTERM)
   22   22        return nil
   23   23 ╭─ Restore ──────────────────────────────────────────────────────────────────╮
   24   24 │                                                                            │
 ┄┄ around │  main.go  @ Aug 22 11:59  6b0f6953                                         │
   42   42 │  from /home/bege/dev/project/src/main.go                                   │
   43   43 │                                                                            │
   44   44 │  1 ( ) Overwrite original             ~/dev/project/src/main.go            │
   45      │  2 (•) Restore next to it             → main.go.2026-08-22_1159            │
        45 │  3 ( ) Restore to ~/Restored/         → ~/Restored/2026-08-22_1159/main.…  │
   46   46 │                                                                            │
        47 │                                                                            │
   47   48 │                                                                            │
   48   49 │                                                                            │
           │  [ Restore ]   [ Cancel ]                      j k choose  ⏎ restore  esc  │
           ╰────────────────────────────────────────────────────────────────────────────╯










 RST  choose where the restored copy goesersions                                       1/19  ? help
```

| # | Option | Behaviour |
|---|---|---|
| 1 | Overwrite original, or "Restore to original location" if it's missing on disk | **Asks for confirmation** when something exists on disk. Moves the current file to `~/.local/share/restoric/undo/<timestamp>/` first (`:undo`). Same as `P`. |
| 2 | Restore next to it (**default**) | `name.2026-09-06_1803` or `dir.2026-09-06_1803/`. Same as `p`. |
| 3 | Restore to `~/Restored/` | `~/Restored/<stamp>/<name>` |
| 4 | Folder only: write a tar archive | `name-<stamp>.tar` |

Showing a file in `$PAGER` is not a restore, so it isn't in the dialog: `o` does it from the folder, versions and diff views.

**Foreign host** (the host shown isn't one of this machine's, §3.18): there is no original on disk to overwrite or sit next to, so `p` and `r` both open one prompt in the status bar, `restore to: ~/dev/project/src█`, with the last directory used this session, else the folder restoric started in. `⏎` restores the item (or the yanked items) to `<dir>/<name>`, with `-2`, `-3`, … on a clash, and says "Restored to …"; `esc` cancels. `~` is expanded; a relative path is taken from the start folder. `P` says "Overwriting is off for another host's snapshots. p restores into a directory you choose." `:undo` is unchanged.

How it works (M5): restores run in the worker. `RusticRepo` uses rustic's restorer, which keeps mode, modification time and symlinks; owner and group are set only when running as root. Folder tar archives are written by restoric from the trees. An overwrite moves what's on disk into `undo/<time>/files/<absolute path>` with a `manifest.json`. If the restore fails, the old version is moved back. `:undo` undoes the newest overwrite, all of its items at once. Moves fall back to copy and delete across file systems. The dialog's "next to it" name comes without the `-2` a clash would add, because the UI doesn't look at the disk; the message after the restore gives the real name. The `:` line handles `:undo`, `:q`, `:help`, `:deleted`, `:latest`/`:now` and `:oldest`/`:first`; the rest of §3.15 comes in M6.

**Progress and stopping.** A restore runs in the background, like a copy in yazi; the UI stays usable. The status bar shows how far it has got (§3.1): `restoring config.go … preparing` while rustic works out what to copy, then a bar with the percentage and bytes of the item being copied, with `2/5` before the name when there are several items. Short of room, `esc stop` goes first, then the bar shrinks. A tar archive walks the folder's trees first to know its size.
- **One restore at a time.** `p`, `P`, `r` and `:undo` say "A restore is running · esc to stop" until it's done.
- **Stopping:** `esc` in the folder view, when it has nothing else to do (no visual mode, selection, search or filter), asks first (below); `:cancel` stops without asking. A restore stops between items, or before an item's contents are copied. Once rustic copies a folder's contents it can't be interrupted, so the status bar says `stopping after src/…` until it's done. A tar archive stops anywhere.
- **After stopping:** copies (next to it, `~/Restored`, tar) keep the finished items and remove the one that was partly written: "Stopped · restored 2 of 5" or "Stopped · nothing was restored". An overwrite is all or nothing: everything it replaced is put back and no undo step is left: "Stopped · nothing was overwritten".
- **Quitting** during a restore asks "A restore is running. Stop it and quit?"; `y` stops it, waits for the cleanup, then quits.
- A restore that fails cleans up the same way and shows the error. It doesn't resume; see `ISSUES.md`.

<sub>`docs/screens/18-stop-restore.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● ·   ·  ● ○ ○  · ●   ○  ○·    ● ·  ○ ·  ·    ●· ·●·● · ○ ·  ·○  · ·○ ·  ·  ┊ ● main.go  ○ src/
                                                      ▲                                    − 1× +

 on disk  ~1          │   ..                                   │ main.go · v7/7 · 108 B  ⇥ content
   ┄ 2 unchanged ┄    │ ▸ api/                                 │ changed here · + new line · − remo…
 Sep 27 08:52 ~1      │ ▸ models/                              │
   ┄ 2 unchanged ┄    │ ◇ config.go                    89 B    │   1  package main
 Sep 20 20:35 ~1      │ ◇ main.go                     108 B  ~ │   2
   ┄ 2 unchanged ┄    │ ◇ server.go                    30 B    │   3  func main() {
 Sep 13 20:51 ~2      ╭─ Stop restoring? ────────────────────────────────────╮
   ┄ 1 unchanged ┄    │                                                      │
▶Sep 09 19:23 ~1      │  config.go is 25% done. The partly restored copy is  │
   ┄ 1 unchanged ┄    │  removed.                                            │eware
 Sep 06 18:03 +1~3−1  │                                                      │re flag
   ┄ 2 unchanged ┄    │  [y] Stop   [n] Keep going                           │ls
 Sep 02 20:16 ~1      ╰──────────────────────────────────────────────────────╯
   ┄ 2 unchanged ┄    │                                        │  11+ // shutdown
 Aug 22 11:59 ~2−1    │                                        │
   ┄ 1 unchanged ┄    │                                        │
 Aug 17 16:04 +1~1    │                                        │
   ┄ 1 unchanged ┄    │                                        │
 Aug 10 15:44 ~1      │                                        │
 Aug 07 08:15 +1      │                                        │
 Aug 03 16:19 +1~1    │                                        │
   ┄ 1 unchanged ┄    │                                        │
 Jul 28 15:59 ~1      │                                        │
 Jul 26 11:57 ~1      │                                        │
 Jul 24 12:16 ~1      │                                        │
   ┄ 2 unchanged ┄    │                                        │
 Jul 14 09:00 +7      │                                        │
                      │   2 deleted · . to show                │
 NOR  restoring config.go  ━━━━────────────  25%  1 B / 4 B  esc stop     Sep 09 19:23  4/6  ? help
```

### 3.12 Help (`?` or `~`)
<sub>`docs/screens/16-help.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026        Aug                         Sep                              on disk
  ● · ·  ○·○ ○ ●  · ● · ○· ○·  ··○  · ●· · · · ·○·  ● ● · ○ · · ○ · · ○  · ·  ┊ ● main.go  ○ src/
                                                                           ▲               − 1× +
             ╭─ Help ─────────────────────────────────────────────────────────────────╮
 now  ~1 unsa│                                                                        │  ⇥ content
▶  ┄ 2 unchan│  Folder view                                                           │3
 Sep 27 08:52│  j k  gg G  C-d C-u     move, top, bottom, half page                   │
   ┄ 2 unchan│  h l  ← →  ⏎            parent / open (a file opens its versions)      │er(w).Encode…
 Sep 20 20:35│  H L                    older / newer change in this folder            │equest
   ┄ 2 unchan│  [ ]   { }              every snapshot / changes of the selected item  │, "id")
 Sep 13 20:51│  ⇥  J K                 preview: content or diff vs disk, scroll       │etUser(ctx, …
   ┄ 1 unchan│  ␣  v                   select / visual select                         │ORT", 8080)
 Sep 09 19:23│  y  p  P                yank, restore next to it, overwrite            │
   ┄ 1 unchan│  r  d  o                restore options / full-screen diff / $PAGER    │
 Sep 06 18:03│  cc cd cf               copy snapshot:path, folder, name               │ontext.Conte…
   ┄ 2 unchan│  / n N   f              search / next, previous / filter               │equest
 Sep 02 20:16│  .  zh   zi zo          show deleted items / zoom timeline             │
   ┄ 5 unchan│  gh   3H 5j             backup root / counts with motions              │hLabelValues…
 Aug 22 11:59│                                                                        │ORT", 8080)
   ┄ 1 unchan│  Commands                                                              │
 Aug 17 16:04│  :sep 1  :2026-09-01    jump to a date (:yesterday :3d :2w)            │
   ┄ 3 unchan│  :find NAME   s         search every snapshot for a name               │ntext.Contex…
 Aug 10 15:44│  :latest :oldest :undo                                                 │eq); err != …
   ┄ 1 unchan│                                                                        │xt.WithTimeo…
 Aug 07 08:15│  Diff  ]c [c or n N changes · c vs disk · p vs previous                │ent-Type", "…
   ┄ 1 unchan│  Press any key to close                                                │ %s", name)
 Aug 03 16:19╰────────────────────────────────────────────────────────────────────────╯return errNo…
   ┄ 2 unchanged ┄    │                                   │  46    defer cancel()
 Jul 28 15:59 ~1      │                                   │  47    for i := 0; i < retries; i++ { t…
 Jul 26 11:57 ~1      │                                   │  48  }
 Jul 24 12:16 ~1      │   1 deleted · . to show           │  49
                      │                                   │
 NOR  Oct 02 12:21  b41050ad  unchanged since Sep 27  2 yanked            Sep 09 18:25  4/6  ? help
```

### 3.13 Narrow terminals
At **under 100 columns** the Versions column folds away, leaving listing (40% of the width, 38 to 50) and preview. **Under 80 columns** the preview goes too. The timeline scales to the width.

<sub>`docs/screens/17-80-columns.txt`</sub>

```text
 restoric  ~/dev/project/src
  Jul 2026    Aug                 Sep                      on disk
  ●· · ○·○○●  ·● ·○·○· ··○ ·●·· ·· ○· ● ●· ○·· ○· · ○ ··  ┊ ● main.go  ○ src/
                                                       ▲               − 1× +

   ..                                 │ main.go · v6/6 · 1.3K        ⇥ content
 ▸ api/                               │ unchanged since Sep 09 19:23
 ▸ models/                            │
 ◇ config.go             789 B        │  26    return json.NewEncoder(w).Encode…
 ◇ main.go                1.3K        │  27    var req CreateOrderRequest         ◂
 ◇ server.go             845 B        │  28    id := chi.URLParam(r, "id")
 ◇ util.go               823 B        │  29    user, err := store.GetUser(ctx, …
                                      │  30    cfg.Port = envInt("PORT", 8080)
                                      │  31  }
                                      │  32
                                      │  33  func handleOrder(ctx context.Conte…
                                      │  34    var req CreateOrderRequest
                                      │  35    return nil
                                      │  36−   metrics.Requests.WithLabelValues…
                                      │  37    cfg.Port = envInt("PORT", 8080)
                                      │  38  }
                                      │  39
                                      │  40  func handleUser(ctx context.Contex…
                                      │  41    if err := validate(req); err != …
                                      │  42    ctx, cancel := context.WithTimeo…
                                      │  43    w.Header().Set("Content-Type", "…
                                      │  44    log.Printf("starting %s", name)
                                      │  45    if len(args) == 0 { return errNo…
                                      │  46    defer cancel()
                                      │  47    for i := 0; i < retries; i++ { t…
                                      │  48  }
   1 deleted · . to show              │  49
                                      │
 NOR  Oct 02 12:21  b41050ad  unchanged since Sep 27  Sep 09 18:25  4/6  ? help
```

### 3.14 Keymap

**Folder view**
| Keys | Action |
|---|---|
| `j` `k` / `↓` `↑` | Move |
| `gg` `G`, `Ctrl-d` `Ctrl-u`, `PgDn` `PgUp` | Top, bottom, half page |
| `h` `←` `-` `⌫` | Parent folder (selects the folder you came from) |
| `l` `→` `⏎` | Open folder / versions of the file. On a deleted item, jump to its last snapshot first. |
| `gh` | Backup root |
| `H` `L` | Older / newer **change in this folder**. `L` past the last change goes to the newest snapshot. |
| `[` `]` / `Shift-←` `Shift-→` | Every snapshot, changed or not |
| `{` `}` | Older / newer change of the **selected item** |
| `Home` `End` | Oldest / newest change |
| `⇥` | Preview: content ↔ diff against disk |
| `J` `K` | Scroll the preview |
| `Space` / `v` | Select and move down / visual select |
| `y` `p` `P` | Yank / restore next to the original / overwrite (with confirmation) |
| `r` | Restore options |
| `o` | Show the file, as it was in that snapshot, in `$PAGER` (yazi's open; read only) |
| `d` | Full-screen diff against disk |
| `cc` `cd` `cf` | Copy `snapshotid:/abs/path` / folder path / file name |
| `.` `zh` | Show / hide deleted items |
| `zi` `zo` | Zoom the timeline |
| `/` then `n` `N` | Search this folder as you type, next / previous match |
| `f` | Filter the listing |
| `s` | Find in every snapshot (opens `:find `) |
| `:` | Command line |
| `?` `~` | Help (any key closes it) |
| `esc` | Leave visual mode, then clear the selection, then clear search and filter |
| `q` | Quit, in every view (the sub-views go back with `h` `esc` `⌫`) |
| *count* | `3H`, `5j`, `2J`, … repeat a motion. Stops at the first boundary message. |

**Versions view:** `j` `k` / `H` `L` / `{` `}` move older and newer · `gg` `G` · `⏎` `d` `l` diff against disk · `p` diff against previous · `⇥` `J` `K` preview · `r` restore · `o` pager · `y` yank · `zi` `zo` · `h` `←` `esc` `⌫` back.

**Diff view:** `j` `k` scroll · `Ctrl-d` `Ctrl-u` `space` · `gg` `G` · `]c` `[c` and `n` `N` next / previous change · `H` `L` older / newer version · `c` against disk · `p` against previous · `r` · `o` · `y` · `h` `←` `esc` `⌫` back.

**Find view:** `j` `k` · `gg` `G` · `⏎` `l` go to the last snapshot with it · `h` `←` `esc` `⌫` back.

**Restore dialog:** `j` `k` / `1`–`3` (`4` tar, for a folder) · `⏎` (twice for overwrite) · `esc` `q`. **Confirmation:** `y` / `⏎` yes, `n` / `esc` no. **`restore to:` prompt** (foreign host, §3.11): type a directory · `⏎` restore · `esc` cancel.

**Repository picker (§3.18):** `j` `k` `↓` `↑` · `gg` `G` `Home` `End` `PgDn` `PgUp` · `⏎` `l` `→` open the repository / pick the row · `/` filter the groups as you type (`⏎` keeps it, `esc` clears it) · `esc` `h` `←` `⌫` clear the filter, else back from the groups to the repositories · `r` read the selected repository's snapshot list again (repo level) · `q` `Ctrl-c` quit. Keyboard only.

Mouse (crossterm mouse events): click timeline dots, rows, breadcrumb parts, `− +`, the preview mode, which-key entries, dialog options and `? help`. Clicking a selected row opens it. Wheel scrolls the column under the pointer.

**yazi alignment:** these keys mean the same as in yazi: `hjkl`, `gg` `G`, `Space`, `v`, `y`, `p`, `P`, `cc` `cd` `cf`, `.`, `/` `n` `N`, `f`, `s`, `J` `K`, `~`, `esc`. Differences: `H` `L` move through time, `[ ]` `{ }` step through snapshots, `d` diffs (yazi: delete; restoric never deletes), and `r` opens restore options (yazi: rename).

### 3.15 Commands
| Command | Effect |
|---|---|
| `:2026-09-01`, `:09-01`, `:sep 1`, `:september 1` | Jump to the last snapshot on or before that day |
| `:today` `:yesterday` `:3d` `:2w` | Relative dates |
| `:latest` `:now` / `:oldest` `:first` | Ends of the timeline |
| `:reload` | Look for new or removed snapshots now |
| `:find NAME` / `:f NAME` (or `s`) | Search every snapshot (path contains NAME, case-insensitive) |
| `:deleted` | Same as `.` |
| `:undo` | Undo the last overwrite (`P` or option 1) |
| `:cancel` | Stop the running restore, without asking (§3.11) |
| `:host NAME`, `:tag T` | Change the snapshot filter *(not in the mockup)* |
| `:set strict` / `:set nostrict` | Tree-id vs fingerprint change detection *(not in the mockup)* |
| `:q` `:quit` | Quit |
| `:help` | Help |

An unknown command shows: `Unknown command ":x". Try :sep 1, :yesterday, :3d, :find NAME, :undo`.

Dates are local days; `:09-01` and `:sep 1` mean this year. `:host` takes one name or several, separated by commas. `:tag` with no tag clears the tag filter. A filter that would leave the folder without snapshots is refused, with the §4.6 explanation. `:set strict` switches the worker's change detection while running and recomputes what's on screen; the status bar then says `strict`. Zoom doubles from 1× up to whatever separates the closest two snapshots (at least 8×, at most 4096×).

### 3.16 Messages (copy the mockup's wording)
- "This is the oldest version of this folder." / "Newest snapshot. Newer changes exist only on disk." / "Newest snapshot. Nothing changed on disk since."
- "No older snapshot of this folder." · "No older change to main.go." · "Select a file or folder first."
- "Jumped to Sep 04 08:08, the last snapshot that has legacy.go"
- "Yanked 2 items from Sep 09 19:23. p restores next to the original, P overwrites."
- "Nothing yanked. Press y on a file first."
- "Restored as main.go.2026-09-09_1923" · "Restored 2 items next to the originals" · "Overwrote main.go. :undo puts the old version back."
- "A restore is running · esc to stop" · "Stopped · restored 2 of 5" · "Stopped · nothing was restored" · "Stopped · nothing was overwritten" · "No restore is running."
- "Copied 3e01f5b8:/home/bege/dev/project/src/util.go"
- "No match for "x" in this folder. Press s to search every snapshot."
- "The file did not exist in these snapshots, so there is nothing to restore."
- "No snapshots that early. The oldest is from Jul 14 09:00."
- "Already at the top of the backup."
- "Restored to ~/tmp/x/main.go" · "Overwriting is off for another host's snapshots. p restores into a directory you choose." (§3.11, foreign host)
- Picker (§3.18): "Can't open /mnt/photos: connection refused" above the rows · "No snapshots in this repository." · "No repositories in the config." · "No rows match "x"."

### 3.17 Look and colours
- **Colours:** by default, the **16 ANSI colours**, so the terminal's theme applies. Respect `NO_COLOR`. Colours can be changed in restoric's own config file. restoric doesn't read yazi's theme: it's a separate app.
  - `+` added: green · `~` changed: blue · `−` deleted: red · not backed up: magenta · accent (current dot, `▶`, `NOR` badge, keys): yellow · `SEL`/`VIS`: blue badge · `FIND`/input: magenta badge · `RST`/overwrite: red badge
  - Selection: a dim background bar across the column (reverse video when only 8 colours are available) · mark bar `┃`: accent
  - Folders: bold blue · deleted: red + strikethrough (where supported) · gone earlier: dim italic
- **Icons:** Nerd Font glyphs per file type (folder, language, markdown, shell, config…), coloured by type. `--no-icons` / `icons = false` switch to the plain set used in the mockup. A terminal can't report whether its font has the glyphs, so there's no automatic fallback: Nerd Font is the default, as in yazi.
- **Lines:** only thin vertical `│` between columns. Popups (restore, confirmation, help, which-key, messages) have rounded corners `╭╮╰╯`.

### 3.18 Repository picker (`--browse`)
A start-up screen for reaching snapshots the folder-anchored flow can't: a renamed or dead machine, another host, a path that has moved. It runs only at start-up (§4.6). To browse another repository, host or path, quit and start again; there is no in-session switch.

- **Repo level**, only when the config has several `[[repo]]` and `--repo` wasn't given. One row per repo: location, the hosts seen (from `repos.json`; `not read yet` when missing, with `· read 3 h ago` when older than the 5-minute recheck). Order: repos whose cached hosts include this machine, then repos holding PATH, then config order. No repo is opened to draw it. `r` refreshes the selected row; `⏎` opens the repo. An open that fails says why above the rows and marks the row `can't open`.
- **Group level**, one row per **(host, backup path)**: host, path, snapshots, latest. Tags are not part of the row. A snapshot with several backup paths counts once in each path's row. This machine's hosts (the config `host` list) come first, in bold, the rest by newest snapshot. `/` filters by text (host or path, case-insensitive). PATH pre-selects the closest row. Built from the snapshot list alone.
- **Keys:** `⏎` select · `esc` back from groups to repos · `q` quit · `/` filter · `r` refresh (repo level). The full list is in §3.14.
- **After picking:** open the folder view on PATH if it lies at or under the group's path, else on the group's path. The picked host replaces the config `host` for the session and shows in the title bar as `dev-vm:` before the breadcrumb. The config `tag` stays only when the picked host is one of this machine's: for another host it means nothing and is dropped.
- **Foreign host** (not in the config `host` list): `p` and `P` ask for a target directory (default: the last one used in the session, else the current folder), and `P` is disabled with a message (§3.11). The `on disk` comparison (§2.5) is unchanged; a path missing here shows as missing.

<sub>`docs/screens/19-picker-groups.txt`</sub>

```text
source: tests/ui_picker.rs
expression: "screen(&p, 100, 34)"
---
 restoric  Pick a host and path                                     rest:http://iridium:8000/dev-vm

   host         path                                                        snapshots  latest
 ▶ bege-laptop  ~/dev/project                                                       2  Sep 06 18:03
   dev-vm       /srv/data                                                           2  Sep 01 10:00
   old-laptop   ~                                                                   1  Mar 01 10:00



























 PICK  ⏎ open · / filter · q quit                                                               1/3
```

<sub>`docs/screens/20-picker-repos.txt`</sub>

```text
source: tests/ui_picker.rs
expression: "screen(&p, 100, 34)"
---
 restoric  Pick a repository

   location                         hosts
 ▶ rest:http://iridium:8000/dev-vm  bege-laptop, dev-vm, old-laptop · read 3 h ago
   /mnt/photos                      nas · read 30 d ago
   sftp:nas:/backup                 not read yet



























 PICK  ⏎ open · r refresh · q quit                                                              1/3
```

| Rows | Content |
|---|---|
| 0 | `restoric` and `Pick a repository` / `Pick a host and path`; at the group level the repository's location at the right |
| 1– | The message, when there is one: the §4.6 dead-end text, or why an open failed. Then a blank row. |
| then | A dim column header and the rows, `▶` and the selected background on the current one. The host column is as wide as the widest host; the path takes the rest. |
| last | Status bar: ` PICK ` badge, the keys, `opening …` while a repository opens, and `2/3`. Typing `/` shows `filter: vm█` like the folder view's filter. |

How it works (M9): the picker is its own state machine (`picker.rs`), not a `View` of `App`: at the repo level there is no repository, no index and no folder. `tui::Term::pick` draws it and blocks on keys; when it asks to open or refresh a repository, `main.rs` does so synchronously (the status bar says `opening …`), records the list in `repos.json` and reports back. After a pick the folder view starts as usual, with the filter's host replaced and `App::mine` holding this machine's hosts, so `App::foreign` also applies after `:host` in a session. Restores on a foreign host go through `How::Into(dir)`.

---

## 4. Architecture

### 4.1 Crates
| Need | Crate | Notes |
|---|---|---|
| Repository | `rustic_core` | Pin an exact version and wrap it behind our own trait (§4.3) |
| Backends | `rustic_backend` | **Every backend rustic supports** (all features enabled): local, sftp, REST, rclone, and the OpenDAL-based ones such as S3, B2 and Azure. restoric adds none of its own. |
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
| Text width | `unicode-width` | Correct column widths for names with wide characters |
| Later | `syntect` for highlighting in the preview and diff, `trash` for undo via the system trash | |

### 4.2 Module layout
```
restoric/
├── Cargo.toml
├── PLAN.md
├── docs/            mockup.html, screens/*.txt, demo.tape → demo.gif (the README's demo; `vhs docs/demo.tape` regenerates it)
├── src/
│   ├── main.rs        clap args, env (RESTIC_*), start-up, terminal setup/teardown, panic hook
│   ├── lib.rs         the modules below, so tests/ can use them
│   ├── log.rs         `restoric log`
│   ├── config.rs      ~/.config/restoric/config.toml
│   ├── repos.rs       which [[repo]] holds the folder; repos.json cache of snapshot roots
│   ├── picker.rs      the repository picker's state and keys (§3.18); drawn by ui/picker.rs
│   ├── repo/
│   │   ├── mod.rs     trait Repo + our own types (SnapshotInfo, TreeId, Node, NodeKind)
│   │   ├── rustic.rs  RusticRepo: rustic_core implementation
│   │   └── fake.rs    FakeRepo: in-memory, built from a small DSL (tests, UI work, demo mode), plus a FakeDisk
│   ├── index/
│   │   ├── mod.rs           Index: tree LRU + cache, path lookups (NodeRef)
│   │   ├── fingerprint.rs   leaf fingerprints
│   │   ├── folder.rs        tree diffs: differs, counts
│   │   ├── timeline.rs      timeline set, change points per path, item tracks
│   │   ├── listing.rs       listing at snapshot n, Δ markers, deleted items
│   │   ├── live.rs          what changed on disk since a snapshot
│   │   └── versions.rs      runs per path
│   ├── cache.rs       redb tables (§4.4)
│   ├── disk.rs        Disk trait: the real file system (never follows symlinks), faked in tests
│   ├── worker.rs      background pool, Request/Response enums, generation ids
│   ├── tui.rs         Term: terminal setup/teardown, panic hook, the picker loop and the event loop
│   ├── restore.rs     next to / overwrite / restore folder / tar, undo log (writes nothing else)
│   ├── diff.rs        imara-diff line diffs, binary detection, margin marks, hunks
│   ├── app/
│   │   ├── mod.rs     App state, view stack (Folder, Versions, Diff, Find), overlays (Dialog, Help)
│   │   ├── keys.rs    key parser: counts, prefixes (g, z, c, ], [), modes (normal, visual, command, search, filter, dialog)
│   │   ├── selection.rs  marks, visual range, yank register
│   │   ├── cmdline.rs `:` parser, dates via jiff
│   │   └── actions.rs one function per action, shared by keys and mouse
│   └── ui/
│       ├── timeline.rs  labels, tracks with right-edge labels, zoom, ‹ ›, caret, clickable areas
│       ├── folder.rs    versions column + listing
│       ├── preview.rs   file content with change marks, inline diff, folder contents
│       ├── versions.rs  diffview.rs  find.rs  statusbar.rs
│       ├── picker.rs    the repository picker screen
│       ├── popup.rs     restore dialog, confirmation, help, which-key, messages (rounded)
│       ├── fmt.rs       dates, sizes, paths, fitting and wrapping text
│       ├── icons.rs     Nerd Font glyphs per file type + plain fallback
│       └── theme.rs
├── yazi-plugin/
│   └── restoric.yazi/main.lua   opens restoric on the hovered folder (M8)
└── tests/
    ├── fixtures/make_repo.sh   builds a real restic repo with a known history
    ├── fixtures/project.dsl    FakeRepo history shaped like the mockup's (UI tests, demo)
    ├── index_*.rs              change points against the fixture
    └── ui_*.rs                 insta snapshots of every screen (the screens in this plan)
```

### 4.3 The `Repo` trait
Our own trait wraps rustic_core: it isolates API changes and lets the UI run against `FakeRepo`.

```rust
pub trait Repo: Send + Sync {
    fn id(&self) -> Id;                                         // repository id, names the cache file
    fn snapshots(&self) -> Result<Vec<SnapshotInfo>>;          // id, time, host, paths, tags, root tree
    fn tree(&self, id: &TreeId) -> Result<Arc<Tree>>;           // nodes: name, kind, size, mode, owner, mtime, content ids, subtree id, raw hash
    fn read_file(&self, node: &Node, limit: u64) -> Result<FileBytes>;
    fn read_at(&self, node: &Node, offset: u64, len: u64) -> Result<Vec<u8>>;   // read_file is built on it
    fn restore(&self, snap: &SnapshotInfo, path: &Path, dest: &Path) -> Result<()>;   // dest must not exist
}
```
Everything in `index/` is written against this trait and holds no rustic types. A node's `raw` hash covers everything restic stored for it; `--strict` compares that.

`RusticRepo` walks trees with rustic's smaller **trees-only index** (`to_indexed_ids`). It loads the full index, which also locates data blobs, only on the first file read.

### 4.4 Cache (redb, `~/.cache/restoric/<repo-id>.redb`)
| Table | Key → value | Notes |
|---|---|---|
| `path_ref` | (snapshot id, path) → missing, folder tree id, or leaf fingerprint + raw hash | Every folder on the way is stored too, so later walks are O(1) |
| `differs` | (mode, tree id, tree id) → whether the content differs | §2.2 |
| `counts` | (mode, tree id or none, tree id or none) → added, changed, deleted | Folder counts. (none, tree) is the number of items under a tree. |
| `meta` | schema version, rustic_core version | Wipe if they don't match |

There's no `snapshots` table: rustic_core keeps snapshot files in its own local cache, and listing them is fast (timing in §11, M0). Every key is content-addressed, so nothing is ever invalidated. Change points are cheap to rebuild from `path_ref` and `differs`, so they aren't stored. A new snapshot costs one lookup per path. Writes collect in memory and go to disk in one transaction per operation.

rustic_core has its own cache for index and tree packs. Check in M0 that tree packs are cached locally, so walks are fast after the first run.

### 4.5 Threads
- **UI thread:** the event loop over `crossbeam::select!` between crossterm events, worker responses and a tick for spinners. It draws only from `App` state and never calls `Repo`.
- **Worker pool** (N = number of CPUs, minimum 2): handles `Request` messages (`ChangePoints{path}`, `Listing{snap,path}`, `ItemTrack{snap_range,path}`, `Versions{path}`, `Preview{snap,path}`, `LoadFile{…}`, `Restore{…}`, `Find{q}`). Each request carries a **generation id**. When the view moves on, the UI bumps the generation, and requests about what was on screen (listings, deleted items, previews) that haven't started yet are skipped, so fast scrolling stays snappy. Requests whose results are kept anyway (change points, counts) always run. `worker::handle` does the work and is called directly by the UI tests.
- **Prefetch:** after a listing loads, prefetch the neighbouring change points (n±1), the item track and the preview for the selection. Previews are cached by content id, so the same file version is only read once.
- **Progress:** the title bar shows `indexing 120/430` while change points are computed. Partial results draw as they arrive, newest first.

### 4.6 Start-up
1. Read the config file. Then find the repository, in this order:
   1. `--repo` / `--repository-file`.
   2. The config's `[[repo]]` whose snapshots of this machine (§2.4, with that entry's `host` and `tag`) hold the folder. restoric doesn't build an index to check this: it lists each repository's snapshots. `~/.cache/restoric/repos.json` remembers each repository's snapshot roots (host, tags, backup paths) and when it was read, so a usual start opens only the repository the cache picks, and checks it against its current snapshot list. On a miss, or when the pick turns out stale, every repository not read in the last 5 minutes is read again, one at a time, with a `checking <repo>…` line on stderr. A repository that can't be opened is skipped with a one-line warning. When several hold the folder, the one with the closest backup path wins (at or above the folder beats below it, deeper beats shallower), then the one listed first.
   3. `RESTIC_REPOSITORY` / `RESTIC_REPOSITORY_FILE`. If there's none and the config has `[[repo]]` entries, the error names the folder and the repositories checked.

   The password comes from `--password-file` / `--password-command` / `--insecure-no-password`, then the chosen `[[repo]]` if it says how to unlock it, then `RESTIC_PASSWORD` / `_FILE` / `_COMMAND`, the same as restic. The repository is chosen once, from the folder restoric starts in.
2. Open the repo (read only). Load the snapshot list from the cache, then fetch new ones in the background.
3. Pick the path: the argument or the current folder, made absolute. The timeline set is snapshots of this machine (§2.4) with a backup path at, above, or below that path (§2.3).
4. If this machine has no snapshots at all: open the picker (§3.18), which lists the hostnames that do have snapshots. Without a terminal, print them, explain how to set `host` (§2.4), and exit non-zero.
5. If the path isn't in any of this machine's snapshots, or no `[[repo]]` holds the folder: open the picker, with the message above it. Without a terminal, print a clear message listing the paths this machine backs up, and exit non-zero.

`--browse` opens the picker even when the folder matches. With one repository, or with `--repo`, the picker starts at the group level; with several `[[repo]]` and no `--repo`, at the repo level. With no repository at all (no `--repo`, no `[[repo]]`, no environment), the error stays and points at `--repo` and the config.

### 4.7 CLI
```
restoric [PATH]                       open the TUI at PATH (default: current folder)
  -r, --repo REPO       --repository-file FILE
  --password-file FILE  --password-command CMD     (plus RESTIC_REPOSITORY, _FILE, RESTIC_PASSWORD, _FILE, _COMMAND;
                                                    a matching [[repo]] in the config comes before the environment, §4.6)
  --insecure-no-password  a repository made with `restic init --insecure-no-password` (empty password; can't be combined with a password)
  --host HOST           --tag TAG                  --strict
  --select NAME         start with NAME selected (used by the yazi plugin)
  --no-icons
  --at DATE             start at a date (:sep 1 syntax)
  --browse              start in the repository picker (§3.18); also opens by itself on the §4.6 dead ends
restoric log PATH [--json]            print the change points of PATH (no TUI)   (M1)
restoric versions FILE [--json]       print the distinct versions of FILE        (M3)
restoric demo                         TUI against FakeRepo with tests/fixtures/project.dsl, its files written to a temporary folder (removed on exit)
```

### 4.8 Scale: any repository size
restoric must work on repositories of **any size**: any number of snapshots, files, folders and versions, and any total size. The UI must stay responsive regardless. Design rules that follow from this:

1. **Work grows with what's on screen, never with the repo.** Opening a folder costs about *depth × snapshots* tree reads (cached after the first time), not the number of files in the repo. Nothing walks a whole snapshot unless the user asks for it (`:find`, restoring a whole folder).
2. **Nothing assumes a list fits on screen or in memory.** The listing, the Versions column, the versions view, find results and the diff are virtualised: only visible rows are built. Long lists load in pages.
3. **Results stream in.** Change points, find results and folder counts appear as they're computed, newest first, with progress in the header (`indexing 1 200/48 000`). Every long operation can be cancelled with `esc` and resumes from the cache next time. (M7: leaving the Find view stops `:find`. A folder's indexing runs to the end in the background, and everything it computed is cached.) Restores show their progress in the status bar and stop on request, except that rustic's copying of one folder's contents runs to the end (§3.11).
4. **Lazy everything.** Folder counts, "compared to disk" stats, item tracks, previews and deleted items are computed only for what's visible, and cached.
5. **Bounded memory.** In-memory caches (trees, previews, diffs) are LRU with a size cap, configurable. The on-disk cache can grow, but has a size cap too. (M7: a cache file over the cap is started afresh at start-up, rather than evicting the least recently used entries; everything in it can be computed again.) Previews are capped at 64 KB and diffs at 2 MB (both configurable). Files are never loaded whole just to show them.
6. **Incremental refresh.** At start-up, only snapshots that are new since the last run are read. Change points are extended, not recomputed.
7. **`:find` uses the cache first.** Paths seen in cached trees answer instantly. The full search across all snapshots then runs in the background and streams in further results.
8. **The timeline handles thousands of snapshots.** At 1× zoom many snapshots share a column, and the info shows how many. Zoom goes as far as needed to separate them (beyond 8× when the repo needs it), and `[` `]` always step one snapshot at a time.

**The one limit restoric can't design away:** rustic_core, like restic, may keep the repository's **index** (the list of every blob) in memory. That memory grows with the number of blobs in the repo, not with what restoric shows. M0 measures it on a large repo. If it's a problem, the options are rustic_core settings that reduce index memory (if they exist), contributing such a feature upstream, or loading only the tree part of the index (restoric reads trees far more often than file data).

**Test repos:** a generator (`tests/fixtures/make_big_repo.sh`, using rustic or restic) builds synthetic repos at three sizes, kept out of git:

| Size | Snapshots | Files per snapshot | Used in |
|---|---|---|---|
| small | 50 | 1 000 | CI, every commit |
| large | 2 000 | 200 000 | M0, M7, before releases |
| huge | 20 000 | 2 000 000, deep trees, folders with 100 000 entries | M7, manual |

**Targets (large repo, warm cache, local disk):** first screen under 1 s; `H`/`L`, `j`/`k` and preview feel instant (under 50 ms); a folder's change points under 2 s. Cold cache and remote backends may be slower, but the UI never freezes: it shows progress and partial results.

---

## 5. Milestones

Each milestone ends in something usable and tested.

### M0: rustic_core test run (1–2 days)
Throwaway binary `spike/`:
- Open **your real repo** (over its real backend) with rustic_core. List this machine's snapshots.
- Walk to one folder in every snapshot, timed cold and warm.
- Count snapshots where the tree id differs vs where the fingerprint differs.
- Read one file from an old snapshot. Restore one file into a temporary folder.

Run it on the **large** synthetic repo (§4.8) as well as yours, and record memory use (especially the index).

**Go/no-go:** a warm walk over all snapshots under ~2 s for a typical folder on the large repo, a cold walk acceptable with a progress indicator, memory use acceptable, the backend works, and the restored file matches. If rustic_core fails on something essential, record what and decide between fixing it upstream and a `restic`-CLI implementation of `Repo` (the trait makes that possible).

### M1: index and `restoric log`
- `Repo` trait, `RusticRepo`, `FakeRepo` + DSL.
- Fingerprints, timeline set, change points, folder counts, cache.
- `restoric log PATH` prints change points with counts.
- `tests/fixtures/make_repo.sh`: uses the real `restic` binary with `backup --time` to build a repo with known history: added, changed, deleted, re-created, metadata-only (`touch`), permission-changed (`chmod`, which counts as a change) and renamed files.
- **Done when:** change points match the fixture's expected list, and metadata-only snapshots are skipped by default but shown with `--strict`.

### M2: read-only folder view
- Terminal setup and teardown (panic hook restores the terminal), event loop, worker, generation ids.
- Header with breadcrumb, the timeline row with the folder's changes and its label, Versions column with folding, listing with icons and Δ, status bar, message popups, help.
- Keys: `j k gg G C-d C-u h l - ⌫ ⏎ gh H L [ ] Home End q ? ~` and counts. Which-key popup for `g`. Mouse clicks on rows, dots, breadcrumb.
- Narrow layouts (under 100 and under 80 columns).
- **Done when:** insta snapshots of screens 01 and 17 (from FakeRepo, preview column empty) match, and it's usable on your real repo.

### M3: preview, item track, versions, deleted items
- Preview column: file content in the snapshot with `+`/`−` margin marks, folder contents, deleted items. `⇥` content / vs disk, `J` `K`. Loaded in the background and cached by content id.
- The Versions column's `on disk` row and the timeline's `on disk` marker (what changed on disk since the newest snapshot). Moved here from M2, because they need the comparison with disk.
- The selected entry's changes in the timeline row (`●` over the folder's `○`) with its label, `{` `}`.
- Versions view with on-disk row, compared-to-disk stats (computed lazily for visible rows) and preview.
- Deleted items (`.` `zh`), jump on deleted items.
- `restoric versions FILE`.
- **Done when:** screens 01, 02, 03, 11 and 13 match.

### M4: diff
- Full-screen diff: load both sides with a size limit and binary detection, imara-diff, hunks, `c`/`p` modes, `]c` `[c` `n` `N`, `H` `L` across versions. Which-key for `]` `[`.
- **Done when:** screen 14 matches, and binary and large files are handled.

### M5: restore
- `Space`, visual mode, `SEL`/`VIS` badges. `y` `p` `P`, confirmation popup, undo log, `:undo`.
- `r` dialog with 4 options. `cc` `cd` `cf` (arboard, then the OSC 52 escape sequence over SSH).
- Files and folders, existing and deleted on disk, several at once.
- **Done when:** screens 05, 06, 07 and 15 match. Integration tests restore from the fixture into a temporary folder with every option, and `P` then `:undo` gives back the original bytes.

### M6: navigation extras
- `/` search with highlighting, `n` `N`. `f` filter. `:` command line with dates, `:find` / `s`, `:latest`, `:oldest`, `:deleted`, `:host`, `:tag`, `:set strict`.
- Find view and jump to the last snapshot with the match.
- Zoom (`zi` `zo`, clickable), "N in column". Which-key for `z` and `c`.
- **Done when:** screens 04, 08, 09, 10, 12 and 16 match.

### M7: polish and release
- Config file (keymap overrides, colours, icons on/off, diff/preview size limit, default host). `NO_COLOR`.
- Nerd Font icons per file type with fallback.
- `restoric demo`.
- Performance pass on the large and huge repos (§4.8): meet the targets, check memory caps and cancellation.
- README with a demo recording: `docs/demo.tape` drives `restoric demo` under [vhs](https://github.com/charmbracelet/vhs) and writes `docs/demo.gif`; `vhs docs/demo.tape` regenerates it. `cargo install`, GitHub release binaries (Linux x86_64/aarch64, macOS), AUR/deb later.
- Optional: syntax highlighting in the preview and diff (`syntect`), restore to the system trash.

### M8: yazi plugin
- A small Lua plugin, `restoric.yazi`, adds a key in yazi (suggested `T`) that suspends yazi, runs `restoric` on the hovered folder (or on a hovered file's folder with that file selected, via `--select`), and returns to yazi when restoric exits.
- Optional: after `p`/`P`, yazi refreshes and shows the restored file.
- Publish it for yazi's package manager (`ya pack`), with install instructions in the README.
- **Done when:** in yazi, pressing the key over a folder opens restoric there, and `q` comes back to the same place in yazi.

How it works (M8): a `@sync` entry reads the hovered file (or the folder, if nothing is hovered) and emits yazi's `shell --block` with `restoric <path>`. A file path makes restoric open its folder with the file selected, so `--select` isn't needed. Arguments after `--` in the keymap pass through; yazi turns `--no-icons` into `no_icons`, so the plugin turns it back. Tested with yazi 26.9.1. Publishing for `ya pkg` needs the plugin at the root of a published repository, so it waits until restoric has a public home; until then the README says to copy the folder.

### M9: repository picker
- **Read first:** §3.18 (the screen), §4.6 and §4.7 (start-up and `--browse`), §2.4 (host), §3.11 (restore) and the 2026-10-08 entry in §8 "Settled".
- `--browse` and the §4.6 dead ends open the picker (§3.18): repo level (several `[[repo]]`) and group level, `/` filter, PATH pre-selection, this machine's rows first.
- The picked host replaces the config `host`; the title bar shows it. On a foreign host `p`/`P` ask for a target directory (remembered per session) and `P` is disabled.
- Insta snapshots for both levels and their empty and error states, from `FakeRepo`. A test that a snapshot path missing on disk shows as missing without an error.
- **Done when:** with a `FakeRepo` holding two hosts, `--browse` shows both, picking the other host opens its timeline, and a restore from it asks for a directory.

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
| Slow first run on big or remote repos | Persistent cache, background indexing with progress, newest-first partial results, cancellable (§4.8) |
| Index memory grows with repo size (rustic_core) | Measure in M0; see §4.8 for options |
| `restic prune` running while restoric reads (rustic doesn't take locks) | Treat missing packs as recoverable: reload the index, retry once, show an error in the status line without crashing |
| Restore overwrites something important | Default is "next to it", confirmation, undo folder |
| Snapshots with different backup roots or hosts | Timeline set filter (§2.3), `:host`, `:tag`, clear message when the path isn't backed up |
| Huge folders (100k entries) | Virtualised listing, counts computed lazily |
| Preview slow over a remote backend | Load in the background, cache by content id, cap at 64 KB for the preview, show "loading…" without blocking |
| No Nerd Font installed | Automatic fallback to the plain icon set, `--no-icons` |

---

## 8. Open questions
1. **AGPL-3.0-only or AGPL-3.0-or-later?** `LICENSE` holds the AGPLv3 text. The plan assumes `-or-later`, which is the usual choice and lets a future AGPL version apply.

Settled during review:
- Accented names (2026-10-08): macOS stores `ä` as `a` + U+0308. Names are drawn grapheme by grapheme (ratatui's `set_stringn`), so the accent stays in its cell. Search (`/`), filter (`f`), `:find` and the picker filter compare lowercased NFC text (`index::fold`), so a typed `ä` finds a decomposed one. Names are never normalized for paths or restores.
- Picker details (2026-10-08, M9 built): a repo row shows the full location and the hosts, no name column and no `name` key. On a foreign host `r` is the same directory prompt as `p` (no reduced dialog); the item lands at `<dir>/<name>`. The config `tag` is dropped for a foreign host and kept for one of this machine's. The picker is keyboard-only.
- Repository picker (2026-10-08), §3.18, M9. It lists (host, path) groups, not a snapshot file tree, so the folder-anchored design stays. It opens with `--browse` and in place of the §4.6 dead ends (no terminal: the old message, exit non-zero). With several `[[repo]]` there is a repo level first (A2), drawn from `repos.json` without opening any repo. A row is (host, path) only, with no tags. No in-session switch of repo, host or path: restart. Restoring from a foreign host always asks for a target directory and disables `P`. The `on disk` comparison is left as it is: it is read-only, and for a renamed machine it is correct.
- Several repositories in the config (2026-10-07): `[[repo]]` blocks with `repository`, `password_file`, `password_command`, `insecure_no_password`, and optional `host` and `tag` that replace the top-level ones. There's no `paths` key: restoric lists each repository's snapshots to find the one that holds the folder, and caches what it saw so a usual start opens one repository (§4.6). `--repo` comes first, then the config, then `RESTIC_REPOSITORY`, so the variable can stay set for restic itself. Principle 5 still holds: with no `[[repo]]`, restic's variables work as before.
- Restores show progress and can be stopped (2026-10-07), after comparing with lazyrestic. Progress goes in the **status bar**, not the header (the header belongs to indexing) and not a modal popup (a big remote restore would lock the user out; yazi runs copies in the background too). restoric keeps rustic's restorer, which can't be interrupted while it copies a folder's contents; writing its own restorer for that was judged not worth losing rustic's parallel reads. `esc` asks before stopping, `:cancel` doesn't; one restore at a time; quitting asks and waits. A stopped overwrite puts everything back; stopped copies keep finished items. No resume after a failure for now (`ISSUES.md`). Details in §3.11.
- The listing widens with the terminal: 40% of the width after the Versions column, at least 40 (38 under 100 columns) and at most 50 (60 felt too wide at 171 columns). The Δ column is as wide as its widest visible entry, so a lone `~` doesn't leave five blank cells before the divider. A fixed 35 left names 18 cells and gave every extra column to the preview. At 100 columns the preview gives up 5 cells so names get 23.
- Labels at the right edge of the timeline row replace the legend.
- The state of the files on disk is called `on disk` everywhere (2026-10-06): the Versions column's top row (`on disk  ~1`), the timeline's right-hand label, the versions view's first row and the messages. It was `now  ~1 unsaved` in the column and `now` on the timeline; "unsaved" suggested work at risk, which is the opposite of what a backup browser should say.
- `o` shows a file in `$PAGER` (2026-10-06). It was restore option 4, but it isn't a restore; the dialog has three options for a file and the tar archive as a fourth for a folder.
- Plain `←` `→` are `h` `l`, as in vim and yazi (2026-10-06). Time moves with `H` `L`, `[` `]`, `{` `}` and `Shift-←` `Shift-→`.
- `q` quits from every view, as in yazi (2026-10-06). The versions, diff and find views go back with `h`, `esc` and `⌫`. Dialogs and confirmations still close on `q`.
- No `◀ version 12 of 16 ▶` counter in the header (removed 2026-10-06). The timeline's ▲ and the Versions column already show where you are, and the counter was read as a second version counter beside the preview's `v5/6`.
- The timeline has one row of dots, not two: `●` where the selected entry changed, `○` where only the current folder did (§3.1, §3.2). The folder's own row repeated the Versions column; merged, the panes get a row back.
- `v` is visual mode, as in yazi, and file versions open with `⏎`/`l` (`i` was a third key until 2026-10-06; a vim user reads it as insert).
- Restored copies are named with the snapshot time after the full name: `main.go.2026-09-09_1923`, `src.2026-09-09_1923/`. If that name already exists (the same version restored twice), add `-2`, `-3`, …
- restoric is a separate app and doesn't read yazi's config or theme. The yazi plugin (M8) is only a launcher.
- Backends: everything rustic supports. M0 tests against the backend of your real repo.
- Permission, owner and group changes count as changes. Modification time alone doesn't.
- License: AGPLv3. Fine with the dependencies, which are MIT or Apache-2.0 (rustic_core, ratatui and the rest).
- Name: `restoric` is free on crates.io and the AUR, and the GitHub account name `restoric` is free. One existing GitHub repo has the same name: [leaanthony/restoric](https://github.com/leaanthony/restoric), "A PoC Restic GUI using the Wails Framework" (12 stars, no license, last push January 2023). It's an abandoned proof of concept, but it's in the same space, so expect some confusion in search results.
- Only snapshots from this machine are shown (§2.4).
- Repository size: any. Design rules, test repos and targets are in §4.8.
- Repositories without a password (`restic init --insecure-no-password`) are supported with `--insecure-no-password`, as in restic (§4.7).
- A snapshot belongs to a folder's timeline set when a backup path is the folder, above it, or below it. Backups that name files (`restic backup dir/a.log dir/b.csv`) open at `dir` (§2.3).

---

## 9. Out of scope (for now)
- Making backups, scheduling, prune/forget. restic/rustic and Backrest already do that.
- Writing to the repository in any way. restoric is read-only towards the repo.
- A graphical (non-terminal) app or file manager plugins. That's a possible future, with the index reusable as a library.

---

## 10. Engineering details

**Platforms:** Linux and macOS. Windows later (rustic supports it, but terminal and path handling need their own pass).

**Toolchain:** Rust stable, edition 2024. Minimum Rust version follows rustic_core's. `cargo fmt` and `cargo clippy -- -D warnings` must pass. CI (GitHub Actions): fmt, clippy, tests against the small fixture repo (CI installs the `restic` binary to build it).

**Errors:** a wrong password, a missing repo, a dropped connection or missing packs show as a message popup and in the log; the UI keeps running. Start-up errors print a plain message and exit non-zero. A panic hook restores the terminal before printing.

**Secrets:** passwords never reach the log, the screen, or the cache.

**Restore safety:** restore writes only inside its target. Symlinks are restored as symlinks and never followed while writing. Mode and modification time are kept; owner and group only when running as root.

**File names:** names that aren't valid UTF-8 are kept as raw bytes for every operation and shown with replacement characters.

**Time:** snapshot times are shown in the local time zone. `:sep 1` and other date commands use the local day.

**The repo changing while restoric runs:** new snapshots (a backup finished) are picked up every 5 minutes and on `:reload`, and the timeline extends without losing your place. Snapshots removed by `forget`/`prune` are dropped from the cache, and the affected change points are recomputed.

**Logging:** `tracing` to `~/.local/state/restoric/restoric.log`. Level from `RESTORIC_LOG` (default `info`).

**Config** (`~/.config/restoric/config.toml`, every key optional):
```toml
host = ["bege-laptop"]        # several names if the machine was renamed (§2.4)
tag = ""                      # only snapshots with this tag (§2.4)
icons = true
preview_max_kb = 64
diff_max_mb = 2
memory_cache_mb = 256
disk_cache_mb = 2048
restore_dir = "~/Restored"

[keys]                        # action = "key", e.g.
# versions = "i"
# half_down = "C-f"

[colors]                      # accent added changed deleted live dim dir code selected
# accent = "magenta"          # ANSI names, "#rrggbb" or 0–255

[[repo]]                      # repositories to look in; the one holding the folder is used (§4.6)
repository = "rest:http://iridium:8000/dev-vm"
insecure_no_password = true   # or password_file = "~/…", password_command = "…"
# host = "dev-vm"             # replace the top-level host and tag for this repository
# tag = "work"
```

How it works (M7): `--host` and `--tag` win over the config, and the config over the hostname. A mistake in the file (an unknown key, action, key name or colour) stops start-up with a message naming it. Key overrides apply in every view, before the built-in keys. Actions: `down up top bottom half_down half_up parent open versions root older_change newer_change older_snapshot newer_snapshot oldest_change newest_change older_item_change newer_item_change preview_mode scroll_down scroll_up deleted diff select visual yank paste overwrite restore show search filter find next_match prev_match command zoom_in zoom_out help quit`. Keys: a character, `C-x`, `Enter`, `Tab`, `Space`, `Backspace`, arrows, `Home`, `End`, `PageUp`, `PageDown`, `F1`–`F12`.

**Mockup vs implementation:** the mockup's sample data is randomly generated, so the screens in §3 are the **layout and behaviour spec**, not byte-exact expected output. `FakeRepo` gets its own small, readable fixture. The insta snapshots generated from it become the exact expected output, reviewed against §3 when first accepted.

---

## 11. Progress

Tick milestones here as they're done, with the commit.

**Not yet done for any milestone:** the manual check on your real repo over its real backend (§6). It needs that repo's credentials; everything so far was checked on the synthetic repos (§4.8) and the fixtures.

- [ ] M0: rustic_core test run
- [x] M1: index and `restoric log` — 6bfcf81
- [x] M2: read-only folder view — cbd4b73
- [x] M3: preview, item track, versions, deleted items — 3be44ca
- [x] M4: diff — 34dc9c5
- [x] M5: restore — adf677b
- [x] M6: navigation extras — 878a221
- [ ] M7: polish and release
- [x] M8: yazi plugin — abb5806 (not yet published for `ya pkg`)
- [x] M9: repository picker — c725494
