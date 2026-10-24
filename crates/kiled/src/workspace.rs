//! The window: the explorer sidebar, the open editors as tabs, the status bar, Quick Open,
//! and every window-level action — opening, saving and closing files, switching editors,
//! going back and forward (DESIGN.md §7).
//!
//! Nothing unsaved is ever dropped without asking: closing an editor, opening another
//! folder and quitting all ask first, and a file changed on disk since it was read is only
//! overwritten when the user says so.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, AnyView, AnyWindowHandle, App, AsyncWindowContext, ClickEvent, Context, Entity,
    EntityId, FocusHandle, Focusable, Hsla, MouseButton, PathPromptOptions, PromptLevel,
    SharedString, Stateful, StyleRefinement, Subscription, Task, WeakEntity, Window, actions, div,
    prelude::*, px,
};
use kiled_core::files;

use crate::{
    config,
    editor::{BLINK_INTERVAL, Editor, EditorEvent},
    file_tree::{FileTree, TreeEvent},
    keymap::{self, Command},
    lsp::{Lsp, LspEvent},
    picker::{Files, Picker, PickerEvent},
    session::{SavedFile, SavedWindow, Session, Store},
    settings::{Settings, abbreviate_home},
    theme::Theme,
    update,
};

actions!(
    workspace,
    [
        NewFile,
        Open,
        Save,
        SaveAs,
        SaveAll,
        RevertFile,
        CloseEditor,
        CloseAllEditors,
        ReopenClosedEditor,
        NextEditor,
        PreviousEditor,
        OpenEditor1,
        OpenEditor2,
        OpenEditor3,
        OpenEditor4,
        OpenEditor5,
        OpenEditor6,
        OpenEditor7,
        OpenEditor8,
        OpenEditor9,
        QuickOpen,
        CommandPalette,
        GoToLine,
        GoBack,
        GoForward,
        ToggleSidebar,
        FocusExplorer,
        ZoomIn,
        ZoomOut,
        ResetZoom,
    ]
);

const SIDEBAR_WIDTH: f32 = 240.;
/// The top bar, under the traffic lights: the sidebar's title strip and the tabs.
pub const TOP_BAR: f32 = 38.;
const STATUS_BAR: f32 = 24.;
/// Room for the traffic lights when the sidebar is hidden.
const TRAFFIC_LIGHTS: f32 = 78.;
/// While anything changes, the session is saved at most this often.
const SAVE_INTERVAL: Duration = Duration::from_secs(2);
/// Quick Open lists at most this many files of a folder.
const PROJECT_FILE_LIMIT: usize = 200_000;
/// The folder is listed again when the window comes back, at most this often.
const RESCAN_INTERVAL: Duration = Duration::from_secs(10);
/// Files opened lately that Quick Open lists first.
const RECENT_LIMIT: usize = 30;

/// What a launch opens.
#[derive(Clone, Debug, Default)]
pub struct Launch {
    pub folder: Option<PathBuf>,
    pub files: Vec<SavedFile>,
    pub active: usize,
    pub sidebar: bool,
}

/// A place Back and Forward return to.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Location {
    path: PathBuf,
    offset: usize,
}

/// Where listing the folder's files for Quick Open stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Listing {
    /// Not since the folder was set.
    Never,
    /// Under way, since then.
    Running(Instant),
    /// Finished; it started then.
    Done(Instant),
}

/// What the window's frame shows, set again only when it changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TitleBar {
    title: SharedString,
    /// macOS's edited mark: a tab has unsaved changes.
    edited: bool,
}

/// Theme colors copied out so `cx` stays free for listeners while rendering.
#[derive(Clone, Copy)]
struct Chrome {
    editor_bg: Hsla,
    sidebar_bg: Hsla,
    bar_bg: Hsla,
    text: Hsla,
    muted: Hsla,
    hover: Hsla,
    accent: Hsla,
    failure: Hsla,
    warning: Hsla,
    divider: Hsla,
    banner: Hsla,
}

impl Chrome {
    fn of(theme: &Theme) -> Self {
        Self {
            editor_bg: theme.editor_bg,
            sidebar_bg: theme.sidebar_bg,
            bar_bg: theme.chrome_bg,
            text: theme.chrome_text,
            muted: theme.muted,
            hover: theme.hover,
            accent: theme.accent,
            failure: theme.failure,
            warning: theme.warning,
            divider: theme.divider,
            banner: theme.banner_bg,
        }
    }
}

pub struct Workspace {
    focus_handle: FocusHandle,
    folder: Option<PathBuf>,
    tree: Entity<FileTree>,
    lsp: Entity<Lsp>,
    editors: Vec<Entity<Editor>>,
    active: Option<usize>,
    /// Files of closed editors, the latest last, for Reopen Closed Editor.
    closed: Vec<PathBuf>,
    picker: Option<Entity<Picker>>,
    /// The folder's files for Quick Open, relative to it.
    project_files: Arc<Vec<PathBuf>>,
    files_task: Option<Task<()>>,
    listing: Listing,
    /// Files opened lately, the latest first, for Quick Open.
    recent: Vec<PathBuf>,
    sidebar: bool,
    notices: Vec<SharedString>,
    config: config::Stamp,
    back: Vec<Location>,
    forward: Vec<Location>,
    next_untitled: u32,
    store: Option<Store>,
    save_task: Option<Task<()>>,
    frame: Option<SavedWindow>,
    titlebar: TitleBar,
    /// Font size from the config file, for Reset Zoom.
    base_font_size: f32,
    blink: Option<Task<()>>,
    /// Unsaved changes were dealt with; the window may close.
    close_confirmed: bool,
    /// The first frame's time after launch was logged.
    drawn: bool,
    editor_subscriptions: HashMap<EntityId, Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Workspace {
    pub fn new(
        launch: Launch,
        store: Option<Store>,
        notices: Vec<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let lsp = cx.new(Lsp::new);
        let tree = cx.new(FileTree::new);
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |this, cx| this.should_close_window(window, cx))
                .unwrap_or(true)
        });
        let subscriptions = vec![
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.on_activated(window, cx);
                }
                this.sync_blink(window, cx);
            }),
            // A logout or restart ends the app without the Quit flow.
            cx.on_app_quit(|this, cx| {
                this.save_now(cx);
                this.lsp.read(cx).terminate_all();
                async {}
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                this.frame = Some(SavedWindow::of(window.window_bounds()));
                this.schedule_save(cx);
            }),
            cx.observe(&lsp, |_, _, cx| cx.notify()),
            cx.subscribe_in(&lsp, window, |this, _, event: &LspEvent, _, cx| {
                let LspEvent::Message(message) = event;
                this.notices.push(message.clone());
                cx.notify();
            }),
            cx.subscribe_in(
                &tree,
                window,
                |this, _, event: &TreeEvent, window, cx| match event {
                    TreeEvent::Open(path) => {
                        this.open_path(path, true, window, cx);
                    }
                    TreeEvent::Dismiss => this.focus_editor(window, cx),
                },
            ),
        ];
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            folder: None,
            tree,
            lsp,
            editors: Vec::new(),
            active: None,
            closed: Vec::new(),
            picker: None,
            project_files: Arc::new(Vec::new()),
            files_task: None,
            listing: Listing::Never,
            recent: Vec::new(),
            sidebar: launch.sidebar,
            notices,
            config: config::stamp(),
            back: Vec::new(),
            forward: Vec::new(),
            next_untitled: 1,
            store: None,
            save_task: None,
            frame: Some(SavedWindow::of(window.window_bounds())),
            titlebar: TitleBar::default(),
            base_font_size: f32::from(cx.global::<Settings>().font_size),
            blink: None,
            close_confirmed: false,
            drawn: false,
            editor_subscriptions: HashMap::new(),
            _subscriptions: subscriptions,
        };
        if let Some(folder) = launch.folder {
            this.set_folder(Some(folder), cx);
        }
        for file in &launch.files {
            if let Some(editor) = this.open_path(&file.path, false, window, cx) {
                editor.update(cx, |editor, cx| {
                    editor.go_to_line(file.line.max(1), Some(file.column.max(1)), cx);
                });
            }
        }
        if this.editors.is_empty() {
            window.focus(&this.focus_handle);
        } else {
            this.activate(launch.active.min(this.editors.len() - 1), window, cx);
        }
        // Set only now, so opening the last session doesn't save it over itself half-done.
        this.store = store;
        this.sync_blink(window, cx);
        this
    }

    // ── folders and files ─────────────────────────────────────────────────────────────

    fn set_folder(&mut self, folder: Option<PathBuf>, cx: &mut Context<Self>) {
        self.folder.clone_from(&folder);
        self.tree.update(cx, |tree, cx| tree.set_root(folder, cx));
        self.project_files = Arc::new(Vec::new());
        self.listing = Listing::Never;
        self.scan_project(cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Lists the folder's files for Quick Open, off the main thread.
    fn scan_project(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.folder.clone() else {
            return;
        };
        let started = match self.listing {
            Listing::Running(at) | Listing::Done(at) => Some(at),
            Listing::Never => None,
        };
        if started.is_some_and(|at| at.elapsed() < RESCAN_INTERVAL) {
            return;
        }
        let now = Instant::now();
        self.listing = Listing::Running(now);
        let scan =
            cx.background_spawn(async move { files::list_project(&root, PROJECT_FILE_LIMIT) });
        self.files_task = Some(cx.spawn(async move |this, cx| {
            let files = scan.await;
            let _ = this.update(cx, |this, cx| {
                this.listing = Listing::Done(now);
                if *this.project_files != files {
                    this.project_files = Arc::new(files);
                }
                this.feed_picker(cx);
            });
        }));
    }

    /// Opens `path` in an editor, or activates the one it is already in.
    pub fn open_path(
        &mut self,
        path: &Path,
        activate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Editor>> {
        if let Some(index) = self.editor_for(path, cx) {
            if activate {
                self.activate(index, window, cx);
            }
            return self.editors.get(index).cloned();
        }
        let (text, stamp) = match files::load(path) {
            Ok(loaded) => loaded,
            Err(err) => {
                let name = path
                    .file_name()
                    .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy());
                self.notices
                    .push(format!("Can't open {name}: {err}.").into());
                cx.notify();
                return None;
            }
        };
        let lsp = self.lsp.clone();
        let path = path.to_owned();
        let editor = cx.new(|cx| Editor::new(Some(path), &text, Some(stamp), 0, lsp, window, cx));
        let folder = self.folder.clone();
        editor.update(cx, |editor, cx| editor.attach_server(folder.as_deref(), cx));
        let index = self.insert_editor(editor.clone(), window, cx);
        if activate {
            self.activate(index, window, cx);
        }
        Some(editor)
    }

    fn editor_for(&self, path: &Path, cx: &App) -> Option<usize> {
        self.editors
            .iter()
            .position(|editor| editor.read(cx).path() == Some(path))
    }

    /// Puts an editor after the active one and listens to it.
    fn insert_editor(
        &mut self,
        editor: Entity<Editor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let subscription = cx.subscribe_in(&editor, window, Self::on_editor_event);
        self.editor_subscriptions
            .insert(editor.entity_id(), subscription);
        // Opened but not yet in front (the last session's files): the least recent.
        if let Some(path) = editor.read(cx).path()
            && !self.recent.iter().any(|recent| recent == path)
        {
            self.recent.push(path.to_owned());
        }
        let index = self.active.map_or(self.editors.len(), |active| active + 1);
        self.editors.insert(index, editor);
        if let Some(active) = self.active
            && active >= index
        {
            self.active = Some(active + 1);
        }
        cx.notify();
        index
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.get(index).cloned() else {
            return;
        };
        self.active = Some(index);
        let path = editor.read(cx).path().map(Path::to_owned);
        if let Some(path) = &path {
            self.recent.retain(|recent| recent != path);
            self.recent.insert(0, path.clone());
            self.recent.truncate(RECENT_LIMIT);
        }
        self.tree
            .update(cx, |tree, cx| tree.reveal(path.as_deref(), cx));
        window.focus(&editor.focus_handle(cx));
        editor.update(cx, Editor::show_cursor);
        self.schedule_save(cx);
        cx.notify();
    }

    fn active_editor(&self) -> Option<&Entity<Editor>> {
        self.active.and_then(|index| self.editors.get(index))
    }

    fn on_editor_event(
        &mut self,
        editor: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::TitleChanged | EditorEvent::Changed => {
                self.schedule_save(cx);
                cx.notify();
            }
            EditorEvent::OpenLocation { path, start, end } => {
                if let Some(target) = self.open_path(path, true, window, cx) {
                    let (start, end) = (*start, *end);
                    target.update(cx, |target, cx| target.go_to(start, end, cx));
                }
            }
            EditorEvent::Jumped(from) => {
                if let Some(path) = editor.read(cx).path() {
                    self.back.push(Location {
                        path: path.to_owned(),
                        offset: *from,
                    });
                    self.forward.clear();
                }
            }
        }
    }

    // ── closing ───────────────────────────────────────────────────────────────────────

    /// Runs `then` once the unsaved changes in `editors` are saved or given up, as the user
    /// chooses; nothing happens if they cancel.
    fn confirm_unsaved(
        &mut self,
        editors: Vec<Entity<Editor>>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let dirty: Vec<Entity<Editor>> = editors
            .into_iter()
            .filter(|editor| editor.read(cx).is_dirty())
            .collect();
        if dirty.is_empty() {
            then(self, window, cx);
            return;
        }
        let names: Vec<SharedString> = dirty.iter().map(|editor| editor.read(cx).title()).collect();
        let (message, detail, buttons) = if let [name] = names.as_slice() {
            (
                format!("Do you want to save the changes you made to {name}?"),
                "Your changes will be lost if you don't save them.".to_owned(),
                ["Save", "Don't Save", "Cancel"],
            )
        } else {
            (
                format!(
                    "Do you want to save the changes you made to {} files?",
                    names.len()
                ),
                format!(
                    "{}\n\nYour changes will be lost if you don't save them.",
                    names.join("\n")
                ),
                ["Save All", "Don't Save", "Cancel"],
            )
        };
        let answer = window.prompt(PromptLevel::Warning, &message, Some(&detail), &buttons, cx);
        cx.spawn_in(window, async move |this, cx| {
            match answer.await {
                Ok(0) => {
                    for editor in dirty {
                        if !save_editor(&this, &editor, cx).await {
                            return;
                        }
                    }
                }
                Ok(1) => {}
                _ => return,
            }
            let _ = this.update_in(cx, |this, window, cx| then(this, window, cx));
        })
        .detach();
    }

    fn close_editor_at(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editors.get(index).cloned() else {
            return;
        };
        let id = editor.entity_id();
        self.confirm_unsaved(vec![editor], window, cx, move |this, window, cx| {
            if let Some(index) = this
                .editors
                .iter()
                .position(|editor| editor.entity_id() == id)
            {
                this.remove_editor(index, window, cx);
            }
        });
    }

    fn remove_editor(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editors.remove(index);
        self.editor_subscriptions.remove(&editor.entity_id());
        editor.update(cx, |editor, cx| {
            if let Some(path) = editor.path() {
                self.closed.push(path.to_owned());
            }
            editor.detach_server(cx);
        });
        if self.editors.is_empty() {
            self.active = None;
            self.tree.update(cx, |tree, cx| tree.reveal(None, cx));
            window.focus(&self.focus_handle);
        } else {
            let next = match self.active {
                Some(active) if active > index => active - 1,
                Some(active) if active == index => index.min(self.editors.len() - 1),
                Some(active) => active,
                None => 0,
            };
            self.active = None;
            self.activate(next, window, cx);
        }
        self.schedule_save(cx);
        cx.notify();
    }

    /// Closes every editor without asking: the caller has dealt with unsaved changes.
    fn remove_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while !self.editors.is_empty() {
            self.remove_editor(self.editors.len() - 1, window, cx);
        }
    }

    fn should_close_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.close_confirmed {
            return true;
        }
        let editors = self.editors.clone();
        if editors.iter().all(|editor| !editor.read(cx).is_dirty()) {
            self.depart(cx);
            return true;
        }
        self.confirm_unsaved(editors, window, cx, |this, window, _| {
            this.close_confirmed = true;
            window.remove_window();
        });
        false
    }

    /// Quits once unsaved changes are saved or given up; with `reopen`, the new Kiled opens
    /// once this one is gone (Restart to Update). A cancelled prompt changes nothing.
    fn quit(&mut self, reopen: bool, window: &mut Window, cx: &mut Context<Self>) {
        let editors = self.editors.clone();
        self.confirm_unsaved(editors, window, cx, move |this, _, cx| {
            this.depart(cx);
            if reopen {
                update::reopen_on_quit(cx);
            }
            cx.quit();
        });
    }

    /// Saves what the next launch reopens and ends the language servers.
    fn depart(&mut self, cx: &mut Context<Self>) {
        self.close_confirmed = true;
        self.save_now(cx);
        self.lsp.read(cx).terminate_all();
    }

    // ── saving ────────────────────────────────────────────────────────────────────────

    fn save_active(&mut self, as_new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        if as_new {
            let folder = self.folder.clone();
            let target = editor.downgrade();
            let directory = editor
                .read(cx)
                .path()
                .and_then(Path::parent)
                .map(Path::to_owned)
                .or(folder.clone())
                .unwrap_or_else(home);
            let name = editor.read(cx).title();
            let chosen = cx.prompt_for_new_path(&directory, Some(&name));
            cx.spawn_in(window, async move |this, cx| {
                let Ok(Ok(Some(path))) = chosen.await else {
                    return;
                };
                let _ = this.update(cx, |this, cx| {
                    let saved =
                        target.update(cx, |editor, cx| editor.save_as(path, folder.as_deref(), cx));
                    if let Ok(Err(err)) = saved {
                        this.notices.push(err.into());
                    }
                    this.tree.update(cx, FileTree::refresh);
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            save_editor(&this, &editor, cx).await;
        })
        .detach();
    }

    fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dirty: Vec<Entity<Editor>> = self
            .editors
            .iter()
            .filter(|editor| editor.read(cx).is_dirty())
            .cloned()
            .collect();
        cx.spawn_in(window, async move |this, cx| {
            for editor in dirty {
                if !save_editor(&this, &editor, cx).await {
                    return;
                }
            }
        })
        .detach();
    }

    /// Puts the file in front back as it is on disk, as one undoable step.
    fn revert_active(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor().cloned() else {
            return;
        };
        if editor.read(cx).path().is_none() {
            return;
        }
        if let Err(err) = editor.update(cx, Editor::reload) {
            self.notices.push(err.into());
            cx.notify();
        }
    }

    // ── session ───────────────────────────────────────────────────────────────────────

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if self.store.is_none() || self.save_task.is_some() {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_INTERVAL).await;
            let _ = this.update(cx, |this, cx| {
                this.save_task = None;
                this.save_now(cx);
            });
        }));
    }

    fn snapshot(&self, cx: &App) -> Session {
        let mut active = 0;
        let mut files = Vec::new();
        for (index, editor) in self.editors.iter().enumerate() {
            let editor = editor.read(cx);
            let Some(path) = editor.path() else {
                continue;
            };
            if Some(index) == self.active {
                active = files.len();
            }
            let (line, column) = editor.cursor_position();
            files.push(SavedFile {
                path: path.to_owned(),
                line,
                column,
            });
        }
        Session {
            folder: self.folder.clone(),
            files,
            active,
            sidebar: self.sidebar,
            window: self.frame,
        }
    }

    fn save_now(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        if let Some(store) = &self.store
            && let Err(err) = store.save(&self.snapshot(cx))
        {
            log::error!("saving the session: {err}");
        }
    }

    // ── window activation ─────────────────────────────────────────────────────────────

    fn on_activated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_config_if_changed(window, cx);
        self.tree.update(cx, FileTree::refresh);
        self.scan_project(cx);
        for editor in self.editors.clone() {
            let (changed, dirty, conflict, title) = {
                let editor = editor.read(cx);
                (
                    editor.changed_on_disk(),
                    editor.is_dirty(),
                    editor.conflict,
                    editor.title(),
                )
            };
            if !changed {
                continue;
            }
            if dirty {
                if !conflict {
                    editor.update(cx, |editor, _| editor.conflict = true);
                    self.notices.push(
                        format!(
                            "{title} changed on disk. Your unsaved edits are kept; saving asks before replacing the file."
                        )
                        .into(),
                    );
                }
            } else if let Err(err) = editor.update(cx, Editor::reload) {
                self.notices.push(err.into());
            }
        }
        cx.notify();
    }

    /// Also called by the Settings window right after it writes a file.
    pub fn reload_config_if_changed(&mut self, window: &Window, cx: &mut Context<Self>) {
        let stamp = config::stamp();
        if stamp != self.config {
            self.config = stamp;
            self.notices = config::apply(cx);
            self.base_font_size = f32::from(cx.global::<Settings>().font_size);
            self.sync_blink(window, cx);
            for editor in &self.editors {
                editor.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        }
    }

    /// Runs one cursor timer, only while blinking is on and the window is active, for the
    /// active editor only.
    fn sync_blink(&mut self, window: &Window, cx: &mut Context<Self>) {
        let wanted = cx.global::<Settings>().cursor_blink && window.is_window_active();
        if wanted == self.blink.is_some() {
            return;
        }
        if wanted {
            self.blink = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    cx.background_executor().timer(BLINK_INTERVAL).await;
                    let blinked = this.update(cx, |this, cx| {
                        if let Some(editor) = this.active_editor() {
                            editor.update(cx, Editor::blink);
                        }
                    });
                    if blinked.is_err() {
                        return;
                    }
                }
            }));
        } else {
            self.blink = None;
            for editor in &self.editors {
                editor.update(cx, Editor::show_cursor);
            }
        }
    }

    fn set_font_size(&mut self, change: impl FnOnce(f32) -> f32, cx: &mut Context<Self>) {
        let settings = cx.global::<Settings>();
        let size = change(f32::from(settings.font_size)).clamp(6., 72.);
        let updated = Settings {
            font_size: px(size),
            ..settings.clone()
        };
        cx.set_global(updated);
        for editor in &self.editors {
            editor.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    // ── actions ───────────────────────────────────────────────────────────────────────

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let number = self.next_untitled;
        self.next_untitled += 1;
        let lsp = self.lsp.clone();
        let editor = cx.new(|cx| Editor::new(None, "", None, number, lsp, window, cx));
        let index = self.insert_editor(editor, window, cx);
        self.activate(index, window, cx);
    }

    #[expect(clippy::unused_self, reason = "an action handler")]
    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.open_paths(paths, window, cx));
        })
        .detach();
    }

    /// Opens files in editors and a folder as the workspace's folder (asking about unsaved
    /// changes first, since its editors close).
    pub fn open_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|path| path.is_dir());
        let open_files = move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            for file in &files {
                this.open_path(file, true, window, cx);
            }
        };
        let Some(folder) = folders
            .into_iter()
            .next()
            .filter(|folder| Some(folder) != self.folder.as_ref())
        else {
            open_files(self, window, cx);
            return;
        };
        // The files come after the switch: switching closes the editors that are open.
        let editors = self.editors.clone();
        self.confirm_unsaved(editors, window, cx, move |this, window, cx| {
            this.remove_all(window, cx);
            this.lsp.update(cx, Lsp::stop_all);
            this.back.clear();
            this.forward.clear();
            this.recent.clear();
            this.set_folder(Some(folder), cx);
            this.sidebar = true;
            open_files(this, window, cx);
        });
    }

    fn reopen_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(path) = self.closed.pop() {
            if self.editor_for(&path, cx).is_none() && path.exists() {
                self.open_path(&path, true, window, cx);
                return;
            }
        }
    }

    fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.editors.len();
        if count < 2 {
            return;
        }
        let active = self.active.unwrap_or(0);
        let next = if forward {
            (active + 1) % count
        } else {
            (active + count - 1) % count
        };
        self.activate(next, window, cx);
    }

    fn open_picker(&mut self, prefix: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.clone() {
            // Pressed again, the box steps down its list, as VS Code's does; another
            // box's key turns it into that box.
            picker.update(cx, |picker, cx| {
                if picker.is_mode_of(prefix, cx) {
                    picker.select_next(cx);
                } else {
                    picker.reset(prefix, cx);
                }
            });
            return;
        }
        let files = self.quick_open_files(cx);
        let (table, _) = keymap::resolve(config::read_keymap().ok().flatten().as_deref());
        let lines = self
            .active_editor()
            .map_or(0, |editor| editor.read(cx).document().rope().len_lines());
        let picker = cx.new(|cx| Picker::new(prefix, files, &table, lines, window, cx));
        let subscription = cx.subscribe_in(&picker, window, Self::on_picker_event);
        self.editor_subscriptions
            .insert(picker.entity_id(), subscription);
        window.focus(&picker.focus_handle(cx));
        self.picker = Some(picker);
        cx.notify();
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.take() {
            self.editor_subscriptions.remove(&picker.entity_id());
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// The keys go to the editor in front, or to the window when no file is open.
    fn focus_editor(&self, window: &mut Window, cx: &App) {
        match self.active_editor() {
            Some(editor) => window.focus(&editor.focus_handle(cx)),
            None => window.focus(&self.focus_handle),
        }
    }

    /// What Quick Open searches: the folder's files, and the files opened lately, the one
    /// in front left out (Enter on it would do nothing).
    fn quick_open_files(&self, cx: &App) -> Files {
        let front = self
            .active_editor()
            .and_then(|editor| editor.read(cx).path().map(Path::to_owned));
        let recent = self
            .recent
            .iter()
            .filter(|path| Some(*path) != front.as_ref() && path.is_file())
            .map(|path| match &self.folder {
                Some(folder) => path
                    .strip_prefix(folder)
                    .map_or_else(|_| path.clone(), Path::to_owned),
                None => path.clone(),
            })
            .collect();
        Files {
            project: Arc::clone(&self.project_files),
            recent,
            listing: matches!(self.listing, Listing::Running(_)),
            folder: self.folder.is_some(),
        }
    }

    /// Gives an open Quick Open box the files as they are now.
    fn feed_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.clone() {
            let files = self.quick_open_files(cx);
            picker.update(cx, |picker, cx| picker.set_files(files, cx));
        }
    }

    fn on_picker_event(
        &mut self,
        _: &Entity<Picker>,
        event: &PickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let event = event.clone();
        self.close_picker(window, cx);
        match event {
            PickerEvent::Open(path) => {
                // A typed path is absolute, and `join` keeps it as it is.
                let path = match &self.folder {
                    Some(folder) => folder.join(path),
                    None => path,
                };
                let here = self.location(cx);
                if self.open_path(&path, true, window, cx).is_some()
                    && let Some(here) = here
                {
                    self.back.push(here);
                    self.forward.clear();
                }
            }
            PickerEvent::Run(command) => {
                if let Some(action) = command.action() {
                    cx.defer_in(window, move |_, window, cx| {
                        window.dispatch_action(action, cx);
                    });
                }
            }
            PickerEvent::GoToLine(line, column) => {
                if let Some(editor) = self.active_editor() {
                    editor.update(cx, |editor, cx| editor.go_to_line(line, column, cx));
                }
            }
            PickerEvent::Dismissed => {}
        }
    }

    /// Where the active editor's cursor is, for Back.
    fn location(&self, cx: &App) -> Option<Location> {
        let editor = self.active_editor()?.read(cx);
        Some(Location {
            path: editor.path()?.to_owned(),
            offset: editor.document().selections().primary().head,
        })
    }

    fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = if back {
            self.back.pop()
        } else {
            self.forward.pop()
        };
        let Some(target) = target else {
            return;
        };
        if let Some(here) = self.location(cx) {
            if back {
                self.forward.push(here);
            } else {
                self.back.push(here);
            }
        }
        if let Some(editor) = self.open_path(&target.path, true, window, cx) {
            editor.update(cx, |editor, cx| {
                editor.select_and_center(target.offset..target.offset, cx);
            });
        }
    }

    fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = !self.sidebar;
        if !self.sidebar
            && self.tree.focus_handle(cx).is_focused(window)
            && let Some(editor) = self.active_editor()
        {
            window.focus(&editor.focus_handle(cx));
        }
        self.schedule_save(cx);
        cx.notify();
    }

    fn focus_explorer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar = true;
        window.focus(&self.tree.focus_handle(cx));
        cx.notify();
    }
}

/// Saves one editor: a file changed on disk is only replaced if the user says so (they may
/// have it reloaded instead), and an untitled one asks for a name. Returns whether the
/// editor is clean afterwards.
async fn save_editor(
    this: &WeakEntity<Workspace>,
    editor: &Entity<Editor>,
    cx: &mut AsyncWindowContext,
) -> bool {
    let Ok(Some(folder)) = this.read_with(cx, |this, _| Some(this.folder.clone())) else {
        return false;
    };
    let Ok((has_path, changed, title)) = editor.read_with(cx, |editor, _| {
        (
            editor.path().is_some(),
            editor.changed_on_disk(),
            editor.title(),
        )
    }) else {
        return false;
    };
    if !has_path {
        let directory = folder.clone().unwrap_or_else(home);
        let Ok(chosen) = cx.update(|_, cx| cx.prompt_for_new_path(&directory, Some(&title))) else {
            return false;
        };
        let Ok(Ok(Some(path))) = chosen.await else {
            return false;
        };
        let saved = editor.update(cx, |editor, cx| editor.save_as(path, folder.as_deref(), cx));
        return report(this, saved, cx);
    }
    if changed {
        let detail = format!(
            "{title} was changed on disk after it was opened. Overwrite replaces those changes with yours. Reload takes theirs; Undo brings yours back."
        );
        let Ok(answer) = cx.update(|window, cx| {
            window.prompt(
                PromptLevel::Warning,
                "The file has been changed on disk.",
                Some(&detail),
                &["Overwrite", "Reload from Disk", "Cancel"],
                cx,
            )
        }) else {
            return false;
        };
        match answer.await {
            Ok(0) => {}
            Ok(1) => {
                let reloaded = editor.update(cx, Editor::reload);
                return report(this, reloaded, cx);
            }
            _ => return false,
        }
    }
    let saved = editor.update(cx, Editor::save);
    report(this, saved, cx)
}

fn report(
    this: &WeakEntity<Workspace>,
    saved: Result<Result<(), String>, anyhow::Error>,
    cx: &mut AsyncWindowContext,
) -> bool {
    match saved {
        Ok(Ok(())) => {
            let _ = this.update(cx, |this, cx| {
                this.tree.update(cx, FileTree::refresh);
                cx.notify();
            });
            true
        }
        Ok(Err(err)) => {
            let _ = this.update(cx, |this, cx| {
                this.notices.push(err.into());
                cx.notify();
            });
            false
        }
        Err(_) => false,
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Runs `Quit` in the editor window, for when another window (Settings) has focus.
pub fn quit_app(cx: &mut App) {
    quit_in_window(false, cx);
}

/// Restart to Update: quits as `quit_app` does, asking about unsaved files, and the new
/// Kiled opens once this one has installed it.
pub fn quit_to_update(cx: &mut App) {
    quit_in_window(true, cx);
}

/// Runs the Quit flow in the editor window once the current update is over: a menu action
/// is dispatched inside the active window's update, and a window can't be updated again
/// from inside its own.
fn quit_in_window(reopen: bool, cx: &mut App) {
    cx.defer(move |cx| {
        let workspace = cx
            .windows()
            .iter()
            .find_map(AnyWindowHandle::downcast::<Workspace>);
        if let Some(workspace) = workspace {
            let _ = workspace.update(cx, |this, window, cx| {
                window.activate_window();
                this.quit(reopen, window, cx);
            });
        } else {
            if reopen {
                update::reopen_on_quit(cx);
            }
            cx.quit();
        }
    });
}

/// Re-applies the config files in the editor window after the Settings window wrote one.
pub fn reload_config(cx: &mut App) {
    for workspace in cx
        .windows()
        .iter()
        .filter_map(AnyWindowHandle::downcast::<Workspace>)
    {
        let _ = workspace.update(cx, |this, window, cx| {
            this.reload_config_if_changed(window, cx);
        });
    }
}

/// Opens files and folders handed to the app (Finder, `open -a Kiled`, the command line).
pub fn open_in_window(paths: Vec<PathBuf>, cx: &mut App) {
    if let Some(workspace) = cx
        .windows()
        .iter()
        .find_map(AnyWindowHandle::downcast::<Workspace>)
    {
        let _ = workspace.update(cx, |this, window, cx| {
            window.activate_window();
            this.open_paths(paths, window, cx);
        });
    }
}

macro_rules! open_editor_handlers {
    ($($action:ident => $index:expr),* $(,)?) => {
        fn bind_open_editor(root: Stateful<gpui::Div>, cx: &mut Context<Workspace>) -> Stateful<gpui::Div> {
            root $(.on_action(cx.listener(|this, _: &$action, window, cx| {
                let index = ($index as usize).min(this.editors.len().saturating_sub(1));
                this.activate(index, window, cx);
            })))*
        }
    };
}

open_editor_handlers!(
    OpenEditor1 => 0, OpenEditor2 => 1, OpenEditor3 => 2, OpenEditor4 => 3, OpenEditor5 => 4,
    OpenEditor6 => 5, OpenEditor7 => 6, OpenEditor8 => 7, OpenEditor9 => usize::MAX,
);

fn bind_actions(root: Stateful<gpui::Div>, cx: &mut Context<Workspace>) -> Stateful<gpui::Div> {
    let root = root
        .on_action(cx.listener(Workspace::new_file))
        .on_action(cx.listener(Workspace::open))
        .on_action(cx.listener(|this, _: &Save, window, cx| this.save_active(false, window, cx)))
        .on_action(cx.listener(|this, _: &SaveAs, window, cx| this.save_active(true, window, cx)))
        .on_action(cx.listener(|this, _: &SaveAll, window, cx| this.save_all(window, cx)))
        .on_action(cx.listener(|this, _: &RevertFile, _, cx| this.revert_active(cx)))
        .on_action(cx.listener(|this, _: &CloseEditor, window, cx| {
            if let Some(index) = this.active {
                this.close_editor_at(index, window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &CloseAllEditors, window, cx| {
            let editors = this.editors.clone();
            this.confirm_unsaved(editors, window, cx, Workspace::remove_all);
        }))
        .on_action(cx.listener(|this, _: &ReopenClosedEditor, window, cx| {
            this.reopen_closed(window, cx);
        }))
        .on_action(cx.listener(|this, _: &NextEditor, window, cx| this.cycle(true, window, cx)))
        .on_action(
            cx.listener(|this, _: &PreviousEditor, window, cx| this.cycle(false, window, cx)),
        )
        .on_action(cx.listener(|this, _: &QuickOpen, window, cx| this.open_picker("", window, cx)))
        .on_action(
            cx.listener(|this, _: &CommandPalette, window, cx| this.open_picker(">", window, cx)),
        )
        .on_action(cx.listener(|this, _: &GoToLine, window, cx| this.open_picker(":", window, cx)))
        .on_action(cx.listener(|this, _: &GoBack, window, cx| this.navigate(true, window, cx)))
        .on_action(cx.listener(|this, _: &GoForward, window, cx| this.navigate(false, window, cx)))
        .on_action(
            cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &FocusExplorer, window, cx| this.focus_explorer(window, cx)),
        )
        .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.set_font_size(|size| size + 1., cx)))
        .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.set_font_size(|size| size - 1., cx)))
        .on_action(cx.listener(|this, _: &ResetZoom, _, cx| {
            let base = this.base_font_size;
            this.set_font_size(|_| base, cx);
        }))
        .on_action(cx.listener(|this, _: &crate::Quit, window, cx| this.quit(false, window, cx)));
    bind_open_editor(root, cx)
}

impl Workspace {
    fn render_sidebar(&self, chrome: Chrome, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.folder.as_ref().map(|folder| {
            folder
                .file_name()
                .map_or_else(
                    || abbreviate_home(folder),
                    |name| name.to_string_lossy().into_owned(),
                )
                .to_uppercase()
        });
        let body: AnyElement = if self.folder.is_some() {
            AnyView::from(self.tree.clone())
                .cached(
                    StyleRefinement::default()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col(),
                )
                .into_any_element()
        } else {
            div()
                .px_3()
                .pt_2()
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(12.))
                .text_color(chrome.muted)
                .child("No folder open.")
                .child(
                    div()
                        .id("open-folder")
                        .h(px(28.))
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(chrome.accent)
                        .text_color(chrome.editor_bg)
                        .cursor_pointer()
                        .child("Open Folder…")
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.open(&Open, window, cx);
                        })),
                )
                .into_any_element()
        };
        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(chrome.sidebar_bg)
            .border_r_1()
            .border_color(chrome.divider)
            .child(title_strip("sidebar-titlebar").h(px(TOP_BAR)).flex_none())
            .children(header.map(|header| {
                div()
                    .h(px(26.))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(chrome.text)
                    .child(header)
            }))
            .child(body)
    }

    fn render_tabs(&self, chrome: Chrome, cx: &mut Context<Self>) -> impl IntoElement {
        let folders = tab_folders(
            &self
                .editors
                .iter()
                .map(|editor| {
                    let editor = editor.read(cx);
                    (editor.title(), editor.path().map(Path::to_owned))
                })
                .collect::<Vec<_>>(),
        );
        let tabs: Vec<AnyElement> = self
            .editors
            .iter()
            .enumerate()
            .map(|(index, editor)| {
                let (title, dirty, path) = {
                    let editor = editor.read(cx);
                    let path: Option<SharedString> =
                        editor.path().map(|path| path.display().to_string().into());
                    (editor.title(), editor.is_dirty(), path)
                };
                let folder = folders.get(index).cloned().flatten();
                let active = Some(index) == self.active;
                let close = Self::close_button(index, dirty, chrome, cx);
                div()
                    .id(("tab", index))
                    .h_full()
                    .pl_3()
                    .pr_2()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .border_r_1()
                    .border_color(chrome.divider)
                    .text_size(px(12.5))
                    .cursor_pointer()
                    .when(active, |tab| {
                        tab.bg(chrome.editor_bg)
                            .text_color(chrome.text)
                            .border_t_2()
                            .border_color(chrome.accent)
                    })
                    .when(!active, |tab| {
                        tab.text_color(chrome.muted)
                            .hover(|style| style.bg(chrome.hover))
                    })
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            this.close_editor_at(index, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.activate(index, window, cx);
                    }))
                    .when_some(path, |tab, path| {
                        tab.tooltip(move |_, cx| cx.new(|_| Tooltip(path.clone())).into())
                    })
                    .child(title)
                    .when_some(folder, |tab, folder| {
                        tab.child(
                            div()
                                .text_size(px(11.))
                                .text_color(chrome.muted)
                                .child(folder),
                        )
                    })
                    .child(close)
                    .into_any_element()
            })
            .collect();
        div()
            .id("tab-bar")
            .h(px(TOP_BAR))
            .flex_none()
            .flex()
            .bg(chrome.bar_bg)
            .border_b_1()
            .border_color(chrome.divider)
            .when(!self.sidebar, |bar| bar.pl(px(TRAFFIC_LIGHTS)))
            .child(
                div()
                    .id("tabs")
                    .h_full()
                    .flex()
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .child(title_strip("tab-bar-empty").flex_1().h_full())
    }

    /// A tab's ×, or ● while its file is unsaved; a click closes the tab.
    fn close_button(
        index: usize,
        dirty: bool,
        chrome: Chrome,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        div()
            .id(("close-tab", index))
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .text_size(px(12.))
            .text_color(chrome.muted)
            .hover(|style| style.bg(chrome.hover).text_color(chrome.text))
            .child(if dirty { "●" } else { "×" })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.close_editor_at(index, window, cx);
            }))
    }

    fn render_status(&self, chrome: Chrome, cx: &mut Context<Self>) -> impl IntoElement {
        let mut left: Vec<AnyElement> = Vec::new();
        let mut right: Vec<AnyElement> = Vec::new();
        if let Some(editor) = self.active_editor() {
            let editor = editor.read(cx);
            left.push(problems(editor.problem_counts(), chrome));
            if let Some((status, failed)) = editor
                .server
                .and_then(|server| self.lsp.read(cx).status(server))
            {
                left.push(
                    div()
                        .max_w(px(520.))
                        .truncate()
                        .text_color(if failed { chrome.failure } else { chrome.muted })
                        .child(status)
                        .into_any_element(),
                );
            }
            if let Some(message) = &editor.message {
                left.push(
                    div()
                        .text_color(chrome.text)
                        .child(message.clone())
                        .into_any_element(),
                );
            }
            let (line, column) = editor.cursor_position();
            let selections = editor.document().selections();
            let selected = selections.primary().range().len();
            let position = match (selections.len(), selected) {
                (1, 0) => format!("Ln {line}, Col {column}"),
                (1, selected) => format!("Ln {line}, Col {column} ({selected} selected)"),
                (count, _) => format!("Ln {line}, Col {column} ({count} selections)"),
            };
            let document = editor.document();
            right.push(div().child(position).into_any_element());
            right.push(div().child(document.indent().label()).into_any_element());
            right.push(
                div()
                    .child(document.line_ending().label())
                    .into_any_element(),
            );
            right.push(
                div()
                    .child(
                        editor
                            .language()
                            .map_or("Plain Text", |language| language.name),
                    )
                    .into_any_element(),
            );
        }
        div()
            .h(px(STATUS_BAR))
            .flex_none()
            .px_3()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .bg(chrome.bar_bg)
            .border_t_1()
            .border_color(chrome.divider)
            .text_size(px(11.5))
            .text_color(chrome.muted)
            .child(div().flex().items_center().gap_4().min_w_0().children(left))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_4()
                    .children(right),
            )
    }

    fn render_empty(chrome: Chrome) -> impl IntoElement {
        let (table, _) = keymap::resolve(config::read_keymap().ok().flatten().as_deref());
        let row = |command: Command| {
            let keys = keymap::keys_for(&table, command)
                .first()
                .map(|keys| keymap::display(keys))
                .unwrap_or_default();
            div()
                .w(px(300.))
                .flex()
                .justify_between()
                .child(div().text_color(chrome.muted).child(command.title()))
                .child(div().text_color(chrome.text).child(keys))
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .text_size(px(13.))
            .child(
                div()
                    .pb_4()
                    .text_size(px(28.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(chrome.muted.opacity(0.6))
                    .child("Kiled"),
            )
            .child(row(Command::Open))
            .child(row(Command::QuickOpen))
            .child(row(Command::Palette))
            .child(row(Command::NewFile))
            .child(row(Command::ToggleSidebar))
    }
}

/// For tabs that share a name, the folder holding each, so that two `mod.rs` tell apart.
fn tab_folders(tabs: &[(SharedString, Option<PathBuf>)]) -> Vec<Option<SharedString>> {
    tabs.iter()
        .map(|(title, path)| {
            let shared = tabs.iter().filter(|(other, _)| other == title).count() > 1;
            shared
                .then(|| path.as_deref()?.parent()?.file_name())
                .flatten()
                .map(|folder| folder.to_string_lossy().into_owned().into())
        })
        .collect()
}

/// A tab's file, in full, under the pointer.
struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        // Below the pointer rather than under it.
        div().pt_5().child(
            div()
                .debug_selector(|| "tooltip".to_owned())
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(theme.divider)
                .bg(theme.popover_bg)
                .shadow_md()
                .text_size(px(12.))
                .text_color(theme.chrome_text)
                .child(self.0.clone()),
        )
    }
}

/// Errors and warnings in the active file; a click goes to the next one.
fn problems((errors, warnings): (usize, usize), chrome: Chrome) -> AnyElement {
    let count = |count: usize, symbol: &str, color: Hsla| {
        div()
            .text_color(if count > 0 { color } else { chrome.muted })
            .child(format!("{symbol} {count}"))
    };
    div()
        .id("problems")
        .flex()
        .gap_2()
        .cursor_pointer()
        .child(count(errors, "⊗", chrome.failure))
        .child(count(warnings, "⚠", chrome.warning))
        .on_click(|_: &ClickEvent, window, cx| {
            window.dispatch_action(Box::new(crate::editor::NextProblem), cx);
        })
        .into_any_element()
}

/// The part of the top bar that is the window's title bar: a double-click there does what
/// System Settings ▸ Desktop & Dock says a title-bar double-click does.
fn title_strip(id: &'static str) -> Stateful<gpui::Div> {
    div().id(id).on_click(|event: &ClickEvent, window, _| {
        if event.click_count() == 2 {
            window.titlebar_double_click();
        }
    })
}

impl Workspace {
    /// The window's title ("file — folder") and edited mark, set when they change.
    fn sync_titlebar(&mut self, window: &mut Window, cx: &App) {
        let folder_name = |folder: &PathBuf| {
            folder
                .file_name()
                .map_or_else(|| folder.to_string_lossy(), |name| name.to_string_lossy())
                .into_owned()
        };
        let title: SharedString = match (self.active_editor(), &self.folder) {
            (Some(editor), Some(folder)) => {
                format!("{} — {}", editor.read(cx).title(), folder_name(folder)).into()
            }
            (Some(editor), None) => editor.read(cx).title(),
            (None, Some(folder)) => folder_name(folder).into(),
            (None, None) => "Kiled".into(),
        };
        let edited = self.editors.iter().any(|editor| editor.read(cx).is_dirty());
        let titlebar = TitleBar { title, edited };
        if titlebar.title != self.titlebar.title {
            window.set_window_title(&titlebar.title);
        }
        if titlebar.edited != self.titlebar.edited {
            window.set_window_edited(titlebar.edited);
        }
        self.titlebar = titlebar;
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.drawn {
            self.drawn = true;
            if let Some(launched) = crate::LAUNCHED.get() {
                log::info!("first frame {:?} after launch", launched.elapsed());
            }
        }
        self.sync_titlebar(window, cx);
        let chrome = Chrome::of(cx.global::<Theme>());
        let content: AnyElement = match self.active_editor() {
            Some(editor) => AnyView::from(editor.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            None => Self::render_empty(chrome).into_any_element(),
        };
        let notices = (!self.notices.is_empty()).then(|| {
            div()
                .id("notices")
                .absolute()
                .top(px(TOP_BAR + 10.))
                .right(px(20.))
                .max_w(px(460.))
                .flex()
                .flex_col()
                .gap_2()
                .cursor_pointer()
                .occlude()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.notices.clear();
                    cx.notify();
                }))
                .children(self.notices.iter().map(|notice| {
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(chrome.banner)
                        .border_1()
                        .border_color(chrome.divider)
                        .text_size(px(12.))
                        .text_color(chrome.text)
                        .child(notice.clone())
                }))
        });
        let picker = self.picker.clone().map(|picker| {
            div()
                .absolute()
                .top(px(TOP_BAR + 6.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(picker)
        });
        let root = div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus_handle);
        bind_actions(root, cx)
            .relative()
            .size_full()
            .flex()
            .text_color(chrome.text)
            .when(self.sidebar, |root| {
                root.child(self.render_sidebar(chrome, cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(chrome.editor_bg)
                    .child(self.render_tabs(chrome, cx))
                    .child(div().flex_1().min_h_0().child(content))
                    .child(self.render_status(chrome, cx)),
            )
            .children(picker)
            .children(notices)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kiled-ws-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "hello\n").unwrap();
        dir
    }

    fn workspace(
        launch: Launch,
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.set_global(Settings::default());
            crate::register_app_actions(cx);
            cx.bind_keys(keymap::bindings(&keymap::resolve(None).0));
            cx.bind_keys(crate::editor::bindings());
            cx.bind_keys(crate::text_field::bindings());
            cx.bind_keys(crate::picker::bindings());
            cx.bind_keys(crate::file_tree::bindings());
        });
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::new(launch, None, Vec::new(), window, cx));
        cx.run_until_parked();
        (workspace, cx)
    }

    #[gpui::test]
    fn quick_open_should_open_a_file_by_fuzzy_name(cx: &mut TestAppContext) {
        let dir = scratch("quick-open");
        let (workspace, cx) = workspace(
            Launch {
                folder: Some(dir.clone()),
                sidebar: true,
                ..Launch::default()
            },
            cx,
        );
        cx.simulate_keystrokes("cmd-p");
        cx.simulate_input("mainrs");
        cx.simulate_keystrokes("enter");
        let opened = workspace.read_with(cx, |this, cx| {
            this.active_editor()
                .and_then(|editor| editor.read(cx).path().map(Path::to_owned))
        });
        assert_eq!(opened, Some(dir.join("src/main.rs")));
    }

    #[gpui::test]
    fn a_typed_path_should_open_a_file_outside_the_folder(cx: &mut TestAppContext) {
        let dir = scratch("typed-path");
        let outside = scratch("typed-path-outside").join("notes.txt");
        let (workspace, cx) = workspace(Launch::default(), cx);
        let opened = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |this, cx| {
                this.active_editor()
                    .and_then(|editor| editor.read(cx).path().map(Path::to_owned))
            })
        };
        // With no folder open, and from the command palette's `>`.
        cx.simulate_keystrokes("cmd-shift-p");
        cx.simulate_input(&outside.to_string_lossy());
        cx.simulate_keystrokes("enter");
        let from_palette = opened(cx);
        workspace.update_in(cx, |this, window, cx| {
            this.open_paths(vec![dir.clone()], window, cx);
        });
        cx.simulate_keystrokes("cmd-p");
        cx.simulate_input(&dir.join("src/main.rs").to_string_lossy());
        cx.simulate_keystrokes("enter");
        assert_eq!(
            [from_palette, opened(cx)],
            [
                Some(outside.canonicalize().unwrap()),
                Some(dir.join("src/main.rs").canonicalize().unwrap())
            ]
        );
    }

    #[gpui::test]
    fn a_tab_should_show_its_path_under_the_pointer(cx: &mut TestAppContext) {
        let dir = scratch("tab-tooltip");
        let (_, cx) = workspace(
            Launch {
                files: vec![SavedFile {
                    path: dir.join("notes.txt"),
                    line: 1,
                    column: 1,
                }],
                ..Launch::default()
            },
            cx,
        );
        let tab = gpui::point(px(TRAFFIC_LIGHTS + 20.), px(TOP_BAR / 2.));
        cx.simulate_mouse_move(tab, None, gpui::Modifiers::none());
        let before = cx.debug_bounds("tooltip");
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(
            (before.is_some(), cx.debug_bounds("tooltip").is_some()),
            (false, true)
        );
    }

    #[gpui::test]
    fn the_command_palette_should_run_an_editor_command(cx: &mut TestAppContext) {
        let dir = scratch("palette");
        let (workspace, cx) = workspace(
            Launch {
                folder: Some(dir.clone()),
                files: vec![SavedFile {
                    path: dir.join("src/main.rs"),
                    line: 1,
                    column: 1,
                }],
                ..Launch::default()
            },
            cx,
        );
        cx.simulate_keystrokes("cmd-shift-p");
        cx.simulate_input("toggle line comment");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let text = workspace.read_with(cx, |this, cx| {
            this.active_editor()
                .map(|editor| editor.read(cx).document().rope().to_string())
        });
        assert_eq!(text.as_deref(), Some("// fn main() {}\n"));
    }

    #[gpui::test]
    fn saving_should_write_the_file_and_make_it_clean(cx: &mut TestAppContext) {
        let dir = scratch("save");
        let file = dir.join("notes.txt");
        let (workspace, cx) = workspace(
            Launch {
                files: vec![SavedFile {
                    path: file.clone(),
                    line: 1,
                    column: 6,
                }],
                ..Launch::default()
            },
            cx,
        );
        cx.simulate_input(" world");
        cx.simulate_keystrokes("cmd-s");
        cx.run_until_parked();
        let dirty = workspace.read_with(cx, |this, cx| {
            this.active_editor()
                .map(|editor| editor.read(cx).is_dirty())
        });
        assert_eq!(
            (std::fs::read_to_string(&file).unwrap(), dirty),
            ("hello world\n".to_owned(), Some(false))
        );
    }

    #[gpui::test]
    fn closing_a_dirty_editor_should_ask_and_keep_it_on_cancel(cx: &mut TestAppContext) {
        let dir = scratch("close");
        let (workspace, cx) = workspace(
            Launch {
                files: vec![SavedFile {
                    path: dir.join("notes.txt"),
                    line: 1,
                    column: 1,
                }],
                ..Launch::default()
            },
            cx,
        );
        cx.simulate_input("x");
        cx.simulate_keystrokes("cmd-w");
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        let open = workspace.read_with(cx, |this, _| this.editors.len());
        cx.simulate_keystrokes("cmd-w");
        cx.simulate_prompt_answer("Don't Save");
        cx.run_until_parked();
        let after = workspace.read_with(cx, |this, _| this.editors.len());
        assert_eq!(
            (
                open,
                after,
                std::fs::read_to_string(dir.join("notes.txt")).unwrap()
            ),
            (1, 0, "hello\n".to_owned())
        );
    }

    #[gpui::test]
    fn saving_an_untitled_file_should_ask_for_a_name_and_take_its_language(
        cx: &mut TestAppContext,
    ) {
        let dir = scratch("untitled");
        let target = dir.join("new.rs");
        let (workspace, cx) = workspace(Launch::default(), cx);
        cx.simulate_keystrokes("cmd-n");
        cx.simulate_input("fn x() {}");
        cx.simulate_keystrokes("cmd-s");
        cx.run_until_parked();
        let asked = cx.did_prompt_for_new_path();
        let chosen = target.clone();
        cx.simulate_new_path_selection(move |_| Some(chosen));
        cx.run_until_parked();
        let (path, language, dirty) = workspace.read_with(cx, |this, cx| {
            let editor = this.active_editor().unwrap().read(cx);
            (
                editor.path().map(Path::to_owned),
                editor.language().map(|language| language.name),
                editor.is_dirty(),
            )
        });
        let written = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            (asked, path, language, dirty, written),
            (
                true,
                Some(target),
                Some("Rust"),
                false,
                "fn x() {}".to_owned()
            )
        );
    }

    #[gpui::test]
    fn opening_a_folder_and_a_file_together_should_keep_the_file_open(cx: &mut TestAppContext) {
        let dir = scratch("folder-and-file");
        let other = scratch("folder-and-file-other");
        let (workspace, cx) = workspace(
            Launch {
                folder: Some(other),
                ..Launch::default()
            },
            cx,
        );
        workspace.update_in(cx, |this, window, cx| {
            this.open_paths(vec![dir.clone(), dir.join("notes.txt")], window, cx);
        });
        cx.run_until_parked();
        let (folder, open) = workspace.read_with(cx, |this, cx| {
            (
                this.folder.clone(),
                this.editors
                    .iter()
                    .filter_map(|editor| editor.read(cx).path().map(Path::to_owned))
                    .collect::<Vec<_>>(),
            )
        });
        assert_eq!(
            (folder, open),
            (Some(dir.clone()), vec![dir.join("notes.txt")])
        );
    }

    #[gpui::test]
    fn a_clean_file_changed_on_disk_should_reload_when_the_window_is_back(cx: &mut TestAppContext) {
        let dir = scratch("reload");
        let file = dir.join("notes.txt");
        let (workspace, cx) = workspace(
            Launch {
                files: vec![SavedFile {
                    path: file.clone(),
                    line: 1,
                    column: 1,
                }],
                ..Launch::default()
            },
            cx,
        );
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&file, "changed elsewhere\n").unwrap();
        workspace.update_in(cx, Workspace::on_activated);
        let text = workspace.read_with(cx, |this, cx| {
            this.active_editor()
                .map(|editor| editor.read(cx).document().rope().to_string())
        });
        assert_eq!(text.as_deref(), Some("changed elsewhere\n"));
    }

    #[test]
    fn tabs_with_the_same_name_should_show_their_folders() {
        let tab = |path: &str| {
            let name = Path::new(path).file_name().unwrap().to_string_lossy();
            (
                SharedString::from(name.into_owned()),
                Some(PathBuf::from(path)),
            )
        };
        assert_eq!(
            tab_folders(&[
                tab("/p/lsp/mod.rs"),
                tab("/p/editor/mod.rs"),
                tab("/p/main.rs")
            ]),
            vec![Some("lsp".into()), Some("editor".into()), None]
        );
    }

    #[gpui::test]
    fn quick_open_should_say_the_folder_is_being_listed_and_fill_in(cx: &mut TestAppContext) {
        let dir = scratch("listing");
        let (workspace, cx) = workspace(Launch::default(), cx);
        workspace.update_in(cx, |this, window, cx| {
            this.open_paths(vec![dir.clone()], window, cx);
            this.open_picker("", window, cx);
        });
        let files = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |this, cx| {
                let files = this.picker.as_ref().unwrap().read(cx).files().clone();
                (files.folder, files.listing, files.project.len())
            })
        };
        let while_listing = files(cx);
        cx.run_until_parked();
        assert_eq!(
            (while_listing, files(cx)),
            ((true, true, 0), (true, false, 2))
        );
    }

    #[gpui::test]
    fn quick_open_should_offer_the_previous_file_first(cx: &mut TestAppContext) {
        let dir = scratch("recent");
        let file = |name: &str| SavedFile {
            path: dir.join(name),
            line: 1,
            column: 1,
        };
        let (workspace, cx) = workspace(
            Launch {
                folder: Some(dir.clone()),
                files: vec![file("notes.txt"), file("src/main.rs")],
                active: 0,
                sidebar: true,
            },
            cx,
        );
        let active = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |this, cx| {
                this.active_editor()
                    .and_then(|editor| editor.read(cx).path().map(Path::to_owned))
            })
        };
        cx.simulate_keystrokes("cmd-p enter");
        cx.run_until_parked();
        let first = active(cx);
        cx.simulate_keystrokes("cmd-p enter");
        cx.run_until_parked();
        assert_eq!(
            (first, active(cx)),
            (Some(dir.join("src/main.rs")), Some(dir.join("notes.txt")))
        );
    }

    #[gpui::test]
    fn cmd_p_again_should_step_down_the_list(cx: &mut TestAppContext) {
        let dir = scratch("step");
        let (workspace, cx) = workspace(
            Launch {
                folder: Some(dir.clone()),
                sidebar: true,
                ..Launch::default()
            },
            cx,
        );
        // The folder's files in order: notes.txt, then src/main.rs.
        cx.simulate_keystrokes("cmd-p cmd-p enter");
        cx.run_until_parked();
        let opened = workspace.read_with(cx, |this, cx| {
            this.active_editor()
                .and_then(|editor| editor.read(cx).path().map(Path::to_owned))
        });
        assert_eq!(opened, Some(dir.join("src/main.rs")));
    }

    #[gpui::test]
    fn escape_in_the_explorer_should_give_the_editor_the_keys_back(cx: &mut TestAppContext) {
        let dir = scratch("escape");
        let (workspace, cx) = workspace(
            Launch {
                folder: Some(dir.clone()),
                files: vec![SavedFile {
                    path: dir.join("notes.txt"),
                    line: 1,
                    column: 1,
                }],
                sidebar: true,
                ..Launch::default()
            },
            cx,
        );
        let focused = |cx: &mut VisualTestContext| {
            workspace.update_in(cx, |this, window, cx| {
                (
                    this.tree.focus_handle(cx).is_focused(window),
                    this.active_editor()
                        .is_some_and(|editor| editor.focus_handle(cx).is_focused(window)),
                )
            })
        };
        cx.simulate_keystrokes("cmd-shift-e");
        let in_tree = focused(cx);
        cx.simulate_keystrokes("escape");
        assert_eq!((in_tree, focused(cx)), ((true, false), (false, true)));
    }

    #[gpui::test]
    fn revert_file_should_take_the_disk_text_and_undo_bring_the_edit_back(cx: &mut TestAppContext) {
        let dir = scratch("revert");
        let (workspace, cx) = workspace(
            Launch {
                files: vec![SavedFile {
                    path: dir.join("notes.txt"),
                    line: 1,
                    column: 6,
                }],
                ..Launch::default()
            },
            cx,
        );
        let state = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |this, cx| {
                let editor = this.active_editor().unwrap().read(cx);
                (editor.document().rope().to_string(), editor.is_dirty())
            })
        };
        cx.simulate_input(" world");
        cx.dispatch_action(RevertFile);
        let reverted = state(cx);
        cx.simulate_keystrokes("cmd-z");
        assert_eq!(
            (reverted, state(cx)),
            (
                ("hello\n".to_owned(), false),
                ("hello world\n".to_owned(), true)
            )
        );
    }

    #[gpui::test]
    fn saving_over_a_file_changed_on_disk_can_reload_it_instead(cx: &mut TestAppContext) {
        let dir = scratch("reload-instead");
        let file = dir.join("notes.txt");
        let (workspace, cx) = workspace(
            Launch {
                files: vec![SavedFile {
                    path: file.clone(),
                    line: 1,
                    column: 1,
                }],
                ..Launch::default()
            },
            cx,
        );
        cx.simulate_input("x");
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&file, "changed elsewhere\n").unwrap();
        cx.simulate_keystrokes("cmd-s");
        cx.simulate_prompt_answer("Reload from Disk");
        cx.run_until_parked();
        let (text, dirty) = workspace.read_with(cx, |this, cx| {
            let editor = this.active_editor().unwrap().read(cx);
            (editor.document().rope().to_string(), editor.is_dirty())
        });
        assert_eq!(
            (text, dirty, std::fs::read_to_string(&file).unwrap()),
            (
                "changed elsewhere\n".to_owned(),
                false,
                "changed elsewhere\n".to_owned()
            )
        );
    }
}
