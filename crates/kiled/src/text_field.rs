//! A one-line text field with IME, for the Quick Open box, the find bar and Settings.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla,
    InspectorElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels,
    Point, ShapedLine, SharedString, Style, Subscription, TextRun, UTF16Selection, UnderlineStyle,
    Window, actions, div, fill, point, prelude::*, px, relative, size,
};
use kiled_core::text::is_word_char;

use crate::theme::Theme;

actions!(
    text_field,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteToStart,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        Home,
        End,
        SelectToHome,
        SelectToEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Confirm,
        Cancel
    ]
);

/// Editing keys; fixed, and live only while a field has focus.
pub fn bindings() -> Vec<KeyBinding> {
    let cx = Some("TextField");
    vec![
        KeyBinding::new("backspace", Backspace, cx),
        KeyBinding::new("shift-backspace", Backspace, cx),
        KeyBinding::new("delete", Delete, cx),
        KeyBinding::new("alt-backspace", DeleteWordLeft, cx),
        KeyBinding::new("cmd-backspace", DeleteToStart, cx),
        KeyBinding::new("left", Left, cx),
        KeyBinding::new("right", Right, cx),
        KeyBinding::new("alt-left", WordLeft, cx),
        KeyBinding::new("alt-right", WordRight, cx),
        KeyBinding::new("shift-left", SelectLeft, cx),
        KeyBinding::new("shift-right", SelectRight, cx),
        KeyBinding::new("home", Home, cx),
        KeyBinding::new("cmd-left", Home, cx),
        KeyBinding::new("ctrl-a", Home, cx),
        KeyBinding::new("end", End, cx),
        KeyBinding::new("cmd-right", End, cx),
        KeyBinding::new("ctrl-e", End, cx),
        KeyBinding::new("shift-home", SelectToHome, cx),
        KeyBinding::new("cmd-shift-left", SelectToHome, cx),
        KeyBinding::new("shift-end", SelectToEnd, cx),
        KeyBinding::new("cmd-shift-right", SelectToEnd, cx),
        KeyBinding::new("cmd-a", SelectAll, cx),
        KeyBinding::new("cmd-c", Copy, cx),
        KeyBinding::new("cmd-x", Cut, cx),
        KeyBinding::new("cmd-v", Paste, cx),
        KeyBinding::new("enter", Confirm, cx),
        KeyBinding::new("escape", Cancel, cx),
    ]
}

/// What a field tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldEvent {
    /// The text changed (typing, pasting, deleting).
    Changed(SharedString),
    /// Enter; or, for a field that commits on blur, focus left after an edit.
    Confirmed(SharedString),
    /// Escape. A field that commits on blur has gone back to its last confirmed text.
    Cancelled,
    /// Focus left without a confirmation.
    Blurred,
}

/// Text and selection in UTF-8 byte offsets on char boundaries. No GPUI types, so it is
/// unit-tested; the IME speaks UTF-16, converted at the edge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Buffer {
    text: String,
    anchor: usize,
    head: usize,
    /// IME preedit.
    marked: Option<Range<usize>>,
}

impl Buffer {
    /// All of `text`, selected.
    fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            anchor: 0,
            head: text.len(),
            marked: None,
        }
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    fn prev(&self, offset: usize) -> usize {
        self.text[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next(&self, offset: usize) -> usize {
        self.text[offset..]
            .chars()
            .next()
            .map_or(self.text.len(), |ch| offset + ch.len_utf8())
    }

    fn word_left(&self, offset: usize) -> usize {
        let before = &self.text[..offset];
        let trimmed = before.trim_end_matches(|ch: char| !is_word_char(ch));
        trimmed
            .char_indices()
            .rev()
            .find(|(_, ch)| !is_word_char(*ch))
            .map_or(0, |(index, ch)| index + ch.len_utf8())
    }

    fn word_right(&self, offset: usize) -> usize {
        let after = &self.text[offset..];
        let skipped = after.len() - after.trim_start_matches(|ch: char| !is_word_char(ch)).len();
        let rest = &after[skipped..];
        let word = rest
            .find(|ch: char| !is_word_char(ch))
            .unwrap_or(rest.len());
        offset + skipped + word
    }

    fn move_to(&mut self, offset: usize, extend: bool) {
        self.head = offset;
        if !extend {
            self.anchor = offset;
        }
    }

    fn left(&mut self) {
        let selection = self.selection();
        let to = if selection.is_empty() {
            self.prev(self.head)
        } else {
            selection.start
        };
        self.move_to(to, false);
    }

    fn right(&mut self) {
        let selection = self.selection();
        let to = if selection.is_empty() {
            self.next(self.head)
        } else {
            selection.end
        };
        self.move_to(to, false);
    }

    fn delete_back_to(&mut self, to: usize) {
        if self.selection().is_empty() {
            self.anchor = to;
        }
        self.replace(None, "");
    }

    fn backspace(&mut self) {
        self.delete_back_to(self.prev(self.head));
    }

    fn delete(&mut self) {
        if self.selection().is_empty() {
            self.head = self.next(self.head);
        }
        self.replace(None, "");
    }

    /// Replaces `range` (else the preedit, else the selection); the caret lands after it.
    fn replace(&mut self, range: Option<Range<usize>>, new: &str) {
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        self.text.replace_range(range.clone(), new);
        self.move_to(range.start + new.len(), false);
        self.marked = None;
    }

    /// IME preedit: replaces like [`Self::replace`], marks `new`, and selects `selected`
    /// (byte offsets within `new`).
    fn mark(&mut self, range: Option<Range<usize>>, new: &str, selected: Option<Range<usize>>) {
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        self.text.replace_range(range.clone(), new);
        let start = range.start;
        self.marked = (!new.is_empty()).then(|| start..start + new.len());
        let selected = selected.unwrap_or(new.len()..new.len());
        self.anchor = start + selected.start;
        self.head = start + selected.end;
    }

    fn to_utf16(&self, offset: usize) -> usize {
        self.text
            .get(..offset)
            .unwrap_or(&self.text)
            .encode_utf16()
            .count()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.to_utf16(range.start)..self.to_utf16(range.end)
    }

    /// Clamped and ordered: the IME's ranges are not trusted to fit.
    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        let (a, b) = (
            byte_offset(&self.text, range.start),
            byte_offset(&self.text, range.end),
        );
        a.min(b)..a.max(b)
    }
}

/// UTF-8 offset of the char at UTF-16 offset `utf16` in `text`, clamped to its end.
fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (index, ch) in text.char_indices() {
        if units >= utf16 {
            return index;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// Where the text was last painted, for clicks and the IME candidate window.
struct Layout {
    line: ShapedLine,
    bounds: Bounds<Pixels>,
    /// Horizontal scroll that keeps the caret visible.
    scroll: Pixels,
}

pub struct TextField {
    focus_handle: FocusHandle,
    buffer: Buffer,
    /// Text as last confirmed (or set); Escape returns to it when committing on blur.
    confirmed: String,
    /// Settings' fields: leaving the field confirms an edit, Escape reverts it.
    commit_on_blur: bool,
    placeholder: SharedString,
    layout: Option<Layout>,
    _blur: Subscription,
}

impl EventEmitter<FieldEvent> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextField {
    /// A field holding `text`, all selected.
    pub fn new(text: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let blur = cx.on_blur(&focus_handle, window, |this, _, cx| this.blurred(cx));
        Self {
            focus_handle,
            buffer: Buffer::new(text),
            confirmed: text.to_owned(),
            commit_on_blur: false,
            placeholder: SharedString::default(),
            layout: None,
            _blur: blur,
        }
    }

    /// Leaving the field confirms an edit, and Escape reverts it, as in Settings.
    #[must_use]
    pub fn committing_on_blur(mut self) -> Self {
        self.commit_on_blur = true;
        self
    }

    /// Shown, dimmed, while the field is empty.
    #[must_use]
    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn text(&self) -> &str {
        &self.buffer.text
    }

    /// Replaces the text, all selected, without emitting.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.buffer = Buffer::new(text);
        text.clone_into(&mut self.confirmed);
        cx.notify();
    }

    /// Replaces the text with the caret at its end, as if typed: emits `Changed`.
    pub fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.edit(cx, |buffer| {
            *buffer = Buffer::new(text);
            buffer.move_to(text.len(), false);
        });
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.buffer.anchor = 0;
        self.buffer.head = self.buffer.text.len();
        cx.notify();
    }

    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        let before = self.buffer.text.clone();
        f(&mut self.buffer);
        if self.buffer.text != before {
            cx.emit(FieldEvent::Changed(self.buffer.text.clone().into()));
        }
        cx.notify();
    }

    fn caret(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Buffer)) {
        f(&mut self.buffer);
        cx.notify();
    }

    fn blurred(&mut self, cx: &mut Context<Self>) {
        self.buffer.marked = None;
        if self.commit_on_blur && self.buffer.text != self.confirmed {
            self.confirm(&Confirm, cx);
        } else {
            cx.emit(FieldEvent::Blurred);
        }
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, cx: &mut Context<Self>) {
        self.confirmed.clone_from(&self.buffer.text);
        cx.emit(FieldEvent::Confirmed(self.confirmed.clone().into()));
    }

    fn cancel(&mut self, _: &Cancel, cx: &mut Context<Self>) {
        if self.commit_on_blur {
            self.buffer = Buffer::new(&self.confirmed);
        }
        cx.emit(FieldEvent::Cancelled);
        cx.notify();
    }

    fn copy(&mut self, _: &Copy, cx: &mut Context<Self>) {
        let selection = self.buffer.selection();
        if !selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.buffer.text[selection].to_owned(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, cx: &mut Context<Self>) {
        self.copy(&Copy, cx);
        self.edit(cx, |buffer| buffer.replace(None, ""));
    }

    fn paste(&mut self, _: &Paste, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let line = text.replace(['\r', '\n'], " ");
            self.edit(cx, |buffer| buffer.replace(None, &line));
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keep the click from reaching what is beneath.
        cx.stop_propagation();
        window.focus(&self.focus_handle);
        if let Some(layout) = &self.layout {
            let x = event.position.x - layout.bounds.left() + layout.scroll;
            let offset = layout.line.closest_index_for_x(x);
            let extend = event.modifiers.shift;
            self.caret(cx, |buffer| buffer.move_to(offset, extend));
        }
    }
}

impl Render for TextField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("TextField")
            .track_focus(&self.focus_handle)
            .w_full()
            .overflow_hidden()
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(|this, _: &Backspace, _, cx| this.edit(cx, Buffer::backspace)))
            .on_action(cx.listener(|this, _: &Delete, _, cx| this.edit(cx, Buffer::delete)))
            .on_action(cx.listener(|this, _: &DeleteWordLeft, _, cx| {
                this.edit(cx, |b| b.delete_back_to(b.word_left(b.head)));
            }))
            .on_action(cx.listener(|this, _: &DeleteToStart, _, cx| {
                this.edit(cx, |b| b.delete_back_to(0));
            }))
            .on_action(cx.listener(|this, _: &Left, _, cx| this.caret(cx, Buffer::left)))
            .on_action(cx.listener(|this, _: &Right, _, cx| this.caret(cx, Buffer::right)))
            .on_action(cx.listener(|this, _: &WordLeft, _, cx| {
                this.caret(cx, |b| b.move_to(b.word_left(b.head), false));
            }))
            .on_action(cx.listener(|this, _: &WordRight, _, cx| {
                this.caret(cx, |b| b.move_to(b.word_right(b.head), false));
            }))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| {
                this.caret(cx, |b| b.move_to(b.prev(b.head), true));
            }))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| {
                this.caret(cx, |b| b.move_to(b.next(b.head), true));
            }))
            .on_action(cx.listener(|this, _: &Home, _, cx| this.caret(cx, |b| b.move_to(0, false))))
            .on_action(cx.listener(|this, _: &End, _, cx| {
                this.caret(cx, |b| b.move_to(b.text.len(), false));
            }))
            .on_action(cx.listener(|this, _: &SelectToHome, _, cx| {
                this.caret(cx, |b| b.move_to(0, true));
            }))
            .on_action(cx.listener(|this, _: &SelectToEnd, _, cx| {
                this.caret(cx, |b| b.move_to(b.text.len(), true));
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, action, _, cx| this.copy(action, cx)))
            .on_action(cx.listener(|this, action, _, cx| this.cut(action, cx)))
            .on_action(cx.listener(|this, action, _, cx| this.paste(action, cx)))
            .on_action(cx.listener(|this, action, _, cx| this.confirm(action, cx)))
            .on_action(cx.listener(|this, action, _, cx| this.cancel(action, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .child(FieldElement { field: cx.entity() })
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range);
        actual.replace(self.buffer.range_to_utf16(&range));
        Some(self.buffer.text[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.buffer.selection()),
            reversed: self.buffer.head < self.buffer.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.buffer
            .marked
            .as_ref()
            .map(|range| self.buffer.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.buffer.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.buffer.range_from_utf16(&range));
        self.edit(cx, |buffer| buffer.replace(range, text));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| self.buffer.range_from_utf16(&range));
        let selected = selected.map(|sel| byte_offset(text, sel.start)..byte_offset(text, sel.end));
        self.caret(cx, |buffer| buffer.mark(range, text, selected));
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let range = self.buffer.range_from_utf16(&range);
        let left = layout.bounds.left() - layout.scroll;
        Some(Bounds::from_corners(
            point(
                left + layout.line.x_for_index(range.start),
                layout.bounds.top(),
            ),
            point(
                left + layout.line.x_for_index(range.end),
                layout.bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let x = position.x - layout.bounds.left() + layout.scroll;
        let index = layout.line.index_for_x(x)?;
        Some(self.buffer.to_utf16(index))
    }
}

/// Shapes and paints the text, preedit underline, selection and caret, and registers the
/// field as the window's input handler while it has focus.
struct FieldElement {
    field: Entity<TextField>,
}

struct Prepainted {
    line: ShapedLine,
    scroll: Pixels,
    selection: Option<PaintQuad>,
    caret: Option<PaintQuad>,
}

impl IntoElement for FieldElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn run(len: usize, font: gpui::Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

impl Element for FieldElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            size: size(relative(1.).into(), window.line_height().into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        &mut (): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = cx.global::<Theme>();
        let field = self.field.read(cx);
        let buffer = &field.buffer;
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        if buffer.text.is_empty() {
            let placeholder = field.placeholder.clone();
            let runs = [run(placeholder.len(), style.font(), theme.muted)];
            let line = window
                .text_system()
                .shape_line(placeholder, font_size, &runs, None);
            let caret = fill(
                Bounds::new(bounds.origin, size(px(1.5), bounds.size.height)),
                theme.accent,
            );
            return Prepainted {
                line,
                scroll: px(0.),
                selection: None,
                caret: Some(caret),
            };
        }
        let base = run(buffer.text.len(), style.font(), style.color);
        let runs: Vec<TextRun> = match &buffer.marked {
            None => vec![base],
            Some(marked) => [
                TextRun {
                    len: marked.start,
                    ..base.clone()
                },
                TextRun {
                    len: marked.len(),
                    underline: Some(UnderlineStyle {
                        color: Some(base.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..base.clone()
                },
                TextRun {
                    len: buffer.text.len() - marked.end,
                    ..base
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
        };
        let line =
            window
                .text_system()
                .shape_line(buffer.text.clone().into(), font_size, &runs, None);

        let caret_x = line.x_for_index(buffer.head);
        let scroll = (caret_x - bounds.size.width + px(2.)).max(px(0.));
        let x = |index| bounds.left() + line.x_for_index(index) - scroll;
        let selected = buffer.selection();
        let selection = (!selected.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(x(selected.start), bounds.top()),
                    point(x(selected.end), bounds.bottom()),
                ),
                theme.selection,
            )
        });
        let caret = selected.is_empty().then(|| {
            fill(
                Bounds::new(
                    point(x(buffer.head), bounds.top()),
                    size(px(1.5), bounds.size.height),
                ),
                theme.accent,
            )
        });
        Prepainted {
            line,
            scroll,
            selection,
            caret,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        &mut (): &mut Self::RequestLayoutState,
        prepainted: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        let focused = focus.is_focused(window);
        if focused && let Some(selection) = prepainted.selection.take() {
            window.paint_quad(selection);
        }
        let origin = point(bounds.left() - prepainted.scroll, bounds.top());
        let _ = prepainted
            .line
            .paint(origin, window.line_height(), window, cx);
        if focused && let Some(caret) = prepainted.caret.take() {
            window.paint_quad(caret);
        }
        let empty = self.field.read(cx).buffer.text.is_empty();
        let layout = Layout {
            line: if empty {
                ShapedLine::default()
            } else {
                prepainted.line.clone()
            },
            bounds,
            scroll: prepainted.scroll,
        };
        self.field
            .update(cx, |field, _| field.layout = Some(layout));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caret_at(text: &str, offset: usize) -> Buffer {
        let mut buffer = Buffer::new(text);
        buffer.move_to(offset, false);
        buffer
    }

    #[test]
    fn new_should_select_all() {
        assert_eq!(Buffer::new("标签").selection(), 0..6);
    }

    #[test]
    fn typing_should_replace_the_selection() {
        let mut buffer = Buffer::new("shell");
        buffer.replace(None, "build");
        assert_eq!((buffer.text.as_str(), buffer.selection()), ("build", 5..5));
    }

    #[test]
    fn backspace_should_remove_one_wide_char() {
        let mut buffer = caret_at("构建", 6);
        buffer.backspace();
        assert_eq!((buffer.text.as_str(), buffer.head), ("构", 3));
    }

    #[test]
    fn word_motions_should_skip_to_word_edges() {
        let buffer = caret_at("foo.bar baz", 0);
        assert_eq!(
            (
                buffer.word_right(0),
                buffer.word_right(3),
                buffer.word_left(11)
            ),
            (3, 7, 8)
        );
    }

    #[test]
    fn utf16_offsets_should_map_to_char_boundaries() {
        // "a" is 1 unit, "😀" is 2, "中" is 1.
        let buffer = Buffer::new("a😀中");
        assert_eq!(buffer.range_from_utf16(&(1..3)), 1..5);
        assert_eq!(buffer.to_utf16(5), 3);
    }

    #[test]
    fn preedit_should_be_marked_then_committed() {
        let mut buffer = caret_at("x", 1);
        buffer.mark(None, "zhong", None);
        assert_eq!(buffer.marked, Some(1..6));
        buffer.replace(None, "中");
        assert_eq!((buffer.text.as_str(), buffer.marked), ("x中", None));
    }
}
