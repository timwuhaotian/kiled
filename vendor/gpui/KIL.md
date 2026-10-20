# gpui 0.2.2, as Kil uses it

This is [gpui](https://crates.io/crates/gpui) 0.2.2 (Apache-2.0, see `LICENSE-APACHE`) from
crates.io, with one change of Kil's (and a fix for newer Rust, below). The root `Cargo.toml` puts it in place of the published
crate with `[patch.crates-io]`. The examples were left out, and so were their `[[example]]`
entries in `Cargo.toml`.

## The change: frames stop while a window has nothing to draw

gpui asks macOS for a frame at every refresh of the display (a `CVDisplayLink`) for as long as
a window is visible, and draws only when something changed. An idle Kil window therefore woke
the main thread 60–120 times a second, which was its whole idle CPU (0.33–0.40 %, DESIGN.md
§6.4, §13). Upstream gpui still works this way.

Now a window that has drawn, presented and run nothing for 8 frames in a row stops its display
link, and starts it again when it becomes dirty, when a callback is scheduled for the next frame
(`on_next_frame`, and with it `request_animation_frame`), or on input (so the scene is still
presented for the second after it, which keeps a ProMotion display from slowing down). The link
is stopped and started, never released, so no late callback can outlive it.

Every changed place is marked `Modified for Kil`:

- `src/platform.rs`: `PlatformWindow::frame_waker` and `pause_frames`, which do nothing by
  default, so only macOS changes.
- `src/platform/mac/window.rs`: the macOS window pauses and resumes its display link.
- `src/platform/mac/display_link.rs`: `start` and `stop` can be called again, keeping the
  dispatch source's resumes and suspends paired.
- `src/window.rs`: `WindowInvalidator` wakes the platform when the window becomes dirty; the
  request-frame callback counts idle frames and pauses; `on_next_frame` and `dispatch_event`
  wake it.

Besides, `src/taffy.rs` types two float literals (`0.0_f32`, `1.0_f32`): as a path dependency
gpui's warnings show, and Rust 1.94 warns that the untyped ones will become an error.

## Updating gpui

Copy the new version's crate from `~/.cargo/registry/src/*/gpui-<version>` here, without
`examples/` and its `[[example]]` entries, and carry the change over by searching the old copy
for `Modified for Kil`. Then measure idle CPU again as DESIGN.md §6.4 describes.
