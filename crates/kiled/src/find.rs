//! The find bar (Cmd-F) and Replace (Cmd-Alt-F), over the top right of an editor, as
//! VS Code has them (DESIGN.md §4.5).

use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, Entity, Focusable, Hsla, KeyBinding, SharedString,
    Subscription, Window, actions, div, prelude::*, px,
};
use kiled_core::{
    edit::Change,
    ops,
    search::{self, MATCH_LIMIT, Query},
};

use crate::{
    editor::{self, Editor},
    text_field::{FieldEvent, TextField},
    theme::Theme,
};

actions!(find, [ToggleCase, ToggleWord, ToggleRegex, ReplaceAll]);

/// The find bar's keys; fixed, and live while one of its fields has focus.
pub fn bindings() -> Vec<KeyBinding> {
    let cx = Some("FindBar");
    vec![
        KeyBinding::new("cmd-f", editor::Find, cx),
        KeyBinding::new("cmd-alt-f", editor::Replace, cx),
        KeyBinding::new("shift-enter", editor::FindPrevious, cx),
        KeyBinding::new("cmd-g", editor::FindNext, cx),
        KeyBinding::new("cmd-shift-g", editor::FindPrevious, cx),
        KeyBinding::new("cmd-alt-c", ToggleCase, cx),
        KeyBinding::new("cmd-alt-w", ToggleWord, cx),
        KeyBinding::new("cmd-alt-r", ToggleRegex, cx),
        KeyBinding::new("cmd-enter", ReplaceAll, cx),
    ]
}

/// An open find bar and its matches.
pub struct FindBar {
    query: Entity<TextField>,
    replacement: Entity<TextField>,
    replacing: bool,
    options: Query,
    /// Every match, in order, as of `revision`.
    pub(crate) matches: Vec<Range<usize>>,
    /// The match the editor's selection is on.
    pub(crate) current: Option<usize>,
    error: Option<SharedString>,
    revision: Option<u64>,
    _subscriptions: Vec<Subscription>,
}

impl Editor {
    /// Cmd-F (or Cmd-Alt-F with `replace`): opens the bar, or focuses it, seeded with the
    /// selection when it is on one line.
    pub(crate) fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let primary = *self.doc.selections().primary();
        let seed = (!primary.is_empty())
            .then(|| self.doc.rope().slice(primary.range()).to_string())
            .filter(|text| !text.contains('\n'));
        if self.find.is_none() {
            let query = cx.new(|cx| {
                TextField::new(seed.as_deref().unwrap_or_default(), window, cx)
                    .with_placeholder("Find")
            });
            let replacement =
                cx.new(|cx| TextField::new("", window, cx).with_placeholder("Replace"));
            let subscriptions = vec![
                cx.subscribe_in(&query, window, Self::query_event),
                cx.subscribe_in(&replacement, window, Self::replacement_event),
            ];
            self.find = Some(FindBar {
                query,
                replacement,
                replacing: replace,
                options: Query::default(),
                matches: Vec::new(),
                current: None,
                error: None,
                revision: None,
                _subscriptions: subscriptions,
            });
        }
        let Some(find) = &mut self.find else {
            return;
        };
        find.replacing |= replace;
        if let Some(seed) = seed {
            find.query.update(cx, |field, cx| field.set_text(&seed, cx));
            find.revision = None;
        }
        let field = if replace && !find.query.read(cx).text().is_empty() {
            find.replacement.clone()
        } else {
            find.query.clone()
        };
        field.update(cx, TextField::select_all);
        window.focus(&field.focus_handle(cx));
        self.refresh_find(cx);
        cx.notify();
    }

    pub(crate) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn query_event(
        &mut self,
        _: &Entity<TextField>,
        event: &FieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FieldEvent::Changed(_) => {
                if let Some(find) = &mut self.find {
                    find.revision = None;
                }
                // As VS Code does, the selection follows the first match from the cursor.
                self.refresh_find(cx);
                self.select_match(true, true, cx);
            }
            FieldEvent::Confirmed(_) => self.find_step(true, window, cx),
            FieldEvent::Cancelled => self.close_find(window, cx),
            FieldEvent::Blurred => {}
        }
    }

    fn replacement_event(
        &mut self,
        _: &Entity<TextField>,
        event: &FieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FieldEvent::Confirmed(_) => self.replace_one(cx),
            FieldEvent::Cancelled => self.close_find(window, cx),
            FieldEvent::Changed(_) | FieldEvent::Blurred => {}
        }
    }

    /// Finds again when the query, its options or the text changed.
    pub(crate) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let revision = self.doc.revision();
        let Some(find) = &mut self.find else {
            return;
        };
        if find.revision == Some(revision) {
            return;
        }
        find.revision = Some(revision);
        let query = Query {
            text: find.query.read(cx).text().to_owned(),
            ..find.options.clone()
        };
        match query.compile() {
            Ok(Some(regex)) => {
                find.matches = search::find_all(self.doc.rope(), &regex);
                find.error = None;
            }
            Ok(None) => {
                find.matches.clear();
                find.error = None;
            }
            Err(err) => {
                find.matches.clear();
                find.error = Some(err.lines().last().unwrap_or("invalid").to_owned().into());
            }
        }
        let primary = self.doc.selections().primary().range();
        find.current = find.matches.iter().position(|found| *found == primary);
    }

    /// Selects the next match after the selection (or before it), wrapping around. With
    /// `from_start`, a match starting at the selection counts, as while typing the query.
    fn select_match(&mut self, forward: bool, from_start: bool, cx: &mut Context<Self>) {
        let primary = self.doc.selections().primary().range();
        let Some(find) = &mut self.find else {
            return;
        };
        if find.matches.is_empty() {
            find.current = None;
            cx.notify();
            return;
        }
        let index = if forward {
            let from = if from_start {
                primary.start
            } else {
                primary.end
            };
            let after = find
                .matches
                .partition_point(|found| found.start < from || (!from_start && *found == primary));
            after % find.matches.len()
        } else {
            let before = find
                .matches
                .partition_point(|found| found.start < primary.start);
            before.checked_sub(1).unwrap_or(find.matches.len() - 1)
        };
        find.current = Some(index);
        let found = find.matches[index].clone();
        ops::select_range(&mut self.doc, found);
        self.moved(cx);
    }

    /// Cmd-G / Cmd-Shift-G, Enter / Shift-Enter in the bar.
    pub(crate) fn find_step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            self.open_find(false, window, cx);
            return;
        }
        self.refresh_find(cx);
        self.select_match(forward, false, cx);
    }

    fn toggle(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Query)) {
        if let Some(find) = &mut self.find {
            f(&mut find.options);
            find.revision = None;
        }
        self.refresh_find(cx);
        cx.notify();
    }

    fn replacement_for(&self, found: &Range<usize>, cx: &Context<Self>) -> Option<String> {
        let find = self.find.as_ref()?;
        let query = Query {
            text: find.query.read(cx).text().to_owned(),
            ..find.options.clone()
        };
        let regex = query.compile().ok()??;
        let matched = self.doc.rope().slice(found.clone()).to_string();
        Some(search::replacement(
            &regex,
            query.regex,
            &matched,
            find.replacement.read(cx).text(),
        ))
    }

    /// Enter in the Replace field: the current match is replaced, and the next selected.
    fn replace_one(&mut self, cx: &mut Context<Self>) {
        self.refresh_find(cx);
        let current = self
            .find
            .as_ref()
            .and_then(|find| find.current.and_then(|index| find.matches.get(index)))
            .cloned();
        let Some(found) = current else {
            self.select_match(true, false, cx);
            return;
        };
        let Some(text) = self.replacement_for(&found, cx) else {
            return;
        };
        if ops::replace_range(&mut self.doc, found, &text) {
            self.edited(cx);
        }
        self.refresh_find(cx);
        self.select_match(true, true, cx);
    }

    /// Every match replaced, as one undo step.
    fn replace_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_find(cx);
        let matches = self
            .find
            .as_ref()
            .map(|find| find.matches.clone())
            .unwrap_or_default();
        let changes: Vec<Change> = matches
            .iter()
            .filter_map(|found| {
                Some(Change {
                    range: found.clone(),
                    text: self.replacement_for(found, cx)?,
                })
            })
            .collect();
        if ops::apply_edits(&mut self.doc, changes) {
            self.edited(cx);
        }
        self.refresh_find(cx);
    }

    pub(crate) fn render_find(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        let theme = cx.global::<Theme>();
        let colors = BarColors {
            bg: theme.popover_bg,
            border: theme.divider,
            field: theme.editor_bg,
            text: theme.chrome_text,
            muted: theme.muted,
            on: theme.accent,
            hover: theme.hover,
            error: theme.failure,
        };
        let count = find.summary();
        let toggles = option_toggles(&find.options, colors, cx);
        let find_row = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                bar_button(
                    "find-expand",
                    if find.replacing { "▾" } else { "▸" },
                    colors,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    if let Some(find) = &mut this.find {
                        find.replacing = !find.replacing;
                    }
                    cx.notify();
                })),
            )
            .child(field_box(&find.query, colors))
            .child(toggles)
            .child(
                div()
                    .w(px(78.))
                    .text_size(px(11.))
                    .text_color(if find.error.is_some() {
                        colors.error
                    } else {
                        colors.muted
                    })
                    .truncate()
                    .child(count),
            )
            .child(bar_button("find-previous", "↑", colors).on_click(
                cx.listener(|this, _: &ClickEvent, window, cx| this.find_step(false, window, cx)),
            ))
            .child(bar_button("find-next", "↓", colors).on_click(
                cx.listener(|this, _: &ClickEvent, window, cx| this.find_step(true, window, cx)),
            ))
            .child(bar_button("find-close", "×", colors).on_click(
                cx.listener(|this, _: &ClickEvent, window, cx| this.close_find(window, cx)),
            ));
        let replace_row = find
            .replacing
            .then(|| replace_row(&find.replacement, colors, cx));
        Some(
            div()
                .id("find-bar")
                .key_context("FindBar")
                .absolute()
                .top(px(6.))
                .right(px(18.))
                .p_1()
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .border_1()
                .border_color(colors.border)
                .bg(colors.bg)
                .shadow_md()
                .text_size(px(12.))
                .text_color(colors.text)
                .on_action(cx.listener(|this, _: &ToggleCase, _, cx| {
                    this.toggle(cx, |query| query.case_sensitive = !query.case_sensitive);
                }))
                .on_action(cx.listener(|this, _: &ToggleWord, _, cx| {
                    this.toggle(cx, |query| query.whole_word = !query.whole_word);
                }))
                .on_action(cx.listener(|this, _: &ToggleRegex, _, cx| {
                    this.toggle(cx, |query| query.regex = !query.regex);
                }))
                .on_action(cx.listener(|this, _: &ReplaceAll, _, cx| this.replace_all(cx)))
                .child(find_row)
                .children(replace_row)
                .into_any_element(),
        )
    }
}

impl FindBar {
    /// "3 of 12", "No results", or why the regex is invalid.
    fn summary(&self) -> SharedString {
        match (&self.error, self.matches.len(), self.current) {
            (Some(error), _, _) => error.clone(),
            (None, 0, _) => "No results".into(),
            (None, count, current) => {
                let total = if count >= MATCH_LIMIT {
                    format!("{count}+")
                } else {
                    count.to_string()
                };
                match current {
                    Some(index) => format!("{} of {total}", index + 1).into(),
                    None => format!("{total} found").into(),
                }
            }
        }
    }
}

/// The Replace field and its buttons.
fn replace_row(
    field: &Entity<TextField>,
    colors: BarColors,
    cx: &mut Context<Editor>,
) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .pl(px(22.))
        .child(field_box(field, colors))
        .child(
            bar_button("replace-one", "Replace", colors)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.replace_one(cx))),
        )
        .child(
            bar_button("replace-all", "All", colors)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.replace_all(cx))),
        )
}

/// Match Case, Match Whole Word and Use Regular Expression.
fn option_toggles(options: &Query, colors: BarColors, cx: &mut Context<Editor>) -> gpui::Div {
    div()
        .flex()
        .gap(px(2.))
        .child(
            toggle("find-case", "Aa", options.case_sensitive, colors).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.toggle(cx, |query| query.case_sensitive = !query.case_sensitive);
                },
            )),
        )
        .child(
            toggle("find-word", "ab", options.whole_word, colors).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| {
                    this.toggle(cx, |query| query.whole_word = !query.whole_word);
                },
            )),
        )
        .child(
            toggle("find-regex", ".*", options.regex, colors).on_click(cx.listener(
                |this, _: &ClickEvent, _, cx| this.toggle(cx, |query| query.regex = !query.regex),
            )),
        )
}

#[derive(Clone, Copy)]
struct BarColors {
    bg: Hsla,
    border: Hsla,
    field: Hsla,
    text: Hsla,
    muted: Hsla,
    on: Hsla,
    hover: Hsla,
    error: Hsla,
}

fn field_box(field: &Entity<TextField>, colors: BarColors) -> impl IntoElement {
    div()
        .w(px(200.))
        .h(px(24.))
        .px_2()
        .flex()
        .items_center()
        .rounded_sm()
        .border_1()
        .border_color(colors.border)
        .bg(colors.field)
        .child(field.clone())
}

fn bar_button(
    id: &'static str,
    label: &'static str,
    colors: BarColors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(22.))
        .min_w(px(22.))
        .px_1()
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .text_color(colors.text)
        .hover(|style| style.bg(colors.hover))
        .child(label)
}

fn toggle(
    id: &'static str,
    label: &'static str,
    on: bool,
    colors: BarColors,
) -> gpui::Stateful<gpui::Div> {
    bar_button(id, label, colors)
        .text_size(px(11.))
        .when(on, |button| {
            button
                .border_1()
                .border_color(colors.on)
                .text_color(colors.on)
        })
        .when(!on, |button| button.text_color(colors.muted))
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use crate::editor::tests::editor_with;

    #[gpui::test]
    fn cmd_f_should_find_as_you_type_and_enter_step_through(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("foo bar foo baz foo", None, cx);
        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("foo");
        let first = editor.read_with(cx, |editor, _| editor.doc.selections().primary().range());
        cx.simulate_keystrokes("enter enter");
        let third = editor.read_with(cx, |editor, _| editor.doc.selections().primary().range());
        cx.simulate_keystrokes("shift-enter");
        let back = editor.read_with(cx, |editor, _| editor.doc.selections().primary().range());
        assert_eq!((first, third, back), (0..3, 16..19, 8..11));
    }

    #[gpui::test]
    fn replace_all_should_be_one_undo_step(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("a1 a2 a3", None, cx);
        cx.simulate_keystrokes("cmd-alt-f");
        cx.simulate_input("a");
        cx.simulate_keystrokes("cmd-alt-f");
        cx.simulate_input("b");
        cx.simulate_keystrokes("cmd-enter");
        let replaced = editor.read_with(cx, |editor, _| editor.doc.rope().to_string());
        cx.simulate_keystrokes("escape cmd-z");
        let undone = editor.read_with(cx, |editor, _| editor.doc.rope().to_string());
        assert_eq!(
            (replaced, undone),
            ("b1 b2 b3".to_owned(), "a1 a2 a3".to_owned())
        );
    }
}
