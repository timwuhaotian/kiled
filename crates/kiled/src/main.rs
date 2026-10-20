//! Kiled: a native macOS code editor.

mod completion;
mod config;
mod editor;
mod editor_element;
mod file_tree;
mod find;
mod keymap;
mod lsp;
mod picker;
mod session;
mod settings;
mod settings_window;
mod text_field;
mod theme;
mod workspace;

use std::{path::PathBuf, sync::OnceLock, time::Instant};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    App, Application, Bounds, Menu, MenuItem, OsAction, SystemMenuType, TitlebarOptions,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, actions, point, prelude::*, px, size,
};
use kiled_core::{language, lsp::protocol};

use crate::{
    session::{SavedFile, Store},
    workspace::{Launch, Workspace},
};

actions!(
    kiled,
    [OpenSettings, OpenKeymap, Hide, HideOthers, ShowAll, Quit]
);

/// When `main` started, for the first frame's log line (DESIGN.md §5.4).
pub static LAUNCHED: OnceLock<Instant> = OnceLock::new();

/// An unbundled binary started from a terminal inherits that app's `__CFBundleIdentifier`,
/// and `AppKit` then names and badges kiled as that app. Inside our own `.app`, launchd sets
/// the variable correctly, so keep it there.
fn forget_launcher_identity() {
    let bundled = std::env::current_exe()
        .is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"));
    if !bundled && std::env::var_os("__CFBundleIdentifier").is_some() {
        // SAFETY: first statement of `main`; no other thread exists yet to read the env.
        #[expect(unsafe_code, reason = "edition 2024 makes env mutation unsafe")]
        unsafe {
            std::env::remove_var("__CFBundleIdentifier");
        }
    }
}

/// The menu bar. Set again whenever the keymap changes, so the shortcuts it shows are the
/// ones bound.
fn menus(_: &App) -> Vec<Menu> {
    use editor as e;
    use workspace as w;
    vec![
        Menu {
            name: "Kiled".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::action("Keyboard Shortcuts…", OpenKeymap),
                MenuItem::separator(),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide Kiled", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Kiled", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New File", w::NewFile),
                MenuItem::action("Open…", w::Open),
                MenuItem::separator(),
                MenuItem::action("Save", w::Save),
                MenuItem::action("Save As…", w::SaveAs),
                MenuItem::action("Save All", w::SaveAll),
                MenuItem::separator(),
                MenuItem::action("Close Editor", w::CloseEditor),
                MenuItem::action("Close All Editors", w::CloseAllEditors),
                MenuItem::action("Reopen Closed Editor", w::ReopenClosedEditor),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::os_action("Undo", e::Undo, OsAction::Undo),
                MenuItem::os_action("Redo", e::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("Cut", e::Cut, OsAction::Cut),
                MenuItem::os_action("Copy", e::Copy, OsAction::Copy),
                MenuItem::os_action("Paste", e::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::action("Find", e::Find),
                MenuItem::action("Replace", e::Replace),
                MenuItem::action("Find Next", e::FindNext),
                MenuItem::action("Find Previous", e::FindPrevious),
                MenuItem::separator(),
                MenuItem::action("Toggle Line Comment", e::ToggleLineComment),
                MenuItem::action("Format Document", e::FormatDocument),
            ],
        },
        Menu {
            name: "Selection".into(),
            items: vec![
                MenuItem::os_action("Select All", e::SelectAll, OsAction::SelectAll),
                MenuItem::action("Expand Line Selection", e::ExpandLineSelection),
                MenuItem::separator(),
                MenuItem::action("Copy Line Up", e::CopyLineUp),
                MenuItem::action("Copy Line Down", e::CopyLineDown),
                MenuItem::action("Move Line Up", e::MoveLineUp),
                MenuItem::action("Move Line Down", e::MoveLineDown),
                MenuItem::separator(),
                MenuItem::action("Add Cursor Above", e::AddCursorAbove),
                MenuItem::action("Add Cursor Below", e::AddCursorBelow),
                MenuItem::action("Add Next Occurrence", e::AddNextOccurrence),
                MenuItem::action("Select All Occurrences", e::SelectAllOccurrences),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", w::CommandPalette),
                MenuItem::separator(),
                MenuItem::action("Toggle Sidebar", w::ToggleSidebar),
                MenuItem::action("Show Explorer", w::FocusExplorer),
                MenuItem::separator(),
                MenuItem::action("Zoom In", w::ZoomIn),
                MenuItem::action("Zoom Out", w::ZoomOut),
                MenuItem::action("Reset Zoom", w::ResetZoom),
            ],
        },
        Menu {
            name: "Go".into(),
            items: vec![
                MenuItem::action("Back", w::GoBack),
                MenuItem::action("Forward", w::GoForward),
                MenuItem::separator(),
                MenuItem::action("Go to File…", w::QuickOpen),
                MenuItem::action("Go to Line…", w::GoToLine),
                MenuItem::action("Go to Definition", e::GoToDefinition),
                MenuItem::separator(),
                MenuItem::action("Next Problem", e::NextProblem),
                MenuItem::action("Previous Problem", e::PreviousProblem),
                MenuItem::separator(),
                MenuItem::action("Next Editor", w::NextEditor),
                MenuItem::action("Previous Editor", w::PreviousEditor),
            ],
        },
    ]
}

/// Fallbacks for actions no focused element handled, e.g. from the Settings window.
fn register_app_actions(cx: &mut App) {
    cx.on_action(|_: &OpenSettings, cx| settings_window::open(cx));
    cx.on_action(|_: &OpenKeymap, cx| match config::keymap_file() {
        Ok(path) => workspace::open_in_window(vec![path], cx),
        Err(err) => log::error!("{err}"),
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Quit, cx| workspace::quit_app(cx));
}

/// Files and folders named on the command line, made absolute.
fn argument_paths() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_default();
    std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with('-'))
        .map(|arg| cwd.join(arg))
        .map(|path| path.canonicalize().unwrap_or(path))
        .collect()
}

/// What to open: the command line's paths (a folder, then files), else the last session
/// and its window frame.
fn launch(
    store: Option<&Store>,
    notices: &mut Vec<gpui::SharedString>,
) -> (Launch, Option<session::SavedWindow>) {
    let fresh = Launch {
        sidebar: true,
        ..Launch::default()
    };
    let paths = argument_paths();
    if !paths.is_empty() {
        let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|path| path.is_dir());
        let files = files
            .into_iter()
            .map(|path| SavedFile {
                path,
                line: 1,
                column: 1,
            })
            .collect();
        return (
            Launch {
                folder: folders.into_iter().next(),
                files,
                ..fresh
            },
            None,
        );
    }
    match store.map(Store::load) {
        Some(Ok(Some(session))) => (
            Launch {
                folder: session.folder.filter(|folder| folder.is_dir()),
                files: session
                    .files
                    .into_iter()
                    .filter(|file| file.path.is_file())
                    .collect(),
                active: session.active,
                sidebar: session.sidebar,
            },
            session.window,
        ),
        Some(Err(err)) => {
            notices.push(err.into());
            (fresh, None)
        }
        _ => (fresh, None),
    }
}

fn main() {
    forget_launcher_identity();
    LAUNCHED.get_or_init(Instant::now);
    env_logger::init();
    let app = Application::new();
    // Paths macOS hands over (Finder's Open With, `open -a Kiled .`), before or after launch.
    let (opened, mut to_open) = mpsc::unbounded::<Vec<PathBuf>>();
    app.on_open_urls(move |urls| {
        let paths: Vec<PathBuf> = urls
            .iter()
            .filter_map(|url| protocol::path_of(url))
            .collect();
        if !paths.is_empty() {
            let _ = opened.unbounded_send(paths);
        }
    });
    app.run(move |cx: &mut App| {
        // Highlight queries compile off the main thread, so the first file of a language
        // opens without waiting for its query (Rust's takes ~20 ms).
        cx.background_spawn(async {
            for language in language::all() {
                language.highlighter();
            }
        })
        .detach();
        let mut notices = config::apply(cx);
        register_app_actions(cx);
        let store = Store::standard();
        let (launch, frame) = launch(store.as_ref(), &mut notices);
        // Settings alone doesn't keep the app alive.
        cx.on_window_closed(|cx| {
            if !cx
                .windows()
                .iter()
                .any(|window| window.downcast::<Workspace>().is_some())
            {
                cx.quit();
            }
        })
        .detach();
        let displays: Vec<_> = cx
            .displays()
            .iter()
            .map(|display| display.bounds())
            .collect();
        let bounds = frame
            .and_then(|frame| frame.on_screen(&displays))
            .unwrap_or_else(|| {
                WindowBounds::Windowed(Bounds::centered(None, size(px(1200.), px(780.)), cx))
            });
        let options = WindowOptions {
            window_bounds: Some(bounds),
            titlebar: Some(TitlebarOptions {
                title: Some("Kiled".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(12.))),
            }),
            window_background: WindowBackgroundAppearance::Blurred,
            window_min_size: Some(size(px(520.), px(320.))),
            ..Default::default()
        };
        if let Err(err) = cx.open_window(options, |window, cx| {
            cx.new(|cx| Workspace::new(launch, store, notices, window, cx))
        }) {
            log::error!("failed to open window: {err:#}");
            cx.quit();
            return;
        }
        cx.spawn(async move |cx| {
            while let Some(paths) = to_open.next().await {
                let _ = cx.update(|cx| workspace::open_in_window(paths, cx));
            }
        })
        .detach();
        cx.activate(true);
    });
}
