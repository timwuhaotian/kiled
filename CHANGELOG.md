# Changelog

Notable changes to Kiled. The download is always the latest build
([`Kiled-arm64.dmg`](https://raw.githubusercontent.com/timwuhaotian/kiled/main/releases/Kiled-arm64.dmg)); the
versions below are how we track releases, and match `Cargo.toml` and the app's Info.plist.

## Unreleased

## 0.0.8 — 2026-11-09

- **Editing acts like VS Code in more corners.** Tab with a selection indents the
  lines instead of replacing the text; a cursor before a closer steps over it even
  when other cursors insert; the status bar counts a tab as one column, so Go to
  Line and reopened sessions land where shown; clicks below the last line land on
  it; shift-click extends from the newest cursor.
- Trigger Suggest and Format Document say there is no language server for the file
  instead of staying silent.
- **Updates download again.** GitHub's `github.com/<repo>/raw/<branch>/<file>` link answers
  **504**, so the updater could not download a release (**Kiled can't update right now**),
  and the README's download link was dead too. Every file Kiled fetches now comes from
  `raw.githubusercontent.com`, which serves the same bytes, and a failure no longer says
  `curl: curl:`. Because the broken link is in the updater itself, this release has to be
  installed by hand once; later ones arrive on their own again.

## 0.0.7 — 2026-11-08

- **Drop files on the window to open them.** Files open as tabs and a folder becomes the
  workspace's folder, as Finder's Open With does; unsaved changes are asked about first.
- **Keyboard shortcuts can be changed again.** Recording a new shortcut in
  **Settings ▸ Keyboard** swallowed the keypress and saved nothing, and
  **Keyboard Shortcuts…** (⌘K ⌘S) wrote `keymap.toml` without ever opening it. Both work
  now, and pressing fn on its own is no longer taken as a shortcut.

## 0.0.6 — 2026-11-08

- **The explorer manages files now.** New File and New Folder (named in place in the tree;
  a new file opens at once), **F2** renames, and **⌘⌫** moves to the Trash after asking —
  a file with unsaved changes is refused until it is saved or closed. Renaming a file or
  folder takes its open editors with it. The sidebar also has Collapse and Refresh, and
  Home/End jump to the first and last rows.
- **Reveal in Finder**, **Copy Path** and **Copy Relative Path** for the file in front (File
  menu, Command Palette).
- **Close Other Editors**, **Close Editors to the Right** and **Close Saved Editors** (⌘K U),
  asking about unsaved changes as closing one does.
- **A new icon**, Kil's sibling redrawn on killi's: the same iced square, stick and spark,
  with code lines and a text cursor where killi has its `>_`.

## 0.0.5 — 2026-10-25

- **Tabs sit on solid backgrounds of their own** instead of the tab bar showing through:
  the active tab is the editor's surface and runs into the editor, inactive tabs are
  distinct and brighten under the pointer. An unsaved file's dot gives way to × under the
  pointer, which is also when an inactive tab shows its ×, and long file names truncate.

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
