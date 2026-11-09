//! The sidebar's explorer: the open folder as a tree, each folder read when it is first
//! expanded and again when the window comes back to the front (DESIGN.md §7).

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    KeyBinding, MouseButton, MouseDownEvent, Pixels, Point, ScrollStrategy, SharedString, Stateful,
    Subscription, UniformListScrollHandle, Window, actions, div, prelude::*, px, uniform_list,
};
use kiled_core::files::{self, Entry};

use crate::{
    text_field::{FieldEvent, TextField},
    theme::Theme,
};

actions!(
    file_tree,
    [
        Next,
        Previous,
        Expand,
        Collapse,
        Activate,
        Dismiss,
        RenameSelected,
        TrashSelected,
        First,
        Last
    ]
);

/// Keys for moving through the tree; fixed, and live while it has focus.
pub fn bindings() -> Vec<KeyBinding> {
    let cx = Some("FileTree");
    vec![
        KeyBinding::new("down", Next, cx),
        KeyBinding::new("up", Previous, cx),
        KeyBinding::new("right", Expand, cx),
        KeyBinding::new("left", Collapse, cx),
        KeyBinding::new("enter", Activate, cx),
        // No Space binding: it would swallow the key before the inline name field (a
        // child of this view, with no Space binding of its own) could type it.
        KeyBinding::new("escape", Dismiss, cx),
        KeyBinding::new("f2", RenameSelected, cx),
        KeyBinding::new("cmd-backspace", TrashSelected, cx),
        KeyBinding::new("home", First, cx),
        KeyBinding::new("end", Last, cx),
    ]
}

const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 12.;

/// What the tree tells the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeEvent {
    /// A file was clicked or entered.
    Open(PathBuf),
    /// Escape: the editor should have the keys again.
    Dismiss,
    /// A new file or folder was named inline; the workspace makes it, since it keeps the
    /// open editors consistent with the disk.
    Create {
        dir: PathBuf,
        name: String,
        is_dir: bool,
    },
    /// An entry was renamed inline; the workspace renames and remaps its editors.
    Rename { from: PathBuf, name: String },
    /// An entry should move to the Trash; the workspace asks and closes its editors.
    Trash(PathBuf),
    /// The user right-clicked an entry; the workspace shows its menu here.
    ContextMenu {
        path: PathBuf,
        is_dir: bool,
        position: Point<Pixels>,
    },
    /// A message for the user (a name that was refused).
    Notice(String),
}

#[derive(Clone, Debug)]
struct Row {
    path: PathBuf,
    name: SharedString,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    /// The inline name field stands in for this row's name (a rename), or this row is the
    /// synthetic one a new entry is named on (a create).
    editing: bool,
}

#[derive(Clone, Debug)]
enum InlineKind {
    Create { dir: PathBuf, is_dir: bool },
    Rename { from: PathBuf },
}

/// A name being typed in the tree. Dropping it drops the subscription, so the field's blur
/// on its way out can't cancel an edit that is already over.
struct InlineEdit {
    kind: InlineKind,
    field: Entity<TextField>,
    _subscription: Subscription,
}

pub struct FileTree {
    focus_handle: FocusHandle,
    root: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    listings: HashMap<PathBuf, Vec<Entry>>,
    rows: Vec<Row>,
    selected: Option<usize>,
    /// The active editor's file, marked in the tree.
    active: Option<PathBuf>,
    scroll: UniformListScrollHandle,
    editing: Option<InlineEdit>,
}

impl EventEmitter<TreeEvent> for FileTree {}

impl Focusable for FileTree {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl FileTree {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            root: None,
            expanded: HashSet::new(),
            listings: HashMap::new(),
            rows: Vec::new(),
            selected: None,
            active: None,
            scroll: UniformListScrollHandle::new(),
            editing: None,
        }
    }

    /// Shows `root`, its first level expanded.
    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
        self.editing = None;
        self.expanded.clear();
        self.listings.clear();
        self.selected = None;
        if let Some(root) = &root {
            self.expanded.insert(root.clone());
        }
        self.root = root;
        self.refresh(cx);
    }

    /// Reads every expanded folder again, for changes made elsewhere.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        // A refresh re-reads the disk the edit was naming something on; the edit goes.
        // The workspace refreshes after a create it handled, whose edit is already over.
        self.editing = None;
        self.listings.clear();
        self.expanded.retain(|dir| dir.is_dir());
        self.rebuild();
        cx.notify();
    }

    fn listing(&mut self, dir: &Path) -> &[Entry] {
        self.listings
            .entry(dir.to_owned())
            .or_insert_with(|| files::list_dir(dir).unwrap_or_default())
    }

    fn rebuild(&mut self) {
        let selected = self
            .selected
            .and_then(|index| self.rows.get(index))
            .cloned();
        let mut rows = Vec::new();
        if let Some(root) = self.root.clone() {
            self.push_children(&root, 0, &mut rows);
        }
        self.rows = rows;
        // A create row has no path of its own (it borrows its folder's), so it is found
        // again by its editing mark instead.
        self.selected = selected.and_then(|row| {
            if row.editing {
                self.rows.iter().position(|row| row.editing)
            } else {
                self.rows.iter().position(|other| other.path == row.path)
            }
        });
    }

    fn push_children(&mut self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let entries = self.listing(dir).to_vec();
        let create = match &self.editing {
            Some(edit) => match &edit.kind {
                InlineKind::Create { dir: into, is_dir } if into == dir => Some(*is_dir),
                _ => None,
            },
            None => None,
        };
        let rename_from = match &self.editing {
            Some(edit) => match &edit.kind {
                InlineKind::Rename { from } => Some(from.clone()),
                InlineKind::Create { .. } => None,
            },
            None => None,
        };
        if let Some(is_dir) = create {
            rows.push(Row {
                path: dir.to_owned(),
                name: SharedString::default(),
                depth,
                is_dir,
                expanded: false,
                editing: true,
            });
        }
        for entry in entries {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            let editing = rename_from.as_deref() == Some(entry.path.as_path());
            rows.push(Row {
                path: entry.path.clone(),
                name: entry.name.clone().into(),
                depth,
                is_dir: entry.is_dir,
                expanded,
                editing,
            });
            if expanded {
                self.push_children(&entry.path, depth + 1, rows);
            }
        }
    }

    /// Expands `path` (a folder the workspace just made, say) and rebuilds.
    pub fn expand(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.expanded.insert(path.to_owned());
        self.rebuild();
        cx.notify();
    }

    /// Only the root stays expanded.
    fn collapse_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_editing = self.editing.take().is_some();
        self.expanded.clear();
        if let Some(root) = &self.root {
            self.expanded.insert(root.clone());
        }
        self.rebuild();
        // A collapsed-away edit would keep the keys in a field no row shows.
        if was_editing {
            window.focus(&self.focus_handle);
        }
        cx.notify();
    }

    // ── inline naming ─────────────────────────────────────────────────────────────────

    fn cancel_editing(&mut self) {
        if self.editing.take().is_some() {
            self.rebuild();
        }
    }

    fn end_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_editing();
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn make_field(
        &mut self,
        text: &str,
        kind: InlineKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        let field = cx.new(|cx| TextField::new(text, window, cx));
        let subscription = cx.subscribe_in(&field, window, Self::field_event);
        self.editing = Some(InlineEdit {
            kind,
            field: field.clone(),
            _subscription: subscription,
        });
        field
    }

    fn show_editing(
        &mut self,
        field: &Entity<TextField>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rebuild();
        if let Some(index) = self.rows.iter().position(|row| row.editing) {
            self.selected = Some(index);
            self.scroll.scroll_to_item(index, ScrollStrategy::Center);
        }
        window.focus(&field.focus_handle(cx));
        cx.notify();
    }

    /// New File / New Folder: the entry is named in the folder the selection points at —
    /// a selected folder, a selected file's folder, or the root.
    fn begin_create(&mut self, is_dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let dir = match self.selected.and_then(|index| self.rows.get(index)) {
            Some(row) if row.is_dir => row.path.clone(),
            Some(row) => row
                .path
                .parent()
                .map_or_else(|| root.clone(), Path::to_owned),
            None => root,
        };
        self.begin_create_in(&dir, is_dir, window, cx);
    }

    /// Begins naming a new file or folder in `dir`: the context menu's New File/Folder.
    pub fn begin_create_in(
        &mut self,
        dir: &Path,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_editing();
        self.expanded.insert(dir.to_owned());
        let field = self.make_field(
            "",
            InlineKind::Create {
                dir: dir.to_owned(),
                is_dir,
            },
            window,
            cx,
        );
        self.show_editing(&field, window, cx);
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // F2 has no binding in the name field, so it bubbles here while editing.
        if self.editing.is_some() {
            return;
        }
        let Some(row) = self
            .selected
            .and_then(|index| self.rows.get(index))
            .cloned()
        else {
            return;
        };
        self.begin_rename_at(&row.path, window, cx);
    }

    /// Begins renaming the entry at `path`: the context menu's Rename.
    pub fn begin_rename_at(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let Some(row) = self.rows.iter().find(|row| row.path == path) else {
            return;
        };
        let name = row.name.to_string();
        let field = self.make_field(
            &name,
            InlineKind::Rename {
                from: path.to_owned(),
            },
            window,
            cx,
        );
        self.show_editing(&field, window, cx);
    }

    /// Whether `path` (a folder) is expanded in the tree.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Expands a collapsed folder or collapses an expanded one: the context menu's toggle.
    pub fn toggle_expanded(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_owned());
        }
        self.rebuild();
        cx.notify();
    }

    fn field_event(
        &mut self,
        _: &Entity<TextField>,
        event: &FieldEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            FieldEvent::Changed(_) => {}
            FieldEvent::Cancelled | FieldEvent::Blurred => self.end_editing(window, cx),
            FieldEvent::Confirmed(text) => {
                let name = text.to_string();
                // A refused name keeps the field open, so the user can fix it.
                if let Err(err) = files::validate_name(&name) {
                    cx.emit(TreeEvent::Notice(err));
                    return;
                }
                // Scoped so the edit (and its subscription) is gone before the focus
                // moves: the field's blur then can't cancel what was just confirmed.
                let event = {
                    let Some(editing) = self.editing.take() else {
                        return;
                    };
                    match editing.kind {
                        InlineKind::Create { dir, is_dir } => {
                            Some(TreeEvent::Create { dir, name, is_dir })
                        }
                        InlineKind::Rename { from } => {
                            let unchanged =
                                from.file_name().is_some_and(|old| old == name.as_str());
                            (!unchanged).then_some(TreeEvent::Rename { from, name })
                        }
                    }
                };
                self.rebuild();
                // The tree takes the keys back first; a created file's editor takes them
                // from the workspace, after this.
                window.focus(&self.focus_handle);
                cx.notify();
                if let Some(event) = event {
                    cx.emit(event);
                }
            }
        }
    }

    fn trash_selected(&mut self, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        if let Some(row) = self.selected.and_then(|index| self.rows.get(index)) {
            cx.emit(TreeEvent::Trash(row.path.clone()));
        }
    }

    fn go_to(&mut self, last: bool, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.rows.is_empty() {
            return;
        }
        let index = if last { self.rows.len() - 1 } else { 0 };
        self.selected = Some(index);
        self.scroll.scroll_to_item(
            index,
            if last {
                ScrollStrategy::Bottom
            } else {
                ScrollStrategy::Top
            },
        );
        cx.notify();
    }

    /// Marks `path` as the active file, expanding the folders above it.
    pub fn reveal(&mut self, path: Option<&Path>, cx: &mut Context<Self>) {
        self.active = path.map(Path::to_owned);
        if let (Some(path), Some(root)) = (path, self.root.clone())
            && path.starts_with(&root)
        {
            let mut grew = false;
            for dir in path.ancestors().skip(1) {
                if !dir.starts_with(&root) {
                    break;
                }
                grew |= self.expanded.insert(dir.to_owned());
            }
            if grew {
                self.rebuild();
            }
            if let Some(index) = self.rows.iter().position(|row| row.path == path) {
                self.selected = Some(index);
                self.scroll.scroll_to_item(index, ScrollStrategy::Center);
            }
        }
        cx.notify();
    }

    fn activate(&mut self, index: usize, cx: &mut Context<Self>) {
        // While a name is being typed the field owns the keys and the clicks.
        if self.editing.is_some() {
            return;
        }
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        self.selected = Some(index);
        if row.is_dir {
            if !self.expanded.remove(&row.path) {
                self.expanded.insert(row.path);
            }
            self.rebuild();
        } else {
            cx.emit(TreeEvent::Open(row.path));
        }
        cx.notify();
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        let index = match self.selected {
            Some(index) => index.saturating_add_signed(by).min(last),
            None => 0,
        };
        self.selected = Some(index);
        let strategy = if by > 0 {
            ScrollStrategy::Bottom
        } else {
            ScrollStrategy::Top
        };
        self.scroll.scroll_to_item(index, strategy);
        cx.notify();
    }

    fn set_expanded(&mut self, expand: bool, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let Some(row) = self
            .selected
            .and_then(|index| self.rows.get(index))
            .cloned()
        else {
            return;
        };
        if row.is_dir && expand != row.expanded {
            if expand {
                self.expanded.insert(row.path);
            } else {
                self.expanded.remove(&row.path);
            }
            self.rebuild();
        } else if !expand {
            // Left on a file or a closed folder goes to the folder holding it.
            let parent = row.path.parent().map(Path::to_owned);
            if let Some(index) =
                parent.and_then(|parent| self.rows.iter().position(|row| row.path == parent))
            {
                self.selected = Some(index);
                self.scroll.scroll_to_item(index, ScrollStrategy::Top);
            }
        }
        cx.notify();
    }

    fn render_toolbar(&self, colors: Colors, cx: &mut Context<Self>) -> Option<gpui::Div> {
        self.root.is_some().then(|| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .pb_1()
                .text_size(px(12.))
                .child(
                    toolbar_button("tree-new-file", "New File", colors).on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| this.begin_create(false, window, cx),
                    )),
                )
                .child(
                    toolbar_button("tree-new-folder", "New Folder", colors).on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| this.begin_create(true, window, cx),
                    )),
                )
                .child(
                    toolbar_button("tree-collapse", "Collapse", colors).on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| this.collapse_all(window, cx),
                    )),
                )
                .child(
                    toolbar_button("tree-refresh", "Refresh", colors).on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| {
                            this.refresh(cx);
                            window.focus(&this.focus_handle);
                        },
                    )),
                )
        })
    }
}

#[derive(Clone, Copy)]
struct Colors {
    text: Hsla,
    muted: Hsla,
    hover: Hsla,
    selected: Hsla,
    accent: Hsla,
    field: Hsla,
    divider: Hsla,
}

fn toolbar_button(
    id: &'static str,
    label: &'static str,
    colors: Colors,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(22.))
        .px_2()
        .flex()
        .items_center()
        .rounded_md()
        .cursor_pointer()
        .text_color(colors.muted)
        .hover(move |style| style.bg(colors.hover))
        .child(label)
}

/// One row: indent, expand mark, name (or the field naming it). A left-click opens it,
/// a right-click selects it and asks the workspace for its menu.
fn tree_row(
    index: usize,
    row: &Row,
    field: Option<Entity<TextField>>,
    tree: Entity<FileTree>,
    selected: bool,
    is_active: bool,
    colors: Colors,
) -> Stateful<gpui::Div> {
    let row_path = row.path.clone();
    let row_is_dir = row.is_dir;
    let click_tree = tree.clone();
    #[expect(clippy::cast_precision_loss, reason = "folder depth")]
    let indent = px(8. + INDENT * row.depth as f32);
    div()
        .id(index)
        .debug_selector(move || format!("tree-row-{index}"))
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(indent)
        .pr_2()
        .flex()
        .items_center()
        .gap_1()
        .rounded_md()
        .cursor_pointer()
        .text_color(colors.text)
        .when(is_active, |row| row.bg(colors.selected))
        .when(selected, |row| row.border_1().border_color(colors.accent))
        .hover(|style| style.bg(colors.hover))
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .text_size(px(10.))
                .text_color(colors.muted)
                .child(match (row.editing, row.is_dir, row.expanded) {
                    (false, true, true) => "▾",
                    (false, true, false) => "▸",
                    _ => "",
                }),
        )
        .child(row_label(row, field, colors))
        .on_mouse_down(
            MouseButton::Right,
            move |event: &MouseDownEvent, window, cx| {
                let position = event.position;
                // Keeps the menu's focus: without it, the tree takes focus
                // back after this handler runs.
                window.prevent_default();
                tree.update(cx, |tree, cx| {
                    tree.selected = Some(index);
                    cx.notify();
                    cx.emit(TreeEvent::ContextMenu {
                        path: row_path.clone(),
                        is_dir: row_is_dir,
                        position,
                    });
                });
            },
        )
        .on_click(move |_: &ClickEvent, _, cx| {
            click_tree.update(cx, |tree, cx| tree.activate(index, cx));
        })
}

/// A row's name, or the field it is being (re)named in.
fn row_label(row: &Row, field: Option<Entity<TextField>>, colors: Colors) -> AnyElement {
    match (row.editing, field) {
        (true, Some(field)) => div()
            .flex_1()
            .min_w_0()
            .px_1()
            .rounded_sm()
            .border_1()
            .border_color(colors.divider)
            .bg(colors.field)
            .child(field)
            .into_any_element(),
        _ => div().truncate().child(row.name.clone()).into_any_element(),
    }
}

impl Render for FileTree {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let colors = Colors {
            text: theme.chrome_text,
            muted: theme.muted,
            hover: theme.hover,
            selected: theme.selected,
            accent: theme.accent,
            field: theme.editor_bg,
            divider: theme.divider,
        };
        let focused = self.focus_handle.is_focused(window);
        let rows = self.rows.clone();
        let selected = self.selected;
        let active = self.active.clone();
        let editing_field = self.editing.as_ref().map(|edit| edit.field.clone());
        let tree = cx.entity();
        let list = uniform_list("file-tree", rows.len(), move |range, _, _| {
            range
                .map(|index| {
                    let row = &rows[index];
                    tree_row(
                        index,
                        row,
                        editing_field.clone(),
                        tree.clone(),
                        focused && selected == Some(index),
                        !row.editing && active.as_ref() == Some(&row.path),
                        colors,
                    )
                })
                .collect()
        })
        .track_scroll(self.scroll.clone())
        .flex_1()
        .px_1();
        let toolbar = self.render_toolbar(colors, cx);
        div()
            .id("file-tree-view")
            .key_context("FileTree")
            .track_focus(&self.focus_handle)
            // Laid out on its own as a cached view: it fills what the sidebar gives it.
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .on_action(cx.listener(|this, _: &Next, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &Previous, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &Expand, _, cx| this.set_expanded(true, cx)))
            .on_action(cx.listener(|this, _: &Collapse, _, cx| this.set_expanded(false, cx)))
            .on_action(cx.listener(|this, _: &Activate, _, cx| {
                if let Some(index) = this.selected {
                    this.activate(index, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(TreeEvent::Dismiss)))
            .on_action(
                cx.listener(|this, _: &RenameSelected, window, cx| this.begin_rename(window, cx)),
            )
            .on_action(cx.listener(|this, _: &TrashSelected, _, cx| this.trash_selected(cx)))
            .on_action(cx.listener(|this, _: &First, _, cx| this.go_to(false, cx)))
            .on_action(cx.listener(|this, _: &Last, _, cx| this.go_to(true, cx)))
            .children(toolbar)
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;
    use crate::theme::Theme;

    #[gpui::test]
    fn expanding_a_folder_should_list_it_and_reveal_should_open_the_way(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("kiled-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/deep")).unwrap();
        std::fs::write(dir.join("src/deep/a.rs"), "").unwrap();
        std::fs::write(dir.join("README.md"), "").unwrap();
        let tree = cx.new(FileTree::new);
        let names = |tree: &FileTree| -> Vec<String> {
            tree.rows
                .iter()
                .map(|row| format!("{}{}", "  ".repeat(row.depth), row.name))
                .collect()
        };
        let collapsed = tree.update(cx, |tree, cx| {
            tree.set_root(Some(dir.clone()), cx);
            names(tree)
        });
        let revealed = tree.update(cx, |tree, cx| {
            tree.reveal(Some(&dir.join("src/deep/a.rs")), cx);
            names(tree)
        });
        assert_eq!(
            (collapsed, revealed),
            (
                vec!["src".to_owned(), "README.md".to_owned()],
                vec![
                    "src".to_owned(),
                    "  deep".to_owned(),
                    "    a.rs".to_owned(),
                    "README.md".to_owned()
                ]
            )
        );
    }

    #[gpui::test]
    fn a_right_click_on_a_row_should_offer_its_menu(cx: &mut TestAppContext) {
        let dir = std::env::temp_dir().join(format!("kiled-tree-menu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.bind_keys(bindings());
        });
        let (tree, cx) = cx.add_window_view(|_, cx| FileTree::new(cx));
        tree.update_in(cx, |tree, window, cx| {
            tree.set_root(Some(dir), cx);
            window.focus(&tree.focus_handle);
        });
        cx.run_until_parked();
        let mut events = cx.events::<TreeEvent, _>(&tree);
        let row = cx.debug_bounds("tree-row-0").expect("first row");
        let point = gpui::point(row.center().x, row.center().y);
        cx.simulate_mouse_down(point, MouseButton::Right, gpui::Modifiers::none());
        cx.simulate_mouse_up(point, MouseButton::Right, gpui::Modifiers::none());
        cx.run_until_parked();
        let event = events.try_recv().expect("a context menu event");
        match event {
            TreeEvent::ContextMenu { is_dir, .. } => assert!(is_dir),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[gpui::test]
    fn naming_a_new_file_inline_should_edit_the_first_row_and_end_on_enter(
        cx: &mut TestAppContext,
    ) {
        let dir = std::env::temp_dir().join(format!("kiled-tree-inline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("README.md"), "").unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.bind_keys(bindings());
            cx.bind_keys(crate::text_field::bindings());
        });
        let (tree, cx) = cx.add_window_view(|_, cx| FileTree::new(cx));
        tree.update_in(cx, |tree, window, cx| {
            tree.set_root(Some(dir), cx);
            window.focus(&tree.focus_handle);
            tree.begin_create(false, window, cx);
        });
        cx.run_until_parked();
        let first = tree.read_with(cx, |tree, _| {
            tree.rows
                .first()
                .map(|row| (row.editing, row.depth, row.is_dir))
        });
        // Through the real key dispatch: Space is the tree's Activate key, and a name
        // being typed must still get it.
        cx.simulate_input("new");
        cx.simulate_keystrokes("space");
        cx.simulate_input("file.txt");
        let typed = tree.read_with(cx, |tree, cx| {
            tree.editing
                .as_ref()
                .map(|edit| edit.field.read(cx).text().to_owned())
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let after = tree.read_with(cx, |tree, _| {
            (
                tree.editing.is_some(),
                tree.rows.iter().any(|row| row.editing),
            )
        });
        assert_eq!(
            (first, typed.as_deref(), after),
            (Some((true, 0, false)), Some("new file.txt"), (false, false))
        );
    }
}
