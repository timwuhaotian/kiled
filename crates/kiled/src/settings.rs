//! `~/.config/kiled/config.toml` (DESIGN.md §9).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use gpui::{Global, Pixels, SharedString, px};
use kiled_core::text::Indent;
use serde::Deserialize;

use crate::theme::ThemeName;

/// User settings; every key is optional in the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub theme: ThemeName,
    pub font_family: SharedString,
    pub font_size: Pixels,
    /// Indentation for files that don't show their own (new files, unindented ones).
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub cursor_blink: bool,
    /// The sidebar background's opacity, 0–1, over the blurred desktop. `None`: the theme's.
    pub sidebar_opacity: Option<f32>,
    /// `[servers]`: a language server's command, replacing the default; an empty one turns
    /// that server off.
    pub servers: BTreeMap<String, Vec<String>>,
}

impl Global for Settings {}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeName::default(),
            font_family: "Menlo".into(),
            font_size: px(13.),
            tab_size: 4,
            insert_spaces: true,
            cursor_blink: true,
            sidebar_opacity: None,
            servers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SettingsFile {
    theme: Option<ThemeName>,
    font_family: Option<String>,
    font_size: Option<f32>,
    tab_size: Option<usize>,
    insert_spaces: Option<bool>,
    cursor_blink: Option<bool>,
    sidebar_opacity: Option<f32>,
    servers: BTreeMap<String, Vec<String>>,
}

impl Settings {
    /// Parses `text` over the defaults.
    ///
    /// # Errors
    /// On invalid TOML, unknown keys or out-of-range values.
    pub fn parse(text: &str) -> Result<Self, String> {
        let file: SettingsFile = toml::from_str(text).map_err(|err| err.to_string())?;
        let defaults = Self::default();
        let font_size = match file.font_size {
            Some(size) if (6.0..=72.0).contains(&size) => px(size),
            Some(size) => return Err(format!("font_size {size} is outside 6–72")),
            None => defaults.font_size,
        };
        let tab_size = match file.tab_size {
            Some(size) if (1..=16).contains(&size) => size,
            Some(size) => return Err(format!("tab_size {size} is outside 1–16")),
            None => defaults.tab_size,
        };
        let sidebar_opacity = match file.sidebar_opacity {
            Some(opacity) if (0.0..=1.0).contains(&opacity) => Some(opacity),
            Some(opacity) => return Err(format!("sidebar_opacity {opacity} is outside 0–1")),
            None => None,
        };
        Ok(Self {
            theme: file.theme.unwrap_or(defaults.theme),
            font_family: file.font_family.map_or(defaults.font_family, Into::into),
            font_size,
            tab_size,
            insert_spaces: file.insert_spaces.unwrap_or(defaults.insert_spaces),
            cursor_blink: file.cursor_blink.unwrap_or(defaults.cursor_blink),
            sidebar_opacity,
            servers: file.servers,
        })
    }

    /// Indentation for files that don't show their own.
    pub fn indent(&self) -> Indent {
        Indent {
            tabs: !self.insert_spaces,
            width: self.tab_size,
        }
    }
}

/// `path` with `$HOME` written as `~`, for showing.
pub fn abbreviate_home(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// A typed path with a leading `~` as `$HOME`.
pub fn expand_home(text: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (home, text.strip_prefix('~')) {
        (Some(home), Some(rest)) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(text),
    }
}

/// `text` with `key = value`: the line assigning `key` replaced, else one added before the
/// first table (so it stays a top-level key). Other lines, comments included, are kept.
pub fn set_key(text: &str, key: &str, value: &toml::Value) -> String {
    let assignment = format!("{key} = {value}");
    let mut found = false;
    let mut in_table = false;
    let mut out = String::with_capacity(text.len() + assignment.len() + 1);
    for line in text.lines() {
        if line.trim_start().starts_with('[') {
            if !found {
                out.push_str(&assignment);
                out.push('\n');
                found = true;
            }
            in_table = true;
        }
        if !found && !in_table && assigns(line, key) {
            found = true;
            out.push_str(&assignment);
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        out.push_str(&assignment);
        out.push('\n');
    }
    out
}

/// `text` without the top-level lines assigning `key`, so the default applies again.
pub fn unset_key(text: &str, key: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_table = false;
    for line in text.lines() {
        if line.trim_start().starts_with('[') {
            in_table = true;
        }
        if in_table || !assigns(line, key) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn assigns(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// `$XDG_CONFIG_HOME/kiled`, else `~/.config/kiled`.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|dir| dir.join("kiled"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_tilde_should_be_home() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(
            [
                expand_home("~/.npmrc"),
                expand_home("~"),
                expand_home("~nobody/x"),
                expand_home("/etc/hosts")
            ],
            [
                home.join(".npmrc"),
                home,
                PathBuf::from("~nobody/x"),
                PathBuf::from("/etc/hosts")
            ]
        );
    }

    #[test]
    fn parse_should_keep_defaults_for_missing_keys() {
        assert_eq!(Settings::parse("").unwrap(), Settings::default());
    }

    #[test]
    fn parse_should_reject_unknown_keys_and_absurd_values() {
        assert!(Settings::parse("fontsize = 15.0").is_err());
        assert!(Settings::parse("font_size = 400.0").is_err());
        assert!(Settings::parse("tab_size = 0").is_err());
    }

    #[test]
    fn parse_should_read_the_servers_table() {
        let settings = Settings::parse(
            "theme = \"paper\"\n[servers]\nrust-analyzer = [\"/opt/ra\"]\ntaplo = []\n",
        )
        .unwrap();
        assert_eq!(
            (
                settings.theme,
                settings.servers.get("rust-analyzer").cloned(),
                settings.servers.get("taplo").map(Vec::len)
            ),
            (ThemeName::Paper, Some(vec!["/opt/ra".to_owned()]), Some(0))
        );
    }

    #[test]
    fn set_key_should_replace_the_line_and_keep_comments() {
        let text = "# mine\nfont_size = 13.0 # big\ncursor_blink = false\n";
        assert_eq!(
            set_key(text, "font_size", &toml::Value::Float(15.)),
            "# mine\nfont_size = 15.0\ncursor_blink = false\n"
        );
    }

    #[test]
    fn set_key_should_add_a_missing_key_above_the_first_table() {
        let text = "theme = \"dusk\"\n\n[servers]\ntaplo = []\n";
        let written = set_key(text, "font_size", &toml::Value::Float(14.));
        assert_eq!(
            Settings::parse(&written).unwrap().font_size,
            px(14.),
            "{written}"
        );
    }

    #[test]
    fn set_key_should_not_match_a_longer_key() {
        assert_eq!(
            set_key("cursor_blinking = 1\n", "cursor_blink", &true.into()),
            "cursor_blinking = 1\ncursor_blink = true\n"
        );
    }

    #[test]
    fn unset_key_should_drop_the_assignment_and_keep_the_rest() {
        assert_eq!(
            unset_key(
                "# mine\nsidebar_opacity = 0.5\ntheme = \"dusk\"",
                "sidebar_opacity"
            ),
            "# mine\ntheme = \"dusk\"\n"
        );
    }
}
