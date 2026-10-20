//! What the next launch reopens: the folder, the open files with their cursors, and the
//! window's frame, in `~/Library/Application Support/kiled/session.toml` (DESIGN.md §7).

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use gpui::{Bounds, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowState {
    #[default]
    Windowed,
    Maximized,
    Fullscreen,
}

/// The window's frame; for maximized and full screen, the frame it returns to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedWindow {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub state: WindowState,
}

impl SavedWindow {
    pub fn of(bounds: WindowBounds) -> Self {
        let (state, frame) = match bounds {
            WindowBounds::Windowed(frame) => (WindowState::Windowed, frame),
            WindowBounds::Maximized(frame) => (WindowState::Maximized, frame),
            WindowBounds::Fullscreen(frame) => (WindowState::Fullscreen, frame),
        };
        Self {
            x: f32::from(frame.origin.x),
            y: f32::from(frame.origin.y),
            width: f32::from(frame.size.width),
            height: f32::from(frame.size.height),
            state,
        }
    }

    pub fn bounds(self) -> WindowBounds {
        let frame = Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width), px(self.height)),
        );
        match self.state {
            WindowState::Windowed => WindowBounds::Windowed(frame),
            WindowState::Maximized => WindowBounds::Maximized(frame),
            WindowState::Fullscreen => WindowBounds::Fullscreen(frame),
        }
    }

    /// The frame, if it is a sane size and still on one of `displays` (a monitor may be gone).
    pub fn on_screen(self, displays: &[Bounds<Pixels>]) -> Option<WindowBounds> {
        let frame = self.bounds().get_bounds();
        let sane = [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite())
            && self.width >= 100.
            && self.height >= 100.;
        (sane && displays.iter().any(|display| display.intersects(&frame))).then(|| self.bounds())
    }
}

/// An open file and where its cursor was (1-based).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedFile {
    pub path: PathBuf,
    #[serde(default)]
    pub line: usize,
    #[serde(default)]
    pub column: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<PathBuf>,
    #[serde(default)]
    pub files: Vec<SavedFile>,
    /// Index of the active file.
    #[serde(default)]
    pub active: usize,
    #[serde(default = "shown")]
    pub sidebar: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<SavedWindow>,
}

fn shown() -> bool {
    true
}

/// Where sessions live; `None` saves nothing (tests).
#[derive(Clone, Debug)]
pub struct Store {
    file: PathBuf,
}

impl Store {
    /// `~/Library/Application Support/kiled/session.toml`.
    pub fn standard() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        Some(Self::at(
            &PathBuf::from(home).join("Library/Application Support/kiled"),
        ))
    }

    pub fn at(dir: &Path) -> Self {
        Self {
            file: dir.join("session.toml"),
        }
    }

    /// The last session, if there is a readable one.
    ///
    /// # Errors
    /// If the file exists but can't be read or parsed; the message says which.
    pub fn load(&self) -> Result<Option<Session>, String> {
        match fs::read_to_string(&self.file) {
            Ok(text) => toml::from_str(&text)
                .map(Some)
                .map_err(|err| format!("{}: {err}", self.file.display())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(format!("{}: {err}", self.file.display())),
        }
    }

    /// Replaces the saved session; unchanged text isn't written again.
    ///
    /// # Errors
    /// If it can't be written.
    pub fn save(&self, session: &Session) -> io::Result<()> {
        let text = toml::to_string(session).map_err(io::Error::other)?;
        if fs::read_to_string(&self.file).is_ok_and(|old| old == text) {
            return Ok(());
        }
        if let Some(dir) = self.file.parent() {
            fs::create_dir_all(dir)?;
        }
        kiled_core::files::save(&self.file, &text).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_should_round_trip() {
        let dir = std::env::temp_dir().join(format!("kiled-session-{}", std::process::id()));
        let store = Store::at(&dir);
        let session = Session {
            folder: Some(PathBuf::from("/tmp/project")),
            files: vec![SavedFile {
                path: PathBuf::from("/tmp/project/src/main.rs"),
                line: 12,
                column: 5,
            }],
            active: 0,
            sidebar: false,
            window: Some(SavedWindow {
                x: 10.,
                y: 20.,
                width: 900.,
                height: 600.,
                state: WindowState::Windowed,
            }),
        };
        store.save(&session).unwrap();
        assert_eq!(store.load().unwrap(), Some(session));
    }

    #[test]
    fn a_window_off_every_display_should_not_be_restored() {
        let frame = SavedWindow {
            x: 5000.,
            y: 5000.,
            width: 800.,
            height: 600.,
            state: WindowState::Windowed,
        };
        let display = Bounds::new(point(px(0.), px(0.)), size(px(1440.), px(900.)));
        assert_eq!(frame.on_screen(&[display]), None);
    }
}
