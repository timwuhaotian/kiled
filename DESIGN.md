# kiled: design and plan

A native macOS code editor written in Rust, and Kil's sibling: the same GPUI stack, the same
themes, the same way of rebinding keys. It edits code and nothing else — no terminal, no
extensions, no git panel. The priorities are, in order: never lose what the user typed,
latency, and keeping every process it starts on a leash. VS Code users should feel at home
on day one: its keys are the defaults.

Every claim marked **[measured]** was taken on an M4 Pro, macOS 27.0, rustc 1.94.0, release
build.

Build status (0.0.1):
- The core (`kiled-core`): text, selections, undo, editing commands, syntax, search, files,
  and the LSP client, unit-tested without a UI (100 tests), plus opt-in tests against real
  rust-analyzer and TypeScript 7 (§6.5).
- The app (`kiled`): window, editor, find, Quick Open, Command Palette, explorer, Settings
  and the keymap; GPUI tests drive keys, input and clicks through the real dispatch (52
  tests). The built app was run against real servers, with screenshots of highlighting,
  diagnostics from all five servers, completion, hover, find, Quick Open, the palette and
  Settings (A6–A9), and measured (A13, A14). Still to be checked by a person: Pinyin input,
  dragging the window by its title bar, Finder's Open With, and the save prompts by hand.
- Release signing and notarization are scripted (§11) like Kil's; the first release is cut
  by `scripts/release.sh`.

---

## 1. Outcome

### Acceptance (0.0.1): observable behavior

| # | Behavior | Proof |
|---|----------|-------|
| A1 | `kiled <folder> [files…]`, `open -a Kiled <path>`, Finder's Open With and **⌘O** open a folder in the explorer and files as tabs. A launch with no paths reopens the last folder, its tabs, their cursors and the window's frame. | `workspace` tests; manual. |
| A2 | Typing goes through the IME handler (Pinyin composes in place, commits as one undo step). Brackets and the language's quotes pair themselves before blanks, wrap a selection, and are typed over; Enter keeps indentation, indents after an opener and splits `{}` onto three lines; Backspace removes an empty pair and steps back to indent stops. | `ops` tests, `editor` tests (IME commit, auto-close); manual Pinyin. |
| A3 | Multiple cursors as VS Code makes them: **⌘D** (word, then each next whole-word match), **⇧⌘L**, **⌥⌘↑/↓**, **⌥-click**; Escape goes back to one. Every edit, copy and paste applies at each cursor; a paste with as many lines as cursors spreads one line each. | `ops` tests; `cmd_d_should_add…`, `alt_click_should_add_a_cursor`. |
| A4 | VS Code's line commands: **⌥↑/↓** move lines, **⇧⌥↑/↓** copy them, **⇧⌘K** deletes them, **⌘↩/⇧⌘↩** open a line, **⌘]/⌘[** indent, **⌘/** toggles comments, **⌘L** selects lines, **⌘C/⌘X** with nothing selected take the whole line and paste it back as a line. | `ops` tests; `the_default_keys_should_edit_like_vs_code`. |
| A5 | Undo and redo restore text and cursors; typing undoes word-runs at a time (one step per pause or cursor move). A document undone back to its saved text is clean again. | `document` tests. |
| A6 | Syntax highlighting with tree-sitter for Rust, TypeScript, TSX, JavaScript, JSX, JSON, JSONC, YAML and TOML; any other file opens as plain text. Only the lines in view are highlighted. | `syntax` / `language` tests; screenshots. |
| A7 | Language servers for those languages start on the first file and stay per project root: rust-analyzer, TypeScript 7's own server (`tsc --lsp`, the project's install first; typescript-language-server for TypeScript ≤ 6), vscode-json-language-server, yaml-language-server, taplo. Diagnostics (pushed or pulled) as squiggles, a count in the status bar, marks on the scrollbar, **F8/⇧F8** to step through; completion as you type and on **⌃Space**; hover on rest and **⌘K ⌘I**; **F12**/⌘-click to the definition (in another file too); **⇧⌥F** formats. A server that isn't installed shows how to install it. | `servers` tests against real servers; screenshots. |
| A8 | Find (**⌘F**) highlights every match as you type, says "3 of 12", steps with Enter/⇧Enter/⌘G/⇧⌘G, and toggles case, whole word and regex (**⌥⌘C/W/R**). Replace (**⌥⌘F**) replaces one or all, all as one undo step. | `find` tests. |
| A9 | **⌘P** opens files of the folder by fuzzy name (`.gitignore` respected), **⇧⌘P** runs any command by name and shows its keys, **⌃G** goes to `line[:column]`. **⌃-** / **⌃⇧-** go back and forward across jumps. | `workspace` / `picker` tests. |
| A10 | Nothing unsaved is lost: closing a dirty tab, opening another folder, closing the window and quitting ask Save / Don't Save / Cancel. A file changed on disk reloads if it has no edits; with edits, the user is told, and saving asks before overwriting. Saving replaces a file atomically (temp file, fsync, rename) with its permissions, group, ACL and extended attributes, through a symlink that stays one; hard links and others' files are written in place and put back if that fails part way; a read-only file is refused. Line endings and a BOM are kept. Binary and non-UTF-8 files are refused, not mangled. | `files` / `document` / `workspace` tests. |
| A11 | **Keys are VS Code's** (§8), all rebindable: **Settings ▸ Keyboard** records a shortcut per command (refusing one that is taken, with **Reset**), and `~/.config/kiled/keymap.toml` takes any binding, two-stroke ones included. **Keyboard Shortcuts…** (**⌘K ⌘S**) opens that file in Kiled. Changes apply when the window is focused again; the menus show the keys bound. | `keymap` / `config` / `settings_window` tests. |
| A12 | **Settings…** (**⌘,**): Kil's five themes, sidebar opacity, font family and size, tab size, spaces or tabs, cursor blink; `config.toml` holds them and `[servers]` commands. **⌘= ⌘- ⌘0** zoom. | `settings` tests; manual. |
| A13 | **Performance.** Idle CPU 0.0% with the cursor not blinking, < 1% blinking. A keystroke (edit, incremental reparse, highlighting a screenful) under 1 ms on average on a 50,000-line file. First frame within 200 ms of launch. | §5.4 [measured]. |
| A14 | **Process hygiene.** Each language server runs in its own process group; stopping it (or its output closing) gives it 1 s, then kills the group and the server itself, then reaps it. Quitting sends the groups SIGTERM and never waits on a server; nothing outlives Kiled, a `kill -9` of Kiled included. A server that doesn't answer `initialize` in 30 s is stopped and says so. | `client` tests (a fake server's background child dies with it; one that leaves its group is still killed, without holding up quitting); `lsp` tests; manual (§6.3). |
| A15 | The release is signed with Developer ID, notarized and stapled; `spctl -a -vv` accepts it. | §11 |

### Non-goals (0.0.1)

Left out on purpose; each can come later without reshaping what exists.
- A terminal, tasks, a debugger, git views, extensions or plugins of any kind. Kiled edits code.
- Split editors and more than one window. Soft wrap, a minimap, folding.
- Find in files, rename symbol, references, code actions, signature help, inlay hints,
  semantic tokens, snippets.
- Creating, renaming and deleting files from the explorer (do it in Finder or a terminal).
- Grapheme clusters: the cursor moves by code point (an emoji with modifiers takes several
  steps).
- Encodings other than UTF-8; files over 256 MB. Over 8 MB a file opens without syntax and
  without a language server.
- Auto-update (Kil has one; Kiled's comes later), Intel builds, the Mac App Store.
- Installing language servers: Kiled finds them, and says how to install one it doesn't find.

---

## 2. Stack

| Concern | Choice | Why |
|---|---|---|
| UI, GPU, text, input | **`gpui = "=0.2.2"`** from `vendor/gpui`, Kil's copy | Metal rendering, actions and key contexts (the rebindable keymap), `EntityInputHandler` (IME), cached views, transparent titlebar. The vendored copy carries Kil's one change: a window with nothing to draw stops its display link, so an idle window costs 0% CPU (`vendor/gpui/KIL.md`). |
| Text | **`ropey` 1.6**, LF-only lines | O(log n) edits and line/char/UTF-16 conversions. Built without CR and Unicode line breaks: text is normalized on load, so the editor, tree-sitter and LSP count lines the same way. |
| Syntax | **`tree-sitter` 0.27** and the grammars' own crates | Incremental reparsing and the grammars' highlight queries. |
| Search | **`regex`** | Unicode case folding without changing offsets. |
| Project files | **`ignore`** | ripgrep's `.gitignore`-aware walk for Quick Open. |
| LSP | Our own client over `serde_json` | Twelve messages are used; owning the process is the hard part (§6.3), not the types. No `lsp-types`. |
| Config | `serde`, `toml` | Same files and conventions as Kil. |
| Errors, logs | `thiserror` in the core, `anyhow` at the app's edge; `log` + `env_logger` | As Kil. |

**Rejected:** Zed's `editor`, `language` and `lsp` crates (GPL-3.0, and far larger than a
plain editor needs); `lsp-types` (its churn for twelve messages); a DOM editor (Monaco in a
webview: the latency budget).

**Licensing.** Kiled is MIT. GPUI is Apache-2.0, tree-sitter and its grammars MIT. Nothing is
copied from Zed's GPL crates; editing behavior is written from VS Code's documented
behavior, LSP from the specification.

---

## 3. Architecture

```
kiled/
├─ crates/
│  ├─ kiled-core/             NO UI deps. Everything that must be right, tested without GPUI.
│  │  ├─ src/text.rs          line endings, indentation detection, display columns
│  │  ├─ src/selection.rs     Selection, Selections (sorted, merged, primary = oldest)
│  │  ├─ src/edit.rs          Change, mapping offsets through simultaneous changes
│  │  ├─ src/document.rs      Document: text + selections + undo + syntax + diagnostics + LSP log + IME preedit
│  │  ├─ src/movement.rs      word / line / vertical motion as VS Code moves
│  │  ├─ src/ops.rs           every editing command, at every cursor, one transaction each
│  │  ├─ src/syntax.rs        tree-sitter tree kept in step; highlights of a byte range
│  │  ├─ src/language.rs      the language and server registry
│  │  ├─ src/search.rs        find / replace queries
│  │  ├─ src/fuzzy.rs         Quick Open / palette scoring
│  │  ├─ src/files.rs         load (refusing binary / non-UTF-8), atomic save, folder listing
│  │  ├─ src/lsp/             client (process, threads, requests), protocol, finding servers
│  │  ├─ src/sys.rs           the ONLY unsafe: killpg, waitid(WNOWAIT), fcopyfile; each with SAFETY
│  │  └─ tests/               latency (§5.4), real servers (§6.5)
│  └─ kiled/                  GPUI app
│     ├─ src/main.rs          launch, menus, app actions
│     ├─ src/workspace.rs     window root: explorer, tabs, status bar, open/save/close/quit
│     ├─ src/editor.rs        Editor view: actions, mouse, IME, LSP sync
│     ├─ src/editor_element.rs  the element that lays out and paints the lines in view
│     ├─ src/completion.rs    completion, hover, definition, formatting, problems
│     ├─ src/find.rs          find bar
│     ├─ src/picker.rs        Quick Open / Command Palette / Go to Line
│     ├─ src/file_tree.rs     explorer
│     ├─ src/lsp.rs           the window's servers, per server and root
│     ├─ src/keymap.rs        the command table: names, titles, scopes, VS Code defaults
│     ├─ src/config.rs  settings.rs  settings_window.rs  theme.rs  text_field.rs  session.rs
├─ vendor/gpui/               Kil's gpui 0.2.2 (vendor/gpui/KIL.md)
├─ packaging/                 Info.plist, entitlements, app-icon.png
└─ scripts/                   bundle.sh, release.sh
```

The split has the same purpose as Kil's: `kiled-core` holds every invariant about text,
undo, positions, files and processes, and compiles and tests in seconds without GPUI. The app
renders and routes input.

**Ownership.** `Workspace` owns the editors (`Entity<Editor>`), the explorer, the picker and
the window's `Lsp`. Each `Editor` owns its `Document`. `Lsp` owns `Arc<Client>`s; each
`Client` owns a writer channel, and one thread owns the child process (§6.3). Editors and the
LSP store reach each other only through `WeakEntity` and never re-enter: an editor calls the
store with what a message needs, and the store reads or updates editors only from its own
tasks (a server finishing `initialize`, an event), after the editor's update is over.

---

## 4. Text and editing

### 4.1 Coordinates

Offsets are chars everywhere in the core. Bytes appear only at the tree-sitter edge, UTF-16
code units at the LSP and IME edges (ropey converts both in O(log n)), and display columns
(tabs expanded, wide chars counted twice by `unicode-width`) where the grid is drawn and for
vertical motion's goal column.

### 4.2 Changes and transactions

Every edit is a set of `Change { range, text }` that apply at once to the same text, sorted
and non-overlapping (`edit::coalesce` merges overlaps, e.g. two cursors deleting into each
other). `Document::transact` applies them **last first**, so each change's offsets are still
valid when it is made and the LSP log reads in order; it edits the syntax tree before each
change, reparses once after, maps diagnostics through the changes, and records one undo
step with the selections on both sides.

Undo steps coalesce while the same kind of edit (typing, deleting) continues within 1.5 s
without the cursor moving; never across a save. A revision number per state makes "dirty"
exact: undoing back to the saved text is clean.

**IME.** The preedit is in the text (so it is drawn in place and the IME's ranges hold) but
in neither the undo history nor the LSP log, and the server is sent the text without it.
Text a composition replaces (a selection typed over) is deleted for real as it starts, an
undoable edit the server hears of. Committing takes the preedit out without a trace and
types the result for real; an edit while composing commits first.

### 4.3 Commands

`ops.rs` implements each command once for any number of cursors: an `Edit` per selection
(range, text, where the selection lands within the replacement), turned into one
transaction. Line commands work on merged blocks of covered lines (a selection ending at a
line's start doesn't cover that line). Behaviors follow VS Code: Home toggles between the
first non-blank and column 0; word motions stop at line ends; ⌘D selects the word, then adds
whole-word matches (case-sensitive), wrapping; Escape keeps the oldest cursor; a copy with
nothing selected is whole lines and pastes above the line (clipboard metadata
`kiled-whole-lines`); a paste of N lines onto N cursors spreads.

Auto-closing: `()[]{}` everywhere, and the language's quotes (`"` only for Rust, where `'`
starts lifetimes; `"'\`` for JS/TS). A pair opens only before a blank, a closer or the line's
end, and a quote not after a word char. A closer is typed over when it is next; a closing
bracket on a blank line outdents it.

### 4.4 Files

`files::load` refuses files over 256 MB, with a NUL in the first 8000 bytes (git's rule),
or not UTF-8. The document notes and removes a BOM and normalizes line endings (CRLF wins if
most breaks were CRLF); `text_for_save` puts both back.

`files::save` follows a symlink to the file it names (the link stays) and replaces that
file whole: write a temp file beside it, give it the old file's group, extended attributes
and permissions, fsync, rename over it, then copy the ACL across (`fcopyfile`; last, as an
ACL may forbid deleting the temp file). A full disk while writing the temp file leaves the
original untouched. A hard-linked file, one someone else owns or whose group we can't
give, one an ACL keeps from being replaced, or one in a folder we can't write is written in
place instead, so its names, owner and metadata stay: never truncated first, and put back
from memory if the write fails part way. A file we may not write is refused ("it is
read-only"); renaming over it would have worked, so that is asked first.

Changes on disk are noticed when the window is focused (no watcher thread, as Kil): a clean
editor reloads (one undoable step), a dirty one keeps its edits and is flagged; saving it
asks before overwriting.

### 4.5 Find

Every mode compiles to one `regex` (`(?i)` for case-insensitive, `\b…\b` for whole word,
escaped for literal). Matches are found over the text on each change while the bar is open,
capped at 10,000. The selection follows the first match from the cursor as you type, as in
VS Code. Replace All is one transaction.

---

## 5. Syntax and rendering

### 5.1 Languages

`language.rs` is the registry: name, LSP language id, extensions and whole file names
(`Cargo.lock`, `tsconfig.json` → JSONC), comment token, quotes, indent-after chars, grammar,
highlight query (TypeScript's is JavaScript's plus its own, as its `tree-sitter.json` says),
and the servers to try. Adding a language is one entry. Queries compile on a background
thread at launch, so the first file of a language doesn't wait for its query (Rust's takes
~20 ms).

### 5.2 Highlighting

The tree is edited per change and reparsed once per transaction. Highlights run the query
over the visible bytes only. Captures paint outer nodes first and inner ones over them; for
two captures of one node, the one with the more specific name wins
(`string.special.key` over `string`), else the later pattern. Both rules are needed: the
grammars' queries are written both ways (JSON lists its key pattern first, JavaScript its
generic `variable` first). Capture names map to 16 highlight kinds, and each theme colors
them from its Kil terminal palette, so code looks the same in a Kil pane and in Kiled.

### 5.3 The editor element

Only lines in view are laid out, and of each only the columns in view. Text sits on a grid,
as in Kil: a run of ASCII is shaped as one line and placed at its column; any other char is
placed alone at its column, so a CJK or fallback glyph can't push the line off the grid and
clicks, cursors and selections are column arithmetic. Shaped lines are reused across frames
by GPUI's line cache, so scrolling shapes only new lines and typing only the changed one.
The text is clipped to its area; line numbers, cursors (2 px bars), selections, find
matches, diagnostic squiggles (wavy underlines; hints aren't underlined), the scrollbar thumb
and problem marks on its track are drawn around it. The editor is a cached view: the status
bar, tabs and sidebar redraw without laying it out again.

### 5.4 Budgets [measured]

| Metric | Target | Measured |
|---|---|---|
| Idle CPU, cursor not blinking | 0.0% | **0.00%** over 30 s (`ps` CPU time) |
| Idle CPU, cursor blinking | < 1% | **0.83%** |
| Keystroke: edit + reparse + highlight a screenful | < 1 ms avg | **0.28–0.33 ms** avg (2.3k-line Rust file), **0.14–0.15 ms** (51k lines, 1.7 MB); worst key ~1.5 ms |
| Open a 51k-line Rust file (rope, indent detection, full parse) | — | **96 ms** |
| First frame after launch, a Rust file open | ≤ 200 ms | **~126 ms** (log line) |
| Memory, one file open | — | **~111 MB** RSS (mostly GPUI and Metal) |

Typing latency is `cargo test --release -p kiled-core --test latency -- --ignored
--nocapture`. A pathological case is noted for the record: many simultaneous syntax errors
make tree-sitter's error recovery expensive. With an unclosed `((` added to every 25th line
of that test's smaller file (2,561 lines, 265 error nodes), a key costs ~6.5 ms with
tree-sitter 0.27 (~8 ms with 0.25).

---

## 6. Language servers

### 6.1 Finding a server

An app opened from the Dock gets launchd's bare `PATH`; servers live where shell profiles
look. On the first file that needs a server, Kiled asks an interactive login shell for its
`PATH` once (5 s timeout, in the background), then searches: the login `PATH`, the app's own,
then `~/.cargo/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.bun/bin`,
`~/.volta/bin`. An npm server is looked for first in the project's own `node_modules/.bin`.
Servers with a minimum package version are checked by reading the npm package's
`package.json` next to the resolved binary, without running it.

A language lists servers in order of preference; the first one installed serves it.
TypeScript and JavaScript use TypeScript 7's built-in server (`tsc --lsp --stdio`, native and
fast) when the TypeScript found is 7 or later, else `typescript-language-server` (which needs
TypeScript ≤ 6, as TypeScript 7 no longer ships `tsserver.js` — found while building 0.0.1).
`[servers]` in `config.toml` replaces a server's command, or turns it off with `[]`.

One server runs per (server, project root). The root is the open folder when the file is in
it, else the nearest folder above with one of the server's markers (`Cargo.toml`,
`package.json`…), else the file's folder.

### 6.2 What Kiled speaks

UTF-16 positions; incremental sync (or the whole text for a server that asks for it);
`didOpen/didChange/didSave/didClose`; `publishDiagnostics` and pull diagnostics
(`textDocument/diagnostic`, asked 250 ms after typing pauses, dropped if the text changed
meanwhile; `workspace/diagnostic/refresh` asks again); completion (plain text, no snippets;
filtered client-side as you type, asked again when the server says the list is incomplete;
the item's extra edits, such as an import, applied, or fetched with `completionItem/resolve`
and applied if nothing was typed since and they lie before the completed word); hover
(markdown reduced to text); definition (location or link); formatting (applied only to the
text it was computed for); `$/progress` (in the status bar); `showMessage` errors (as a
notice). The server's own requests are answered by the client: no configuration, one
workspace folder, yes to registration and progress.

### 6.3 Processes

Each server runs with piped stdio in a **process group of its own**. Four threads per server:
a writer (owns stdin; the channel closing closes it), a reader (frames, routes answers to
waiting requests by id, events to the app; on EOF fails what is waiting), a stderr reader
(logs, keeps the last lines to explain an exit), and an owner thread that owns the `Child`.

The owner waits for a stop request, the output closing, or the client going away; then gives
the server 1 s to exit (polling with `waitid(…WNOWAIT)`, which doesn't reap), then
`killpg(SIGKILL)` on the group and SIGKILL to the server's own pid (in case it left the
group), and only then reaps. **A pid isn't reused until it is reaped**, so the group
signalled is always the server's — this is what makes killing the group safe, and what
kills a server's own children (tsserver, rust-analyzer's proc-macro server) with it.
Quitting can't wait, so it sends SIGTERM to every group still unreaped, checked under the
lock the reaper sets; a server whose reaper holds that lock is being killed already, and is
passed over rather than waited for.

Stopping politely is `shutdown` (≤ 1 s for the answer), `exit`, then the above. Quitting
Kiled, closing the window, or a `kill -9` of Kiled leaves no server behind **[measured]**: the
first two signal the groups, and on the last every server sees its stdin close and exits.

### 6.4 Status

The status bar shows the active file's server: `starting…`, its progress
(`rust-analyzer: Indexing 45%`), or why it failed (`not found; install with …`; or
`didn't start, install with … (exited (1): <its last stderr line>)` when it died before
initializing, as a rustup proxy without its component does — the hint first, since the bar
cuts long text at its end). One that doesn't answer `initialize` within 30 s, or answers it
with an error, is stopped and keeps that reason (`didn't answer in 30 s`).

### 6.5 Tests

`kiled-core/tests/servers.rs` runs real servers when installed (`KILED_TEST_PATH` adds
directories): rust-analyzer (diagnostics, completion after incremental edits, hover,
definition) and TypeScript 7 (pulled diagnostics, completion, hover). They are `#[ignore]`d,
so CI doesn't need the servers. `client.rs` tests a fake server written in shell, including
that a background child it started dies with its group.

---

## 7. UI

**Window.** One window, transparent titlebar, traffic lights at (14, 12), Kil's blurred
background behind a sidebar that is 92% opaque by default (file names must read over a light
desktop too; Settings lowers it). Top bar 38 px: the sidebar's title strip and the tabs;
double-clicking empty title bar does what System Settings says.

**Sidebar** (240 px, **⌘B**): the folder's name, then the explorer — folders first, each
read when first expanded and again when the window is focused; the active file revealed and
marked; ↑↓←→ and Enter when focused (**⇧⌘E**).

**Tabs**: one per open file, the active one with an accent top border; ● for unsaved, × on
the others; middle-click closes.

**Status bar**: problems (click: next), the server's status, a short message ("No definition
found"); then `Ln, Col` with the selection or cursor count, indentation, line ending,
language.

**Overlays**: Quick Open / palette under the top bar; completion under the word, hover
under it too, both drawn above everything; find bar at the editor's top right; notices at
the top right, a click dismisses them.

**Session**: `~/Library/Application Support/kiled/session.toml` — folder, files with cursors,
active file, sidebar, window frame — saved at most every 2 s after a change and when quitting;
only reopened when the launch names no paths.

---

## 8. Input and keymap

VS Code's macOS defaults, by name in `keymap.rs`'s one table (name, title, group, scope,
action, keys). Scopes are GPUI key contexts: `Workspace` (window), `Editor`, or none (app:
Settings, Hide, Quit, which work in the Settings window too).

| Keys | Command | Keys | Command |
|---|---|---|---|
| ⌘N / ⌘O | new file / open file or folder | ⌘P | go to file |
| ⌘S / ⇧⌘S / ⌥⌘S | save / save as / save all | ⇧⌘P, F1 | command palette |
| ⌘W / ⌘K ⌘W | close editor / close all | ⌃G | go to line |
| ⇧⌘T | reopen closed editor | F12, ⌘-click | go to definition |
| ⌥⌘→ ⌥⌘←, ⇧⌘] ⇧⌘[, ⌃⇥ ⌃⇧⇥ | next / previous editor | ⌃- / ⌃⇧- | back / forward |
| ⌃1 … ⌃9 | editor 1 … last | F8 / ⇧F8 | next / previous problem |
| ⌘Z / ⇧⌘Z | undo / redo | ⌃Space, ⌘I | trigger suggest |
| ⌘X ⌘C ⌘V | cut / copy / paste | ⌘K ⌘I | show hover |
| ⌘F / ⌥⌘F | find / replace | ⇧⌥F | format document |
| ⌘G / ⇧⌘G | find next / previous | ⌘B / ⇧⌘E | sidebar / explorer |
| ⌘D / ⇧⌘L | add next occurrence / all | ⌘= ⌘- ⌘0 | zoom |
| ⌥⌘↑ / ⌥⌘↓, ⌥-click | add cursor above / below | ⌘, | settings |
| ⌘L / ⌘A | expand line selection / all | ⌘K ⌘S | keyboard shortcuts (keymap.toml) |
| ⌥↑ ⌥↓ / ⇧⌥↑ ⇧⌥↓ | move / copy lines | ⌘/ | toggle line comment |
| ⇧⌘K | delete line | ⌘] ⌘[ , ⇧⇥ | indent / outdent |
| ⌘↩ / ⇧⌘↩ | insert line below / above | Esc | single cursor / close |
| ⌥⌫ ⌥⌦, ⌘⌫ ⌘⌦ ⌃K | delete word / to line edge | ⌘← ⌘→ Home End ⌃A ⌃E | line start (smart) / end |

Arrows, ⌥←→ (words), ⌘↑↓ (top/bottom), PgUp/PgDn and their ⇧ forms select; ⌃B ⌃F ⌃P ⌃N
move as in every macOS text field.

**Overrides** are Kil's design, unchanged: `~/.config/kiled/keymap.toml` `[bindings]` maps
keys to command names; an entry replaces whatever its keys did in any scope (so a user key
always wins), `"unbind"` removes a default, invalid entries are reported in a notice and
the rest apply. **Settings ▸ Keyboard** lists every command by group: click its keys and
press new ones — written with `rebind` (the command's lines go, its other defaults are
unbound so they stop working), refused by name if taken; **Reset** restores the defaults.
Recording intercepts keystrokes before bindings resolve, so ⌘W can be recorded. Two-stroke
keys (⌘K ⌘W) are a file feature. The bindings are applied with `clear_key_bindings` and
`bind_keys`, the fixed keys of lists and fields after them (completion menu, picker, find
bar, explorer, text fields, Settings' ⌘W), and the menus are set again so they show the keys
bound.

---

## 9. Config (`~/.config/kiled/config.toml`, `$XDG_CONFIG_HOME` respected)

```toml
theme = "night"            # night, paper, fjord, ember or dusk
font_family = "Menlo"
font_size = 13.0
tab_size = 4               # for files that don't show their own indentation
insert_spaces = true
cursor_blink = true
sidebar_opacity = 0.8      # 0–1; leave out for 0.92

[servers]
rust-analyzer = ["/opt/homebrew/bin/rust-analyzer"]   # replace a server's command
taplo = []                                            # or turn it off
```

The Settings window writes one key per change (replacing its line or adding one above the
first table, keeping comments), saved as any file is (§4.4), so a symlinked dotfile stays a
symlink; nothing is written that wouldn't parse. Both files are re-read when the window is focused and their
mtime changed. Indentation is detected per file (the most common step between indented
lines; tabs if most indented lines start with one); the settings are the fallback.

---

## 10. Rust practice

As Kil (its §10): `unsafe_code = "deny"` with the only exceptions in `kiled-core/src/sys.rs`
(each block `#[expect(unsafe_code)]` with a `SAFETY` comment) and the first statement of
`main`; `#![deny(missing_docs)]` on the core; clippy `all` denied and `pedantic` warned,
`-D warnings` in CI; `unwrap`/`expect` denied outside tests; suppressions as
`#[expect(…, reason = "…")]`; tests named `<unit>_should_<behavior>`; imports grouped by
nightly rustfmt; comments say why.

---

## 11. Packaging and release

As Kil: `scripts/bundle.sh [debug|release]` builds `Kiled.app` (icon from
`packaging/app-icon.png` through `sips` and `iconutil`, ad-hoc signed).
`scripts/release.sh` signs with the Developer ID identity (team `43VLF3KTFZ`) and the
hardened runtime, notarizes and staples the app and then the DMG, checks both with `spctl`,
and writes `releases/Kiled-arm64.dmg`, its `.sha256` and `latest.toml`, committed to the repo.
CI never builds releases; it runs fmt, clippy and the tests on Apple silicon.

Kiled needs **no entitlements** under the hardened runtime: it doesn't load unsigned code,
use JIT or reach the camera; the language servers it starts are separate programs. The
Info.plist declares Kiled an **Alternate** editor for text, source code and folders (Open
With, drops on the Dock icon, `open -a`), so it never takes a file type from the app that has
it, and folder-access strings for Desktop, Documents, Downloads and volumes.

**The icon** is Kil's, made a sibling: the same squircle, the same lightning-bolt K in the same
material, scaled down beside a text cursor (an I-beam) drawn in the K's own gradient and glow.

---

## 12. Risks and triggers

| Risk | Mitigation now | Trigger to change course |
|---|---|---|
| GPUI API churn | Exact pin, Kil's vendored copy; GPUI only in the app crate. | Upgrade with Kil, one bump per PR. |
| Opening a huge file blocks the UI (~100 ms per 50k lines) | Syntax and LSP off above 8 MB; open refused above 256 MB. | Users open files > 20 MB regularly → read and parse off the main thread. |
| Highlight queries disagree on precedence | Specificity, then order (§5.2), tested per language. | A grammar's colors look wrong → fix in the query table, with a test. |
| A server ignores stdin closing after Kiled is killed | Every server tested exits on EOF; groups are killed on every orderly exit. | One doesn't → a small supervisor process holding the groups. |
| tree-sitter error recovery with many errors (~6.5 ms/key, §5.4) | Within a 60 Hz frame. | Frame drops while typing → reparse off the main thread with a snapshot. |
| Language servers change (TypeScript 7 dropped tsserver) | Ordered server candidates per language, version checks, `[servers]`. | A new default appears → one table entry. |
| The cursor blink redraws the editor (0.83%) | Under the 1% budget. | Above it → the cursor as a cached view of its own, as Kil does. |
