//! Applies `config.toml` and `keymap.toml`; re-applied when either file changes. The
//! Settings window writes `config.toml` through [`write_setting`].

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};

use gpui::{App, AppContext as _, SharedString};
use kiled_core::files;

use crate::{
    editor, file_tree, find, keymap, picker, settings,
    settings::{Settings, config_dir},
    settings_window, text_field,
    theme::Theme,
};

/// Modification times of both files; a change triggers a reload.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp {
    settings: Option<SystemTime>,
    keymap: Option<SystemTime>,
}

fn file(name: &str) -> Option<PathBuf> {
    config_dir().map(|dir| dir.join(name))
}

fn modified(name: &str) -> Option<SystemTime> {
    file(name)
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
}

pub fn stamp() -> Stamp {
    Stamp {
        settings: modified("config.toml"),
        keymap: modified("keymap.toml"),
    }
}

/// Reads `path`; a missing file is `Ok(None)`.
fn read_path(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Reads `name` from the config directory; a missing file is `Ok(None)`.
fn load(name: &str) -> Result<Option<String>, String> {
    file(name)
        .map_or(Ok(None), |path| read_path(&path))
        .map_err(|err| format!("{name}: {err}"))
}

/// Reads `name`, reporting a failure as a notice.
fn read(name: &str, notices: &mut Vec<SharedString>) -> Option<String> {
    load(name).unwrap_or_else(|err| {
        notices.push(err.into());
        None
    })
}

/// What `config.toml` holds over the defaults; unlike the global, without the zoom.
///
/// # Errors
/// If the file can't be read or doesn't parse.
pub fn read_settings() -> Result<Settings, String> {
    match load("config.toml")? {
        Some(text) => Settings::parse(&text).map_err(|err| format!("config.toml: {err}")),
        None => Ok(Settings::default()),
    }
}

/// Sets one key in `config.toml`, keeping the rest of the file, and returns what the file
/// now holds. Nothing is written if the result would not parse.
///
/// # Errors
/// If the file can't be read or written, or the new value is invalid.
pub fn write_setting(key: &str, value: &toml::Value) -> Result<Settings, String> {
    let path = file("config.toml").ok_or("config.toml: no home directory")?;
    edit_settings_at(&path, |text| settings::set_key(text, key, value))
        .map_err(|err| format!("config.toml: {err}"))
}

/// Removes `key` from `config.toml`, so its default applies again.
///
/// # Errors
/// If the file can't be read or written.
pub fn unset_setting(key: &str) -> Result<Settings, String> {
    let path = file("config.toml").ok_or("config.toml: no home directory")?;
    edit_settings_at(&path, |text| settings::unset_key(text, key))
        .map_err(|err| format!("config.toml: {err}"))
}

fn edit_settings_at(path: &Path, edit: impl FnOnce(&str) -> String) -> Result<Settings, String> {
    let current = read_path(path).map_err(|err| err.to_string())?;
    let text = edit(&current.unwrap_or_default());
    let parsed = Settings::parse(&text)?;
    path.parent()
        .map_or(Ok(()), fs::create_dir_all)
        // Saved as any file is: whole or not at all, and a symlink into dotfiles stays one.
        .and_then(|()| files::save(path, &text))
        .map_err(|err| err.to_string())?;
    Ok(parsed)
}

/// Applies both files and returns problems to show the user. An invalid `config.toml`
/// keeps the settings already in effect; invalid keymap entries are skipped one by one.
pub fn apply(cx: &mut App) -> Vec<SharedString> {
    let mut notices = Vec::new();
    match read("config.toml", &mut notices).map(|text| Settings::parse(&text)) {
        Some(Ok(settings)) => cx.set_global(settings),
        Some(Err(err)) => notices.push(format!("config.toml: {err}").into()),
        None => cx.set_global(Settings::default()),
    }
    if !cx.has_global::<Settings>() {
        cx.set_global(Settings::default());
    }
    let settings = cx.global::<Settings>();
    cx.set_global(Theme::named(settings.theme).with_sidebar_opacity(settings.sidebar_opacity));
    let user_keymap = read("keymap.toml", &mut notices);
    let (table, errors) = keymap::resolve(user_keymap.as_deref());
    notices.extend(
        errors
            .into_iter()
            .map(|err| SharedString::from(format!("keymap.toml: {}: {}", err.key, err.reason))),
    );
    cx.clear_key_bindings();
    // The fixed keys of lists and fields go last: within one context they win.
    cx.bind_keys(
        keymap::bindings(&table)
            .into_iter()
            .chain(editor::bindings())
            .chain(find::bindings())
            .chain(picker::bindings())
            .chain(file_tree::bindings())
            .chain(text_field::bindings())
            .chain(settings_window::bindings()),
    );
    // A menu shows the keys bound when it was set, so it is set again with these.
    cx.set_menus(crate::menus(cx));
    notices
}

/// `keymap.toml`'s text, if there is one.
pub fn read_keymap() -> Result<Option<String>, String> {
    load("keymap.toml")
}

/// Rewrites `keymap.toml` with `edit` applied to its text — the template if there is no file
/// yet — and saves it as `config.toml` is saved.
///
/// # Errors
/// If the file can't be created or written.
pub fn update_keymap(edit: impl FnOnce(&str) -> String) -> Result<(), String> {
    let path = file("keymap.toml").ok_or("keymap.toml: no home directory")?;
    update_keymap_at(&path, edit).map_err(|err| format!("keymap.toml: {err}"))
}

fn update_keymap_at(path: &Path, edit: impl FnOnce(&str) -> String) -> io::Result<()> {
    let current = read_path(path)?;
    let text = edit(current.as_deref().unwrap_or(keymap::TEMPLATE));
    path.parent()
        .map_or(Ok(()), fs::create_dir_all)
        .and_then(|()| files::save(path, &text))
        .map(drop)
}

/// The path of `keymap.toml`, written first as a commented template if there is none.
///
/// # Errors
/// If the file can't be written.
pub fn keymap_file() -> Result<PathBuf, String> {
    let path = file("keymap.toml").ok_or("keymap.toml: no home directory")?;
    if !path.exists() {
        path.parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, keymap::TEMPLATE))
            .map_err(|err| format!("keymap.toml: {err}"))?;
    }
    Ok(path)
}

/// Opens the config folder in Finder.
///
/// # Errors
/// If the folder can't be created.
pub fn reveal_folder(cx: &App) -> Result<(), String> {
    let dir = config_dir().ok_or("no home directory")?;
    fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let mut open = Command::new("/usr/bin/open")
        .arg(&dir)
        .spawn()
        .map_err(|err| err.to_string())?;
    cx.background_spawn(async move {
        let _ = open.wait();
    })
    .detach();
    Ok(())
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kiled-config-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    #[test]
    fn edit_settings_should_create_the_file_and_return_what_it_holds() {
        let path = scratch("create");
        let settings = edit_settings_at(&path, |text| {
            settings::set_key(text, "font_size", &15.0.into())
        })
        .unwrap();
        assert_eq!(
            (settings.font_size, fs::read_to_string(&path).unwrap()),
            (px(15.), "font_size = 15.0\n".to_owned())
        );
    }

    #[test]
    fn edit_settings_should_leave_the_file_alone_when_the_value_is_invalid() {
        let path = scratch("invalid");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# mine\nfont_size = 13.0\n").unwrap();
        assert!(
            edit_settings_at(&path, |text| settings::set_key(
                text,
                "font_size",
                &400.0.into()
            ))
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# mine\nfont_size = 13.0\n"
        );
    }

    #[test]
    fn update_keymap_should_start_from_the_template_and_keep_the_comments() {
        let path = scratch("keymap-new").with_file_name("keymap.toml");
        update_keymap_at(&path, |text| {
            keymap::rebind(text, keymap::Command::ToggleSidebar, "cmd-shift-b")
        })
        .unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(keymap::TEMPLATE), "{text}");
        let (table, errors) = keymap::resolve(Some(&text));
        assert_eq!(
            (
                errors,
                keymap::keys_for(&table, keymap::Command::ToggleSidebar)
            ),
            (vec![], vec!["cmd-shift-b"])
        );
    }
}
