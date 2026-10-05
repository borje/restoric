# restoric

A Rust TUI for browsing and restoring a restic repository by time, anchored on a folder. Built on rustic_core and ratatui, styled after yazi.

`PLAN.md` is the spec and the single source of truth. Read the parts you need before working:
- **Starting work:** §11 Progress (which milestone is next), then that milestone in §5.
- **UI work:** §3 (screens, keymap, commands, message wording). The mockup is `docs/mockup.html`; the text screens are `docs/screens/`.
- **Index or repo code:** §2 (change detection, only this machine) and §4 (crates, modules, `Repo` trait, cache, threads, scale rules in §4.8).
- **A design question:** §8 lists what's open and what's settled. A settled item is a decision; to change one, ask the user.

## How to work
- Work milestone by milestone. A milestone is done when its "Done when" criteria in §5 pass. Then tick it in §11 with the commit hash.
- When the user decides something new, record it in `PLAN.md` (the relevant section, and §8 "Settled") in the same change.
- Keep `PLAN.md` describing what is true. Update it whenever the code departs from it.

## Boundaries that hold the design together
- Everything outside `src/repo/` talks to the repository only through the `Repo` trait. rustic_core types stay inside `src/repo/rustic.rs`.
- The UI thread draws from `App` state and caches only. Repository access goes through `worker.rs` requests.
- Work scales with what's on screen (§4.8). Code that walks a whole snapshot or loads a whole list needs a reason the user asked for it.
- restoric never writes to the repository.
- Every new screen or state gets an insta snapshot test driven by `FakeRepo`.
