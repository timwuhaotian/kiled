//! Kiled ▸ Check for Updates…: a small window that follows the check from start to end
//! (DESIGN.md §13), so a click is answered at once and a download shows how far it has got.

use std::{sync::Arc, time::Duration};

use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, Bounds, ClickEvent, Context,
    Div, FocusHandle, Focusable, FontWeight, Hsla, KeyBinding, SharedString, Stateful, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, point, prelude::*, px,
    relative, size,
};

use crate::{
    settings_window::{Colors, button},
    theme::Theme,
    update::{self, Progress, Step},
};

actions!(update_window, [Dismiss, Confirm]);

const WIDTH: f32 = 360.;
const HEIGHT: f32 = 156.;
/// Of the track, for the bar that sweeps while the size of the work isn't known.
const SWEEP_WIDTH: f32 = 0.35;
/// One pass of that bar.
const SWEEP: Duration = Duration::from_millis(1200);
/// A finished check's mark popping in.
const MARK: Duration = Duration::from_millis(160);

pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-w", Dismiss, Some("UpdateWindow")),
        KeyBinding::new("escape", Dismiss, Some("UpdateWindow")),
        KeyBinding::new("enter", Confirm, Some("UpdateWindow")),
    ]
}

/// What the window says.
pub enum Shown {
    /// A check under way, read from its thread.
    Working(Arc<Progress>),
    /// The version that installs when Kiled quits.
    Ready(String),
    UpToDate,
    Failed(String),
}

/// Shows `shown` in the update window, opening it if it isn't open.
pub fn show(shown: Shown, cx: &mut App) {
    if let Some(open) = cx
        .windows()
        .iter()
        .find_map(AnyWindowHandle::downcast::<UpdateWindow>)
    {
        // Deferred: chosen from the menu with this window in front, the action runs inside
        // its update, and a window can't be updated again from inside its own.
        cx.defer(move |cx| {
            let _ = open.update(cx, |this, window, cx| {
                this.show(shown, cx);
                window.activate_window();
            });
        });
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(WIDTH), px(HEIGHT)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Software Update".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.), px(13.))),
        }),
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        cx.new(|cx| {
            let mut this = UpdateWindow {
                focus_handle: cx.focus_handle(),
                shown: Shown::UpToDate,
                step: Step::Checking,
                poll: None,
            };
            this.show(shown, cx);
            window.focus(&this.focus_handle);
            this
        })
    });
    if let Err(err) = opened {
        log::error!("failed to open the update window: {err:#}");
    }
}

pub struct UpdateWindow {
    focus_handle: FocusHandle,
    shown: Shown,
    /// The working check's step, as last read.
    step: Step,
    /// Reads the working check's step until it ends; idle otherwise.
    poll: Option<Task<()>>,
}

impl Focusable for UpdateWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl UpdateWindow {
    fn show(&mut self, shown: Shown, cx: &mut Context<Self>) {
        self.poll = match &shown {
            Shown::Working(progress) => Some(self.poll(progress.clone(), cx)),
            _ => None,
        };
        self.shown = shown;
        cx.notify();
    }

    fn poll(&mut self, progress: Arc<Progress>, cx: &mut Context<Self>) -> Task<()> {
        self.observe(progress.step(), cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(update::POLL).await;
                let step = progress.step();
                if this.update(cx, |this, cx| this.observe(step, cx)).is_err() {
                    return;
                }
            }
        })
    }

    fn observe(&mut self, step: Step, cx: &mut Context<Self>) {
        if step != self.step {
            self.step = step;
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.shown, Shown::Ready(_)) {
            update::restart(cx);
        } else {
            window.remove_window();
        }
    }

    /// The download bar's fill, from 0 to 1, while a download of a known size is under way.
    fn bar(&self) -> Option<f32> {
        match (&self.shown, &self.step) {
            (
                Shown::Working(_),
                Step::Downloading {
                    got,
                    total: Some(total),
                    ..
                },
            ) => Some(fraction(*got, *total)),
            _ => None,
        }
    }

    /// The headline and the line under it.
    fn words(&self) -> (SharedString, SharedString) {
        match (&self.shown, &self.step) {
            (Shown::Working(_), Step::Checking) => (
                "Checking for updates…".into(),
                concat!("You have Kiled ", env!("CARGO_PKG_VERSION"), ".").into(),
            ),
            (
                Shown::Working(_),
                Step::Downloading {
                    version,
                    got,
                    total,
                },
            ) => (
                format!("Downloading Kiled {version}…").into(),
                match total {
                    Some(total) => format!("{} of {}", megabytes(*got), megabytes(*total)),
                    None => megabytes(*got),
                }
                .into(),
            ),
            (Shown::Working(_), Step::Verifying { version }) => (
                format!("Verifying Kiled {version}…").into(),
                "Checking its signature and Apple's notarization.".into(),
            ),
            (Shown::Ready(version), _) => (
                format!("Kiled {version} is ready").into(),
                "Restart now, or it installs when Kiled quits.".into(),
            ),
            (Shown::UpToDate, _) => (
                "Kiled is up to date".into(),
                concat!(env!("CARGO_PKG_VERSION"), " is the newest version.").into(),
            ),
            (Shown::Failed(err), _) => ("Kiled can't update right now".into(), err.clone().into()),
        }
    }

    /// The bar while working, a mark once done.
    fn indicator(&self, c: Colors) -> AnyElement {
        let track = div()
            .relative()
            .w_full()
            .h(px(4.))
            .rounded_full()
            .overflow_hidden()
            .bg(c.off);
        match (&self.shown, self.bar()) {
            (Shown::Working(_), Some(fill)) => track
                .child(div().h_full().w(relative(fill)).rounded_full().bg(c.accent))
                .into_any_element(),
            (Shown::Working(_), None) => track
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .h_full()
                        .w(relative(SWEEP_WIDTH))
                        .rounded_full()
                        .bg(c.accent)
                        .with_animation(
                            "update-sweep",
                            Animation::new(SWEEP).repeat().with_easing(ease_in_out),
                            |bar, t| bar.left(relative(t * (1. + SWEEP_WIDTH) - SWEEP_WIDTH)),
                        ),
                )
                .into_any_element(),
            (Shown::Ready(_) | Shown::UpToDate, _) => mark("✓", c.accent, c),
            (Shown::Failed(_), _) => mark("!", c.error, c),
        }
    }

    fn buttons(&self, c: Colors, cx: &mut Context<Self>) -> Div {
        let dismiss =
            |id, label| button(id, label, c).on_click(|_, window, _| window.remove_window());
        let row = div().flex().justify_end().gap_2();
        match self.shown {
            Shown::Working(_) => row.child(dismiss("update-hide", "Hide")),
            Shown::Ready(_) => row.child(dismiss("update-later", "Later")).child(
                primary("update-restart", "Restart Now", c)
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| update::restart(cx))),
            ),
            Shown::UpToDate | Shown::Failed(_) => row.child(dismiss("update-ok", "OK")),
        }
    }
}

/// A finished check's mark, popped in once.
fn mark(glyph: &'static str, color: Hsla, c: Colors) -> AnyElement {
    div()
        .size(px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(color)
        .text_size(px(13.))
        .text_color(c.bg)
        .child(glyph)
        .with_animation(
            "update-mark",
            Animation::new(MARK).with_easing(ease_out_quint),
            |mark, t| mark.opacity(t).size(px(22. * (0.6 + 0.4 * t))),
        )
        .into_any_element()
}

/// The answer Return gives, filled like a switch that is on; `button`'s hover would hide it.
fn primary(id: &'static str, label: &'static str, c: Colors) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(c.accent)
        .text_color(c.bg)
        .cursor_pointer()
        .hover(|style| style.opacity(0.85))
        .child(label)
}

fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    if t < 0.5 {
        2. * t * t
    } else {
        1. - (-2. * t + 2.).powi(2) / 2.
    }
}

fn ease_out_quint(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(5)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a ratio for a bar, which needs no more than f32's 24 bits"
)]
fn fraction(got: u64, total: u64) -> f32 {
    (got as f32 / total as f32).clamp(0., 1.)
}

/// Decimal, as Finder counts.
#[expect(
    clippy::cast_precision_loss,
    reason = "shown to one decimal; a download is far below 2^52 bytes"
)]
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1e6)
}

impl Render for UpdateWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = Colors::of(cx.global::<Theme>());
        let (headline, detail) = self.words();
        let working = matches!(self.shown, Shown::Working(_));
        div()
            .key_context("UpdateWindow")
            .track_focus(&self.focus_handle)
            .on_action(|_: &Dismiss, window, _| window.remove_window())
            .on_action(cx.listener(Self::confirm))
            .size_full()
            .flex()
            .flex_col()
            .pt(px(36.))
            .px_5()
            .pb_4()
            .gap_3()
            .bg(c.bg)
            .text_size(px(13.))
            .text_color(c.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!working, |head| head.child(self.indicator(c)))
                    .child(div().font_weight(FontWeight::MEDIUM).child(headline)),
            )
            .when(working, |body| body.child(self.indicator(c)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .text_size(px(11.))
                    .text_color(c.muted)
                    .line_clamp(3)
                    .child(detail),
            )
            .child(self.buttons(c, cx))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, WindowHandle};

    use super::*;

    fn open_windows(cx: &mut TestAppContext) -> Vec<WindowHandle<UpdateWindow>> {
        cx.windows()
            .iter()
            .filter_map(AnyWindowHandle::downcast::<UpdateWindow>)
            .collect()
    }

    fn headline(window: WindowHandle<UpdateWindow>, cx: &mut TestAppContext) -> String {
        window
            .update(cx, |this, _, _| this.words().0.to_string())
            .unwrap()
    }

    #[gpui::test]
    fn the_window_should_follow_the_check_to_its_end(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::default());
            cx.bind_keys(bindings());
        });
        let progress = Arc::new(Progress::default());
        cx.update(|cx| show(Shown::Working(progress.clone()), cx));
        cx.run_until_parked();
        let [window] = open_windows(cx)[..] else {
            panic!("one update window");
        };
        assert_eq!(headline(window, cx), "Checking for updates…");

        progress.set(Step::Downloading {
            version: "9.9.9".into(),
            got: 1_500_000,
            total: Some(3_000_000),
        });
        cx.executor().advance_clock(update::POLL);
        cx.run_until_parked();
        let (words, bar) = window
            .update(cx, |this, _, _| (this.words(), this.bar()))
            .unwrap();
        assert_eq!(
            (words.0.as_ref(), words.1.as_ref(), bar),
            ("Downloading Kiled 9.9.9…", "1.5 MB of 3.0 MB", Some(0.5))
        );

        // Shown again while open: the same window, now saying the update is ready.
        cx.update(|cx| show(Shown::Ready("9.9.9".into()), cx));
        cx.run_until_parked();
        assert_eq!(open_windows(cx).len(), 1);
        assert_eq!(headline(window, cx), "Kiled 9.9.9 is ready");
        assert!(
            window
                .update(cx, |this, _, _| this.poll.is_none() && this.bar().is_none())
                .unwrap()
        );

        // Later: the window goes, and the update waits for Kiled to quit.
        window
            .update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(Dismiss), cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(open_windows(cx).len(), 0);
    }

    #[test]
    fn sizes_should_read_as_finder_shows_them() {
        assert_eq!(
            (megabytes(0), megabytes(4_570_610), fraction(5, 4)),
            ("0.0 MB".to_owned(), "4.6 MB".to_owned(), 1.)
        );
    }
}
