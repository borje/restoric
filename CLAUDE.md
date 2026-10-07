# restoric

A Rust TUI for browsing and restoring a restic repository by time, anchored on a folder. Built on rustic_core and ratatui, styled after yazi.

The mockup for the UI is `docs/mockup.html`.

## Boundaries that hold the design together
- Everything outside `src/repo/` talks to the repository only through the `Repo` trait. rustic_core types stay inside `src/repo/rustic.rs`.
- The UI thread draws from `App` state and caches only. Repository access goes through `worker.rs` requests.
- Work scales with what's on screen. Code that walks a whole snapshot or loads a whole list needs a reason the user asked for it.
- restoric never writes to the repository.
- Every new screen or state gets an insta snapshot test driven by `FakeRepo`.
- Files restoric keeps live in the XDG folders on every platform, through `src/dirs.rs`. Nothing else calls `directories` directly, and nothing goes under `~/Library` on macOS.
