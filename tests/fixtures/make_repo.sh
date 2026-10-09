#!/usr/bin/env bash
# Builds a small restic repository with a known history, using the real
# `restic` binary, for the index tests.
#
#   make_repo.sh OUTDIR
#
# OUTDIR gets `repo/` (password: restoric) and `src/` (the backed-up folder).
# Snapshots are taken with `--host restoric-fixture` and `--time` one day
# apart from 2026-01-01 00:00 UTC. tests/index_fixture.rs holds the expected
# change points; keep the two in step.
set -euo pipefail

out=${1:?usage: make_repo.sh OUTDIR}
export RESTIC_REPOSITORY="$out/repo"
export RESTIC_PASSWORD=restoric
src="$out/src"
day=0

mkdir -p "$src"
restic init --quiet

snap() {
  local t
  t=$(python3 -c "import datetime as d; print((d.datetime(2026,1,1)+d.timedelta(days=$day)).strftime('%Y-%m-%d %H:%M:%S'))")
  restic backup --quiet --host restoric-fixture --time "$t" "$src"
  day=$((day + 1))
}

# Times written to files are set explicitly, so `touch` always changes them.
stamp() { touch -t "202506010000.$1" "${@:2}"; }

# 0: first version
printf 'alpha\n' >"$src/a.txt"
printf 'bravo\n' >"$src/b.txt"
mkdir "$src/sub"
printf 'charlie\n' >"$src/sub/c.txt"
printf 'delta\n' >"$src/sub/d.txt"
printf 'keep\n' >"$src/keep.txt"
stamp 00 "$src/a.txt" "$src/b.txt" "$src/sub/c.txt" "$src/sub/d.txt" "$src/keep.txt"
snap
# 1: a.txt changes
printf 'alpha 2\n' >"$src/a.txt"
snap
# 2: b.txt is only touched (metadata only)
stamp 02 "$src/b.txt"
snap
# 3: sub/d.txt is deleted
rm "$src/sub/d.txt"
snap
# 4: a.txt permissions change
chmod 600 "$src/a.txt"
snap
# 5: nothing happens
snap
# 6: sub/d.txt comes back
printf 'delta\n' >"$src/sub/d.txt"
snap
# 7: keep.txt is renamed
mv "$src/keep.txt" "$src/kept.txt"
snap
# 8: metadata only: sub/c.txt touched, a.txt rewritten with the same bytes
stamp 08 "$src/sub/c.txt"
printf 'alpha 2\n' >"$src/a.txt"
snap
# 9: sub is deleted
rm -r "$src/sub"
snap
# 10: sub/c.txt comes back
mkdir "$src/sub"
printf 'charlie\n' >"$src/sub/c.txt"
snap
