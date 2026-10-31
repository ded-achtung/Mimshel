# The platform boundary (Wayland plan, phase 1)

> **Status: done (2026-10-31)**, branch `platform/boundary`. Every part goes
> through `apps/mimshel/src/platform.rs`; `only_the_door_names_the_backend`
> keeps it so. The lock screen's crash test is in the table below.

Phase 1 of `docs/wayland-plan.md`: the shell's parts stop talking to X11
directly and go through one door, so a Wayland backend can later stand
behind the same door. **No behaviour changes** in this phase: every step must
leave the X11 session exactly as it was.

## Shape

| Piece | What it is |
|---|---|
| `crates/platform` (`mimshel-platform`) | What parts know about the screen whatever draws it: `Event`, `Property`, `Key`/`KeyPress`, `Error`/`Result`. No display protocol. |
| `crates/x11` (`mimshel-x11`) | The X11 backend. Produces the `mimshel-platform` events. |
| `apps/mimshel/src/platform.rs` | The door: `Display`, `AppWindow`, … wrapping the backend. Today each wraps the X11 type; on Wayland each becomes an enum of two backends, chosen at start. |
| `apps/mimshel/src/*_ui.rs` | The parts. Use `crate::platform` only. |

Why wrappers and not traits: the parts stay non-generic (a trait with
associated types would spread type parameters through 10 000 lines), and the
choice between X11 and Wayland is made at run time, by the session.

## Rules for moving a part

1. A part imports from `crate::platform`, never `mimshel_x11`. Code not moved
   yet reaches X11 through `platform::Display::x11()`; that accessor goes away
   when the last part has moved.
2. `platform.rs` holds only what moved parts use. A part that moves adds the
   methods it needs, each a thin call into the backend.
3. **Name methods for what the part needs, not for how X11 does it**, and say
   in the doc comment what it becomes on Wayland when that is not obvious
   (see the table below). Do not expose X11 ideas: the root window, atoms,
   EWMH property names, `xfwm4`. Where a part asks "is this event about the
   whole screen?" it asks the door (`is_screen(window)`), not `window == root`.
4. Same behaviour: same calls into the backend in the same order. If moving a
   part seems to need a behaviour change, stop and ask.
5. Each part is its own commit: `cargo fmt`, `clippy -D warnings`, all tests,
   the X11 tests (`cargo test -p mimshel-x11 --test windows -- --ignored`).
   Then a check on the laptop.
6. The lock screen (`lock_ui.rs`) is part of the lock path: after moving it,
   the crash test of CLAUDE.md lesson 1 (`kill -ABRT` the lock screen, the
   screen must stay locked).
7. **The door costs nothing** (the user's requirement): no allocations, copies,
   clones, extra system calls or extra round trips to the server on top of the
   backend's own; no state kept in the door; nothing cached that can grow. A
   method that has to compose several backend calls (`raise_app_window`) makes
   the same calls the part made before. After a part moves, compare idle CPU and
   memory on the laptop with the build before it (`tools/measure-service.sh`):
   PSS, threads and open files must stay flat over minutes.

## Cost

`tools/measure-service.sh` on the laptop, service idle, 60 s (2026-10-31):

| Build | Idle CPU | PSS | Threads | Open files |
|---|---|---|---|---|
| v0.0.6 (no door) | 0.17 % | 32.8 MB, flat | 34 | 20 |
| `platform/boundary`, parts 1–7 | 0.17 % | 31.7 MB, flat | 34 | 20 |
| `platform/boundary`, parts 1–8, 180 s | 0.16 % | 31.8 MB, flat | 34 | 20 |
| `platform/boundary`, parts 1–9 | 0.17 % | 31.9 MB, flat | 34 | 20 |

Under load (parts 1–8, three rounds of 25 × open and close a folder window,
Search, Apps, one notification): 31.8 → 32.6 → 33.0 → 33.1 MB, threads 34 and
open files 20 throughout, the same service process. The growth stops once the
notification history (50) and the icon caches are full: no leak.

## Order

| # | Part | Used from `mimshel_x11` | Done by |
|---|---|---|---|
| 1 | `transfer_ui` | `AppWindow`; `create_app_window`, `active_window`, `scale`, `screen_size`, `sync` | Claude |
| 2 | `about_ui` | `AppWindow`; `activate`, `active_window` | Codex |
| 3 | `settings_ui` (own process) | `AppWindow`; `activate`, `next_event`, `window_info` | Codex |
| 4 | `notify_ui` | `Popup`; `create_popup`, `primary_area` | Codex |
| 5 | `term_ui` (own process) | `AppWindow`, clipboard text; `modifiers`, `watch_root`, `capture_window` | Codex |
| 6 | `menubar_ui` | `DockWindow` (as the bar), `Popup`; other programs' windows, keyboard layouts, pointer grab for drags | Codex |
| 7 | `dock_ui` | `DockWindow`, `Popup`; other programs' windows (list, icons, titles, activate, minimize, close, new-window watching) | Codex |
| 8 | `desktop_ui` | `DesktopWindow`, `Popup`, monitors, work area, drag and drop, clipboard files | Codex |
| 9 | `folder_ui` | `AppWindow`, `Popup`, `DragTarget`, drag and drop, clipboard files | Codex |
| 10 | `ui.rs` | Search and Apps popups, `capture_screen` (Apps backdrop) | Claude |
| 11 | `lock_ui` (own process) | `Popup` as a lock surface (`show_lock`, `regrab`), `restore_focus`, `watch_stacking` | Claude |
| 12 | finish | `Display::x11()` removed; `main.rs` and `guard.rs` (`ServerWatch`, `system_scale`) moved too; the test `only_the_door_names_the_backend` | Claude |

## What things become on Wayland

| Door (X11 today) | Wayland |
|---|---|
| `AppWindow` | `xdg_toplevel` |
| `Popup` | `xdg_popup`, or a layer-shell overlay for full-screen ones (Search, Apps) |
| `Panel` as the menu bar or the dock, reserved space | `wlr-layer-shell` surface, exclusive zone |
| `Background` (X11 `DesktopWindow`) | layer-shell background layer |
| `SelectionOverlay` (X11 `SelectionRenderer`, an XRender picture with XFixes shaping) | small overlay surface, or drawn by the compositor |
| lock screen popup (`show_lock`, grabs) | `ext-session-lock-v1` |
| other programs' windows: list, active, title, icon, activate, minimize, close | `ext-foreign-toplevel-list` + `wlr-foreign-toplevel-management` (our compositor implements them) |
| new program windows and their first frame (`watch_new_windows`, `undrawn_over`) | our compositor tells the shell directly |
| monitors, primary area, work area | outputs from the compositor; work area = output minus exclusive zones |
| `watch_desktop` (active window, window list, work area, screen/monitor changes) | desktop state and output changes from our compositor |
| keyboard layouts and groups | the compositor's xkb keymap |
| clipboard files (`copy_files`, `clipboard_files`) | `wl_data_device` with `text/uri-list` and `x-special/gnome-copied-files` |
| drag and drop (`drag_begin`, `drag_motion`, `drag_end`, `take_drop`) | `wl_data_device` / `wl_data_source` |
| clipboard text | `wl_data_device` |
| `capture_screen` / `capture_window` | the compositor renders them (screencopy) |
| `origin` of our own window | not known to a Wayland client; only our own compositor can tell |
