//! The Quick Open box, as VS Code has it: files (Cmd-P), commands after `>` (Cmd-Shift-P)
//! and a line after `:` (Ctrl-G) (DESIGN.md §7).

use std::{path::PathBuf, sync::Arc};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    KeyBinding, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window,
    actions, div, prelude::*, px, uniform_list,
};
use kiled_core::fuzzy;

use crate::{
    keymap::{self, Command},
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

/// What was chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerEvent {
    /// A file, relative to the folder.
    Open(PathBuf),
    Run(Command),
    /// A line, 1-based, and a column if typed (`:12:5`).
    GoToLine(usize, Option<usize>),
    Dismissed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Files,
    Commands,
    Line,
}

fn mode_of(query: &str) -> (Mode, &str) {
    if let Some(rest) = query.strip_prefix('>') {
        (Mode::Commands, rest.trim_start())
    } else if let Some(rest) = query.strip_prefix(':') {
        (Mode::Line, rest.trim())
    } else {
        (Mode::Files, query.trim())
    }
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
    files: Arc<Vec<PathBuf>>,
    commands: Vec<(Command, SharedString)>,
    /// Lines in the active editor, for Go to Line's hint.
    lines: usize,
    /// Matches as indices into the files or commands, with the matched chars.
    matches: Vec<(usize, Vec<usize>)>,
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
        files: Arc<Vec<PathBuf>>,
        bound: &[(String, Command)],
        lines: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            let mut field = TextField::new("", window, cx)
                .with_placeholder("Search files by name, > for commands, : for a line");
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
            files,
            commands,
            lines,
            matches: Vec::new(),
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

    fn update_matches(&mut self, cx: &mut Context<Self>) {
        let text = self.query.read(cx).text().to_owned();
        let (mode, query) = mode_of(&text);
        let candidates: Vec<String> = match mode {
            Mode::Files => self
                .files
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            Mode::Commands => self
                .commands
                .iter()
                .map(|(command, _)| command.title().to_owned())
                .collect(),
            Mode::Line => Vec::new(),
        };
        let mut scored: Vec<(i64, usize, Vec<usize>)> = candidates
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                fuzzy::score(query, candidate).map(|(score, positions)| (score, index, positions))
            })
            .collect();
        if !query.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        scored.truncate(SHOWN_LIMIT);
        self.matches = scored
            .into_iter()
            .map(|(_, index, positions)| (index, positions))
            .collect();
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
                .and_then(|(index, _)| self.files.get(*index))
                .map(|path| PickerEvent::Open(path.clone())),
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
        self.matches
            .iter()
            .map(|(index, positions)| match mode {
                Mode::Files => {
                    let path = self
                        .files
                        .get(*index)
                        .map(|path| path.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    // The name first, its folder after it, dimmed.
                    let split = path.rfind('/').map_or(0, |slash| slash + 1);
                    let name_start = path[..split].chars().count();
                    let name_positions = positions
                        .iter()
                        .filter_map(|at| at.checked_sub(name_start))
                        .collect();
                    (
                        path[split..].to_owned().into(),
                        path[..split].trim_end_matches('/').to_owned().into(),
                        name_positions,
                    )
                }
                Mode::Commands | Mode::Line => {
                    let (command, keys) = &self.commands[*index];
                    (command.title().into(), keys.clone(), positions.clone())
                }
            })
            .collect()
    }
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
        } else if self.matches.is_empty() {
            div()
                .px_3()
                .py_2()
                .text_color(colors.muted)
                .child(if mode == Mode::Files && self.files.is_empty() {
                    "Open a folder to search its files"
                } else {
                    "No matching results"
                })
                .into_any_element()
        } else {
            let rows = self.rows(mode);
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
    fn a_line_may_carry_a_column() {
        assert_eq!(
            [parse_line("12"), parse_line("12:5"), parse_line("x")],
            [Some((12, None)), Some((12, Some(5))), None]
        );
    }
}
