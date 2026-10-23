//! The Quick Open box, as VS Code has it: files (Cmd-P), commands after `>` (Cmd-Shift-P)
//! and a line after `:` (Ctrl-G) (DESIGN.md §7). The files opened lately come first, so
//! Enter with nothing typed goes back to the previous one. A path starting with `~` or `/`
//! opens any file on the Mac, after `>` too.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    KeyBinding, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window,
    actions, div, prelude::*, px, uniform_list,
};
use kiled_core::fuzzy;

use crate::{
    keymap::{self, Command},
    settings,
    text_field::{FieldEvent, TextField},
    theme::Theme,
};

actions!(picker, [SelectNext, SelectPrevious]);

/// The list's keys; fixed, and live while the box is open.
pub fn bindings() -> Vec<KeyBinding> {
    let cx = Some("Picker");
    vec![
        KeyBinding::new("down", SelectNext, cx),
        KeyBinding::new("ctrl-n", SelectNext, cx),
        KeyBinding::new("up", SelectPrevious, cx),
        KeyBinding::new("ctrl-p", SelectPrevious, cx),
    ]
}

/// Rows shown before the list scrolls; files beyond the best matches aren't listed.
const ROWS: usize = 12;
const SHOWN_LIMIT: usize = 300;
const ROW_HEIGHT: f32 = 26.;
/// What a recent file scores on top of its match: ahead of an equally good file, behind a
/// clearly better one (a word start is worth as much).
const RECENT_BONUS: i64 = 16;

/// The files Quick Open searches.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Files {
    /// The folder's files, relative to it and sorted; empty with no folder open, and until
    /// the folder has been listed.
    pub project: Arc<Vec<PathBuf>>,
    /// Files opened lately, the latest first: relative to the folder when in it, else
    /// absolute.
    pub recent: Vec<PathBuf>,
    /// The folder is being listed; `project` is what the last listing found.
    pub listing: bool,
    /// A folder is open.
    pub folder: bool,
}

/// What was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerEvent {
    /// A file, relative to the folder, or absolute when a path was typed.
    Open(PathBuf),
    Run(Command),
    /// A line, 1-based, and a column if typed (`:12:5`).
    GoToLine(usize, Option<usize>),
    Dismissed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Files,
    Path,
    Commands,
    Line,
}

fn mode_of(query: &str) -> (Mode, &str) {
    let (mode, rest) = if let Some(rest) = query.strip_prefix('>') {
        (Mode::Commands, rest.trim_start())
    } else if let Some(rest) = query.strip_prefix(':') {
        (Mode::Line, rest.trim())
    } else {
        (Mode::Files, query.trim())
    };
    // No project file or command title starts with these, so a path typed after `>` opens
    // as well.
    if mode != Mode::Line && rest.starts_with(['~', '/']) {
        (Mode::Path, rest.trim())
    } else {
        (mode, rest)
    }
}

/// The prefix that opened the box `query` is in: `>`, `:` or nothing. A path typed in the
/// file box keeps it the file box.
fn prefix_of(query: &str) -> &'static str {
    match query.chars().next() {
        Some('>') => ">",
        Some(':') => ":",
        _ => "",
    }
}

/// The file a typed path names, canonical as the command line opens it, so a file is open
/// once however it was reached.
fn file_at(text: &str) -> Option<PathBuf> {
    let path = settings::expand_home(text);
    if !path.is_absolute() {
        return None;
    }
    let path = path.canonicalize().ok()?;
    path.is_file().then_some(path)
}

/// `12` or `12:5` as a line and column.
fn parse_line(text: &str) -> Option<(usize, Option<usize>)> {
    let mut parts = text.splitn(2, [':', ',']);
    let line = parts.next()?.trim().parse().ok()?;
    let column = parts.next().and_then(|column| column.trim().parse().ok());
    Some((line, column))
}

pub struct Picker {
    query: Entity<TextField>,
    files: Files,
    /// Project files that are recent too, by index, sorted: listed once, among the recent.
    duplicates: Vec<usize>,
    commands: Vec<(Command, SharedString)>,
    /// Lines in the active editor, for Go to Line's hint.
    lines: usize,
    /// Matches as indices into the recent files and then the project's, or into the
    /// commands, with the matched chars.
    matches: Vec<(usize, Vec<usize>)>,
    /// The file a typed path names.
    found: Option<PathBuf>,
    selected: usize,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PickerEvent> for Picker {}

impl Focusable for Picker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.focus_handle(cx)
    }
}

impl Picker {
    /// A box starting with `prefix` typed (`>`, `:` or nothing).
    pub fn new(
        prefix: &str,
        files: Files,
        bound: &[(String, Command)],
        lines: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            let mut field = TextField::new("", window, cx)
                .with_placeholder("Search files by name or path, > for commands, : for a line");
            field.type_text(prefix, cx);
            field
        });
        let subscriptions = vec![cx.subscribe_in(&query, window, Self::query_event)];
        let commands = Command::all()
            .map(|command| {
                let keys = keymap::keys_for(bound, command)
                    .first()
                    .map(|keys| keymap::display(keys))
                    .unwrap_or_default();
                (command, keys.into())
            })
            .collect();
        let mut this = Self {
            query,
            duplicates: duplicates(&files),
            files,
            commands,
            lines,
            matches: Vec::new(),
            found: None,
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.update_matches(cx);
        this
    }

    fn query_event(
        &mut self,
        _: &Entity<TextField>,
        event: &FieldEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FieldEvent::Changed(_) => self.update_matches(cx),
            FieldEvent::Confirmed(_) => self.confirm(cx),
            FieldEvent::Cancelled | FieldEvent::Blurred => cx.emit(PickerEvent::Dismissed),
        }
    }

    /// The folder has been listed, or listed again; the typed query stays.
    pub fn set_files(&mut self, files: Files, cx: &mut Context<Self>) {
        if files == self.files {
            return;
        }
        self.duplicates = duplicates(&files);
        self.files = files;
        self.update_matches(cx);
    }

    #[cfg(test)]
    pub(crate) fn files(&self) -> &Files {
        &self.files
    }

    /// Down one row, as the box's own key pressed again does.
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    /// Starts over as the box `prefix` opens (`>`, `:` or nothing).
    pub fn reset(&mut self, prefix: &str, cx: &mut Context<Self>) {
        self.query.update(cx, |field, cx| {
            field.set_text("", cx);
            field.type_text(prefix, cx);
        });
        self.update_matches(cx);
    }

    /// Whether the box is the one `prefix` opens.
    pub fn is_mode_of(&self, prefix: &str, cx: &App) -> bool {
        prefix_of(self.query.read(cx).text()) == prefix_of(prefix)
    }

    /// The file a match's index names: a recent one, or the project's.
    fn candidate(&self, index: usize) -> Option<&PathBuf> {
        let recent = self.files.recent.len();
        if index < recent {
            self.files.recent.get(index)
        } else {
            self.files.project.get(index - recent)
        }
    }

    fn update_matches(&mut self, cx: &mut Context<Self>) {
        let text = self.query.read(cx).text().to_owned();
        let (mode, query) = mode_of(&text);
        self.matches = match mode {
            Mode::Files => rank(
                query,
                &self.files.recent,
                &self.files.project,
                &self.duplicates,
            ),
            Mode::Commands => {
                let scored = self
                    .commands
                    .iter()
                    .enumerate()
                    .filter_map(|(index, (command, _))| {
                        fuzzy::score(query, command.title())
                            .map(|(score, positions)| (score, index, positions))
                    })
                    .collect();
                ordered(query, scored)
            }
            Mode::Path | Mode::Line => Vec::new(),
        };
        self.found = if mode == Mode::Path {
            file_at(query)
        } else {
            None
        };
        self.selected = 0;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.matches.is_empty() {
            return;
        }
        let count = self.matches.len().cast_signed();
        self.selected = (self.selected.cast_signed() + by)
            .rem_euclid(count)
            .unsigned_abs();
        let strategy = if by > 0 {
            ScrollStrategy::Bottom
        } else {
            ScrollStrategy::Top
        };
        self.scroll.scroll_to_item(self.selected, strategy);
        cx.notify();
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let text = self.query.read(cx).text().to_owned();
        let (mode, query) = mode_of(&text);
        let chosen = match mode {
            Mode::Line => {
                parse_line(query).map(|(line, column)| PickerEvent::GoToLine(line, column))
            }
            Mode::Files => self
                .matches
                .get(self.selected)
                .and_then(|(index, _)| self.candidate(*index))
                .map(|path| PickerEvent::Open(path.clone())),
            Mode::Path => self.found.clone().map(PickerEvent::Open),
            Mode::Commands => self
                .matches
                .get(self.selected)
                .and_then(|(index, _)| self.commands.get(*index))
                .map(|(command, _)| PickerEvent::Run(*command)),
        };
        if let Some(chosen) = chosen {
            cx.emit(chosen);
        }
    }

    fn choose(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index;
        self.confirm(cx);
    }

    fn rows(&self, mode: Mode) -> Vec<(SharedString, SharedString, Vec<usize>)> {
        if mode == Mode::Path {
            return self
                .found
                .iter()
                .map(|path| {
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let folder = path.parent().map(settings::abbreviate_home);
                    (name.into(), folder.unwrap_or_default().into(), Vec::new())
                })
                .collect();
        }
        self.matches
            .iter()
            .map(|(index, positions)| match mode {
                Mode::Files => {
                    let path = self
                        .candidate(*index)
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    // The name first, its folder after it, dimmed; one outside the open
                    // folder is absolute, shown as `~` shortens it.
                    let split = path.rfind('/').map_or(0, |slash| slash + 1);
                    let name_start = path[..split].chars().count();
                    let name_positions = positions
                        .iter()
                        .filter_map(|at| at.checked_sub(name_start))
                        .collect();
                    let folder = path[..split].trim_end_matches('/');
                    let folder = if path.starts_with('/') {
                        settings::abbreviate_home(Path::new(folder))
                    } else {
                        folder.to_owned()
                    };
                    (
                        path[split..].to_owned().into(),
                        folder.into(),
                        name_positions,
                    )
                }
                Mode::Commands | Mode::Line | Mode::Path => {
                    let (command, keys) = &self.commands[*index];
                    (command.title().into(), keys.clone(), positions.clone())
                }
            })
            .collect()
    }
}

/// Project files that are among the recent, by index, sorted.
fn duplicates(files: &Files) -> Vec<usize> {
    let mut found: Vec<usize> = files
        .recent
        .iter()
        .filter_map(|path| files.project.binary_search(path).ok())
        .collect();
    found.sort_unstable();
    found
}

/// The recent files and the project's scored against `query`, best first: with nothing
/// typed, the recent ones, latest first, then the project's in order. Indices count the
/// recent files first, then the project's, whose `duplicates` (sorted) are left out.
fn rank(
    query: &str,
    recent: &[PathBuf],
    project: &[PathBuf],
    duplicates: &[usize],
) -> Vec<(usize, Vec<usize>)> {
    let from = recent.len();
    let candidates = recent
        .iter()
        .enumerate()
        .map(|(index, path)| (index, path, RECENT_BONUS))
        .chain(
            project
                .iter()
                .enumerate()
                .filter(|(index, _)| duplicates.binary_search(index).is_err())
                .map(|(index, path)| (from + index, path, 0)),
        );
    let scored = candidates
        .filter_map(|(index, path, bonus)| {
            let (score, positions) = fuzzy::score(query, &path.to_string_lossy())?;
            Some((score + bonus, index, positions))
        })
        .collect();
    ordered(query, scored)
}

/// Matches best first, ties in their order; with nothing typed, all in their order.
fn ordered(query: &str, mut scored: Vec<(i64, usize, Vec<usize>)>) -> Vec<(usize, Vec<usize>)> {
    if !query.is_empty() {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    scored.truncate(SHOWN_LIMIT);
    scored
        .into_iter()
        .map(|(_, index, positions)| (index, positions))
        .collect()
}

#[derive(Clone, Copy)]
struct Colors {
    bg: Hsla,
    border: Hsla,
    text: Hsla,
    muted: Hsla,
    selected: Hsla,
    accent: Hsla,
    field: Hsla,
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let colors = Colors {
            bg: theme.popover_bg,
            border: theme.divider,
            text: theme.chrome_text,
            muted: theme.muted,
            selected: theme.selected,
            accent: theme.accent,
            field: theme.editor_bg,
        };
        let text = self.query.read(cx).text().to_owned();
        let (mode, query) = mode_of(&text);
        let rows = self.rows(mode);
        let list: AnyElement = if mode == Mode::Line {
            let hint = match parse_line(query) {
                Some((line, Some(column))) => format!("Go to line {line}, column {column}"),
                Some((line, None)) => format!("Go to line {line}"),
                None => format!("Type a line number between 1 and {}", self.lines.max(1)),
            };
            div()
                .px_3()
                .py_2()
                .text_color(colors.muted)
                .child(hint)
                .into_any_element()
        } else if rows.is_empty() {
            div()
                .px_3()
                .py_2()
                .text_color(colors.muted)
                .child(match mode {
                    Mode::Path => "No file at this path",
                    Mode::Files if !self.files.folder => {
                        "Open a folder to search its files, or type a path like ~/.zshrc"
                    }
                    Mode::Files if self.files.listing && self.files.project.is_empty() => {
                        "Listing the folder's files…"
                    }
                    _ => "No matching results",
                })
                .into_any_element()
        } else {
            let selected = self.selected;
            let count = rows.len();
            let picker = cx.entity();
            #[expect(clippy::cast_precision_loss, reason = "a dozen rows")]
            let height = px(ROW_HEIGHT * count.min(ROWS) as f32);
            uniform_list("picker-rows", count, move |range, _, _| {
                range
                    .map(|index| {
                        let (title, detail, positions) = rows[index].clone();
                        let picker = picker.clone();
                        row(index, &title, detail, &positions, index == selected, colors).on_click(
                            move |_: &ClickEvent, _, cx| {
                                picker.update(cx, |picker, cx| picker.choose(index, cx));
                            },
                        )
                    })
                    .collect()
            })
            .track_scroll(self.scroll.clone())
            .h(height)
            .px_2()
            .into_any_element()
        };
        div()
            .id("picker")
            .key_context("Picker")
            .occlude()
            .w(px(580.))
            .flex()
            .flex_col()
            .rounded_lg()
            .border_1()
            .border_color(colors.border)
            .bg(colors.bg)
            .shadow_lg()
            .text_size(px(13.))
            .text_color(colors.text)
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &SelectPrevious, _, cx| this.step(-1, cx)))
            .child(
                div().p_2().child(
                    div()
                        .h(px(28.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(colors.accent)
                        .bg(colors.field)
                        .child(self.query.clone()),
                ),
            )
            .child(div().pb_1().child(list))
    }
}

fn row(
    index: usize,
    title: &str,
    detail: SharedString,
    positions: &[usize],
    selected: bool,
    colors: Colors,
) -> gpui::Stateful<gpui::Div> {
    let mut pieces: Vec<AnyElement> = Vec::new();
    let mut run = String::new();
    let mut run_matched = false;
    for (at, ch) in title.chars().enumerate() {
        let matched = positions.contains(&at);
        if matched != run_matched && !run.is_empty() {
            pieces.push(piece(std::mem::take(&mut run), run_matched, colors));
        }
        run_matched = matched;
        run.push(ch);
    }
    if !run.is_empty() {
        pieces.push(piece(run, run_matched, colors));
    }
    div()
        .id(index)
        .w_full()
        .h(px(ROW_HEIGHT))
        .px_2()
        .flex()
        .items_center()
        .gap_3()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |row| row.bg(colors.selected))
        .child(div().flex().flex_none().children(pieces))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .text_color(colors.muted)
                .child(detail),
        )
}

fn piece(text: String, matched: bool, colors: Colors) -> AnyElement {
    div()
        .text_color(if matched { colors.accent } else { colors.text })
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_files_should_come_first_and_only_once() {
        let recent = [PathBuf::from("src/b.rs"), PathBuf::from("/tmp/notes.txt")];
        let project = [
            PathBuf::from("a.rs"),
            PathBuf::from("src/b.rs"),
            PathBuf::from("src/c.rs"),
        ];
        let files = Files {
            project: Arc::new(project.to_vec()),
            recent: recent.to_vec(),
            listing: false,
            folder: true,
        };
        let duplicates = duplicates(&files);
        let indices = |query: &str| -> Vec<usize> {
            rank(query, &recent, &project, &duplicates)
                .into_iter()
                .map(|(index, _)| index)
                .collect()
        };
        // Nothing typed: the recent files, then the rest in order, `src/b.rs` once. A
        // recent file wins a tie ("rs"), not a clearly better match ("c.rs").
        assert_eq!(
            (
                duplicates.clone(),
                indices(""),
                indices("rs"),
                indices("c.rs")
            ),
            (vec![1], vec![0, 1, 2, 4], vec![0, 2, 4], vec![4, 0])
        );
    }

    #[test]
    fn the_prefix_should_pick_the_mode() {
        assert_eq!(
            [mode_of("main"), mode_of("> save"), mode_of(":12")],
            [
                (Mode::Files, "main"),
                (Mode::Commands, "save"),
                (Mode::Line, "12")
            ]
        );
    }

    #[test]
    fn a_path_should_be_one_with_or_without_the_command_prefix() {
        assert_eq!(
            [
                mode_of("~/.npmrc"),
                mode_of("/etc/hosts "),
                mode_of("> ~/.npmrc"),
                mode_of(":~")
            ],
            [
                (Mode::Path, "~/.npmrc"),
                (Mode::Path, "/etc/hosts"),
                (Mode::Path, "~/.npmrc"),
                (Mode::Line, "~")
            ]
        );
    }

    #[test]
    fn a_path_should_name_a_file_or_nothing() {
        let dir = std::env::temp_dir().join(format!("kiled-picker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "hello\n").unwrap();
        let file = dir.join("notes.txt").canonicalize().unwrap();
        let typed = |path: PathBuf| path.to_string_lossy().into_owned();
        assert_eq!(
            [
                file_at(&typed(dir.join("notes.txt"))),
                file_at(&typed(dir.join("missing.txt"))),
                file_at(&typed(dir.clone())),
                file_at("~nobody/notes.txt"),
            ],
            [Some(file), None, None, None]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_line_may_carry_a_column() {
        assert_eq!(
            [parse_line("12"), parse_line("12:5"), parse_line("x")],
            [Some((12, None)), Some((12, Some(5))), None]
        );
    }
}
