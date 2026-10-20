//! VS Code's macOS keys as the defaults, plus `~/.config/kiled/keymap.toml` overrides
//! (DESIGN.md §8).
//!
//! Every command a key can run is one row of [`commands!`]: its name in the file, its title
//! in Settings and the Command Palette, the context it binds in, its action, and its default
//! keys.

use std::collections::BTreeMap;

use gpui::{Action, KeyBinding, Keystroke};
use serde::Deserialize;

use crate::{editor as e, workspace as w};

/// Where a command's keys work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every window, Settings included.
    App,
    /// The editor window, whatever has focus in it.
    Workspace,
    /// A focused editor.
    Editor,
}

impl Scope {
    fn context(self) -> Option<&'static str> {
        match self {
            Self::App => None,
            Self::Workspace => Some("Workspace"),
            Self::Editor => Some("Editor"),
        }
    }
}

/// How Settings groups the commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    App,
    File,
    Edit,
    Selection,
    Cursor,
    Go,
    View,
    Language,
}

impl Group {
    pub const ALL: [Self; 8] = [
        Self::File,
        Self::Edit,
        Self::Selection,
        Self::Cursor,
        Self::Go,
        Self::Language,
        Self::View,
        Self::App,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::App => "APP",
            Self::File => "FILE",
            Self::Edit => "EDIT",
            Self::Selection => "SELECTION",
            Self::Cursor => "CURSOR",
            Self::Go => "GO",
            Self::View => "VIEW",
            Self::Language => "LANGUAGE",
        }
    }
}

macro_rules! commands {
    ($($variant:ident: $name:literal, $title:literal, $group:ident, $scope:ident, $action:expr,
       [$($keys:literal),*];)*) => {
        /// What a key can be bound to. Names in the file are `snake_case`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Command {
            $($variant,)*
            /// Takes a default binding away.
            Unbind,
        }

        const TABLE: &[(Command, &str, &str, Group, Scope, &[&str])] = &[
            $((Command::$variant, $name, $title, Group::$group, Scope::$scope, &[$($keys),*]),)*
        ];

        impl Command {
            /// Its action, to run it from the Command Palette.
            pub fn action(self) -> Option<Box<dyn Action>> {
                Some(match self {
                    $(Self::$variant => Box::new($action),)*
                    Self::Unbind => return None,
                })
            }

            fn binding(self, keys: &str) -> Option<KeyBinding> {
                let context = self.scope().context();
                Some(match self {
                    $(Self::$variant => KeyBinding::new(keys, $action, context),)*
                    Self::Unbind => return None,
                })
            }
        }
    };
}

commands! {
    // File
    NewFile: "new_file", "New File", File, Workspace, w::NewFile, ["cmd-n"];
    Open: "open", "Open…", File, Workspace, w::Open, ["cmd-o"];
    Save: "save", "Save", File, Workspace, w::Save, ["cmd-s"];
    SaveAs: "save_as", "Save As…", File, Workspace, w::SaveAs, ["cmd-shift-s"];
    SaveAll: "save_all", "Save All", File, Workspace, w::SaveAll, ["cmd-alt-s"];
    CloseEditor: "close_editor", "Close Editor", File, Workspace, w::CloseEditor, ["cmd-w"];
    CloseAllEditors: "close_all_editors", "Close All Editors", File, Workspace,
        w::CloseAllEditors, ["cmd-k cmd-w"];
    ReopenClosedEditor: "reopen_closed_editor", "Reopen Closed Editor", File, Workspace,
        w::ReopenClosedEditor, ["cmd-shift-t"];
    // Edit
    Undo: "undo", "Undo", Edit, Editor, e::Undo, ["cmd-z"];
    Redo: "redo", "Redo", Edit, Editor, e::Redo, ["cmd-shift-z"];
    Cut: "cut", "Cut", Edit, Editor, e::Cut, ["cmd-x"];
    Copy: "copy", "Copy", Edit, Editor, e::Copy, ["cmd-c"];
    Paste: "paste", "Paste", Edit, Editor, e::Paste, ["cmd-v"];
    Find: "find", "Find", Edit, Editor, e::Find, ["cmd-f"];
    Replace: "replace", "Replace", Edit, Editor, e::Replace, ["cmd-alt-f"];
    FindNext: "find_next", "Find Next", Edit, Editor, e::FindNext, ["cmd-g"];
    FindPrevious: "find_previous", "Find Previous", Edit, Editor, e::FindPrevious,
        ["cmd-shift-g"];
    ToggleLineComment: "toggle_line_comment", "Toggle Line Comment", Edit, Editor,
        e::ToggleLineComment, ["cmd-/"];
    Indent: "indent", "Indent Line", Edit, Editor, e::Indent, ["cmd-]"];
    Outdent: "outdent", "Outdent Line", Edit, Editor, e::Outdent, ["cmd-[", "shift-tab"];
    MoveLineUp: "move_line_up", "Move Line Up", Edit, Editor, e::MoveLineUp, ["alt-up"];
    MoveLineDown: "move_line_down", "Move Line Down", Edit, Editor, e::MoveLineDown,
        ["alt-down"];
    CopyLineUp: "copy_line_up", "Copy Line Up", Edit, Editor, e::CopyLineUp, ["shift-alt-up"];
    CopyLineDown: "copy_line_down", "Copy Line Down", Edit, Editor, e::CopyLineDown,
        ["shift-alt-down"];
    DeleteLine: "delete_line", "Delete Line", Edit, Editor, e::DeleteLine, ["cmd-shift-k"];
    InsertLineBelow: "insert_line_below", "Insert Line Below", Edit, Editor,
        e::InsertLineBelow, ["cmd-enter"];
    InsertLineAbove: "insert_line_above", "Insert Line Above", Edit, Editor,
        e::InsertLineAbove, ["cmd-shift-enter"];
    Newline: "newline", "New Line", Edit, Editor, e::Newline, ["enter", "shift-enter"];
    Tab: "tab", "Tab", Edit, Editor, e::Tab, ["tab"];
    Backspace: "backspace", "Delete Left", Edit, Editor, e::Backspace,
        ["backspace", "shift-backspace"];
    Delete: "delete", "Delete Right", Edit, Editor, e::Delete, ["delete"];
    DeleteWordLeft: "delete_word_left", "Delete Word Left", Edit, Editor, e::DeleteWordLeft,
        ["alt-backspace"];
    DeleteWordRight: "delete_word_right", "Delete Word Right", Edit, Editor,
        e::DeleteWordRight, ["alt-delete"];
    DeleteToLineStart: "delete_to_line_start", "Delete to Line Start", Edit, Editor,
        e::DeleteToLineStart, ["cmd-backspace"];
    DeleteToLineEnd: "delete_to_line_end", "Delete to Line End", Edit, Editor,
        e::DeleteToLineEnd, ["cmd-delete", "ctrl-k"];
    // Selection
    SelectAll: "select_all", "Select All", Selection, Editor, e::SelectAll, ["cmd-a"];
    ExpandLineSelection: "expand_line_selection", "Expand Line Selection", Selection, Editor,
        e::ExpandLineSelection, ["cmd-l"];
    AddNextOccurrence: "add_next_occurrence", "Add Next Occurrence", Selection, Editor,
        e::AddNextOccurrence, ["cmd-d"];
    SelectAllOccurrences: "select_all_occurrences", "Select All Occurrences", Selection,
        Editor, e::SelectAllOccurrences, ["cmd-shift-l"];
    AddCursorAbove: "add_cursor_above", "Add Cursor Above", Selection, Editor,
        e::AddCursorAbove, ["cmd-alt-up"];
    AddCursorBelow: "add_cursor_below", "Add Cursor Below", Selection, Editor,
        e::AddCursorBelow, ["cmd-alt-down"];
    Cancel: "cancel", "Single Cursor / Close", Selection, Editor, e::Cancel, ["escape"];
    // Cursor
    MoveLeft: "move_left", "Left", Cursor, Editor, e::MoveLeft, ["left", "ctrl-b"];
    MoveRight: "move_right", "Right", Cursor, Editor, e::MoveRight, ["right", "ctrl-f"];
    MoveUp: "move_up", "Up", Cursor, Editor, e::MoveUp, ["up", "ctrl-p"];
    MoveDown: "move_down", "Down", Cursor, Editor, e::MoveDown, ["down", "ctrl-n"];
    SelectLeft: "select_left", "Select Left", Cursor, Editor, e::SelectLeft, ["shift-left"];
    SelectRight: "select_right", "Select Right", Cursor, Editor, e::SelectRight,
        ["shift-right"];
    SelectUp: "select_up", "Select Up", Cursor, Editor, e::SelectUp, ["shift-up"];
    SelectDown: "select_down", "Select Down", Cursor, Editor, e::SelectDown, ["shift-down"];
    MoveWordLeft: "move_word_left", "Word Left", Cursor, Editor, e::MoveWordLeft, ["alt-left"];
    MoveWordRight: "move_word_right", "Word Right", Cursor, Editor, e::MoveWordRight,
        ["alt-right"];
    SelectWordLeft: "select_word_left", "Select Word Left", Cursor, Editor, e::SelectWordLeft,
        ["shift-alt-left"];
    SelectWordRight: "select_word_right", "Select Word Right", Cursor, Editor,
        e::SelectWordRight, ["shift-alt-right"];
    MoveToLineStart: "move_to_line_start", "Line Start", Cursor, Editor, e::MoveToLineStart,
        ["cmd-left", "home", "ctrl-a"];
    MoveToLineEnd: "move_to_line_end", "Line End", Cursor, Editor, e::MoveToLineEnd,
        ["cmd-right", "end", "ctrl-e"];
    SelectToLineStart: "select_to_line_start", "Select to Line Start", Cursor, Editor,
        e::SelectToLineStart, ["cmd-shift-left", "shift-home"];
    SelectToLineEnd: "select_to_line_end", "Select to Line End", Cursor, Editor,
        e::SelectToLineEnd, ["cmd-shift-right", "shift-end"];
    MoveToStart: "move_to_start", "Top", Cursor, Editor, e::MoveToStart, ["cmd-up"];
    MoveToEnd: "move_to_end", "Bottom", Cursor, Editor, e::MoveToEnd, ["cmd-down"];
    SelectToStart: "select_to_start", "Select to Top", Cursor, Editor, e::SelectToStart,
        ["cmd-shift-up"];
    SelectToEnd: "select_to_end", "Select to Bottom", Cursor, Editor, e::SelectToEnd,
        ["cmd-shift-down"];
    PageUp: "page_up", "Page Up", Cursor, Editor, e::PageUp, ["pageup"];
    PageDown: "page_down", "Page Down", Cursor, Editor, e::PageDown, ["pagedown"];
    SelectPageUp: "select_page_up", "Select Page Up", Cursor, Editor, e::SelectPageUp,
        ["shift-pageup"];
    SelectPageDown: "select_page_down", "Select Page Down", Cursor, Editor,
        e::SelectPageDown, ["shift-pagedown"];
    // Go
    QuickOpen: "quick_open", "Go to File…", Go, Workspace, w::QuickOpen, ["cmd-p"];
    GoToLine: "go_to_line", "Go to Line…", Go, Workspace, w::GoToLine, ["ctrl-g"];
    GoToDefinition: "go_to_definition", "Go to Definition", Go, Editor, e::GoToDefinition,
        ["f12"];
    GoBack: "go_back", "Back", Go, Workspace, w::GoBack, ["ctrl--"];
    GoForward: "go_forward", "Forward", Go, Workspace, w::GoForward, ["ctrl-shift--"];
    NextProblem: "next_problem", "Next Problem", Go, Editor, e::NextProblem, ["f8"];
    PreviousProblem: "previous_problem", "Previous Problem", Go, Editor, e::PreviousProblem,
        ["shift-f8"];
    NextEditor: "next_editor", "Next Editor", Go, Workspace, w::NextEditor,
        ["cmd-alt-right", "cmd-shift-]", "ctrl-tab"];
    PreviousEditor: "previous_editor", "Previous Editor", Go, Workspace, w::PreviousEditor,
        ["cmd-alt-left", "cmd-shift-[", "ctrl-shift-tab"];
    OpenEditor1: "open_editor_1", "Open Editor 1", Go, Workspace, w::OpenEditor1, ["ctrl-1"];
    OpenEditor2: "open_editor_2", "Open Editor 2", Go, Workspace, w::OpenEditor2, ["ctrl-2"];
    OpenEditor3: "open_editor_3", "Open Editor 3", Go, Workspace, w::OpenEditor3, ["ctrl-3"];
    OpenEditor4: "open_editor_4", "Open Editor 4", Go, Workspace, w::OpenEditor4, ["ctrl-4"];
    OpenEditor5: "open_editor_5", "Open Editor 5", Go, Workspace, w::OpenEditor5, ["ctrl-5"];
    OpenEditor6: "open_editor_6", "Open Editor 6", Go, Workspace, w::OpenEditor6, ["ctrl-6"];
    OpenEditor7: "open_editor_7", "Open Editor 7", Go, Workspace, w::OpenEditor7, ["ctrl-7"];
    OpenEditor8: "open_editor_8", "Open Editor 8", Go, Workspace, w::OpenEditor8, ["ctrl-8"];
    OpenEditor9: "open_editor_9", "Open Last Editor", Go, Workspace, w::OpenEditor9,
        ["ctrl-9"];
    // Language
    TriggerSuggest: "trigger_suggest", "Trigger Suggest", Language, Editor, e::TriggerSuggest,
        ["ctrl-space", "cmd-i"];
    ShowHover: "show_hover", "Show Hover", Language, Editor, e::ShowHover, ["cmd-k cmd-i"];
    FormatDocument: "format_document", "Format Document", Language, Editor,
        e::FormatDocument, ["shift-alt-f"];
    // View
    Palette: "command_palette", "Command Palette…", View, Workspace,
        w::CommandPalette, ["cmd-shift-p", "f1"];
    ToggleSidebar: "toggle_sidebar", "Toggle Sidebar", View, Workspace, w::ToggleSidebar,
        ["cmd-b"];
    FocusExplorer: "focus_explorer", "Show Explorer", View, Workspace, w::FocusExplorer,
        ["cmd-shift-e"];
    ZoomIn: "zoom_in", "Zoom In", View, Workspace, w::ZoomIn, ["cmd-=", "cmd-+"];
    ZoomOut: "zoom_out", "Zoom Out", View, Workspace, w::ZoomOut, ["cmd--"];
    ResetZoom: "reset_zoom", "Reset Zoom", View, Workspace, w::ResetZoom, ["cmd-0"];
    // App
    OpenSettings: "open_settings", "Settings…", App, App, crate::OpenSettings, ["cmd-,"];
    OpenKeymap: "open_keymap", "Keyboard Shortcuts…", App, App, crate::OpenKeymap,
        ["cmd-k cmd-s"];
    Hide: "hide", "Hide Kiled", App, App, crate::Hide, ["cmd-h"];
    HideOthers: "hide_others", "Hide Others", App, App, crate::HideOthers, ["cmd-alt-h"];
    Quit: "quit", "Quit Kiled", App, App, crate::Quit, ["cmd-q"];
}

/// A command's row: itself, its name, title, group, scope and default keys.
type Row = (
    Command,
    &'static str,
    &'static str,
    Group,
    Scope,
    &'static [&'static str],
);

fn row(command: Command) -> Option<&'static Row> {
    TABLE.iter().find(|row| row.0 == command)
}

impl Command {
    /// Every command, in the order Settings lists them within a group.
    pub fn all() -> impl Iterator<Item = Self> {
        TABLE.iter().map(|row| row.0)
    }

    pub fn parse(name: &str) -> Option<Self> {
        if name == "unbind" {
            return Some(Self::Unbind);
        }
        TABLE.iter().find(|row| row.1 == name).map(|row| row.0)
    }

    /// The name it has in `keymap.toml`.
    pub fn key(self) -> &'static str {
        row(self).map_or("unbind", |row| row.1)
    }

    /// The name it has in Settings and the Command Palette.
    pub fn title(self) -> &'static str {
        row(self).map_or("Unbind", |row| row.2)
    }

    pub fn group(self) -> Group {
        row(self).map_or(Group::App, |row| row.3)
    }

    pub fn scope(self) -> Scope {
        row(self).map_or(Scope::App, |row| row.4)
    }

    /// The keys it runs on by default, as written in `keymap.toml`.
    pub fn default_keys(self) -> &'static [&'static str] {
        row(self).map_or(&[], |row| row.5)
    }
}

/// A user entry that was skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeymapError {
    pub key: String,
    pub reason: String,
}

/// What Kiled ▸ Keyboard Shortcuts… writes when there is no `keymap.toml` yet.
pub const TEMPLATE: &str = r#"# Kiled key bindings. The defaults are VS Code's on macOS; each entry here replaces
# whatever its keys did, and "unbind" takes a default away. Saved changes apply when the
# Kiled window is focused again, and the menus show them. Settings ▸ Keyboard lists every
# command with its keys, and records new ones.
#
# Keys: cmd, ctrl, alt, shift and a key, joined with "-" ("cmd-shift-p"); "cmd-k cmd-w" is
# two strokes. Commands are named as Settings ▸ Keyboard shows them, in snake_case
# ("go_to_line", "toggle_line_comment").

[bindings]
# Examples:
# "cmd-shift-d" = "add_next_occurrence"
# "ctrl-tab"    = "unbind"
"#;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeymapFile {
    #[serde(default)]
    bindings: BTreeMap<String, String>,
}

/// Merges the defaults with `user` (the file's contents). A user entry replaces whatever the
/// defaults bound to the same keys; invalid entries are reported and skipped.
pub fn resolve(user: Option<&str>) -> (Vec<(String, Command)>, Vec<KeymapError>) {
    let mut table: Vec<(String, Command)> = TABLE
        .iter()
        .flat_map(|row| row.5.iter().map(move |keys| (row.0, *keys)))
        .filter_map(|(command, keys)| Some((normalize(keys).ok()?, command)))
        .collect();
    let mut errors = Vec::new();
    let file = match user.map(toml::from_str::<KeymapFile>) {
        None => None,
        Some(Ok(file)) => Some(file),
        Some(Err(err)) => {
            errors.push(KeymapError {
                key: "keymap.toml".into(),
                reason: err.message().to_owned(),
            });
            None
        }
    };
    for (keys, name) in file.map(|file| file.bindings).unwrap_or_default() {
        let keys = match normalize(&keys) {
            Ok(keys) => keys,
            Err(reason) => {
                errors.push(KeymapError { key: keys, reason });
                continue;
            }
        };
        let Some(command) = Command::parse(&name) else {
            errors.push(KeymapError {
                key: keys,
                reason: format!("unknown command \"{name}\""),
            });
            continue;
        };
        table.retain(|(bound, _)| *bound != keys);
        if command != Command::Unbind {
            table.push((keys, command));
        }
    }
    (table, errors)
}

/// GPUI bindings for a resolved table. Keys were validated by `resolve`.
pub fn bindings(table: &[(String, Command)]) -> Vec<KeyBinding> {
    table
        .iter()
        .filter_map(|(keys, command)| command.binding(keys))
        .collect()
}

/// The keys `command` runs on in a resolved table.
pub fn keys_for(table: &[(String, Command)], command: Command) -> Vec<&str> {
    table
        .iter()
        .filter(|(_, bound)| *bound == command)
        .map(|(keys, _)| keys.as_str())
        .collect()
}

/// Parses and re-prints each stroke so `shift-cmd-d` and `cmd-shift-d` are the same key.
fn normalize(keys: &str) -> Result<String, String> {
    let strokes = keys
        .split_whitespace()
        .map(|stroke| {
            Keystroke::parse(stroke)
                .map(|k| k.unparse())
                .map_err(|err| err.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    if strokes.is_empty() {
        return Err("empty key".into());
    }
    Ok(strokes.join(" "))
}

/// The keys a pressed chord runs on, as the file writes them: `cmd-shift-d`.
pub fn keystroke_of(event: &Keystroke) -> Option<String> {
    let keys = event.unparse();
    // Releasing a lone modifier arrives as a keystroke named after it; that is not a shortcut.
    let alone = matches!(
        keys.as_str(),
        "shift" | "control" | "alt" | "platform" | "fn"
    );
    (!alone).then_some(keys)
}

/// The same keys as macOS writes them in a menu, modifiers in its order:
/// `cmd-shift-d` → `⇧⌘D`.
pub fn display(keys: &str) -> String {
    keys.split_whitespace()
        .map(|stroke| {
            let Ok(parsed) = Keystroke::parse(stroke) else {
                return stroke.to_owned();
            };
            let modifiers = parsed.modifiers;
            let mut out = String::new();
            for (on, symbol) in [
                (modifiers.function, "fn"),
                (modifiers.control, "⌃"),
                (modifiers.alt, "⌥"),
                (modifiers.shift, "⇧"),
                (modifiers.platform, "⌘"),
            ] {
                if on {
                    out.push_str(symbol);
                }
            }
            let key = match parsed.key.as_str() {
                "left" => "←".to_owned(),
                "right" => "→".to_owned(),
                "up" => "↑".to_owned(),
                "down" => "↓".to_owned(),
                "enter" => "↩".to_owned(),
                "space" => "Space".to_owned(),
                "tab" => "⇥".to_owned(),
                "escape" => "Esc".to_owned(),
                "backspace" => "⌫".to_owned(),
                "delete" => "⌦".to_owned(),
                "pageup" => "PgUp".to_owned(),
                "pagedown" => "PgDn".to_owned(),
                "home" => "Home".to_owned(),
                "end" => "End".to_owned(),
                key => key.to_uppercase(),
            };
            out.push_str(&key);
            out
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Drops every line in `[bindings]` that names `command`, and the unbinding of its
/// defaults, so it runs on its defaults again. Other lines are kept.
pub fn clear_command(text: &str, command: Command) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_bindings = false;
    for line in text.lines() {
        let header = line.trim();
        if header.starts_with('[') {
            in_bindings = header == "[bindings]";
        }
        let mine = parses(line).is_some_and(|(keys, name)| {
            name == command.key()
                || (name == "unbind"
                    && command.default_keys().iter().any(|default| {
                        normalize(default).ok().as_deref() == normalize(keys).ok().as_deref()
                    }))
        });
        if !(in_bindings && mine) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// `text` with `command` running on `keys` and nowhere else: whatever the file said about it
/// goes, and its other defaults are unbound.
pub fn rebind(text: &str, command: Command, keys: &str) -> String {
    let mut text = clear_command(text, command);
    let defaults = command.default_keys();
    for default in defaults {
        if normalize(default).ok().as_deref() != Some(keys) {
            text = set_binding(&text, default, "unbind");
        }
    }
    if !defaults
        .iter()
        .any(|default| normalize(default).ok().as_deref() == Some(keys))
    {
        text = set_binding(&text, keys, command.key());
    }
    text
}

/// Binds `keys` to `name` in `[bindings]`: an existing line for those keys is replaced, else
/// one is added to the table, which is written if the file has none. Comments are kept.
pub fn set_binding(text: &str, keys: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_bindings = false;
    let mut found = false;
    for line in text.lines() {
        let header = line.trim();
        if header.starts_with('[') {
            in_bindings = header == "[bindings]";
        }
        if in_bindings && !found && parses(line).is_some_and(|(written, _)| written == keys) {
            out.push_str(&assignment(keys, name));
            found = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        if !in_bindings {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("[bindings]\n");
        }
        out.push_str(&assignment(keys, name));
        out.push('\n');
    }
    out
}

/// The line a `[bindings]` entry is written as.
fn assignment(keys: &str, name: &str) -> String {
    format!("\"{keys}\" = \"{name}\"")
}

/// A `[bindings]` line as its keys and command name, when it is one. A trailing comment
/// after the value is ignored.
fn parses(line: &str) -> Option<(&str, &str)> {
    let (keys, rest) = line.trim_start().strip_prefix('"')?.split_once('"')?;
    let name = rest
        .trim_start()
        .strip_prefix('=')?
        .trim_start()
        .strip_prefix('"')?;
    Some((keys, name.split_once('"')?.0))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use gpui::Modifiers;

    use super::*;

    fn command_for(table: &[(String, Command)], keys: &str) -> Option<Command> {
        let keys = normalize(keys).unwrap();
        table
            .iter()
            .find(|(k, _)| *k == keys)
            .map(|(_, command)| *command)
    }

    #[test]
    fn template_should_parse_and_change_nothing() {
        assert_eq!(resolve(Some(TEMPLATE)), resolve(None));
    }

    #[test]
    fn no_two_commands_should_share_a_default_key() {
        let (table, errors) = resolve(None);
        let mut seen = HashSet::new();
        let shared: Vec<&String> = table
            .iter()
            .filter(|(keys, _)| !seen.insert(keys.clone()))
            .map(|(keys, _)| keys)
            .collect();
        assert_eq!((errors, shared), (vec![], Vec::<&String>::new()));
    }

    #[test]
    fn defaults_should_be_vs_code_keys() {
        let (table, _) = resolve(None);
        assert_eq!(
            [
                "cmd-p",
                "cmd-shift-p",
                "cmd-d",
                "alt-up",
                "cmd-/",
                "shift-alt-f",
                "ctrl-g",
                "cmd-k cmd-w"
            ]
            .map(|keys| command_for(&table, keys)),
            [
                Some(Command::QuickOpen),
                Some(Command::Palette),
                Some(Command::AddNextOccurrence),
                Some(Command::MoveLineUp),
                Some(Command::ToggleLineComment),
                Some(Command::FormatDocument),
                Some(Command::GoToLine),
                Some(Command::CloseAllEditors)
            ]
        );
    }

    #[test]
    fn a_user_entry_should_replace_whatever_the_keys_did() {
        let (table, _) = resolve(Some("[bindings]\n\"cmd-d\" = \"toggle_sidebar\"\n"));
        assert_eq!(
            table
                .iter()
                .filter(|(keys, _)| keys == "cmd-d")
                .map(|(_, command)| *command)
                .collect::<Vec<_>>(),
            vec![Command::ToggleSidebar]
        );
    }

    #[test]
    fn resolve_should_treat_modifier_order_as_same_key_and_unbind() {
        let (table, _) = resolve(Some(
            "[bindings]\n\"shift-cmd-y\" = \"copy\"\n\"cmd-b\" = \"unbind\"\n",
        ));
        assert_eq!(
            (
                command_for(&table, "cmd-shift-y"),
                command_for(&table, "cmd-b")
            ),
            (Some(Command::Copy), None)
        );
    }

    #[test]
    fn resolve_should_report_unknown_command_and_keep_the_rest() {
        let (table, errors) = resolve(Some(
            "[bindings]\n\"cmd-x\" = \"cutt\"\n\"cmd-y\" = \"save\"\n",
        ));
        assert_eq!(
            (errors.len(), command_for(&table, "cmd-y")),
            (1, Some(Command::Save))
        );
    }

    #[test]
    fn bindings_should_build_one_per_bound_key() {
        let (table, _) = resolve(None);
        assert_eq!(bindings(&table).len(), table.len());
    }

    #[test]
    fn keystroke_should_ignore_a_lone_modifier() {
        let shift = Keystroke {
            key: "shift".into(),
            key_char: None,
            modifiers: Modifiers::default(),
        };
        assert_eq!(keystroke_of(&shift), None);
    }

    #[test]
    fn display_should_write_keys_as_macos_shows_them() {
        assert_eq!(
            [
                "cmd-d",
                "cmd-shift-]",
                "ctrl-tab",
                "cmd-alt-left",
                "cmd-k cmd-w",
                "cmd--",
                "f12"
            ]
            .map(display)
            .join(" | "),
            "⌘D | ⇧⌘] | ⌃⇥ | ⌥⌘← | ⌘K ⌘W | ⌘- | F12"
        );
    }

    #[test]
    fn rebind_should_move_every_default_key_off_and_onto_the_pressed_one() {
        let rebound = rebind(TEMPLATE, Command::MoveToLineStart, "cmd-shift-h");
        let (table, errors) = resolve(Some(&rebound));
        assert_eq!(
            (
                errors,
                keys_for(&table, Command::MoveToLineStart),
                command_for(&table, "home")
            ),
            (vec![], vec!["cmd-shift-h"], None)
        );
    }

    #[test]
    fn clearing_a_rebound_command_should_give_it_the_defaults_back() {
        let rebound = rebind(TEMPLATE, Command::MoveToLineStart, "cmd-shift-h");
        let (table, _) = resolve(Some(&clear_command(&rebound, Command::MoveToLineStart)));
        assert_eq!(
            keys_for(&table, Command::MoveToLineStart),
            vec!["cmd-left", "home", "ctrl-a"]
        );
    }

    #[test]
    fn set_binding_should_write_the_table_when_the_file_has_none() {
        assert_eq!(
            set_binding("# mine\n", "cmd-k", "copy"),
            "# mine\n\n[bindings]\n\"cmd-k\" = \"copy\"\n"
        );
    }
}
