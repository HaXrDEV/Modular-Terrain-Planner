# Contributing

## Building

Install [Rust](https://rustup.rs). The toolchain version is pinned in
`rust-toolchain.toml` and installs itself on the first build.

```bash
cargo run
```

Everything CI checks, which should pass before a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

`cargo deny check` and `cargo machete` run in CI too. Render tests skip
without a graphics adapter; set `MTP_REQUIRE_GPU=1` to make them fail
instead, and `MTP_SOFTWARE_GPU=1` to use a software one.

## Branches and pull requests

- Every change gets a short-lived branch named after it in kebab-case
  (`paint-and-fill`, `review-fixes`) and a pull request against `main`.
  Nothing goes to `main` directly.
- The pull request title is a plain sentence saying what it does ("Paint
  and fill tiles"). The description starts with why, groups the changes
  under headings, and ends with **Checked:** (what was run and tried) and,
  where it applies, **Not tried yet**.
- Commits follow [Conventional Commits](https://www.conventionalcommits.org)
  with lowercase types (`feat`, `fix`, `refactor`, `docs`, `test`,
  `build`, `ci`, `chore`). Each commit is one coherent step that builds and
  passes the checks.
- Pull requests are merged with a merge commit, and the branch is deleted.

## Releases

1. A pull request "Release X.Y.Z" sets the version in `Cargo.toml` and
   `CHANGELOG.md`.
2. Once it's merged, tag `vX.Y.Z` on `main`. A `-rc1` suffix makes a
   prerelease.
3. The release workflow checks the tag against `Cargo.toml`, reruns every
   check, builds and self-tests each download, and publishes them together
   with checksums. If any build fails, nothing is published.

## Bug reports

Please include the app version, your system and graphics card (both are in
*Help > About*), and the end of the log (*Help > Open log folder*). Logs
contain file paths, which may include your user name.
