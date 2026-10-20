//! The sidebar's explorer: the open folder as a tree, each folder read when it is first
//! expanded and again when the window comes back to the front (DESIGN.md §7).

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use gpui::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, Hsla, KeyBinding,
    ScrollStrategy, SharedString, UniformListScrollHandle, Window, actions, div, prelude::*, px,
    uniform_list,
};
use kiled_core::files::{self, Entry};

use crate::theme::Theme;

actions!(file_tree, [Next, Previous, Expand, Collapse, Activate]);

/// Keys for moving through the tree; fixed, and live while it has focus.
pub fn bindings() -> Vec<KeyBinding> {
    let cx = Some("FileTree");
    vec![
        KeyBinding::new("down", Next, cx),
        KeyBinding::new("up", Previous, cx),
        KeyBinding::new("right", Expand, cx),
        KeyBinding::new("left", Collapse, cx),
        KeyBinding::new("enter", Activate, cx),
        KeyBinding::new("space", Activate, cx),
    ]
}

const ROW_HEIGHT: f32 = 24.;
const INDENT: f32 = 12.;

/// What the tree tells the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeEvent {
    /// A file was clicked or entered.
    Open(PathBuf),
}

#[derive(Clone, Debug)]
struct Row {
    path: PathBuf,
    name: SharedString,
    depth: usize,
    is_dir: bool,
    expanded: bool,
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
        }
    }

    /// Shows `root`, its first level expanded.
    pub fn set_root(&mut self, root: Option<PathBuf>, cx: &mut Context<Self>) {
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
            .map(|row| row.path.clone());
        let mut rows = Vec::new();
        if let Some(root) = self.root.clone() {
            self.push_children(&root, 0, &mut rows);
        }
        self.rows = rows;
        self.selected = selected.and_then(|path| self.rows.iter().position(|row| row.path == path));
    }

    fn push_children(&mut self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        let entries = self.listing(dir).to_vec();
        for entry in entries {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            rows.push(Row {
                path: entry.path.clone(),
                name: entry.name.clone().into(),
                depth,
                is_dir: entry.is_dir,
                expanded,
            });
            if expanded {
                self.push_children(&entry.path, depth + 1, rows);
            }
        }
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
        if self.rows.is_empty() {
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
}

#[derive(Clone, Copy)]
struct Colors {
    text: Hsla,
    muted: Hsla,
    hover: Hsla,
    selected: Hsla,
    accent: Hsla,
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
        };
        let focused = self.focus_handle.is_focused(window);
        let rows = self.rows.clone();
        let selected = self.selected;
        let active = self.active.clone();
        let tree = cx.entity();
        let list = uniform_list("file-tree", rows.len(), move |range, _, _| {
            range
                .map(|index| {
                    let row = &rows[index];
                    let is_active = active.as_ref() == Some(&row.path);
                    let tree = tree.clone();
                    #[expect(clippy::cast_precision_loss, reason = "folder depth")]
                    let indent = px(8. + INDENT * row.depth as f32);
                    div()
                        .id(index)
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
                        .when(focused && selected == Some(index), |row| {
                            row.border_1().border_color(colors.accent)
                        })
                        .hover(|style| style.bg(colors.hover))
                        .child(
                            div()
                                .w(px(12.))
                                .flex_none()
                                .text_size(px(10.))
                                .text_color(colors.muted)
                                .child(match (row.is_dir, row.expanded) {
                                    (true, true) => "▾",
                                    (true, false) => "▸",
                                    (false, _) => "",
                                }),
                        )
                        .child(div().truncate().child(row.name.clone()))
                        .on_click(move |_: &ClickEvent, _, cx| {
                            tree.update(cx, |tree, cx| tree.activate(index, cx));
                        })
                })
                .collect()
        })
        .track_scroll(self.scroll.clone())
        .flex_1()
        .px_1();
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
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

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
}
