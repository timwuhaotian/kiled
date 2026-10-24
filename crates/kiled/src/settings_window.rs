//! Kiled ▸ Settings…: a separate window that edits `config.toml` one key at a time and
//! records new keys into `keymap.toml` (DESIGN.md §8, §9). Each change is written, then
//! applied by the same reload that runs when the editor window regains focus.

use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, ClickEvent, Context, Div, ElementId, Entity,
    FocusHandle, Focusable, Hsla, KeyBinding, Keystroke, MouseButton, MouseDownEvent, SharedString,
    Stateful, Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    point, prelude::*, px, size,
};

use crate::{
    config,
    keymap::{self, Command, Group},
    settings::Settings,
    text_field::{FieldEvent, TextField},
    theme::{Theme, ThemeName},
    update, workspace,
};

actions!(settings_window, [CloseWindow]);

const TITLEBAR: f32 = 40.;

pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("cmd-w", CloseWindow, Some("Settings"))]
}

/// Opens the window, or brings the open one forward.
pub fn open(cx: &mut App) {
    if let Some(open) = cx
        .windows()
        .iter()
        .find_map(AnyWindowHandle::downcast::<SettingsWindow>)
    {
        let _ = open.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(480.), px(720.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.), px(13.))),
        }),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let this = SettingsWindow::new(window, cx);
            window.focus(&this.focus_handle);
            this
        })
    });
    if let Err(err) = opened {
        log::error!("failed to open settings: {err:#}");
    }
}

pub struct SettingsWindow {
    focus_handle: FocusHandle,
    /// What `config.toml` holds, not the live zoom.
    file: Settings,
    font_family: Entity<TextField>,
    /// `keymap.toml` resolved over the defaults: which keys run which command.
    keys: Vec<(String, Command)>,
    /// The command whose next keypress is being recorded.
    recording: Option<Command>,
    /// Swallows every keypress while recording, so `cmd-w` is read and not acted on.
    recorder: Option<Subscription>,
    /// Last problem reading or writing a file.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (file, error) = match config::read_settings() {
            Ok(file) => (file, None),
            Err(err) => (cx.global::<Settings>().clone(), Some(err.into())),
        };
        let font_family =
            cx.new(|cx| TextField::new(&file.font_family, window, cx).committing_on_blur());
        let subscriptions = vec![cx.subscribe_in(&font_family, window, Self::font_family_edited)];
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            file,
            font_family,
            keys: Vec::new(),
            recording: None,
            recorder: None,
            error,
            _subscriptions: subscriptions,
        };
        this.load_keys();
        this
    }

    fn write(&mut self, key: &str, value: &toml::Value, cx: &mut Context<Self>) {
        self.applied(config::write_setting(key, value), cx);
    }

    fn applied(&mut self, written: Result<Settings, String>, cx: &mut Context<Self>) {
        match written {
            Ok(file) => {
                self.file = file;
                self.error = None;
                workspace::reload_config(cx);
            }
            Err(err) => self.error = Some(err.into()),
        }
        cx.notify();
    }

    fn font_family_edited(
        &mut self,
        field: &Entity<TextField>,
        event: &FieldEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let FieldEvent::Confirmed(text) = event else {
            return;
        };
        match text.trim() {
            "" => self.error = Some("Font family can't be empty.".into()),
            family => self.write("font_family", &family.into(), cx),
        }
        let shown = self.file.font_family.clone();
        field.update(cx, |field, cx| field.set_text(&shown, cx));
    }

    /// `keymap.toml` resolved over the defaults.
    fn load_keys(&mut self) {
        match config::read_keymap() {
            Ok(text) => self.keys = keymap::resolve(text.as_deref()).0,
            Err(err) => self.error = Some(err.into()),
        }
    }

    /// Rewrites `keymap.toml` with `edit`, then shows and applies what it now holds.
    fn write_keymap(&mut self, edit: impl FnOnce(&str) -> String, cx: &mut Context<Self>) {
        match config::update_keymap(edit) {
            Ok(()) => {
                self.error = None;
                self.load_keys();
                workspace::reload_config(cx);
            }
            Err(err) => self.error = Some(err.into()),
        }
        cx.notify();
    }

    /// Waits for a keypress and saves it as `command`'s shortcut. Keys are read, not acted
    /// on, so `cmd-w` can be recorded without closing the window. Clicking the row that is
    /// already listening puts it back.
    fn record(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.recording == Some(command) {
            self.recording = None;
            self.recorder = None;
            cx.notify();
            return;
        }
        self.recording = Some(command);
        self.recorder = Some(cx.intercept_keystrokes(|event, window, cx| {
            if let Some(handle) = window.window_handle().downcast::<SettingsWindow>() {
                let keystroke = event.keystroke.clone();
                let _ = handle.update(cx, |this, _, cx| this.recorded(&keystroke, cx));
            }
            cx.stop_propagation();
        }));
        cx.notify();
    }

    /// A key arrived while recording.
    fn recorded(&mut self, event: &Keystroke, cx: &mut Context<Self>) {
        let Some(command) = self.recording else {
            return;
        };
        // A modifier on its own arrives as a keystroke of its own; the shortcut is the key
        // pressed with it.
        let Some(keys) = keymap::keystroke_of(event) else {
            return;
        };
        self.recording = None;
        self.recorder = None;
        let taken = self
            .keys
            .iter()
            .find(|(bound, other)| bound == &keys && *other != command)
            .map(|(_, other)| *other);
        match taken {
            Some(taken) => {
                self.error = Some(
                    format!("{} is already {}.", keymap::display(&keys), taken.title()).into(),
                );
                cx.notify();
            }
            None => self.write_keymap(|text| keymap::rebind(text, command, &keys), cx),
        }
    }

    /// Drops the user's shortcuts for `command`, so its defaults run again.
    fn reset_key(&mut self, command: Command, cx: &mut Context<Self>) {
        self.recording = None;
        self.recorder = None;
        self.write_keymap(|text| keymap::clear_command(text, command), cx);
    }

    fn step_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let size = (f32::from(self.file.font_size) + delta).clamp(6., 72.);
        self.write("font_size", &f64::from(size).into(), cx);
    }

    fn step_tab_size(&mut self, delta: i64, cx: &mut Context<Self>) {
        let size = (i64::try_from(self.file.tab_size).unwrap_or(4) + delta).clamp(1, 16);
        self.write("tab_size", &size.into(), cx);
    }

    fn step_sidebar_opacity(&mut self, up: bool, cx: &mut Context<Self>) {
        let opacity = step_percent(sidebar_opacity(&self.file), up);
        self.write("sidebar_opacity", &f64::from(opacity).into(), cx);
    }

    fn unset(&mut self, key: &str, cx: &mut Context<Self>) {
        self.applied(config::unset_setting(key), cx);
    }

    /// Opens a config file in an editor tab of the main window, creating it if needed.
    fn edit_file(&mut self, name: &str, cx: &mut Context<Self>) {
        let path = if name == "keymap.toml" {
            config::keymap_file()
        } else {
            crate::settings::config_dir()
                .ok_or_else(|| "no home directory".to_owned())
                .and_then(|dir| {
                    let path = dir.join(name);
                    if !path.exists() {
                        std::fs::create_dir_all(&dir)
                            .and_then(|()| std::fs::write(&path, ""))
                            .map_err(|err| format!("{name}: {err}"))?;
                    }
                    Ok(path)
                })
        };
        match path {
            Ok(path) => workspace::open_in_window(vec![path], cx),
            Err(err) => self.error = Some(err.into()),
        }
        cx.notify();
    }
}

/// The sidebar opacity in effect: the file's, else the theme's.
fn sidebar_opacity(file: &Settings) -> f32 {
    file.sidebar_opacity
        .unwrap_or_else(|| Theme::named(file.theme).sidebar_bg.a)
}

/// `opacity` moved one 5 % step up or down to a multiple of 5 %, within 0–1.
fn step_percent(opacity: f32, up: bool) -> f32 {
    let percent = (opacity * 100.).round();
    let stepped = if up {
        (percent / 5.).floor() * 5. + 5.
    } else {
        (percent / 5.).ceil() * 5. - 5.
    };
    (stepped / 100.).clamp(0., 1.)
}

/// A theme's card: its background, its accent and a run of its code colors.
fn swatch(name: ThemeName, chosen: bool, c: Colors) -> Stateful<Div> {
    let theme = Theme::named(name);
    let strip =
        [1, 2, 3, 4, 5, 6].map(|index| div().size(px(7.)).rounded_full().bg(theme.ansi[index]));
    div()
        .id(name.key())
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .child(
            div()
                .w(px(64.))
                .h(px(44.))
                .p(px(7.))
                .flex()
                .flex_col()
                .justify_between()
                .rounded_md()
                .border_2()
                .border_color(if chosen { c.accent } else { c.divider })
                .bg(theme.editor_bg)
                .child(div().w(px(26.)).h(px(4.)).rounded_full().bg(theme.accent))
                .child(div().flex().gap(px(2.)).children(strip)),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(if chosen { c.text } else { c.muted })
                .child(name.label()),
        )
}

/// Theme colors copied out so `cx` stays free for listeners while rendering; the update
/// window draws with the same ones.
#[derive(Clone, Copy)]
pub struct Colors {
    pub bg: Hsla,
    pub card: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub hover: Hsla,
    pub off: Hsla,
    pub accent: Hsla,
    pub divider: Hsla,
    pub error: Hsla,
}

impl Colors {
    pub fn of(theme: &Theme) -> Self {
        Self {
            bg: theme.editor_bg,
            card: theme.hover,
            text: theme.chrome_text,
            muted: theme.muted,
            hover: theme.selected,
            off: theme.selected,
            accent: theme.accent,
            divider: theme.divider,
            error: theme.failure,
        }
    }
}

fn section(title: &'static str, c: Colors, rows: Vec<AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .px_1()
                .text_size(px(11.))
                .text_color(c.muted)
                .child(title),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .px_3()
                .rounded_lg()
                .bg(c.card)
                .children(rows),
        )
}

fn row(
    label: &'static str,
    hint: Option<&'static str>,
    c: Colors,
    control: impl IntoElement,
) -> AnyElement {
    div()
        .h(px(44.))
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .child(div().truncate().child(label))
                .children(hint.map(|hint| {
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(c.muted)
                        .child(hint)
                })),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    c: Colors,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .min_w(px(24.))
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(c.divider)
        .cursor_pointer()
        .hover(|style| style.bg(c.hover))
        .child(label.into())
}

fn stepper(
    id: &'static str,
    value: String,
    c: Colors,
    cx: &mut Context<SettingsWindow>,
    step: fn(&mut SettingsWindow, bool, &mut Context<SettingsWindow>),
) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            button(ElementId::Name(format!("{id}-less").into()), "−", c)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| step(this, false, cx))),
        )
        .child(div().w(px(44.)).flex().justify_center().child(value))
        .child(
            button(ElementId::Name(format!("{id}-more").into()), "+", c)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| step(this, true, cx))),
        )
}

/// A command and its keys. The key box is a button: press it, then press the keys.
fn key_row(
    command: Command,
    keys: &[&str],
    recording: Option<Command>,
    c: Colors,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let name = command.key();
    let listening = recording == Some(command);
    let defaults: Vec<String> = command
        .default_keys()
        .iter()
        .map(|keys| keymap::display(keys))
        .collect();
    let shown: Vec<String> = keys.iter().map(|keys| keymap::display(keys)).collect();
    let changed = shown != defaults;
    let keys_box = div()
        .id(ElementId::Name(format!("keys-{name}").into()))
        .h(px(22.))
        .min_w(px(58.))
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .border_1()
        .border_color(if listening { c.accent } else { c.divider })
        .cursor_pointer()
        .hover(|style| style.bg(c.hover))
        .text_size(px(12.))
        .text_color(if changed { c.accent } else { c.text })
        .child(match (shown.as_slice(), listening) {
            (_, true) => "Press keys…".to_owned(),
            ([], false) => "None".to_owned(),
            (shown, false) => shown.join(", "),
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.record(command, cx)));
    let reset = button(ElementId::Name(format!("reset-{name}").into()), "Reset", c)
        .text_size(px(11.))
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.reset_key(command, cx)));
    div()
        .h(px(32.))
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child(command.title()),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .child(keys_box)
                .child(
                    div()
                        .w(px(48.))
                        .flex()
                        .justify_end()
                        .when(changed, |row| row.child(reset)),
                ),
        )
        .into_any_element()
}

fn toggle(id: &'static str, on: bool, c: Colors) -> Stateful<Div> {
    div()
        .id(id)
        .w(px(34.))
        .h(px(20.))
        .p(px(2.))
        .flex()
        .rounded_full()
        .cursor_pointer()
        .bg(if on { c.accent } else { c.off })
        .when(on, Styled::justify_end)
        .child(div().size(px(16.)).rounded_full().bg(c.text))
}

impl SettingsWindow {
    fn appearance_rows(&self, c: Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let chosen = self.file.theme;
        let swatches = ThemeName::ALL.map(|name| {
            swatch(name, name == chosen, c).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| this.write("theme", &name.key().into(), cx),
            ))
        });
        let custom = self.file.sidebar_opacity.is_some();
        let percent = (sidebar_opacity(&self.file) * 100.).round();
        vec![
            div()
                .py_3()
                .flex()
                .justify_between()
                .children(swatches)
                .into_any_element(),
            row(
                "Sidebar opacity",
                Some("Lower lets the desktop show through"),
                c,
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(custom, |row| {
                        row.child(
                            button("sidebar-opacity-reset", "Reset", c)
                                .text_size(px(11.))
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.unset("sidebar_opacity", cx);
                                })),
                        )
                    })
                    .child(stepper(
                        "sidebar-opacity",
                        format!("{percent}%"),
                        c,
                        cx,
                        Self::step_sidebar_opacity,
                    )),
            ),
        ]
    }

    fn font_rows(&self, c: Colors, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let focused = self.font_family.focus_handle(cx).is_focused(window);
        vec![
            row(
                "Family",
                Some("A monospace font"),
                c,
                div()
                    .w(px(180.))
                    .h(px(26.))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .border_1()
                    .border_color(if focused { c.accent } else { c.divider })
                    .bg(c.bg)
                    .child(self.font_family.clone()),
            ),
            row(
                "Size",
                Some("⌘= and ⌘- zoom without saving"),
                c,
                stepper(
                    "font-size",
                    f32::from(self.file.font_size).to_string(),
                    c,
                    cx,
                    |this, up, cx| this.step_font_size(if up { 1. } else { -1. }, cx),
                ),
            ),
        ]
    }

    fn editor_rows(&self, c: Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            row(
                "Tab size",
                Some("For files that don't show their own"),
                c,
                stepper(
                    "tab-size",
                    self.file.tab_size.to_string(),
                    c,
                    cx,
                    |this, up, cx| this.step_tab_size(if up { 1 } else { -1 }, cx),
                ),
            ),
            row(
                "Indent with spaces",
                Some("Off indents with tabs"),
                c,
                toggle("insert-spaces", self.file.insert_spaces, c).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        let on = !this.file.insert_spaces;
                        this.write("insert_spaces", &on.into(), cx);
                    },
                )),
            ),
            row(
                "Cursor blink",
                None,
                c,
                toggle("cursor-blink", self.file.cursor_blink, c).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        let on = !this.file.cursor_blink;
                        this.write("cursor_blink", &on.into(), cx);
                    },
                )),
            ),
        ]
    }

    fn update_rows(&self, c: Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            row(
                "Update automatically",
                Some("Downloads in the background, installs when Kiled quits"),
                c,
                toggle("auto-update", self.file.auto_update, c).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        let on = !this.file.auto_update;
                        this.write("auto_update", &on.into(), cx);
                    },
                )),
            ),
            row(
                concat!("Kiled ", env!("CARGO_PKG_VERSION")),
                None,
                c,
                // Called, not dispatched, so it doesn't depend on which window is active.
                button("check-for-updates", "Check Now", c).on_click(|_, _, cx| {
                    update::check_now(cx);
                }),
            ),
        ]
    }

    /// Every command with the keys it runs on, by group: click a shortcut and press the new
    /// one.
    fn keyboard_sections(&self, c: Colors, cx: &mut Context<Self>) -> Vec<Div> {
        Group::ALL
            .iter()
            .map(|group| {
                let rows = Command::all()
                    .filter(|command| command.group() == *group)
                    .map(|command| {
                        let keys = keymap::keys_for(&self.keys, command);
                        key_row(command, &keys, self.recording, c, cx)
                    })
                    .collect();
                section(group.title(), c, rows)
            })
            .collect()
    }

    fn file_rows(c: Colors, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            row(
                "config.toml",
                Some("These settings, and [servers] for language servers"),
                c,
                button("edit-config", "Edit", c).on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| this.edit_file("config.toml", cx)),
                ),
            ),
            row(
                "keymap.toml",
                Some("Every key, including two-stroke ones"),
                c,
                button("edit-keymap", "Edit", c).on_click(
                    cx.listener(|this, _: &ClickEvent, _, cx| this.edit_file("keymap.toml", cx)),
                ),
            ),
            row(
                "Folder",
                Some("~/.config/kiled"),
                c,
                button("open-folder", "Show in Finder", c).on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        if let Err(err) = config::reveal_folder(cx) {
                            this.error = Some(err.into());
                            cx.notify();
                        }
                    },
                )),
            ),
        ]
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::of(cx.global::<Theme>());
        let appearance = self.appearance_rows(c, cx);
        let font = self.font_rows(c, window, cx);
        let editor = self.editor_rows(c, cx);
        let updates = self.update_rows(c, cx);
        let keyboard = self.keyboard_sections(c, cx);
        let files = Self::file_rows(c, cx);
        div()
            .key_context("Settings")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            // A click on empty space ends a field edit (blur confirms it).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, _| {
                    window.focus(&this.focus_handle);
                }),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(c.bg)
            .text_size(px(13.))
            .text_color(c.text)
            .child(
                div()
                    .h(px(TITLEBAR))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(c.muted)
                    .child("Settings"),
            )
            .child(
                div()
                    .id("settings-body")
                    .flex_1()
                    .min_h_0()
                    .px_5()
                    .pb_4()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .min_h(px(16.))
                            .px_1()
                            .text_size(px(11.))
                            .text_color(c.error)
                            .children(self.error.clone()),
                    )
                    .child(section("THEME", c, appearance))
                    .child(section("FONT", c, font))
                    .child(section("EDITOR", c, editor))
                    .child(section("UPDATES", c, updates))
                    .child(section("FILES", c, files))
                    .child(
                        div()
                            .px_1()
                            .text_size(px(11.))
                            .text_color(c.muted)
                            .child("KEYBOARD — VS Code's keys by default. Click a shortcut and press new keys."),
                    )
                    .children(keyboard),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Modifiers, TestAppContext, WindowHandle};

    use super::*;

    fn settings_window(cx: &mut TestAppContext) -> WindowHandle<SettingsWindow> {
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.set_global(Settings::default());
            crate::register_app_actions(cx);
            cx.dispatch_action(&crate::OpenSettings);
        });
        cx.run_until_parked();
        cx.windows()
            .iter()
            .find_map(AnyWindowHandle::downcast::<SettingsWindow>)
            .expect("the Settings window")
    }

    #[test]
    fn opacity_steps_should_land_on_multiples_of_5_percent_within_0_to_1() {
        let steps = [
            (0.72, true),
            (0.72, false),
            (0.7, false),
            (0.98, true),
            (0.02, false),
        ]
        .map(|(opacity, up)| (step_percent(opacity, up) * 100.).round());
        assert_eq!(steps, [75., 70., 65., 100., 0.]);
    }

    #[gpui::test]
    fn a_modifier_on_its_own_should_not_end_recording(cx: &mut TestAppContext) {
        let settings = settings_window(cx);
        let modifier = Keystroke {
            key: "shift".into(),
            key_char: None,
            modifiers: Modifiers::default(),
        };
        settings
            .update(cx, |this, _, cx| {
                this.record(Command::ToggleSidebar, cx);
                this.recorded(&modifier, cx);
                assert_eq!(this.recording, Some(Command::ToggleSidebar));
            })
            .unwrap();
        settings
            .update(cx, |this, _, _| {
                this.recording = None;
                this.recorder = None;
            })
            .unwrap();
    }

    #[gpui::test]
    fn open_settings_should_open_one_window_and_reuse_it(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.set_global(Settings::default());
            crate::register_app_actions(cx);
            cx.dispatch_action(&crate::OpenSettings);
            cx.dispatch_action(&crate::OpenSettings);
        });
        cx.run_until_parked();
        let open = cx
            .windows()
            .iter()
            .filter_map(AnyWindowHandle::downcast::<SettingsWindow>)
            .count();
        assert_eq!(open, 1);
    }
}
