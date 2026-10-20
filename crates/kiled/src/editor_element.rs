//! Draws an editor: the lines in view with their highlights, selections, find matches,
//! diagnostics and cursors, the line numbers and the scrollbar (DESIGN.md §5.3).
//!
//! Text sits on a grid, as in a Kil pane: each run of ASCII is shaped as one line and placed
//! at its column, and every other char is placed by itself, so a wide or fallback glyph
//! can't push the rest of the line off the grid. Only the lines in view are laid out, and
//! only their columns in view.

use std::ops::Range;

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, ElementInputHandler, Entity, Font, Global,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UnderlineStyle, Window, fill, point, px, relative,
    size,
};
use kiled_core::{
    document::Severity,
    language::Highlight,
    text::{self, char_width},
};

use crate::{
    editor::{Editor, Geometry, Reveal},
    settings::Settings,
    theme::Theme,
};

/// Line height, as a multiple of the font size (VS Code's on macOS).
const LINE_HEIGHT: f32 = 1.5;
/// Lines kept between the cursor and the view's edge when scrolling to it.
const MARGIN_LINES: f32 = 2.;
const SCROLLBAR_WIDTH: Pixels = px(10.);
/// Space between the line numbers and the text.
const GUTTER_GAP: Pixels = px(18.);

pub struct EditorElement {
    editor: Entity<Editor>,
}

impl EditorElement {
    pub fn new(editor: Entity<Editor>) -> Self {
        Self { editor }
    }
}

/// What prepaint laid out and paint draws.
pub struct Layout {
    line_height: Pixels,
    text_area: Bounds<Pixels>,
    backgrounds: Vec<PaintQuad>,
    text: Vec<(Point<Pixels>, ShapedLine)>,
    numbers: Vec<(Point<Pixels>, ShapedLine)>,
    cursors: Vec<PaintQuad>,
    overlays: Vec<PaintQuad>,
}

/// The monospace cell width, cached per font and size: asking the text system every
/// frame costs more than the rest of the layout.
#[derive(Default)]
struct CellWidth {
    key: Option<(SharedString, Pixels)>,
    width: Pixels,
}

impl Global for CellWidth {}

fn cell_width(font: &Font, size: Pixels, window: &Window, cx: &mut App) -> Pixels {
    let cached = cx.default_global::<CellWidth>();
    if let Some((family, at)) = &cached.key
        && *family == font.family
        && *at == size
    {
        return cached.width;
    }
    let text_system = window.text_system();
    let width = text_system
        .advance(text_system.resolve_font(font), size, 'm')
        .map_or(size * 0.6, |advance| advance.width);
    cx.set_global(CellWidth {
        key: Some((font.family.clone(), size)),
        width,
    });
    width
}

/// Colors copied out of the theme, so the editor can be borrowed while laying out.
#[derive(Clone, Copy)]
struct Colors {
    text: Hsla,
    muted: Hsla,
    numbers_current: Hsla,
    cursor: Hsla,
    selection: Hsla,
    current_line: Hsla,
    find_match: Hsla,
    find_current: Hsla,
    scrollbar: Hsla,
}

/// How one char is drawn.
#[derive(Clone, Copy, PartialEq)]
struct CharStyle {
    color: Hsla,
    underline: Option<(Hsla, bool)>,
}

/// Chars of one line that are shaped together and placed at `col`.
struct Segment {
    col: usize,
    text: String,
    styles: Vec<(usize, CharStyle)>,
}

impl Segment {
    fn new(col: usize) -> Self {
        Self {
            col,
            text: String::new(),
            styles: Vec::new(),
        }
    }

    fn push(&mut self, ch: char, style: CharStyle) {
        match self.styles.last_mut() {
            Some((len, last)) if *last == style => *len += ch.len_utf8(),
            _ => self.styles.push((ch.len_utf8(), style)),
        }
        self.text.push(ch);
    }

    fn runs(&self, font: &Font) -> Vec<TextRun> {
        self.styles
            .iter()
            .map(|(len, style)| TextRun {
                len: *len,
                font: font.clone(),
                color: style.color,
                background_color: None,
                underline: style.underline.map(|(color, wavy)| UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.),
                    wavy,
                }),
                strikethrough: None,
            })
            .collect()
    }
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Layout;

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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
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
        let settings = cx.global::<Settings>();
        let font = gpui::font(settings.font_family.clone());
        let font_size = settings.font_size;
        let theme = cx.global::<Theme>();
        let colors = Colors {
            text: theme.text,
            muted: theme.muted,
            numbers_current: theme.chrome_text,
            cursor: theme.cursor,
            selection: theme.selection,
            current_line: theme.current_line,
            find_match: theme.find_match,
            find_current: theme.accent.opacity(0.5),
            scrollbar: theme.muted.opacity(0.35),
        };
        let syntax: Vec<(Highlight, Hsla)> = HIGHLIGHTS
            .iter()
            .map(|highlight| (*highlight, theme.syntax(*highlight)))
            .collect();
        let severities: Vec<(Severity, Hsla)> = SEVERITIES
            .iter()
            .map(|severity| (*severity, theme.severity(*severity)))
            .collect();
        let cell = cell_width(&font, font_size, window, cx);
        let line_height = (font_size * LINE_HEIGHT).round();
        let focused = self.editor.read(cx).focus_handle.is_focused(window);
        self.editor.update(cx, |editor, _| {
            layout(
                editor,
                bounds,
                &font,
                font_size,
                cell,
                line_height,
                focused,
                colors,
                &syntax,
                &severities,
                window,
            )
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        &mut (): &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );
        let line_height = layout.line_height;
        for quad in layout.backgrounds.drain(..) {
            window.paint_quad(quad);
        }
        window.with_content_mask(
            Some(ContentMask {
                bounds: layout.text_area,
            }),
            |window| {
                for (origin, line) in &layout.text {
                    let _ = line.paint(*origin, line_height, window, cx);
                }
                for quad in layout.cursors.drain(..) {
                    window.paint_quad(quad);
                }
            },
        );
        for (origin, line) in &layout.numbers {
            let _ = line.paint(*origin, line_height, window, cx);
        }
        for quad in layout.overlays.drain(..) {
            window.paint_quad(quad);
        }
    }
}

const HIGHLIGHTS: [Highlight; 16] = [
    Highlight::Comment,
    Highlight::Keyword,
    Highlight::String,
    Highlight::Escape,
    Highlight::Number,
    Highlight::Constant,
    Highlight::Function,
    Highlight::Type,
    Highlight::Property,
    Highlight::Builtin,
    Highlight::Variable,
    Highlight::Operator,
    Highlight::Punctuation,
    Highlight::Attribute,
    Highlight::Tag,
    Highlight::Label,
];

const SEVERITIES: [Severity; 4] = [
    Severity::Error,
    Severity::Warning,
    Severity::Information,
    Severity::Hint,
];

/// A line or column count as a float, for geometry.
#[expect(
    clippy::cast_precision_loss,
    reason = "line and column counts fit an f32"
)]
pub(crate) fn f(value: usize) -> f32 {
    value as f32
}

/// A geometric ratio as a whole count of lines or columns; below zero is zero.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped at zero, and far below usize::MAX"
)]
pub(crate) fn whole(value: f32) -> usize {
    value.max(0.) as usize
}

/// Scrolls to what the editor asked to reveal, keeping a margin of lines and columns.
fn apply_reveal(editor: &mut Editor, visible: f32, text_width: Pixels, cell: Pixels) {
    let Some(reveal) = editor.reveal.take() else {
        return;
    };
    let rope = editor.doc.rope();
    let tab = editor.tab_width();
    let offset = match reveal {
        Reveal::Newest => editor.doc.selections().newest().head,
        Reveal::Center(offset) => offset.min(rope.len_chars()),
    };
    let line = f(rope.char_to_line(offset));
    let margin = MARGIN_LINES.min((visible / 4.).floor());
    let in_view = line >= editor.scroll_top && line + 1. <= editor.scroll_top + visible;
    match reveal {
        Reveal::Center(_) if !in_view => editor.scroll_top = line - (visible / 3.).floor(),
        _ => {
            if line < editor.scroll_top + margin {
                editor.scroll_top = line - margin;
            } else if line + 1. > editor.scroll_top + visible - margin {
                editor.scroll_top = line + 1. - visible + margin;
            }
        }
    }
    editor.scroll_top = editor.scroll_top.clamp(0., editor.max_scroll_top());
    let x = cell * f(text::column(rope, offset, tab));
    if x < editor.scroll_left {
        editor.scroll_left = (x - cell * 4.).max(px(0.));
    } else if x > editor.scroll_left + text_width - cell * 2. {
        editor.scroll_left = x - text_width + cell * 4.;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the one layout pass of the editor"
)]
#[expect(clippy::too_many_lines, reason = "one pass over the lines in view")]
fn layout(
    editor: &mut Editor,
    bounds: Bounds<Pixels>,
    font: &Font,
    font_size: Pixels,
    cell: Pixels,
    line_height: Pixels,
    focused: bool,
    colors: Colors,
    syntax: &[(Highlight, Hsla)],
    severities: &[(Severity, Hsla)],
    window: &mut Window,
) -> Layout {
    let line_count = editor.doc.rope().len_lines();
    let digits = line_count.to_string().len().max(2);
    let gutter = cell * f(digits) + GUTTER_GAP * 2.;
    let text_left = bounds.left() + gutter;
    let visible = (bounds.size.height / line_height).max(1.);
    let tall = f(line_count) > visible.floor();
    let text_width = bounds.size.width - gutter - if tall { SCROLLBAR_WIDTH } else { px(0.) };
    apply_reveal(editor, visible.floor(), text_width, cell);
    editor.scroll_top = editor.scroll_top.clamp(0., editor.max_scroll_top());
    let scrollbar = tall.then(|| {
        Bounds::new(
            point(bounds.right() - SCROLLBAR_WIDTH, bounds.top()),
            size(SCROLLBAR_WIDTH, bounds.size.height),
        )
    });
    editor.geometry = Some(Geometry {
        bounds,
        text_left,
        line_height,
        cell_width: cell,
        visible_lines: whole(visible),
        scrollbar,
    });

    let scroll_top = editor.scroll_top;
    let first = whole(scroll_top);
    let last = (first + whole(visible.ceil()) + 1).min(line_count);
    let scroll_left = editor.scroll_left;
    let tab = editor.tab_width();
    let first_col = whole(scroll_left / cell);
    let last_col = first_col + whole((text_width / cell).ceil()) + 1;
    let y_of = |line: usize| bounds.top() + line_height * (f(line) - scroll_top);
    let x_of = |col: usize| text_left + cell * f(col) - scroll_left;
    let text_area = Bounds::from_corners(
        point(text_left - px(2.), bounds.top()),
        point(text_left + text_width, bounds.bottom()),
    );

    let highlights = editor.doc.highlights(first..last);
    let rope = editor.doc.rope();
    let selections = editor.doc.selections();
    let mut backgrounds = Vec::new();
    let mut cursor_lines: Vec<usize> = Vec::new();
    for selection in selections.iter() {
        let line = rope.char_to_line(selection.head);
        if selection.is_empty() && !cursor_lines.contains(&line) {
            cursor_lines.push(line);
        }
    }
    for &line in &cursor_lines {
        if (first..last).contains(&line) {
            backgrounds.push(fill(
                Bounds::new(
                    point(bounds.left(), y_of(line)),
                    size(bounds.size.width, line_height),
                ),
                colors.current_line,
            ));
        }
    }

    let clip = |left: Pixels, right: Pixels| -> Option<(Pixels, Pixels)> {
        let left = left.max(text_area.left());
        let right = right.min(text_area.right());
        (right > left).then_some((left, right))
    };
    let range_quads = |range: &Range<usize>, color: Hsla, quads: &mut Vec<PaintQuad>| {
        if range.is_empty() {
            return;
        }
        let start_line = rope.char_to_line(range.start);
        let end_line = rope.char_to_line(range.end);
        for line in start_line.max(first)..=end_line.min(last.saturating_sub(1)) {
            let start = if line == start_line {
                text::column(rope, range.start, tab)
            } else {
                0
            };
            let end = if line == end_line {
                text::column(rope, range.end, tab)
            } else {
                text::column(rope, text::line_end(rope, line), tab) + 1
            };
            if let Some((left, right)) = clip(x_of(start), x_of(end)) {
                quads.push(fill(
                    Bounds::from_corners(
                        point(left, y_of(line)),
                        point(right, y_of(line) + line_height),
                    ),
                    color,
                ));
            }
        }
    };
    for selection in selections.iter() {
        range_quads(&selection.range(), colors.selection, &mut backgrounds);
    }
    if let Some(find) = &editor.find {
        let first_char = rope.line_to_char(first);
        let last_char = rope.line_to_char(last.min(rope.len_lines().saturating_sub(1)));
        let from = find.matches.partition_point(|found| found.end < first_char);
        for (index, found) in find.matches.iter().enumerate().skip(from) {
            if found.start > text::line_end(rope, rope.char_to_line(last_char)) {
                break;
            }
            let color = if Some(index) == find.current {
                colors.find_current
            } else {
                colors.find_match
            };
            range_quads(found, color, &mut backgrounds);
        }
    }

    // Text, segment by segment.
    let marked = editor.doc.marked();
    let diagnostics = editor.doc.diagnostics();
    let mut text_runs = Vec::new();
    let mut spans = highlights.iter().peekable();
    let color_of = |highlight: Highlight| {
        syntax
            .iter()
            .find(|(kind, _)| *kind == highlight)
            .map_or(colors.text, |(_, color)| *color)
    };
    let severity_color = |severity: Severity| {
        severities
            .iter()
            .find(|(kind, _)| *kind == severity)
            .map_or(colors.text, |(_, color)| *color)
    };
    let view_start = rope.line_to_char(first.min(line_count - 1));
    let view_end = text::line_end(rope, last.saturating_sub(1));
    let in_view: Vec<(Range<usize>, Severity)> = diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.range.start <= view_end && diagnostic.range.end >= view_start
        })
        .map(|diagnostic| {
            // An empty range still marks the char it sits on.
            let range = &diagnostic.range;
            (
                range.start..range.end.max(range.start + 1),
                diagnostic.severity,
            )
        })
        .collect();
    for line in first..last {
        let line_start = rope.line_to_char(line);
        let line_end = text::line_end(rope, line);
        let mut byte = rope.line_to_byte(line);
        let y = y_of(line);
        let mut col = 0;
        let mut segment = Segment::new(0);
        let mut flush = |segment: &mut Segment, next_col: usize| {
            if !segment.text.is_empty() {
                let runs = segment.runs(font);
                let shaped = window.text_system().shape_line(
                    std::mem::take(&mut segment.text).into(),
                    font_size,
                    &runs,
                    None,
                );
                text_runs.push((point(x_of(segment.col), y), shaped));
            }
            *segment = Segment::new(next_col);
        };
        // Hints (an unused name) aren't underlined, as VS Code doesn't; F8 and hover show them.
        let line_diagnostics: Vec<&(Range<usize>, Severity)> = in_view
            .iter()
            .filter(|(range, severity)| {
                *severity != Severity::Hint && range.start <= line_end && range.end > line_start
            })
            .collect();
        for (index, ch) in rope.slice(line_start..line_end).chars().enumerate() {
            let offset = line_start + index;
            let width = char_width(ch, col, tab);
            while spans.peek().is_some_and(|(range, _)| range.end <= byte) {
                spans.next();
            }
            let color = spans
                .peek()
                .filter(|(range, _)| range.start <= byte)
                .map_or(colors.text, |(_, highlight)| color_of(*highlight));
            byte += ch.len_utf8();
            if col > last_col {
                break;
            }
            if col + width < first_col || ch == '\t' {
                flush(&mut segment, col + width);
                col += width;
                continue;
            }
            let underline = if marked
                .as_ref()
                .is_some_and(|marked| marked.contains(&offset))
            {
                Some((colors.text, false))
            } else {
                line_diagnostics
                    .iter()
                    .filter(|(range, _)| range.contains(&offset))
                    .map(|(_, severity)| *severity)
                    .min()
                    .map(|severity| (severity_color(severity), true))
            };
            let style = CharStyle { color, underline };
            if ch.is_ascii() || width == 0 {
                if segment.text.is_empty() {
                    segment.col = col;
                }
                segment.push(ch, style);
            } else {
                flush(&mut segment, col);
                segment.push(ch, style);
                flush(&mut segment, col + width);
            }
            col += width;
        }
        flush(&mut segment, col);
    }

    // Line numbers, right-aligned, the cursors' lines brighter.
    let mut numbers = Vec::new();
    for line in first..last {
        let label = (line + 1).to_string();
        let color = if cursor_lines.contains(&line) {
            colors.numbers_current
        } else {
            colors.muted
        };
        let runs = [TextRun {
            len: label.len(),
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        let x = text_left - GUTTER_GAP - cell * f(label.len());
        let shaped = window
            .text_system()
            .shape_line(label.into(), font_size, &runs, None);
        numbers.push((point(x, y_of(line)), shaped));
    }

    let mut cursors = Vec::new();
    if focused && editor.cursor_visible {
        for selection in selections.iter() {
            let line = rope.char_to_line(selection.head);
            if !(first..last).contains(&line) {
                continue;
            }
            let x = x_of(text::column(rope, selection.head, tab));
            cursors.push(fill(
                Bounds::new(point(x, y_of(line)), size(px(2.), line_height)),
                colors.cursor,
            ));
        }
    }

    let mut overlays = Vec::new();
    if let Some((thumb, track)) = editor.scrollbar_thumb() {
        // Problems mark the track where they are, as VS Code's overview ruler does.
        for diagnostic in diagnostics.iter().take(1000) {
            let line = rope.char_to_line(diagnostic.range.start.min(rope.len_chars()));
            let y = track.top() + track.size.height * (f(line) / f(line_count));
            overlays.push(fill(
                Bounds::new(point(track.right() - px(4.), y), size(px(4.), px(2.))),
                severity_color(diagnostic.severity),
            ));
        }
        overlays.push(
            fill(
                Bounds::new(
                    point(thumb.left() + px(2.), thumb.top()),
                    size(thumb.size.width - px(4.), thumb.size.height),
                ),
                colors.scrollbar,
            )
            .corner_radii(px(3.)),
        );
    }

    Layout {
        line_height,
        text_area,
        backgrounds,
        text: text_runs,
        numbers,
        cursors,
        overlays,
    }
}
