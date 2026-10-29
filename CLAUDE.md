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
- Cargo.toml `version` must match the spec's `Version` (a test checks this).
- Keep Rust compatible with `rust-version = "1.84"` (AlmaLinux build).
- Layout must work in portrait and landscape.

## Versioning

Semantic versioning `MAJOR.MINOR.PATCH`; every package build from `main` must
have a version that has not been built before.

- **New feature** (a feature branch): bump the **minor** version and reset the
  patch to 0 (0.3.0 -> 0.4.0).
- **Bug fix**: bump the **patch** version only (0.4.0 -> 0.4.1).
- **Major** version: only the user changes it, when they say so. Never bump it
  on your own.
- Include the bump in the feature/fix branch itself. Update all of:
  `Cargo.toml` `version`, `Cargo.lock` (the `ax206mon` entry, e.g. by running
  `cargo build`), `packaging/ax206mon.spec` `Version:` (keep `Release: 1`), and
  add a `%changelog` entry at the top of the spec with the new version and a
  short summary.
- If two open branches bump to the same number, the one merged second takes
  the next number when it is updated from `main`.
