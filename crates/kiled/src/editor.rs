//! A document in a tab: its text and cursors, how it scrolls, and what keys, the mouse, the
//! IME and the language server do to it (DESIGN.md §4, §6).
//!
//! The editing itself is `kiled_core`'s; this view routes input to it, keeps the language
//! server in step after every edit, and draws through [`EditorElement`].

use std::{
    ops::Range,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, KeyBinding, KeyContext, MouseButton, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Stateful, Subscription, Task,
    UTF16Selection, Window, actions, div, point, prelude::*, px,
};
use kiled_core::{
    document::{Diagnostic, Document, Position, offset_of},
    files::{self, Stamp},
    language::{self, Language},
    lsp::protocol,
    movement, ops,
    selection::{Selection, Selections},
    text,
};

use crate::{
    completion::{CompletionMenu, Hover},
    editor_element::{EditorElement, f, whole},
    find::FindBar,
    lsp::{Lsp, ServerId},
    settings::Settings,
};

actions!(
    editor,
    [
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        Find,
        Replace,
        FindNext,
        FindPrevious,
        ToggleLineComment,
        Indent,
        Outdent,
        MoveLineUp,
        MoveLineDown,
        CopyLineUp,
        CopyLineDown,
        DeleteLine,
        InsertLineBelow,
        InsertLineAbove,
        Newline,
        Tab,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        SelectAll,
        ExpandLineSelection,
        AddNextOccurrence,
        SelectAllOccurrences,
        AddCursorAbove,
        AddCursorBelow,
        Cancel,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveToLineStart,
        MoveToLineEnd,
        SelectToLineStart,
        SelectToLineEnd,
        MoveToStart,
        MoveToEnd,
        SelectToStart,
        SelectToEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        GoToDefinition,
        NextProblem,
        PreviousProblem,
        TriggerSuggest,
        ShowHover,
        FormatDocument,
    ]
);

actions!(
    completion,
    [
        CompletionNext,
        CompletionPrevious,
        CompletionNextPage,
        CompletionPreviousPage,
        CompletionAccept,
        CompletionDismiss,
    ]
);

/// The completion menu's keys; fixed, and live only while it is open.
pub fn bindings() -> Vec<KeyBinding> {
    let cx = Some("Editor && completing");
    vec![
        KeyBinding::new("down", CompletionNext, cx),
        KeyBinding::new("ctrl-n", CompletionNext, cx),
        KeyBinding::new("up", CompletionPrevious, cx),
        KeyBinding::new("ctrl-p", CompletionPrevious, cx),
        KeyBinding::new("pagedown", CompletionNextPage, cx),
        KeyBinding::new("pageup", CompletionPreviousPage, cx),
        KeyBinding::new("enter", CompletionAccept, cx),
        KeyBinding::new("tab", CompletionAccept, cx),
        KeyBinding::new("escape", CompletionDismiss, cx),
    ]
}

/// The cursor blinks this often while the window is active and nothing is typed.
pub const BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// What an editor tells its workspace.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorEvent {
    /// Its title or dirty state changed: the tab and window title follow.
    TitleChanged,
    /// Its text or cursors changed: worth saving the session for.
    Changed,
    /// Go to Definition found a place in another file.
    OpenLocation {
        /// The file.
        path: PathBuf,
        /// Where in it.
        start: Position,
        /// And to where.
        end: Position,
    },
    /// The cursor jumped from `offset` (a definition, a problem): Back returns there.
    Jumped(usize),
}

/// Where the next frame scrolls to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reveal {
    /// The newest cursor, with as little scrolling as will do.
    Newest,
    /// `offset`, in the middle of the view unless it is already in view.
    Center(usize),
}

/// How a mouse drag extends the selection it started.
#[derive(Clone, Debug)]
enum DragMode {
    Char,
    Word(Range<usize>),
    Line(Range<usize>),
}

#[derive(Clone, Debug)]
struct Drag {
    anchor: usize,
    mode: DragMode,
    /// Alt-click adds a cursor; the drag then shapes that new selection, not all of them.
    adding: bool,
}

/// Where the editor was last drawn, for the mouse, the IME and the popovers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// The editor's whole area.
    pub bounds: Bounds<Pixels>,
    /// Where column 0 is drawn, before scrolling.
    pub text_left: Pixels,
    pub line_height: Pixels,
    pub cell_width: Pixels,
    /// Lines that fit in the view.
    pub visible_lines: usize,
    /// The scrollbar's track, when the text is taller than the view.
    pub scrollbar: Option<Bounds<Pixels>>,
}

pub struct Editor {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) doc: Document,
    path: Option<PathBuf>,
    /// The file as last read or written, to notice changes made elsewhere.
    stamp: Option<Stamp>,
    /// "Untitled-N" while it has no path.
    untitled: u32,
    pub(crate) lsp: Entity<Lsp>,
    pub(crate) server: Option<ServerId>,
    pub(crate) lsp_version: i64,
    /// First line in view, fractional while scrolling smoothly.
    pub(crate) scroll_top: f32,
    pub(crate) scroll_left: Pixels,
    pub(crate) reveal: Option<Reveal>,
    pub(crate) geometry: Option<Geometry>,
    pub(crate) cursor_visible: bool,
    last_input: Instant,
    drag: Option<Drag>,
    /// The pointer's offset from the scrollbar thumb's top while dragging it.
    scrollbar_drag: Option<Pixels>,
    /// Cmd-D started from a bare cursor, so it matches whole words.
    whole_word_search: bool,
    pub(crate) find: Option<FindBar>,
    pub(crate) completion: Option<CompletionMenu>,
    pub(crate) completion_task: Option<Task<()>>,
    pub(crate) hover: Option<Hover>,
    pub(crate) hover_task: Option<Task<()>>,
    pub(crate) pull_task: Option<Task<()>>,
    /// A short message in the status bar ("No definition found").
    pub(crate) message: Option<SharedString>,
    /// The file changed on disk while this had unsaved edits, and the user was told.
    pub(crate) conflict: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Editor {
    /// An editor for `text`, read from `path` (or a new untitled document numbered
    /// `untitled`).
    pub fn new(
        path: Option<PathBuf>,
        text: &str,
        stamp: Option<Stamp>,
        untitled: u32,
        lsp: Entity<Lsp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = cx.global::<Settings>();
        let language = path.as_deref().and_then(language::for_path);
        let doc = Document::new(text, language, settings.indent());
        let focus_handle = cx.focus_handle();
        let subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| this.show_cursor(cx)),
            cx.on_blur(&focus_handle, window, |this, _, cx| {
                this.drag = None;
                this.hover = None;
                cx.notify();
            }),
        ];
        Self {
            focus_handle,
            doc,
            path,
            stamp,
            untitled,
            lsp,
            server: None,
            lsp_version: 0,
            scroll_top: 0.,
            scroll_left: px(0.),
            reveal: None,
            geometry: None,
            cursor_visible: true,
            last_input: Instant::now(),
            drag: None,
            scrollbar_drag: None,
            whole_word_search: false,
            find: None,
            completion: None,
            completion_task: None,
            hover: None,
            hover_task: None,
            pull_task: None,
            message: None,
            conflict: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn language(&self) -> Option<&'static Language> {
        self.doc.language()
    }

    /// The tab's title: the file name, or "Untitled-N".
    pub fn title(&self) -> SharedString {
        match &self.path {
            Some(path) => path
                .file_name()
                .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy())
                .into_owned()
                .into(),
            None => format!("Untitled-{}", self.untitled).into(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.is_dirty()
    }

    /// The LSP URI of its file.
    pub fn uri(&self) -> Option<String> {
        self.path.as_deref().map(protocol::uri_of)
    }

    /// What `textDocument/didOpen` needs: URI, language id, version and text.
    pub fn lsp_document(&self) -> Option<(String, &'static str, i64, String)> {
        Some((
            self.uri()?,
            self.language()?.lsp_id,
            self.lsp_version,
            self.doc.text_for_server().to_string(),
        ))
    }

    /// Registers the document with its language's server, starting it if needed.
    pub fn attach_server(&mut self, folder: Option<&Path>, cx: &mut Context<Self>) {
        let (Some(path), Some(language)) = (self.path.clone(), self.language()) else {
            return;
        };
        if self.doc.is_large() {
            return;
        }
        let editor = cx.entity().downgrade();
        let rope = self.doc.text_for_server();
        let version = self.lsp_version;
        self.server = self.lsp.update(cx, |lsp, cx| {
            lsp.attach(editor, language, &path, folder, version, &rope, cx)
        });
        self.pull_diagnostics(cx);
        cx.notify();
    }

    /// Tells the server the document is closed.
    pub fn detach_server(&mut self, cx: &mut Context<Self>) {
        if let (Some(server), Some(uri)) = (self.server.take(), self.uri()) {
            let editor = cx.entity().downgrade();
            self.lsp
                .update(cx, |lsp, _| lsp.detach(server, &editor, &uri));
        }
        self.doc.set_diagnostics(Vec::new());
    }

    /// The server's diagnostics for this document, in its current positions.
    pub fn set_diagnostics(
        &mut self,
        list: Vec<protocol::Diagnostic>,
        version: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        // Diagnostics for an older version would land in the wrong places.
        if version.is_some_and(|version| version != self.lsp_version) {
            return;
        }
        let rope = self.doc.rope();
        let diagnostics = list
            .into_iter()
            .map(|item| {
                let start = offset_of(rope, item.start);
                let end = offset_of(rope, item.end).max(start);
                Diagnostic {
                    range: start..end,
                    severity: item.severity,
                    message: item.message,
                    source: item.source,
                }
            })
            .collect();
        self.doc.set_diagnostics(diagnostics);
        cx.notify();
    }

    /// Saves to its path. A file changed on disk since it was read is the caller's to ask
    /// about first ([`Self::changed_on_disk`]).
    ///
    /// # Errors
    /// If it has no path, or writing fails.
    pub fn save(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        self.commit_marked(cx);
        let path = self.path.clone().ok_or("an untitled file needs a name")?;
        let text = self.doc.text_for_save();
        let stamp =
            files::save(&path, &text).map_err(|err| format!("{}: {err}", path.display()))?;
        self.stamp = Some(stamp);
        self.conflict = false;
        self.doc.mark_saved();
        if let (Some(server), Some(uri)) = (self.server, self.uri()) {
            self.lsp.update(cx, |lsp, _| lsp.did_save(server, &uri));
        }
        cx.emit(EditorEvent::TitleChanged);
        cx.notify();
        Ok(())
    }

    /// Saves to `path` and keeps editing it there: its language may change, and with it the
    /// server.
    ///
    /// # Errors
    /// If writing fails.
    pub fn save_as(
        &mut self,
        path: PathBuf,
        folder: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.detach_server(cx);
        let language = language::for_path(&path);
        self.path = Some(path);
        self.stamp = None;
        if language != self.doc.language() {
            self.doc.set_language(language);
        }
        self.save(cx)?;
        self.attach_server(folder, cx);
        Ok(())
    }

    /// The file on disk is not what was last read or written here.
    pub fn changed_on_disk(&self) -> bool {
        match (&self.path, self.stamp) {
            (Some(path), Some(stamp)) => files::stamp(path).is_ok_and(|now| now != stamp),
            _ => false,
        }
    }

    /// Takes the file's new contents from disk, as one undoable step.
    ///
    /// # Errors
    /// If it can't be read.
    pub fn reload(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.path.clone().ok_or("no file")?;
        let (text, stamp) =
            files::load(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        self.doc.reload(&text);
        self.stamp = Some(stamp);
        self.conflict = false;
        self.edited(cx);
        cx.emit(EditorEvent::TitleChanged);
        Ok(())
    }

    // ── after every change ────────────────────────────────────────────────────────────

    /// The text changed: the server hears of it, the menus follow, the cursor shows.
    pub(crate) fn edited(&mut self, cx: &mut Context<Self>) {
        let changes = self.doc.take_lsp_changes();
        if let (Some(server), Some(uri), false) = (self.server, self.uri(), changes.is_empty()) {
            self.lsp_version += 1;
            let version = self.lsp_version;
            let text = self.doc.text_for_server();
            self.lsp.update(cx, |lsp, _| {
                lsp.did_change(server, &uri, version, &changes, &text);
            });
            self.pull_diagnostics(cx);
        }
        self.hover = None;
        self.message = None;
        self.refresh_completion(cx);
        self.moved(cx);
        cx.emit(EditorEvent::TitleChanged);
    }

    /// The cursors moved (or the text changed): scroll to them and keep the cursor solid.
    pub(crate) fn moved(&mut self, cx: &mut Context<Self>) {
        self.reveal = Some(Reveal::Newest);
        self.last_input = Instant::now();
        self.cursor_visible = true;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// Every edit goes through here: a composition ends first, and the server hears of each
    /// change at once.
    pub(crate) fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Document) -> bool) {
        self.commit_marked(cx);
        if f(&mut self.doc) {
            self.edited(cx);
        }
    }

    /// Ends an IME composition, typing what it composed.
    pub(crate) fn commit_marked(&mut self, cx: &mut Context<Self>) {
        if self.doc.commit_marked() {
            self.edited(cx);
        }
    }

    fn motion(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Document)) {
        self.commit_marked(cx);
        f(&mut self.doc);
        self.close_completion();
        self.hover = None;
        self.moved(cx);
    }

    pub(crate) fn show_cursor(&mut self, cx: &mut Context<Self>) {
        if !self.cursor_visible {
            self.cursor_visible = true;
            cx.notify();
        }
    }

    /// One tick of the window's blink timer: toggles the cursor unless typing just happened.
    pub fn blink(&mut self, cx: &mut Context<Self>) {
        let typing = self.last_input.elapsed() < BLINK_INTERVAL;
        let visible = typing || !self.cursor_visible;
        if visible != self.cursor_visible {
            self.cursor_visible = visible;
            cx.notify();
        }
    }

    pub(crate) fn tab_width(&self) -> usize {
        self.doc.indent().width
    }

    // ── geometry ──────────────────────────────────────────────────────────────────────

    /// The char offset under `position`, as a click places the cursor.
    pub(crate) fn offset_at(&self, position: Point<Pixels>) -> Option<usize> {
        let geometry = self.geometry?;
        let rope = self.doc.rope();
        let y = (position.y - geometry.bounds.top()) / geometry.line_height + self.scroll_top;
        let line = whole(y);
        if line >= rope.len_lines() {
            return Some(rope.len_chars());
        }
        let x = (position.x - geometry.text_left + self.scroll_left) / geometry.cell_width;
        let col = whole(x + 0.5);
        Some(text::offset_at_column(rope, line, col, self.tab_width()))
    }

    /// The top-left of the cell at `offset`, in window coordinates.
    pub(crate) fn point_of(&self, offset: usize) -> Option<Point<Pixels>> {
        let geometry = self.geometry?;
        let rope = self.doc.rope();
        let line = rope.char_to_line(offset.min(rope.len_chars()));
        let col = text::column(rope, offset, self.tab_width());
        #[expect(
            clippy::cast_precision_loss,
            reason = "line and column counts fit an f32"
        )]
        let (line, col) = (line as f32, col as f32);
        Some(point(
            geometry.text_left + geometry.cell_width * col - self.scroll_left,
            geometry.bounds.top() + geometry.line_height * (line - self.scroll_top),
        ))
    }

    pub(crate) fn max_scroll_top(&self) -> f32 {
        #[expect(clippy::cast_precision_loss, reason = "line counts fit an f32")]
        let last = self.doc.rope().len_lines().saturating_sub(1) as f32;
        last
    }

    fn scroll_by(&mut self, lines: f32, x: Pixels, cx: &mut Context<Self>) {
        self.scroll_top = (self.scroll_top + lines).clamp(0., self.max_scroll_top());
        self.scroll_left = (self.scroll_left + x).max(px(0.));
        self.reveal = None;
        self.hover = None;
        cx.notify();
    }

    fn page(&self) -> isize {
        self.geometry
            .map_or(20, |geometry| {
                geometry.visible_lines.saturating_sub(1).max(1)
            })
            .cast_signed()
    }

    // ── actions ───────────────────────────────────────────────────────────────────────

    fn undo(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        self.edit(cx, Document::undo);
    }

    fn redo(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        self.edit(cx, Document::redo);
    }

    fn copy(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let (text, lines) = ops::copy(&self.doc);
        write_clipboard(text, lines, cx);
    }

    fn cut(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let mut copied = None;
        self.edit(cx, |doc| {
            let (text, lines) = ops::cut(doc);
            copied = Some((text, lines));
            true
        });
        if let Some((text, lines)) = copied {
            write_clipboard(text, lines, cx);
        }
    }

    fn paste(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        let lines = item.metadata().is_some_and(|meta| meta == WHOLE_LINES);
        self.close_completion();
        self.edit(cx, |doc| ops::paste(doc, &text, lines));
    }

    fn newline(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.close_completion();
        self.edit(cx, ops::newline);
    }

    fn tab(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, ops::tab);
    }

    fn backspace(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, ops::backspace);
    }

    fn delete(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, ops::delete);
    }

    fn delete_word_left(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::delete_to(doc, movement::word_left));
    }

    fn delete_word_right(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::delete_to(doc, movement::word_right));
    }

    fn delete_to_line_start(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::delete_to(doc, ops::line_start_or_break));
    }

    fn delete_to_line_end(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::delete_to(doc, ops::line_end_or_break));
    }

    fn toggle_line_comment(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, ops::toggle_line_comment);
    }

    fn indent(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, ops::indent_lines);
    }

    fn outdent(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, ops::outdent_lines);
    }

    fn move_line_up(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::move_lines(doc, true));
    }

    fn move_line_down(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::move_lines(doc, false));
    }

    fn copy_line_up(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::copy_lines(doc, false));
    }

    fn copy_line_down(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::copy_lines(doc, true));
    }

    fn delete_line(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, ops::delete_lines);
    }

    fn insert_line_below(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::insert_line(doc, true));
    }

    fn insert_line_above(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |doc| ops::insert_line(doc, false));
    }

    fn select_all(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, ops::select_all);
        self.reveal = None;
    }

    fn expand_line_selection(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, ops::expand_line_selection);
    }

    fn add_next_occurrence(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.doc.selections().len() == 1 && self.doc.selections().primary().is_empty() {
            self.whole_word_search = true;
        } else if self.doc.selections().len() == 1 {
            self.whole_word_search = false;
        }
        let whole_word = self.whole_word_search;
        self.motion(cx, |doc| {
            ops::add_next_occurrence(doc, whole_word);
        });
    }

    fn select_all_occurrences(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, ops::select_all_occurrences);
    }

    fn add_cursor_above(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            ops::add_cursor_vertically(doc, true);
        });
    }

    fn add_cursor_below(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            ops::add_cursor_vertically(doc, false);
        });
    }

    /// Escape: closes the innermost thing open, else leaves one cursor.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.completion.is_some() {
            self.close_completion();
        } else if self.hover.is_some() {
            self.hover = None;
        } else if self.find.is_some() {
            self.close_find(window, cx);
        } else if !ops::keep_primary(&mut self.doc) {
            return;
        }
        self.moved(cx);
    }

    fn move_by(
        &mut self,
        extend: bool,
        cx: &mut Context<Self>,
        to: fn(&ropey::Rope, usize) -> usize,
    ) {
        self.motion(cx, |doc| {
            ops::move_heads(doc, extend, |rope, selection| to(rope, selection.head));
        });
    }

    fn vertical(&mut self, lines: isize, extend: bool, cx: &mut Context<Self>) {
        self.motion(cx, |doc| ops::move_vertically(doc, lines, extend));
    }

    fn move_left(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            ops::move_heads(doc, false, |rope, selection| {
                movement::left(rope, selection, false)
            });
        });
    }

    fn move_right(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            ops::move_heads(doc, false, |rope, selection| {
                movement::right(rope, selection, false)
            });
        });
    }

    fn select_left(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            ops::move_heads(doc, true, |rope, selection| {
                movement::left(rope, selection, true)
            });
        });
    }

    fn select_right(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            ops::move_heads(doc, true, |rope, selection| {
                movement::right(rope, selection, true)
            });
        });
    }

    fn move_up(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, false, cx);
    }

    fn move_down(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, false, cx);
    }

    fn select_up(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, true, cx);
    }

    fn select_down(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, true, cx);
    }

    fn page_up(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let page = self.page();
        self.scroll_top = (self.scroll_top - f(page.unsigned_abs())).max(0.);
        self.vertical(-page, false, cx);
    }

    fn page_down(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let page = self.page();
        self.scroll_top = (self.scroll_top + f(page.unsigned_abs())).min(self.max_scroll_top());
        self.vertical(page, false, cx);
    }

    fn select_page_up(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let page = self.page();
        self.vertical(-page, true, cx);
    }

    fn select_page_down(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let page = self.page();
        self.vertical(page, true, cx);
    }

    fn move_word_left(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.move_by(false, cx, movement::word_left);
    }

    fn move_word_right(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.move_by(false, cx, movement::word_right);
    }

    fn select_word_left(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.move_by(true, cx, movement::word_left);
    }

    fn select_word_right(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.move_by(true, cx, movement::word_right);
    }

    fn move_to_line_start(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.move_by(false, cx, movement::home);
    }

    fn move_to_line_end(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.move_by(false, cx, movement::end);
    }

    fn select_to_line_start(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.move_by(true, cx, movement::home);
    }

    fn select_to_line_end(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.move_by(true, cx, movement::end);
    }

    fn move_to_start(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            doc.set_selections(Selections::single(Selection::cursor(0)));
        });
    }

    fn move_to_end(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            let end = doc.rope().len_chars();
            doc.set_selections(Selections::single(Selection::cursor(end)));
        });
    }

    fn select_to_start(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            let anchor = doc.selections().primary().anchor;
            doc.set_selections(Selections::single(Selection::new(anchor, 0)));
        });
    }

    fn select_to_end(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.motion(cx, |doc| {
            let anchor = doc.selections().primary().anchor;
            let end = doc.rope().len_chars();
            doc.set_selections(Selections::single(Selection::new(anchor, end)));
        });
    }

    /// Puts a single cursor (or selection) at `range` and scrolls it to the middle, as a
    /// jump does.
    pub fn select_and_center(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.commit_marked(cx);
        let len = self.doc.rope().len_chars();
        let range = range.start.min(len)..range.end.min(len);
        ops::select_range(&mut self.doc, range.clone());
        self.close_completion();
        self.moved(cx);
        self.reveal = Some(Reveal::Center(range.start));
    }

    /// Jumps to an LSP position (Go to Definition from another file, Go to Line).
    pub fn go_to(&mut self, start: Position, end: Position, cx: &mut Context<Self>) {
        let rope = self.doc.rope();
        let range = offset_of(rope, start)..offset_of(rope, end);
        self.select_and_center(range, cx);
    }

    /// Goes to line `line` (1-based), at `column` if given.
    pub fn go_to_line(&mut self, line: usize, column: Option<usize>, cx: &mut Context<Self>) {
        let rope = self.doc.rope();
        let line = line.saturating_sub(1).min(rope.len_lines() - 1);
        let start = rope.line_to_char(line);
        let offset = match column {
            Some(column) => (start + column.saturating_sub(1)).min(text::line_end(rope, line)),
            None => movement::home(rope, start).max(start),
        };
        let from = self.doc.selections().primary().head;
        self.select_and_center(offset..offset, cx);
        cx.emit(EditorEvent::Jumped(from));
    }

    /// Where the primary cursor is, 1-based, as the status bar shows it.
    pub fn cursor_position(&self) -> (usize, usize) {
        let rope = self.doc.rope();
        let head = self.doc.selections().primary().head;
        let line = rope.char_to_line(head);
        (line + 1, text::column(rope, head, self.tab_width()) + 1)
    }

    // ── mouse ─────────────────────────────────────────────────────────────────────────

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        if let Some(track) = self.geometry.and_then(|geometry| geometry.scrollbar)
            && track.contains(&event.position)
        {
            self.start_scrollbar_drag(event.position, cx);
            return;
        }
        let Some(offset) = self.offset_at(event.position) else {
            return;
        };
        self.commit_marked(cx);
        self.close_completion();
        self.hover = None;
        let rope = self.doc.rope().clone();
        let modifiers = event.modifiers;
        if modifiers.platform && event.click_count == 1 {
            self.doc
                .set_selections(Selections::single(Selection::cursor(offset)));
            self.moved(cx);
            self.go_to_definition_at(offset, window, cx);
            return;
        }
        let adding = modifiers.alt;
        let (anchor, head, mode) = match event.click_count {
            2 => {
                let word = movement::word_at(&rope, offset);
                (word.start, word.end, DragMode::Word(word))
            }
            3.. => {
                let line = rope.char_to_line(offset);
                let start = rope.line_to_char(line);
                let end = if line + 1 < rope.len_lines() {
                    rope.line_to_char(line + 1)
                } else {
                    rope.len_chars()
                };
                (start, end, DragMode::Line(start..end))
            }
            _ if modifiers.shift => {
                let anchor = self.doc.selections().primary().anchor;
                (anchor, offset, DragMode::Char)
            }
            _ => (offset, offset, DragMode::Char),
        };
        let mut selections = if adding {
            self.doc.selections().clone()
        } else {
            Selections::single(Selection::cursor(0))
        };
        let selection = Selection::new(anchor, head);
        if adding {
            selections.add(selection);
        } else {
            selections = Selections::single(selection);
        }
        self.doc.set_selections(selections);
        self.drag = Some(Drag {
            anchor,
            mode,
            adding,
        });
        self.moved(cx);
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.scrollbar_drag.is_some() {
            if event.pressed_button == Some(MouseButton::Left) {
                self.drag_scrollbar(event.position, cx);
            } else {
                self.scrollbar_drag = None;
            }
            return;
        }
        let Some(drag) = self.drag.clone() else {
            self.hover_at(event.position, event.modifiers.platform, window, cx);
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        let Some(offset) = self.offset_at(event.position) else {
            return;
        };
        let rope = self.doc.rope().clone();
        let (anchor, head) = match &drag.mode {
            DragMode::Char => (drag.anchor, offset),
            DragMode::Word(first) => {
                let word = movement::word_at(&rope, offset);
                if offset < first.start {
                    (first.end, word.start)
                } else {
                    (first.start, word.end.max(first.end))
                }
            }
            DragMode::Line(first) => {
                let line = rope.char_to_line(offset);
                let start = rope.line_to_char(line);
                let end = if line + 1 < rope.len_lines() {
                    rope.line_to_char(line + 1)
                } else {
                    rope.len_chars()
                };
                if start < first.start {
                    (first.end, start)
                } else {
                    (first.start, end)
                }
            }
        };
        let mut selections = self.doc.selections().clone();
        if drag.adding {
            let newest = selections.newest().id;
            selections.map(|selection| {
                if selection.id == newest {
                    Selection {
                        anchor,
                        head,
                        ..*selection
                    }
                } else {
                    *selection
                }
            });
        } else {
            selections = Selections::single(Selection::new(anchor, head));
        }
        self.doc.set_selections(selections);
        self.moved(cx);
    }

    fn start_scrollbar_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some((thumb, _)) = self.scrollbar_thumb() else {
            return;
        };
        let grab = if thumb.contains(&position) {
            position.y - thumb.top()
        } else {
            thumb.size.height / 2.
        };
        self.scrollbar_drag = Some(grab);
        self.drag_scrollbar(position, cx);
    }

    fn drag_scrollbar(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(grab), Some((thumb, track))) = (self.scrollbar_drag, self.scrollbar_thumb())
        else {
            return;
        };
        let travel = (track.size.height - thumb.size.height).max(px(1.));
        let fraction = ((position.y - grab - track.top()) / travel).clamp(0., 1.);
        self.scroll_top = fraction * self.max_scroll_top();
        self.reveal = None;
        cx.notify();
    }

    /// The scrollbar's thumb and track, when there is a scrollbar.
    pub(crate) fn scrollbar_thumb(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let geometry = self.geometry?;
        let track = geometry.scrollbar?;
        #[expect(clippy::cast_precision_loss, reason = "line counts fit an f32")]
        let (lines, visible) = (
            self.doc.rope().len_lines() as f32 + geometry.visible_lines as f32 - 1.,
            geometry.visible_lines as f32,
        );
        let height = (track.size.height * (visible / lines)).max(px(24.));
        let travel = track.size.height - height;
        let top = track.top() + travel * (self.scroll_top / self.max_scroll_top().max(1.));
        Some((
            Bounds::new(
                point(track.left(), top),
                gpui::size(track.size.width, height),
            ),
            track,
        ))
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(geometry) = self.geometry else {
            return;
        };
        let (lines, x) = match event.delta {
            ScrollDelta::Pixels(delta) => (-delta.y / geometry.line_height, -delta.x),
            ScrollDelta::Lines(delta) => (-delta.y * 3., geometry.cell_width * (-delta.x * 3.)),
        };
        // A mostly vertical swipe doesn't drift sideways.
        let x = if x.abs() * 2. < geometry.line_height * lines.abs() {
            px(0.)
        } else {
            x
        };
        self.scroll_by(lines, x, cx);
    }

    /// The editor proper: its key context, input and mouse handlers, and the element.
    fn render_editor(&mut self, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let mut context = KeyContext::new_with_defaults();
        context.add("Editor");
        if self.completion.is_some() {
            context.add("completing");
        }
        let root = div()
            .id("editor")
            .key_context(context)
            .track_focus(&self.focus_handle)
            .size_full()
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.drag = None;
                    this.scrollbar_drag = None;
                }),
            )
            .on_scroll_wheel(cx.listener(Self::scroll_wheel));
        let root = bind_editing(root, cx);
        let root = bind_motion(root, cx);
        bind_language(root, cx).child(EditorElement::new(cx.entity()))
    }
}

/// Clipboard metadata marking whole lines copied from bare cursors.
const WHOLE_LINES: &str = "kiled-whole-lines";

fn write_clipboard(text: String, whole_lines: bool, cx: &mut App) {
    let item = if whole_lines {
        ClipboardItem::new_string_with_metadata(text, WHOLE_LINES.to_owned())
    } else {
        ClipboardItem::new_string(text)
    };
    cx.write_to_clipboard(item);
}

macro_rules! bind {
    ($root:expr, $cx:expr, { $($action:ty => $method:ident),* $(,)? }) => {
        $root $(.on_action($cx.listener(|this: &mut Editor, _: &$action, window, cx| {
            this.$method(window, cx);
        })))*
    };
}

fn bind_editing(root: Stateful<gpui::Div>, cx: &mut Context<Editor>) -> Stateful<gpui::Div> {
    bind!(root, cx, {
        Undo => undo,
        Redo => redo,
        Cut => cut,
        Copy => copy,
        Paste => paste,
        Newline => newline,
        Tab => tab,
        Backspace => backspace,
        Delete => delete,
        DeleteWordLeft => delete_word_left,
        DeleteWordRight => delete_word_right,
        DeleteToLineStart => delete_to_line_start,
        DeleteToLineEnd => delete_to_line_end,
        ToggleLineComment => toggle_line_comment,
        Indent => indent,
        Outdent => outdent,
        MoveLineUp => move_line_up,
        MoveLineDown => move_line_down,
        CopyLineUp => copy_line_up,
        CopyLineDown => copy_line_down,
        DeleteLine => delete_line,
        InsertLineBelow => insert_line_below,
        InsertLineAbove => insert_line_above,
        SelectAll => select_all,
        ExpandLineSelection => expand_line_selection,
        AddNextOccurrence => add_next_occurrence,
        SelectAllOccurrences => select_all_occurrences,
        AddCursorAbove => add_cursor_above,
        AddCursorBelow => add_cursor_below,
        Cancel => cancel,
    })
}

fn bind_motion(root: Stateful<gpui::Div>, cx: &mut Context<Editor>) -> Stateful<gpui::Div> {
    bind!(root, cx, {
        MoveLeft => move_left,
        MoveRight => move_right,
        MoveUp => move_up,
        MoveDown => move_down,
        SelectLeft => select_left,
        SelectRight => select_right,
        SelectUp => select_up,
        SelectDown => select_down,
        MoveWordLeft => move_word_left,
        MoveWordRight => move_word_right,
        SelectWordLeft => select_word_left,
        SelectWordRight => select_word_right,
        MoveToLineStart => move_to_line_start,
        MoveToLineEnd => move_to_line_end,
        SelectToLineStart => select_to_line_start,
        SelectToLineEnd => select_to_line_end,
        MoveToStart => move_to_start,
        MoveToEnd => move_to_end,
        SelectToStart => select_to_start,
        SelectToEnd => select_to_end,
        PageUp => page_up,
        PageDown => page_down,
        SelectPageUp => select_page_up,
        SelectPageDown => select_page_down,
    })
}

fn bind_language(root: Stateful<gpui::Div>, cx: &mut Context<Editor>) -> Stateful<gpui::Div> {
    bind!(root, cx, {
        GoToDefinition => go_to_definition,
        NextProblem => next_problem,
        PreviousProblem => previous_problem,
        TriggerSuggest => trigger_suggest,
        ShowHover => show_hover,
        FormatDocument => format_document,
        CompletionNext => completion_next,
        CompletionPrevious => completion_previous,
        CompletionNextPage => completion_next_page,
        CompletionPreviousPage => completion_previous_page,
        CompletionAccept => completion_accept,
        CompletionDismiss => completion_dismiss,
    })
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_find(cx);
        let editor = self.render_editor(cx);
        let find = self.render_find(window, cx);
        let completion = self.render_completion(cx);
        let hover = self.render_hover(cx);
        div()
            .id("editor-view")
            .relative()
            .size_full()
            .on_action(cx.listener(|this, _: &Find, window, cx| this.open_find(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &Replace, window, cx| this.open_find(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FindNext, window, cx| this.find_step(true, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &FindPrevious, window, cx| this.find_step(false, window, cx)),
            )
            .child(editor)
            .children(find)
            .children(completion)
            .children(hover)
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let rope = self.doc.rope();
        let range = chars_from_utf16(rope, &range);
        actual.replace(utf16_from_chars(rope, &range));
        Some(rope.slice(range).to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let primary = *self.doc.selections().primary();
        Some(UTF16Selection {
            range: utf16_from_chars(self.doc.rope(), &primary.range()),
            reversed: primary.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.doc
            .marked()
            .map(|range| utf16_from_chars(self.doc.rope(), &range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.commit_marked(cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let composed = self.doc.marked().is_some();
        if composed {
            // The composition ends: its preedit goes, and what it produced is typed.
            self.doc.cancel_marked();
        } else if let Some(range) = range {
            let range = chars_from_utf16(self.doc.rope(), &range);
            let mut selections = self.doc.selections().clone();
            selections.keep_primary();
            selections.map(|selection| selection.with_range(range.clone()));
            self.doc.set_selections(selections);
        }
        let mut chars = text.chars();
        let typed = match (chars.next(), chars.next()) {
            (Some(ch), None) => ops::type_char(&mut self.doc, ch),
            _ => ops::insert(&mut self.doc, text),
        };
        if typed || composed {
            self.edited(cx);
        }
        if typed {
            self.typed(text, cx);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rope = self.doc.rope();
        let range = range.map(|range| chars_from_utf16(rope, &range));
        let selected = selected.map(|selected| {
            let to_chars = |units: usize| {
                let mut seen = 0;
                text.chars()
                    .take_while(|ch| {
                        seen += ch.len_utf16();
                        seen <= units
                    })
                    .count()
            };
            to_chars(selected.start)..to_chars(selected.end)
        });
        self.close_completion();
        // A selection it replaces is deleted for real, so the server hears of it.
        self.doc.set_marked(range, text, selected);
        self.edited(cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = chars_from_utf16(self.doc.rope(), &range);
        let origin = self.point_of(range.start)?;
        let geometry = self.geometry?;
        Some(Bounds::new(
            origin,
            gpui::size(geometry.cell_width, geometry.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_at(position)?;
        Some(self.doc.rope().char_to_utf16_cu(offset))
    }
}

/// A UTF-16 range of the whole text (as the IME counts) as chars, clamped and ordered.
fn chars_from_utf16(rope: &ropey::Rope, range: &Range<usize>) -> Range<usize> {
    let total = rope.len_utf16_cu();
    let a = rope.utf16_cu_to_char(range.start.min(total));
    let b = rope.utf16_cu_to_char(range.end.min(total));
    a.min(b)..a.max(b)
}

fn utf16_from_chars(rope: &ropey::Rope, range: &Range<usize>) -> Range<usize> {
    rope.char_to_utf16_cu(range.start)..rope.char_to_utf16_cu(range.end)
}

#[cfg(test)]
pub(crate) mod tests {
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;
    use crate::theme::Theme;

    pub(crate) fn editor_with<'a>(
        text: &str,
        path: Option<&str>,
        cx: &'a mut TestAppContext,
    ) -> (Entity<Editor>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.set_global(Settings::default());
            cx.bind_keys(crate::keymap::bindings(&crate::keymap::resolve(None).0));
            cx.bind_keys(bindings());
            cx.bind_keys(crate::text_field::bindings());
            cx.bind_keys(crate::find::bindings());
        });
        let text = text.to_owned();
        let path = path.map(PathBuf::from);
        let (editor, cx) = cx.add_window_view(|window, cx| {
            let lsp = cx.new(Lsp::new);
            let editor = Editor::new(path, &text, None, 1, lsp, window, cx);
            window.focus(&editor.focus_handle);
            editor
        });
        cx.run_until_parked();
        (editor, cx)
    }

    fn text(editor: &Entity<Editor>, cx: &mut VisualTestContext) -> String {
        editor.read_with(cx, |editor, _| editor.doc.rope().to_string())
    }

    #[gpui::test]
    fn typing_should_go_through_the_input_handler_and_pair_brackets(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("", Some("a.rs"), cx);
        cx.simulate_input("fn f(");
        assert_eq!(text(&editor, cx), "fn f()");
    }

    #[gpui::test]
    fn the_default_keys_should_edit_like_vs_code(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("one\ntwo\n", Some("a.rs"), cx);
        // Alt-Down moves the line, Cmd-/ comments it, Cmd-Z undoes the comment.
        cx.simulate_keystrokes("alt-down cmd-/");
        let commented = text(&editor, cx);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(
            (commented, text(&editor, cx)),
            ("two\n// one\n".to_owned(), "two\none\n".to_owned())
        );
    }

    #[gpui::test]
    fn cmd_d_should_add_the_next_occurrence_and_typing_replace_both(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("let a = a + b;", Some("a.rs"), cx);
        editor.update(cx, |editor, _| {
            editor
                .doc
                .set_selections(Selections::single(Selection::cursor(4)));
        });
        cx.simulate_keystrokes("cmd-d cmd-d");
        cx.simulate_input("x");
        assert_eq!(text(&editor, cx), "let x = x + b;");
    }

    #[gpui::test]
    fn copy_without_a_selection_should_copy_the_line(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("first\nsecond\n", None, cx);
        cx.simulate_keystrokes("cmd-c down cmd-v");
        assert_eq!(text(&editor, cx), "first\nfirst\nsecond\n");
    }

    #[gpui::test]
    fn ime_composition_should_commit_as_one_typed_step(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("x", None, cx);
        editor.update_in(cx, |editor, window, cx| {
            editor
                .doc
                .set_selections(Selections::single(Selection::cursor(1)));
            editor.replace_and_mark_text_in_range(None, "zhong", None, window, cx);
            editor.replace_text_in_range(None, "中", window, cx);
        });
        let typed = text(&editor, cx);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(
            (typed, text(&editor, cx)),
            ("x中".to_owned(), "x".to_owned())
        );
    }

    #[gpui::test]
    fn ime_over_a_selection_should_tell_the_server_and_undo_to_the_selection(
        cx: &mut TestAppContext,
    ) {
        let (editor, cx) = editor_with("hello world", None, cx);
        let unsent = editor.update_in(cx, |editor, window, cx| {
            editor
                .doc
                .set_selections(Selections::single(Selection::new(0, 5)));
            editor.replace_and_mark_text_in_range(None, "n", None, window, cx);
            let unsent = editor.doc.take_lsp_changes().len();
            editor.replace_text_in_range(None, "你", window, cx);
            unsent
        });
        let typed = text(&editor, cx);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(
            (unsent, typed, text(&editor, cx)),
            (0, "你 world".to_owned(), "hello world".to_owned())
        );
    }

    #[gpui::test]
    fn a_click_that_ends_a_composition_should_tell_the_server(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("x\ny\n", Some("a.rs"), cx);
        let opened = editor.update_in(cx, |editor, window, cx| {
            editor
                .doc
                .set_selections(Selections::single(Selection::cursor(1)));
            editor.replace_and_mark_text_in_range(None, "zh", None, window, cx);
            editor.lsp_document().map(|(.., text)| text)
        });
        let below = editor.read_with(cx, |editor, _| editor.point_of(3).unwrap());
        cx.simulate_click(point(below.x + px(1.), below.y + px(2.)), Modifiers::none());
        let unsent = editor.update(cx, |editor, _| editor.doc.take_lsp_changes().len());
        assert_eq!(
            (opened, unsent, text(&editor, cx)),
            (Some("x\ny\n".to_owned()), 0, "xzh\ny\n".to_owned())
        );
    }

    #[gpui::test]
    fn alt_click_should_add_a_cursor(cx: &mut TestAppContext) {
        let (editor, cx) = editor_with("aaaa\nbbbb\n", None, cx);
        cx.run_until_parked();
        let (first, second) = editor.read_with(cx, |editor, _| {
            (editor.point_of(1).unwrap(), editor.point_of(7).unwrap())
        });
        let lh = editor.read_with(cx, |editor, _| editor.geometry.unwrap().line_height);
        let inside = |p: Point<Pixels>| point(p.x + px(1.), p.y + lh / 2.);
        cx.simulate_click(inside(first), Modifiers::none());
        cx.simulate_click(inside(second), Modifiers::alt());
        cx.simulate_input("X");
        assert_eq!(text(&editor, cx), "aXaaa\nbbXbb\n");
    }
}
