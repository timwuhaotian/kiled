//! What the language server adds to an editor: completion as you type, hover, Go to
//! Definition, Format Document, and stepping through problems (DESIGN.md §6.2).

use std::{ops::Range, time::Duration};

use gpui::{
    AnyElement, Context, Hsla, MouseButton, Pixels, Point, ScrollStrategy, SharedString,
    UniformListScrollHandle, Window, anchored, deferred, div, point, prelude::*, px, uniform_list,
};
use kiled_core::{
    document::{Severity, offset_of, position_of},
    edit::{Change, len_chars},
    fuzzy,
    lsp::protocol::{self, CompletionItem},
    ops,
    text::is_word_char,
};

use crate::{
    editor::{Editor, EditorEvent},
    editor_element::f,
    theme::Theme,
};

/// Rows the completion menu shows before it scrolls.
const MENU_ROWS: usize = 10;
const MENU_ROW_HEIGHT: Pixels = px(22.);
/// How long the pointer rests on a word before its hover is asked for.
const HOVER_DELAY: Duration = Duration::from_millis(350);
/// Typing pauses this long before a server that waits to be asked is asked for problems.
const PULL_DELAY: Duration = Duration::from_millis(250);

/// An open completion menu.
pub struct CompletionMenu {
    items: Vec<CompletionItem>,
    /// Indices into `items` that match what is typed, best first.
    shown: Vec<(usize, Vec<usize>)>,
    selected: usize,
    /// Where the word being completed starts.
    start: usize,
    /// Typing more asks the server again instead of narrowing these.
    incomplete: bool,
    scroll: UniformListScrollHandle,
}

/// A hover popover, over the word it is about.
pub struct Hover {
    range: Range<usize>,
    text: SharedString,
}

/// LSP's completion kinds, as a short label beside the item.
fn kind_label(kind: Option<u64>) -> &'static str {
    match kind {
        Some(2..=4) => "fn",
        Some(5 | 10) => "field",
        Some(6) => "var",
        Some(7 | 22) => "type",
        Some(8) => "trait",
        Some(9) => "mod",
        Some(12) => "value",
        Some(13 | 20) => "enum",
        Some(14) => "kw",
        Some(21) => "const",
        _ => "",
    }
}

impl Editor {
    /// The word being typed: where it starts, and the text from there to the cursor.
    fn typed_word(&self) -> (usize, String) {
        let rope = self.doc.rope();
        let head = self.doc.selections().primary().head;
        let mut start = head;
        while start > 0 && is_word_char(rope.char(start - 1)) {
            start -= 1;
        }
        (start, rope.slice(start..head).to_string())
    }

    /// After typing `text`: opens completion on a trigger char or the start of a word.
    pub(crate) fn typed(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(server) = self.server else {
            return;
        };
        let Some(caps) = self.lsp.read(cx).capabilities(server) else {
            return;
        };
        if !caps.completion {
            return;
        }
        let mut chars = text.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return;
        };
        let trigger = caps
            .completion_triggers
            .iter()
            .any(|trigger| trigger.ends_with(ch));
        if trigger {
            self.request_completion(Some(ch), cx);
        } else if is_word_char(ch) && self.completion.is_none() && self.completion_task.is_none() {
            self.request_completion(None, cx);
        }
    }

    fn request_completion(&mut self, trigger: Option<char>, cx: &mut Context<Self>) {
        let (Some(server), Some(uri)) = (self.server, self.uri()) else {
            return;
        };
        let head = self.doc.selections().primary().head;
        let at = position_of(self.doc.rope(), head);
        let Some(request) = self.lsp.read(cx).request(
            server,
            "textDocument/completion",
            protocol::completion_params(&uri, at, trigger),
        ) else {
            return;
        };
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                this.completion_task = None;
                let Ok(result) = result else {
                    return;
                };
                let (items, incomplete) = protocol::completion(&result);
                let (start, _) = this.typed_word();
                this.completion = Some(CompletionMenu {
                    items,
                    shown: Vec::new(),
                    selected: 0,
                    start,
                    incomplete,
                    scroll: UniformListScrollHandle::new(),
                });
                this.filter_completion(cx);
            });
        }));
    }

    /// Narrows the menu to what matches the word typed so far; closes it when nothing does.
    fn filter_completion(&mut self, cx: &mut Context<Self>) {
        let (start, typed) = self.typed_word();
        let Some(menu) = &mut self.completion else {
            return;
        };
        menu.start = start;
        let mut shown: Vec<(i64, usize, Vec<usize>)> = menu
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let (score, _) = fuzzy::score(&typed, &item.filter_text)?;
                let positions = fuzzy::score(&typed, &item.label)
                    .map(|(_, positions)| positions)
                    .unwrap_or_default();
                Some((score, index, positions))
            })
            .collect();
        if typed.is_empty() {
            shown.sort_by(|a, b| menu.items[a.1].sort_text.cmp(&menu.items[b.1].sort_text));
        } else {
            shown.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then_with(|| menu.items[a.1].sort_text.cmp(&menu.items[b.1].sort_text))
            });
        }
        menu.shown = shown
            .into_iter()
            .map(|(_, index, positions)| (index, positions))
            .collect();
        menu.selected = 0;
        if menu.shown.is_empty() {
            self.completion = None;
        }
        cx.notify();
    }

    /// After an edit: the menu follows the word being typed, or closes when it is left.
    pub(crate) fn refresh_completion(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = &self.completion else {
            return;
        };
        let (start, _) = self.typed_word();
        let head = self.doc.selections().primary().head;
        // The menu belongs to the word it opened in; typing past its end closes it.
        let left = self.doc.selections().len() > 1 || head < menu.start || start != menu.start;
        if left {
            self.close_completion();
        } else if menu.incomplete {
            self.request_completion(None, cx);
        } else {
            self.filter_completion(cx);
        }
    }

    pub(crate) fn close_completion(&mut self) {
        self.completion = None;
        self.completion_task = None;
    }

    pub(crate) fn trigger_suggest(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.request_completion(None, cx);
    }

    fn step_completion(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.completion {
            let count = menu.shown.len().cast_signed();
            let selected = menu.selected.cast_signed() + by;
            menu.selected = if by.abs() > 1 {
                selected.clamp(0, count - 1)
            } else {
                selected.rem_euclid(count)
            }
            .unsigned_abs();
            let strategy = if by > 0 {
                ScrollStrategy::Bottom
            } else {
                ScrollStrategy::Top
            };
            menu.scroll.scroll_to_item(menu.selected, strategy);
            cx.notify();
        }
    }

    pub(crate) fn completion_next(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.step_completion(1, cx);
    }

    pub(crate) fn completion_previous(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.step_completion(-1, cx);
    }

    pub(crate) fn completion_next_page(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.step_completion(MENU_ROWS.cast_signed(), cx);
    }

    pub(crate) fn completion_previous_page(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.step_completion(-MENU_ROWS.cast_signed(), cx);
    }

    pub(crate) fn completion_dismiss(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        cx.notify();
    }

    /// Enter or Tab: the selected item replaces the word typed, and its extra edits (an
    /// import) go in too, asked for from the server if it hadn't sent them.
    pub(crate) fn completion_accept(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.completion.take() else {
            return;
        };
        self.completion_task = None;
        let Some(item) = menu
            .shown
            .get(menu.selected)
            .and_then(|(index, _)| menu.items.get(*index))
            .cloned()
        else {
            return;
        };
        let rope = self.doc.rope();
        let head = self.doc.selections().primary().head;
        let start = item
            .edit
            .as_ref()
            .map_or(menu.start, |edit| offset_of(rope, edit.start).min(head));
        let mut changes = vec![Change {
            range: start..head,
            text: item.insert_text.clone(),
        }];
        changes.extend(extra_edits(rope, &item.additional, start));
        let end = start + len_chars(&item.insert_text);
        let shift: isize = changes.iter().skip(1).map(Change::delta).sum();
        self.edit(cx, |doc| {
            let accepted = ops::apply_edits(doc, changes);
            if accepted {
                let cursor = end.saturating_add_signed(shift);
                ops::select_range(doc, cursor..cursor);
            }
            accepted
        });
        let resolve = item.additional.is_empty()
            && self.server.is_some_and(|server| {
                self.lsp
                    .read(cx)
                    .capabilities(server)
                    .is_some_and(|caps| caps.completion_resolve)
            });
        if resolve {
            self.resolve_completion(item.raw, start, cx);
        }
    }

    /// Asks the server for an accepted item's extra edits, and makes them if nothing was
    /// typed since and they stay clear of the inserted text.
    fn resolve_completion(
        &mut self,
        item: serde_json::Value,
        start: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.server else {
            return;
        };
        let Some(request) = self
            .lsp
            .read(cx)
            .request(server, "completionItem/resolve", item)
        else {
            return;
        };
        let revision = self.doc.revision();
        cx.spawn(async move |this, cx| {
            let Ok(resolved) = request.await else {
                return;
            };
            let edits = protocol::text_edits(&resolved["additionalTextEdits"]);
            if edits.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if this.doc.revision() != revision || this.doc.marked().is_some() {
                    return;
                }
                let changes = extra_edits(this.doc.rope(), &edits, start);
                this.edit(cx, |doc| ops::apply_edits(doc, changes));
            });
        })
        .detach();
    }

    // ── diagnostics a server waits to be asked for ────────────────────────────────────

    /// Asks a server that doesn't push its diagnostics for them, once typing pauses. A
    /// result for a text that has changed since is dropped; the next pause asks again.
    pub(crate) fn pull_diagnostics(&mut self, cx: &mut Context<Self>) {
        let (Some(server), Some(uri)) = (self.server, self.uri()) else {
            return;
        };
        let pulls = self
            .lsp
            .read(cx)
            .capabilities(server)
            .is_some_and(|caps| caps.pull_diagnostics);
        if !pulls {
            return;
        }
        let lsp = self.lsp.clone();
        self.pull_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PULL_DELAY).await;
            let Ok((request, revision)) = this.update(cx, |this, cx| {
                let request = lsp.read(cx).request(
                    server,
                    "textDocument/diagnostic",
                    protocol::document(&uri),
                );
                (request, this.doc.revision())
            }) else {
                return;
            };
            let Some(request) = request else {
                return;
            };
            let Ok(result) = request.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.doc.revision() != revision {
                    return;
                }
                if let Some(list) = protocol::pulled_diagnostics(&result) {
                    this.set_diagnostics(list, None, cx);
                }
            });
        }));
    }

    // ── hover ─────────────────────────────────────────────────────────────────────────

    /// The pointer moved over the text: after a rest, the hover for the word under it.
    pub(crate) fn hover_at(
        &mut self,
        position: Point<Pixels>,
        _: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = self.geometry else {
            return;
        };
        if !geometry.bounds.contains(&position) || position.x < geometry.text_left {
            if self.hover.take().is_some() {
                cx.notify();
            }
            self.hover_task = None;
            return;
        }
        let Some(offset) = self.offset_at(position) else {
            return;
        };
        if self
            .hover
            .as_ref()
            .is_some_and(|hover| hover.range.contains(&offset))
        {
            return;
        }
        if self.hover.take().is_some() {
            cx.notify();
        }
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |this, cx| this.request_hover(offset, cx));
        }));
    }

    pub(crate) fn show_hover(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let head = self.doc.selections().primary().head;
        self.request_hover(head, cx);
    }

    /// Shows what the server says about `offset`, with the problems there.
    fn request_hover(&mut self, offset: usize, cx: &mut Context<Self>) {
        let rope = self.doc.rope();
        let word = kiled_core::movement::word_at(rope, offset);
        let problems: Vec<String> = self
            .doc
            .diagnostics()
            .iter()
            .filter(|diagnostic| {
                diagnostic.range.start <= offset
                    && offset <= diagnostic.range.end.max(diagnostic.range.start + 1)
            })
            .map(|diagnostic| match &diagnostic.source {
                Some(source) => format!("{} ({source})", diagnostic.message),
                None => diagnostic.message.clone(),
            })
            .collect();
        let range = if word.is_empty() {
            offset..offset + 1
        } else {
            word
        };
        let request = match (self.server, self.uri()) {
            (Some(server), Some(uri)) => self.lsp.read(cx).request(
                server,
                "textDocument/hover",
                protocol::document_position(&uri, position_of(rope, offset)),
            ),
            _ => None,
        };
        let revision = self.doc.revision();
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            let info = match request {
                Some(request) => request
                    .await
                    .ok()
                    .and_then(|result| protocol::hover(&result)),
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                this.hover_task = None;
                if this.doc.revision() != revision {
                    return;
                }
                let text: Vec<String> = problems.into_iter().chain(info).collect();
                if text.is_empty() {
                    return;
                }
                this.hover = Some(Hover {
                    range,
                    text: text.join("\n\n").into(),
                });
                cx.notify();
            });
        }));
    }

    // ── definition, formatting, problems ──────────────────────────────────────────────

    pub(crate) fn go_to_definition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let head = self.doc.selections().primary().head;
        self.go_to_definition_at(head, window, cx);
    }

    pub(crate) fn go_to_definition_at(
        &mut self,
        offset: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(server), Some(uri)) = (self.server, self.uri()) else {
            self.message = Some("No language server for this file".into());
            cx.notify();
            return;
        };
        let Some(request) = self.lsp.read(cx).request(
            server,
            "textDocument/definition",
            protocol::document_position(&uri, position_of(self.doc.rope(), offset)),
        ) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                let found = result
                    .ok()
                    .map(|result| protocol::locations(&result))
                    .and_then(|locations| locations.into_iter().next());
                let Some(found) = found else {
                    this.message = Some("No definition found".into());
                    cx.notify();
                    return;
                };
                let from = this.doc.selections().primary().head;
                if Some(&found.uri) == this.uri().as_ref() {
                    this.go_to(found.start, found.end, cx);
                    cx.emit(EditorEvent::Jumped(from));
                } else if let Some(path) = protocol::path_of(&found.uri) {
                    cx.emit(EditorEvent::Jumped(from));
                    cx.emit(EditorEvent::OpenLocation {
                        path,
                        start: found.start,
                        end: found.end,
                    });
                }
            });
        })
        .detach();
    }

    pub(crate) fn format_document(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(server), Some(uri)) = (self.server, self.uri()) else {
            return;
        };
        self.commit_marked(cx);
        let indent = self.doc.indent();
        let Some(request) = self.lsp.read(cx).request(
            server,
            "textDocument/formatting",
            protocol::formatting_params(&uri, indent),
        ) else {
            self.message = Some("The language server doesn't format".into());
            cx.notify();
            return;
        };
        let revision = self.doc.revision();
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                let edits = match result {
                    Ok(result) => protocol::text_edits(&result),
                    Err(err) => {
                        this.message = Some(format!("Format failed: {err}").into());
                        cx.notify();
                        return;
                    }
                };
                // Edits for an older text, or one without the IME's preedit, would land in
                // the wrong places.
                if this.doc.revision() != revision || this.doc.marked().is_some() {
                    return;
                }
                let rope = this.doc.rope();
                let changes = edits
                    .iter()
                    .map(|edit| Change {
                        range: offset_of(rope, edit.start)..offset_of(rope, edit.end),
                        text: edit.text.clone(),
                    })
                    .collect();
                this.edit(cx, |doc| ops::apply_edits(doc, changes));
            });
        })
        .detach();
    }

    fn step_problem(&mut self, forward: bool, cx: &mut Context<Self>) {
        let head = self.doc.selections().primary().head;
        let diagnostics = self.doc.diagnostics();
        let found = if forward {
            diagnostics
                .iter()
                .find(|diagnostic| diagnostic.range.start > head)
                .or_else(|| diagnostics.first())
        } else {
            diagnostics
                .iter()
                .rev()
                .find(|diagnostic| diagnostic.range.start < head)
                .or_else(|| diagnostics.last())
        };
        let Some(found) = found.cloned() else {
            self.message = Some("No problems".into());
            cx.notify();
            return;
        };
        self.select_and_center(found.range.start..found.range.start, cx);
        let text = match &found.source {
            Some(source) => format!("{} ({source})", found.message),
            None => found.message.clone(),
        };
        self.hover = Some(Hover {
            range: found.range.start..found.range.end.max(found.range.start + 1),
            text: text.into(),
        });
        cx.notify();
    }

    pub(crate) fn next_problem(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.step_problem(true, cx);
    }

    pub(crate) fn previous_problem(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.step_problem(false, cx);
    }

    /// Errors and warnings in the document, for the status bar.
    pub fn problem_counts(&self) -> (usize, usize) {
        let diagnostics = self.doc.diagnostics();
        (
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == Severity::Error)
                .count(),
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.severity == Severity::Warning)
                .count(),
        )
    }

    // ── drawing ───────────────────────────────────────────────────────────────────────

    pub(crate) fn render_completion(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.completion.as_ref()?;
        let anchor = self.point_of(menu.start)?;
        let line_height = self.geometry?.line_height;
        let theme = cx.global::<Theme>();
        let colors = PopoverColors::of(theme);
        let rows: Vec<(SharedString, Vec<usize>, SharedString, &'static str)> = menu
            .shown
            .iter()
            .map(|(index, positions)| {
                let item = &menu.items[*index];
                (
                    item.label.clone().into(),
                    positions.clone(),
                    item.detail.clone().unwrap_or_default().into(),
                    kind_label(item.kind),
                )
            })
            .collect();
        let selected = menu.selected;
        let count = rows.len();
        let height = MENU_ROW_HEIGHT * f(count.min(MENU_ROWS)) + px(8.);
        let list = uniform_list("completions", count, move |range, _, _| {
            range
                .map(|index| {
                    let (label, positions, detail, kind) = rows[index].clone();
                    completion_row(
                        index,
                        &label,
                        &positions,
                        detail,
                        kind,
                        index == selected,
                        colors,
                    )
                })
                .collect()
        })
        .track_scroll(menu.scroll.clone())
        .h(height - px(8.));
        Some(
            deferred(
                anchored()
                    .position(point(anchor.x - px(8.), anchor.y + line_height))
                    .child(
                        div()
                            .id("completion-menu")
                            .occlude()
                            .w(px(420.))
                            .h(height)
                            .p_1()
                            .rounded_md()
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.bg)
                            .shadow_lg()
                            .text_size(px(12.5))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(list),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    pub(crate) fn render_hover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hover = self.hover.as_ref()?;
        let anchor = self.point_of(hover.range.start)?;
        let geometry = self.geometry?;
        if !geometry
            .bounds
            .contains(&point(anchor.x.max(geometry.bounds.left()), anchor.y))
        {
            return None;
        }
        let colors = PopoverColors::of(cx.global::<Theme>());
        let font = cx.global::<crate::settings::Settings>().font_family.clone();
        Some(
            deferred(
                anchored()
                    .position(point(anchor.x, anchor.y + geometry.line_height))
                    .child(
                        div()
                            .id("hover")
                            .occlude()
                            .max_w(px(560.))
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(colors.border)
                            .bg(colors.bg)
                            .shadow_lg()
                            .font_family(font)
                            .text_size(px(12.))
                            .text_color(colors.text)
                            .whitespace_normal()
                            .child(hover.text.clone()),
                    ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

/// A completion's extra edits as changes, keeping only those clear of the word being
/// completed: the server placed them in the text as it was when it was asked, and only
/// text before the word is sure to be unchanged since.
fn extra_edits(rope: &ropey::Rope, edits: &[protocol::TextEdit], start: usize) -> Vec<Change> {
    edits
        .iter()
        .map(|edit| Change {
            range: offset_of(rope, edit.start)..offset_of(rope, edit.end),
            text: edit.text.clone(),
        })
        .filter(|change| change.range.end <= start)
        .collect()
}

#[derive(Clone, Copy)]
struct PopoverColors {
    bg: Hsla,
    border: Hsla,
    text: Hsla,
    muted: Hsla,
    selected: Hsla,
    accent: Hsla,
}

impl PopoverColors {
    fn of(theme: &Theme) -> Self {
        Self {
            bg: theme.popover_bg,
            border: theme.divider,
            text: theme.chrome_text,
            muted: theme.muted,
            selected: theme.selected,
            accent: theme.accent,
        }
    }
}

fn completion_row(
    index: usize,
    label: &str,
    positions: &[usize],
    detail: SharedString,
    kind: &'static str,
    selected: bool,
    colors: PopoverColors,
) -> AnyElement {
    let chars: Vec<char> = label.chars().collect();
    let mut pieces: Vec<AnyElement> = Vec::new();
    let mut run = String::new();
    let mut run_matched = false;
    for (at, ch) in chars.iter().enumerate() {
        let matched = positions.contains(&at);
        if matched != run_matched && !run.is_empty() {
            pieces.push(label_piece(std::mem::take(&mut run), run_matched, colors));
        }
        run_matched = matched;
        run.push(*ch);
    }
    if !run.is_empty() {
        pieces.push(label_piece(run, run_matched, colors));
    }
    div()
        .id(index)
        .w_full()
        .h(MENU_ROW_HEIGHT)
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded_sm()
        .when(selected, |row| row.bg(colors.selected))
        .child(
            div()
                .w(px(34.))
                .flex_none()
                .text_size(px(10.))
                .text_color(colors.muted)
                .child(kind),
        )
        .child(div().flex().flex_none().children(pieces))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_right()
                .text_color(colors.muted)
                .child(detail),
        )
        .into_any_element()
}

fn label_piece(text: String, matched: bool, colors: PopoverColors) -> AnyElement {
    div()
        .text_color(if matched { colors.accent } else { colors.text })
        .child(text)
        .into_any_element()
}
