# Kiled FAQ

## What is Kiled?

Kiled is a native macOS code editor written in Rust, and the sibling of the [Kil](https://github.com/timwuhaotian/kil) terminal. It edits code and nothing else: no terminal, no extensions, no git panel. It is built on GPUI (Metal rendering) and tree-sitter, and talks to the language servers you already have installed.

## How big is Kiled?

Kiled is one binary plus its icon: a 4.8 MB download and 13.5 MB installed, as Finder counts it. Measured the same way on 21 Oct 2026, TextMate is 38.8 MB, Sublime Text 146 MB, Zed 269 MB, VS Code 920 MB and Cursor 1,039 MB. So Kiled is roughly 1/20th of Zed and 1/68th of VS Code on disk.

## How is Kiled different from VS Code?

Kiled keeps VS Code's macOS keys (⌘P, ⇧⌘P, ⌘D, ⌥↑↓, F12, F8, ⇧⌥F…) so your fingers transfer on day one, but drops the platform around them: no extensions, terminal, git panel, or split editors. The installed app is 13.5 MB vs VS Code's 920 MB, idles at 0.00% CPU, and answers a keystroke in 0.14–0.33 ms on average. If you live in extensions or remote development, stay with VS Code; if you want a fast native editor with familiar keys, Kiled is that.

## How is Kiled different from Zed?

Both render with GPUI and Metal — Kiled builds on GPUI from the Zed project — but Kiled is far smaller in size and scope: 13.5 MB installed vs Zed's 269 MB measured the same way. Kiled is one window for code only, with VS Code's keys as defaults and Kil's five themes (Night, Paper, Fjord, Ember, Dusk). It is MIT-licensed and Apple silicon only, like its sibling Kil.

## Which languages and language servers does Kiled support?

Kiled highlights Rust, TypeScript, TSX, JavaScript, JSX, JSON/JSONC, YAML and TOML with tree-sitter, incrementally as you type and only for the lines on screen; anything else opens as plain text. It uses rust-analyzer, TypeScript 7's own server (`tsc --lsp`, your project's install first), vscode-json-language-server, yaml-language-server and taplo — whichever you have installed. You get squiggles with F8/⇧F8, completion, hover, go to definition (F12 or ⌘-click), and Format Document (⇧⌥F); a missing server shows its install command. Files over 8 MB open without syntax or LSP, and files over 256 MB are refused.

## Does Kiled use VS Code keybindings?

Yes: the defaults are VS Code's macOS keys, including ⌘P, ⇧⌘P, ⌘D, ⇧⌘L, ⌥↑↓, ⇧⌥↓, ⌘/, ⌘L, ⇧⌘K, ⌃G, F12, F8, ⇧⌥F and ⌘K ⌘W. Every command is rebindable in Settings ▸ Keyboard (click, press, done) or in `~/.config/kiled/keymap.toml`, two-stroke keys included. Menus and the Command Palette always show the keys currently bound.

## How do I install Kiled?

Download [`Kiled-arm64.dmg`](https://raw.githubusercontent.com/timwuhaotian/kiled/main/releases/Kiled-arm64.dmg) (4.8 MB), open it, and drag **Kiled** to **Applications**. The app is signed with a Developer ID and notarized by Apple; verify it with `shasum -a 256 -c Kiled-arm64.dmg.sha256`. You need Apple silicon and macOS 14 or later (tested on macOS 27); building from source needs Xcode with the Metal toolchain and Rust 1.94+.

## Does Kiled auto-update?

Yes, quietly: 5 seconds after launch and every 6 hours it checks `releases/latest.toml`, downloads a newer DMG in the background, verifies its SHA-256, version, Developer ID team and Apple notarization, and installs it when you quit. **Kiled ▸ Check for Updates…** checks right away in a small progress window with **Restart Now** / **Later**, and the menu becomes **Restart to Update** once a release is staged. Turn it off with `auto_update = false` in Settings ▸ Updates.

## Is Kiled free and open source?

Yes: Kiled is MIT-licensed and free to use, with its source, design doc and changelog on GitHub. Its main dependencies are permissively licensed too — GPUI is Apache-2.0, tree-sitter and its grammars are MIT. Contributions are welcome under `CONTRIBUTING.md`, and security issues go through the private channel in `SECURITY.md`.

## Is Kiled macOS only?

Yes: Apple silicon Macs running macOS 14 or later, tested on macOS 27. There are no Intel, Windows or Linux builds and no Mac App Store version. It declares itself an Alternate editor for text, source and folders, so it never steals file types from your current apps.
