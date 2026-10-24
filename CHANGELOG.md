# Changelog

Notable changes to Kiled. The download is always the latest build
([`Kiled-arm64.dmg`](https://github.com/timwuhaotian/kiled/raw/main/releases/Kiled-arm64.dmg)); the
versions below are how we track releases, and match `Cargo.toml` and the app's Info.plist.

## Unreleased

## 0.0.4 — 2026-10-24

- **Kiled updates itself.** It checks at launch and every six hours, downloads a new release in
  the background, checks its checksum, Developer ID signature and notarization, and installs
  it when Kiled quits. **Kiled ▸ Check for Updates…** (in the Command Palette too) checks now
  and offers **Restart Now**, which asks about unsaved files as Quit does. Turn it off with
  **Settings ▸ Updates** or `auto_update = false`. This release has to be installed by hand
  once; later ones arrive on their own.

## 0.0.3 — 2026-10-24

- **Go to File** lists the files you opened lately first, so ⌘P and Enter go back to the
  previous file. While a big folder is still being listed it says so, instead of asking you
  to open a folder, and fills in when done. ⌘P pressed again steps down the list.
- **Revert File** (File menu, Command Palette) puts the file back as it is on disk, as one
  undoable step. Saving a file someone else changed offers **Reload from Disk** beside
  Overwrite.
- A language server that isn't installed is noted quietly in the status bar, with how to
  install it, not in the color of a failure.
- Two tabs with the same file name show their folders. Escape in the explorer goes back to
  the editor. The window's close button shows the edited dot while a tab is unsaved.

## 0.0.2 — 2026-10-24

- **Tabs** show their file's full path when the pointer rests on them.
- **Go to File** and the **Command Palette** open any file on the Mac by its path: type
  `~/.npmrc` or `/etc/hosts`, with or without a folder open.

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
- **Files**: an explorer, tabs, atomic saves that keep permissions, group, ACLs, extended
  attributes, links and line endings (read-only files are refused), prompts before anything
  unsaved is closed, reloading files changed on disk, and the last session reopened at
  launch.
