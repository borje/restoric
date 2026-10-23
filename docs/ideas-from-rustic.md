# Ideas from `rustic snapshots -i`

What rustic's interactive TUI (v0.11.4, `src/commands/tui/`) does that restoric could learn from, best first.

## Worth taking

1. **Say what changed when the content didn't.** *Closed in `1642d71`.* A version where only the mode, owner or group changed is marked `~`, but the diff view said "No differences. The two versions are identical." It now shows `mode 644 → 755 · owner 1000 → 0` under the title and "Same content. Only the mode or owner changed." The disk side compares mode only, because disk has no owner. rustic's diff shows metadata and has `m` to ignore it.
   Still open:
   - In the versions view, a version that only changed the mode looks the same as the one before it (same size, same vs-disk stat). It could show the mode change in its row.
   - The on-disk view marks a file `~` when only its mtime changed (`DiskEntry::differs_from`), and its diff still says "identical". It could say "only the modification time changed".
   - Setuid, setgid and sticky changes are counted (the fingerprint hashes the whole mode) but not shown: restic stores Go's file mode, and those bits sit outside the low 9 bits.

2. **Snapshot info popup (rustic: `i`).** *Closed in `ecb1cdd`.* Today restoric shows only the time and short id. The popup would add the full id, host, paths, tags, label, description, the program that made it, the parent, and the summary: files and dirs new/changed/unchanged, data added (raw and packed), the command and the duration. It needs no tree walk, because it's all in `SnapshotFile`. `SnapshotInfo` needs the extra fields. `i` isn't bound by default. It helps spot an odd backup, like 40 GB added because a Lightroom catalog was rebuilt.

3. **Changes-only toggle (rustic diff: `d`).** *Closed in `786f815`.* It hides unchanged rows in the listing. In a folder like `Bilder` with thousands of photos and three changed, finding the `~` rows means scrolling. It fits with `.` and `zn`, for example as `zc`.

4. **Label and description on the timeline.** *Closed in `70e0a1e`: a `▵` under the dot, the label in the status bar.* rustic shows a label column, `🗎` for a description and `🛡` for delete protection. restoric could mark those dots and name the label in the status bar. It's cheap once 2 adds the fields.

5. **Size on request (rustic: `s` / `S`).** *Closed in `dcb41c3`. Snapshots only; the on-disk version shows no folder sizes.* restoric shows no folder sizes. rustic walks the tree only when the key is pressed, which fits "work scales with what's on screen" because the user asked. Two uses: the folder's total size in this snapshot, and the bytes this version added.

6. **Compare any two snapshots (rustic: `D` with two marked).** *Closed in `383ec2e` as `b`. The file preview still compares with the version before.* restoric compares against disk or the previous version only. A "pin this snapshot as the base" mode would make the Δ column and `d` compare against it.

7. **Picker from anywhere.** *Closed, won't do: start with `--browse` and `q` goes back to the picker.* rustic changes its filter live (`V` edits it, `v` cycles Filtered / All / Marked / Modified). restoric reaches the picker only if it started there. A `:browse` command would open it from the folder view.

## Second round

1. **The year in dates.** *Closed in `1055b86`.* rustic shows full timestamps. restoric showed `Jul 02 10:00` everywhere, so in a repository going back years it could be any July. Now, as `ls -l` does, a date more than about six months back shows its year instead of the time of day, in the same 12 columns.

2. **Content-only mode (rustic diff: `m`, ignore metadata).** *Closed in `e2187ad` as `--bytes-only` / `:set bytes`.* restoric counted permission and owner changes as changes. A macOS external drive set to ignore ownership can change the owner of every file at once.

3. **Say what a restore will change (rustic's restore popup).** *Closed in `9edf0c9`.* `P` and the dialog's overwrite now say what differs on disk and what the restore writes before asking. This comes from the index, not a dry run.

4. **Permissions, owner and time of a file (rustic ls: Mode, User, Group, Time; `n` for numeric ids).** restoric shows size and mtime, but permissions and owner only in a diff. They could go on the preview's header line, `rw-r--r--  bege:staff  Jul 02 2024 10:00`, with names from restic's node `user` / `group`.

5. **More in `i`.** The user who ran the backup, delete protection ("kept forever" / "until DATE", rustic's 🛡), and the original id of a copied snapshot. Also `i` in the diff view, like rustic's diff `I`.

6. **Spot the odd backup at a glance.** rustic's table has a size column to scan. Options are `:biggest`, which jumps to the snapshot that added the most, or an optional timeline row of data added per snapshot.

Skipped: deduplication and repository-size statistics (whole index, about what forgetting would free), diffing two different paths (`:find` covers renames mostly), and marking several snapshots to sum them (`b` and `S` cover it).

## Already better in restoric

- rustic's `(+N)` folds snapshots with the same tree. restoric folds per folder, which is finer.
- `--filter-paths` matches the exact backup path only. restoric anchors on any folder inside a backup.
- rustic groups by host, label and paths. restoric's picker groups by host and backup path.
- rustic reloads with F5. restoric has `:reload` and a quiet reload every 5 minutes.
- rustic's `v` file view handles text only, up to 1 MiB, and not on cold repos. restoric's preview and diff do more.

## Not for restoric

- Setting labels, descriptions, tags, hostname or delete protection, marking to forget, and `w` to write: restoric never writes to the repository.
- The always-on key-hint footer: it doesn't fit the yazi style, the status bar has no room, and `?` covers it.
- `--password ""`: `--insecure-no-password` covers it.
