# restoric.yazi

Opens [restoric](../../README.md) on the hovered folder, to browse its history in your restic backups and restore old versions. On a file, it opens the file's folder with the file selected. `q` in restoric comes back to the same place in yazi.

It runs restoric through yazi's `shell --block`, so yazi steps aside while restoric has the terminal. Needs yazi 25.5 or newer.

## Install

Copy (or link) this folder to `~/.config/yazi/plugins/restoric.yazi`, then bind a key in `~/.config/yazi/keymap.toml`:

```toml
[[mgr.prepend_keymap]]
on   = "T"
run  = "plugin restoric"
desc = "Browse this folder's history (restoric)"
```

restoric reads the repository and password from the usual `RESTIC_*` environment variables, so set them where yazi can see them. Anything after `--` goes to restoric, for example `run = "plugin restoric -- --no-icons --host=old-name"`.
