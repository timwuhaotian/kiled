//! Kil's five themes, with the editor's colors derived from each theme's terminal palette, so
//! code in Kiled looks like it does in a Kil pane (DESIGN.md §7).

use gpui::{Global, Hsla, rgb, rgba};
use kiled_core::{document::Severity, language::Highlight};
use serde::Deserialize;

/// How much of the desktop the sidebar hides, unless Settings says otherwise.
const SIDEBAR_OPACITY: f32 = 0.92;

/// A theme's name in `config.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    #[default]
    Night,
    Paper,
    Fjord,
    Ember,
    Dusk,
}

impl ThemeName {
    pub const ALL: [Self; 5] = [
        Self::Night,
        Self::Paper,
        Self::Fjord,
        Self::Ember,
        Self::Dusk,
    ];

    /// As written in `config.toml`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Night => "night",
            Self::Paper => "paper",
            Self::Fjord => "fjord",
            Self::Ember => "ember",
            Self::Dusk => "dusk",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Night => "Night",
            Self::Paper => "Paper",
            Self::Fjord => "Fjord",
            Self::Ember => "Ember",
            Self::Dusk => "Dusk",
        }
    }
}

/// Every color the app draws with.
pub struct Theme {
    /// ANSI colors 0–15, as Kil's terminal palette has them; syntax colors come from these.
    pub ansi: [Hsla; 16],
    pub editor_bg: Hsla,
    pub text: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
    pub current_line: Hsla,
    pub find_match: Hsla,
    pub sidebar_bg: Hsla,
    pub chrome_bg: Hsla,
    pub chrome_text: Hsla,
    pub muted: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub accent: Hsla,
    pub failure: Hsla,
    pub warning: Hsla,
    pub divider: Hsla,
    pub banner_bg: Hsla,
    pub popover_bg: Hsla,
    orange: Hsla,
}

impl Global for Theme {}

impl Default for Theme {
    fn default() -> Self {
        Self::named(ThemeName::Night)
    }
}

/// A theme's colors as `0xRRGGBB` or, for the translucent ones, `0xRRGGBBAA`.
struct Colors {
    ansi: [u32; 16],
    foreground: u32,
    background: u32,
    cursor: u32,
    orange: u32,
    selection: u32,
    sidebar: u32,
    chrome: u32,
    text: u32,
    muted: u32,
    hover: u32,
    selected: u32,
    accent: u32,
    failure: u32,
    divider: u32,
    banner: u32,
}

#[expect(clippy::unreadable_literal, reason = "hex colors read as #RRGGBB(AA)")]
impl Theme {
    pub fn named(name: ThemeName) -> Self {
        match name {
            ThemeName::Night => Self::night(),
            ThemeName::Paper => Self::paper(),
            ThemeName::Fjord => Self::fjord(),
            ThemeName::Ember => Self::ember(),
            ThemeName::Dusk => Self::dusk(),
        }
    }

    /// This theme with the sidebar at `opacity` (Settings), or as the theme has it.
    #[must_use]
    pub fn with_sidebar_opacity(mut self, opacity: Option<f32>) -> Self {
        if let Some(opacity) = opacity {
            self.sidebar_bg.a = opacity.clamp(0., 1.);
        }
        self
    }

    fn night() -> Self {
        Self::build(&Colors {
            ansi: [
                0x1d1f24, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x5c6370, 0xef7b85, 0xa8d38a, 0xf0cd8a, 0x74bdf7, 0xd48ae8, 0x67c7d2, 0xe6e9ef,
            ],
            foreground: 0xd7dae0,
            background: 0x111316,
            cursor: 0xf2f3f5,
            orange: 0xd19a66,
            selection: 0x3d5a80cc,
            sidebar: 0x16181db8,
            chrome: 0x16181d,
            text: 0xe3e6eb,
            muted: 0x7d8490,
            hover: 0xffffff0f,
            selected: 0xffffff1f,
            accent: 0x6ca8ff,
            failure: 0xf06c6c,
            divider: 0x262930,
            banner: 0x1d2026f2,
        })
    }

    fn paper() -> Self {
        Self::build(&Colors {
            ansi: [
                0x24292f, 0xc4242f, 0x1a7f37, 0x845600, 0x0550ae, 0x8250df, 0x1b7c83, 0x6e7781,
                0x57606a, 0xa40e26, 0x116329, 0x633c01, 0x1a71d0, 0x835dc7, 0x297b8f, 0x8c959f,
            ],
            foreground: 0x2e3138,
            background: 0xfbfaf7,
            cursor: 0x3a3f4b,
            orange: 0x9a4c00,
            selection: 0xb6d4fecc,
            sidebar: 0xf1efeac8,
            chrome: 0xf1efea,
            text: 0x23262c,
            muted: 0x7a7f88,
            hover: 0x0000000d,
            selected: 0x00000017,
            accent: 0x2f6fde,
            failure: 0xcf3a3a,
            divider: 0xdcd9d2,
            banner: 0xe9e7e2f2,
        })
    }

    fn fjord() -> Self {
        Self::build(&Colors {
            ansi: [
                0x2a303c, 0xd0707a, 0xa3c48c, 0xe4c98a, 0x7fa6d6, 0xb896c8, 0x82c3c9, 0xd4dae6,
                0x637089, 0xdd8890, 0xb4d29e, 0xedd7a0, 0x95b8e3, 0xc7a9d6, 0x98d1d6, 0xeef1f6,
            ],
            foreground: 0xd4dae6,
            background: 0x1c212b,
            cursor: 0xe5e9f0,
            orange: 0xd08770,
            selection: 0x3e5679cc,
            sidebar: 0x191d26b8,
            chrome: 0x191d26,
            text: 0xdde3ee,
            muted: 0x7c8698,
            hover: 0xffffff0f,
            selected: 0xffffff1f,
            accent: 0x88b4e6,
            failure: 0xe07a84,
            divider: 0x2a313d,
            banner: 0x232936f2,
        })
    }

    fn ember() -> Self {
        Self::build(&Colors {
            ansi: [
                0x2b2520, 0xe06c50, 0xa9b665, 0xe3a64f, 0x7daea3, 0xd3869b, 0x89b482, 0xd4c4a8,
                0x766859, 0xf08a6e, 0xbfcc7c, 0xf0bd6b, 0x95c4b8, 0xe29db0, 0xa2c99a, 0xf2e8d5,
            ],
            foreground: 0xe8dcc6,
            background: 0x1d1915,
            cursor: 0xf2c48d,
            orange: 0xe78a4e,
            selection: 0x5a4632cc,
            sidebar: 0x1a1612b8,
            chrome: 0x1a1612,
            text: 0xece2d0,
            muted: 0x8f8170,
            hover: 0xffffff0f,
            selected: 0xffffff1c,
            accent: 0xe8a15a,
            failure: 0xea6b55,
            divider: 0x2f2822,
            banner: 0x2a241ef2,
        })
    }

    fn dusk() -> Self {
        Self::build(&Colors {
            ansi: [
                0x262233, 0xe4708e, 0x8fd1a0, 0xe8c88a, 0x8aa8f0, 0xc09af0, 0x7cc8d8, 0xcbc5dc,
                0x6b628a, 0xf088a4, 0xa6deb4, 0xf0d6a2, 0xa2baf5, 0xd0b2f6, 0x96d6e3, 0xf1eefa,
            ],
            foreground: 0xddd8ea,
            background: 0x17141f,
            cursor: 0xe9e4f6,
            orange: 0xf0a46c,
            selection: 0x4b3d6ecc,
            sidebar: 0x15121cb8,
            chrome: 0x15121c,
            text: 0xe3def0,
            muted: 0x8a8199,
            hover: 0xffffff0f,
            selected: 0xffffff1f,
            accent: 0xb48cff,
            failure: 0xef7591,
            divider: 0x2b2638,
            banner: 0x221e2df2,
        })
    }

    fn build(colors: &Colors) -> Self {
        let opaque = |hex: u32| -> Hsla { rgb(hex).into() };
        let translucent = |hex: u32| -> Hsla { rgba(hex).into() };
        let accent = opaque(colors.accent);
        Self {
            ansi: colors.ansi.map(opaque),
            editor_bg: opaque(colors.background),
            text: opaque(colors.foreground),
            cursor: opaque(colors.cursor),
            selection: translucent(colors.selection),
            current_line: translucent(colors.hover),
            find_match: accent.opacity(0.28),
            // Mostly opaque: file names must read over a light desktop as well as a dark
            // one. Settings ▸ Sidebar opacity lets more of it through.
            sidebar_bg: Hsla {
                a: SIDEBAR_OPACITY,
                ..translucent(colors.sidebar)
            },
            chrome_bg: opaque(colors.chrome),
            chrome_text: opaque(colors.text),
            muted: opaque(colors.muted),
            hover: translucent(colors.hover),
            selected: translucent(colors.selected),
            accent,
            failure: opaque(colors.failure),
            warning: opaque(colors.ansi[3]),
            divider: opaque(colors.divider),
            banner_bg: translucent(colors.banner),
            popover_bg: translucent(colors.banner | 0xff),
            orange: opaque(colors.orange),
        }
    }

    /// The color code of `highlight` is drawn in; `None` is the plain text color.
    pub fn syntax(&self, highlight: Highlight) -> Hsla {
        let ansi = &self.ansi;
        match highlight {
            Highlight::Comment => ansi[8],
            Highlight::Keyword | Highlight::Label => ansi[5],
            Highlight::String => ansi[2],
            Highlight::Escape | Highlight::Operator => ansi[6],
            Highlight::Number | Highlight::Constant => self.orange,
            Highlight::Function => ansi[4],
            Highlight::Type | Highlight::Attribute => ansi[3],
            Highlight::Property | Highlight::Builtin | Highlight::Tag => ansi[1],
            Highlight::Variable => self.text,
            Highlight::Punctuation => ansi[7],
        }
    }

    /// The squiggle under a diagnostic.
    pub fn severity(&self, severity: Severity) -> Hsla {
        match severity {
            Severity::Error => self.failure,
            Severity::Warning => self.warning,
            Severity::Information => self.accent,
            Severity::Hint => self.muted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn every_theme_should_round_trip_through_config_toml() {
        for name in ThemeName::ALL {
            let text = format!("theme = \"{}\"", name.key());
            assert_eq!(Settings::parse(&text).unwrap().theme, name);
        }
    }

    /// Relative luminance, WCAG 2.
    fn luminance(color: Hsla) -> f32 {
        let rgba = color.to_rgb();
        let channel = |c: f32| {
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgba.r) + 0.7152 * channel(rgba.g) + 0.0722 * channel(rgba.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn code_colors_should_read_against_the_background() {
        let kinds = [
            Highlight::Keyword,
            Highlight::String,
            Highlight::Number,
            Highlight::Function,
            Highlight::Type,
            Highlight::Property,
            Highlight::Variable,
        ];
        for name in ThemeName::ALL {
            let theme = Theme::named(name);
            for kind in kinds {
                let ratio = contrast(theme.syntax(kind), theme.editor_bg);
                assert!(ratio >= 4.5, "{name:?} {kind:?}: {ratio:.2}");
            }
        }
    }
}
