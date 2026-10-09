#!/usr/bin/env bash
# Builds a synthetic restic repository for scale testing.
#
#   make_big_repo.sh small|large|huge OUTDIR
#
# OUTDIR gets `repo/` (password: restoric) and `src/` (the backed-up tree).
# Snapshots are made with `restic backup --time` at one-hour steps and
# `--host restoric-big`. Between snapshots a few files change content, a few
# are only touched (metadata churn: same content, new mtime/ctime), and now
# and then files are added or deleted, so tree ids change more often than
# content fingerprints.
#
# Re-running continues from the last snapshot already in the repo.
# The repos are big; keep them out of git.
set -euo pipefail

size=${1:?usage: make_big_repo.sh small|large|huge OUTDIR}
out=${2:?usage: make_big_repo.sh small|large|huge OUTDIR}

case $size in
  small) snaps=50;    top=10;  mid=10;  files=10 ;;  #     1 000 files
  large) snaps=2000;  top=20;  mid=20;  files=500 ;; #   200 000 files
  huge)  snaps=20000; top=40;  mid=50;  files=1000 ;; # 2 000 000 files
  *) echo "unknown size: $size" >&2; exit 2 ;;
esac

export RESTIC_REPOSITORY="$out/repo"
export RESTIC_PASSWORD=restoric
export RESTIC_PROGRESS_FPS=0.016
src="$out/src"
host=restoric-big

mkdir -p "$out"
if [ ! -f "$RESTIC_REPOSITORY/config" ]; then
  restic init --quiet
fi

# The source tree: data/tNN/mNN/fNNNN.txt, plus one deep folder that changes
# rarely (data/deep/a/b/c/d/e) and one that never changes (data/static).
if [ ! -d "$src/data" ]; then
  python3 - "$src" "$top" "$mid" "$files" <<'EOF'
import os, sys
src, top, mid, files = sys.argv[1], *map(int, sys.argv[2:])
for t in range(top):
    for m in range(mid):
        d = f"{src}/data/t{t:02}/m{m:02}"
        os.makedirs(d, exist_ok=True)
        for f in range(files):
            with open(f"{d}/f{f:04}.txt", "w") as fh:
                fh.write(f"t{t} m{m} f{f} v0\n")
deep = f"{src}/data/deep/a/b/c/d/e"
os.makedirs(deep, exist_ok=True)
for f in range(20):
    with open(f"{deep}/notes{f:02}.md", "w") as fh:
        fh.write(f"# notes {f}\n\nversion 0\n")
os.makedirs(f"{src}/data/static", exist_ok=True)
for f in range(20):
    with open(f"{src}/data/static/s{f:02}.txt", "w") as fh:
        fh.write(f"static {f}\n")
EOF
fi

done_snaps=$(restic snapshots --host "$host" --json | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))')
echo "$size: $done_snaps/$snaps snapshots already there"

for ((i = done_snaps; i < snaps; i++)); do
  if ((i > 0)); then
    python3 - "$src" "$top" "$mid" "$files" "$i" <<'EOF'
import os, random, sys
src, top, mid, files, i = sys.argv[1], *map(int, sys.argv[2:])
rnd = random.Random(i)
def pick():
    return f"{src}/data/t{rnd.randrange(top):02}/m{rnd.randrange(mid):02}/f{rnd.randrange(files):04}.txt"
# content changes
for _ in range(rnd.randint(0, 5)):
    p = pick()
    if os.path.exists(p):
        with open(p, "a") as fh:
            fh.write(f"edit {i}\n")
# metadata-only changes: rewrite the same bytes (new mtime, ctime)
for _ in range(rnd.randint(1, 8)):
    p = pick()
    if os.path.exists(p):
        data = open(p, "rb").read()
        with open(p, "wb") as fh:
            fh.write(data)
# adds and deletes
if rnd.random() < 0.2:
    p = pick().replace(".txt", f".new{i}.txt")
    with open(p, "w") as fh:
        fh.write(f"added in {i}\n")
if rnd.random() < 0.1:
    p = pick()
    if os.path.exists(p):
        os.remove(p)
# the deep folder changes every 50 snapshots, and gets touched every 10
deep = f"{src}/data/deep/a/b/c/d/e"
if i % 50 == 0:
    with open(f"{deep}/notes{rnd.randrange(20):02}.md", "a") as fh:
        fh.write(f"edit {i}\n")
elif i % 10 == 0:
    os.utime(f"{deep}/notes{rnd.randrange(20):02}.md")
EOF
  fi
  t=$(date -u -d "2024-01-01 00:00:00 UTC + $i hours" '+%Y-%m-%d %H:%M:%S')
  (cd "$src" && restic backup --quiet --host "$host" --time "$t" "$src/data")
  if (( (i + 1) % 50 == 0 )); then echo "$size: $((i + 1))/$snaps"; fi
done
echo "$size: done"
