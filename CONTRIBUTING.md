# Contributing to Kiled

Thanks for helping. Bug reports, fixes and features are all welcome. For anything larger than
a fix, open an issue first so we can agree on the shape before you write the code. Kiled
stays a code editor and nothing more: features that belong to another tool (a terminal,
git, tasks) or need a plugin system are out of scope.

## Setup

You need a Mac with Apple silicon, Xcode with its Metal toolchain, and Rust 1.94 or later. The
formatter needs a nightly toolchain for its import options:

```sh
rustup toolchain install nightly --component rustfmt
cargo run -p kiled -- .          # debug build, straight from the tree, on this folder
scripts/bundle.sh debug          # or a Kiled.app in target/debug
```

## Before you open a pull request

CI runs these on Apple silicon; run them first:

```sh
cargo +nightly fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

With Homebrew's `cargo` first on `PATH`, `+nightly` isn't understood; use
`rustup run nightly cargo fmt --all` instead. Two kinds of tests are opt-in: the typing
latency measurement (`--test latency`, in release) and the tests against real language
servers (`--test servers`), which skip any server that isn't installed.

## How we work

- **[`DESIGN.md`](DESIGN.md) is the spec.** Its acceptance table (A1, A2, …) says what Kiled
  does and how each behavior is proven. A change in behavior updates `DESIGN.md`, and
  `README.md` if users see it, in the same pull request.
- **Every fix comes with a test** that fails without it, where the behavior can be tested
  (the core's pure logic, a real server, or GPUI's `TestAppContext`). Things only a person
  can check, like Pinyin input, go in the pull request's description.
- **Keep the layers.** `kiled-core` has no UI and owns the text, undo, positions, files and
  the language server processes; `kiled` is the GPUI app. Unsafe code lives in
  `kiled-core/src/sys.rs`, each block with a `SAFETY` comment.
- **Keys follow VS Code.** A new command gets VS Code's macOS default, if it has one, in
  `keymap.rs`'s table.
- **`vendor/gpui`** is Kil's gpui 0.2.2; its `KIL.md` says what changed and how to update it.
  Mark any change there `Modified for Kil`.
- **Comments explain why**, not what. Match the density and style of the code around you.
- **Commit messages** start with a short summary of what now happens ("Pulled diagnostics
  follow the text"), then explain the problem and the fix.
- **Licensing:** Kiled is MIT. Don't copy code from Zed's `editor`, `language` or `lsp`
  crates; they are GPL-3.0. GPUI (Apache-2.0) and tree-sitter (MIT) are fine to build on.

## Releases

Releases are built and notarized locally by the maintainer; [`RELEASING.md`](RELEASING.md)
has the steps. Each one replaces `releases/Kiled-arm64.dmg`, bumps the version in `Cargo.toml`,
and dates the `Unreleased` entries in [`CHANGELOG.md`](CHANGELOG.md).

By contributing, you agree that your contributions are licensed under the MIT license, and
to follow the [code of conduct](CODE_OF_CONDUCT.md).
