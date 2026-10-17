# ax206mon

Rust dashboard for AX206-protocol USB mini displays (1908:0102), run as a
systemd service. Config: `/etc/ax206mon/ax206mon.conf` (TOML, `[global]` +
`[[page]]`); the documented reference is `packaging/ax206mon.conf`.

## Workflow

- **One branch per feature or fix.** Start every new request from a fresh
  branch off the latest `origin/main` (`git fetch origin && git checkout -b
  claude/<short-feature-name> origin/main`). Don't stack unrelated work on an
  existing branch, and don't reuse a branch whose PR is merged.
- Keep commits focused with descriptive messages; push with
  `git push -u origin <branch>`.
- Open a pull request only when asked.

## Checks before committing

- `cargo test` and `cargo clippy --all-targets` must be clean.
- `AX206MON_PREVIEW_DIR=<dir> cargo test` dumps page PNGs for a visual check.
- `ax206mon preview --page N out.png` renders a page without hardware.

## Conventions

- Keep `packaging/ax206mon.conf`, `Config::default()` and `README.md` in sync
  (a test checks the packaged config against the defaults).
- Cargo.toml `version` must match the spec's `Version`.
- Keep Rust compatible with `rust-version = "1.84"` (AlmaLinux build).
- Layout must work in portrait and landscape.
