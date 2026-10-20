# Changelog

Notable changes to Kiled. The download is always the latest build
([`Kiled-arm64.dmg`](https://github.com/timwuhaotian/kiled/raw/main/releases/Kiled-arm64.dmg)); the
versions below are how we track releases, and match `Cargo.toml` and the app's Info.plist.

## Unreleased

## 0.0.1 — 2026-10-21

The first version: a code editor and nothing else.

- **VS Code's keys** by default, all rebindable in **Settings ▸ Keyboard** or
  `~/.config/kiled/keymap.toml`, as in Kil; the menus show the keys bound.
- **Editing** with multiple cursors (⌘D, ⇧⌘L, ⌥⌘↑↓, ⌥-click), VS Code's line commands, brackets
  and quotes that close themselves, auto-indent, comment toggling, whole-line copy and paste,
  undo that restores cursors, and IME input.
- **Syntax highlighting** with tree-sitter for Rust, TypeScript, TSX, JavaScript, JSX, JSON,
  JSONC, YAML and TOML, from Kil's five themes.
- **Language servers** for the same languages: diagnostics (pushed or pulled), completion,
  hover, go to definition, Format Document. TypeScript uses TypeScript 7's own server.
- **Find and replace**, **Go to File**, the **Command Palette**, **Go to Line**, Back and
  Forward.
- **Files**: an explorer, tabs, atomic saves that keep permissions, links and line endings,
  prompts before anything unsaved is closed, reloading files changed on disk, and the last
  session reopened at launch.
