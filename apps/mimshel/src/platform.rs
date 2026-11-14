//! The shell's one door to the screen (docs/platform.md). Shell parts
//! (`*_ui.rs`) reach windows, input and outputs through this module only,
//! never through `mimshel_x11` or `mimshel_wayland`. The backend is chosen
//! once, when the display is connected: Wayland when `WAYLAND_DISPLAY` is
//! set (inside our compositor), X11 otherwise; `MIMSHEL_BACKEND=x11|wayland`
//! decides by hand. The parts do not change (docs/wayland-plan.md).
//! On Wayland only what one app window needs exists so far (step 2.4 of
//! docs/compositor.md); the rest answers "nothing" there, or an error for
//! calls that create something, until it gets its Wayland side.
//!
//! Methods are named for what a part needs, not for how X11 does it, and
//! each says what it maps to on Wayland when that is not obvious. Only
//! what the parts use is here, and nothing else in the program names
//! `mimshel_x11` (a test below checks it). The door costs nothing: thin
//! calls, no state, no copies (docs/platform.md, rule 7).

pub use mimshel_core::windows::WindowInfo;
pub use mimshel_platform::{Capability, DragTarget, Event, Key, KeyPress, Property, Result};
use mimshel_render::Image;

/// The connection to the screen: one per process.
pub struct Display {
    backend: Backend,
}

enum Backend {
    X11(mimshel_x11::Display),
    Wayland(mimshel_wayland::Display),
}

/// What a door call answers on Wayland while it has no Wayland side yet.
const NOT_ON_WAYLAND: &str = "not available on Wayland yet";

/// What the Wayland side cannot do yet: every door call that answers
/// "nothing" there names one of these, and a test below keeps the list and
/// the calls the same. A capability leaves the list when the last of its
/// calls has its Wayland side.
const WAYLAND_LACKS: &[Capability] = &[
    Capability::DesktopLayer,
    Capability::ScreenArea,
    Capability::DragAndDrop,
    Capability::OtherWindows,
    Capability::KeyboardLayouts,
    Capability::PointerPosition,
    Capability::ScreenCapture,
];

/// Calls the X11 backend; on Wayland gives `$none` until that call has its
/// Wayland side, and `$lacks` names the [`Capability`] that is missing
/// there for it. Harmless actions (watching, activating, grabbing) answer
/// `Ok` there and do nothing, so a part keeps running; calls that would
/// create something answer an error through [`Display::x11`].
macro_rules! x11_or {
    ($d:expr, $lacks:ident, $none:expr, |$x:ident| $body:expr) => {
        match &$d.backend {
            Backend::X11($x) => $body,
            Backend::Wayland(_) => {
                const _: Capability = Capability::$lacks;
                $none
            }
        }
    };
}

/// Calls a wrapper's own backend with the matching display: `$x11` with the
/// X11 object and display, `$wl` with the Wayland ones; a wrapper and a
/// display of different backends cannot meet (one display per process), so
/// that case is an error.
macro_rules! on_backend {
    ($obj:expr, $kind:ident, $d:expr, |$o:ident, $disp:ident| $x11:expr, $wl:expr) => {
        match ($obj, &$d.backend) {
            ($kind::X11($o), Backend::X11($disp)) => $x11,
            ($kind::Wayland($o), Backend::Wayland($disp)) => $wl,
            _ => Err(NOT_ON_WAYLAND.into()),
        }
    };
}

impl Display {
    pub fn connect() -> Result<Self> {
        let wanted = mimshel_core::names::env("BACKEND").and_then(|v| v.into_string().ok());
        let wayland = match wanted.as_deref() {
            Some("wayland") => true,
            Some("x11") => false,
            _ => std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()),
        };
        let backend = if wayland {
            Backend::Wayland(mimshel_wayland::Display::connect()?)
        } else {
            Backend::X11(mimshel_x11::Display::connect()?)
        };
        Ok(Self { backend })
    }

    /// The X11 display, or an error on Wayland (for calls with no Wayland
    /// side yet): `lacks` is the capability missing there.
    fn x11(&self, lacks: Capability) -> Result<&mimshel_x11::Display> {
        match &self.backend {
            Backend::X11(x) => Ok(x),
            Backend::Wayland(_) => Err(format!("{NOT_ON_WAYLAND}: {lacks:?}").into()),
        }
    }

    /// Whether this screen can do `what`. A part that offers something asks
    /// this — what the screen can do, never which screen it is — so that a
    /// control with nothing behind it is not shown. X11 can do all of it;
    /// Wayland cannot do what is in [`WAYLAND_LACKS`] yet.
    pub fn can(&self, what: Capability) -> bool {
        match &self.backend {
            Backend::X11(_) => true,
            Backend::Wayland(_) => !WAYLAND_LACKS.contains(&what),
        }
    }

    /// The wallpaper and icons layer. Wayland: layer-shell background layer.
    pub fn create_background(&self) -> Result<Background> {
        Ok(Background(
            self.x11(Capability::DesktopLayer)?.create_desktop()?,
        ))
    }

    /// Whether other programs have desktop windows (a video wallpaper),
    /// and those of them above our background `ours` (`None` while the
    /// window manager has not taken ours). Wayland: other clients'
    /// background-layer surfaces, which our compositor orders (none yet).
    pub fn foreign_desktops(&self, ours: u32) -> (bool, Option<Vec<u32>>) {
        x11_or!(self, DesktopLayer, (false, Some(Vec::new())), |xd| xd
            .foreign_desktops(ours))
    }

    /// Output rectangles in points. Wayland: outputs from our compositor.
    pub fn monitors(&self) -> Vec<(i32, i32, u32, u32)> {
        match &self.backend {
            Backend::X11(x) => x.monitors(),
            Backend::Wayland(w) => w.monitors(),
        }
    }

    /// Usable output area in points. Wayland: output minus exclusive zones.
    pub fn workarea(&self) -> Option<(i32, i32, u32, u32)> {
        x11_or!(self, ScreenArea, None, |xd| xd.workarea())
    }

    /// Screen size in physical pixels. Wayland: compositor output bounds.
    pub fn screen_pixels(&self) -> (u16, u16) {
        x11_or!(self, ScreenArea, (0, 0), |xd| xd.screen_pixels())
    }

    /// Begin offering files. Wayland: `wl_data_device` / `wl_data_source`.
    pub fn drag_begin(&self, source: u32, paths: &[std::path::PathBuf]) {
        x11_or!(self, DragAndDrop, (), |xd| xd.drag_begin(source, paths));
    }

    /// Update a file drag. Wayland: `wl_data_device` / `wl_data_source`.
    pub fn drag_motion(&self, x: i16, y: i16, copy: bool) -> DragTarget {
        x11_or!(self, DragAndDrop, DragTarget::Nothing, |xd| xd
            .drag_motion(x, y, copy))
    }

    /// End a file drag. Wayland: `wl_data_device` / `wl_data_source`.
    pub fn drag_end(&self) -> DragTarget {
        x11_or!(self, DragAndDrop, DragTarget::Nothing, |xd| xd.drag_end())
    }

    /// Take received file paths. Wayland: read the `wl_data_device` offer.
    pub fn take_drop(&self, ticket: u64) -> Vec<std::path::PathBuf> {
        x11_or!(self, DragAndDrop, Vec::new(), |xd| xd.take_drop(ticket))
    }

    /// A drop agreed as a move is carried out as a copy (another disk).
    /// Wayland: the same, `wl_data_offer.set_actions` before `finish`.
    pub fn drop_copied(&self, ticket: u64) {
        x11_or!(self, DragAndDrop, (), |xd| xd.drop_copied(ticket));
    }

    /// Report completion after the file operation. Wayland: finish the
    /// `wl_data_offer` with the accepted result.
    pub fn finish_drop(&self, ticket: u64, success: bool) {
        x11_or!(self, DragAndDrop, (), |xd| xd.finish_drop(ticket, success));
    }

    /// Wait for the next event (blocks).
    pub fn next_event(&self) -> Result<Event> {
        match &self.backend {
            Backend::X11(x) => x.next_event(),
            Backend::Wayland(w) => w.next_event(),
        }
    }

    /// Points to pixels: 1 point is this many pixels.
    pub fn scale(&self) -> f32 {
        match &self.backend {
            Backend::X11(x) => x.scale(),
            Backend::Wayland(w) => w.scale(),
        }
    }

    /// Convert points to pixels at the display's scale.
    pub fn px(&self, points: u32) -> u32 {
        match &self.backend {
            Backend::X11(x) => x.px(points),
            Backend::Wayland(w) => w.px(points),
        }
    }

    /// The whole screen in pixels. Wayland: the bounding box of the outputs.
    pub fn screen_size(&self) -> (u16, u16) {
        match &self.backend {
            Backend::X11(x) => x.screen_size(),
            Backend::Wayland(w) => w.screen_size(),
        }
    }

    /// Wait until everything sent so far has been handled. Wayland: a
    /// roundtrip.
    pub fn sync(&self) -> Result<()> {
        match &self.backend {
            Backend::X11(x) => x.sync(),
            Backend::Wayland(w) => w.sync(),
        }
    }

    /// The window that has keyboard focus, if it is one of ours or a
    /// program's. Wayland: foreign-toplevel state from our compositor;
    /// a part only compares it with its own windows.
    pub fn active_window(&self) -> Option<u32> {
        match &self.backend {
            Backend::X11(x) => x.active_window(),
            Backend::Wayland(w) => w.active_window(),
        }
    }

    /// Subscribe to active-window, window-list, work-area and screen/monitor
    /// changes. Wayland: our compositor sends desktop state and output changes.
    pub fn watch_desktop(&self) -> Result<()> {
        match &self.backend {
            Backend::X11(x) => x.watch_root(),
            // Focus changes come with the keyboard; the rest has no
            // Wayland side yet.
            Backend::Wayland(_) => Ok(()),
        }
    }

    /// Whether Ctrl and Shift are pressed, in that order; false on query failure.
    /// Wayland: the compositor knows the modifier state.
    pub fn modifiers(&self) -> (bool, bool) {
        match &self.backend {
            Backend::X11(x) => x.modifiers(),
            Backend::Wayland(w) => w.modifiers(),
        }
    }

    /// Bring a window forward and give it keyboard focus. Wayland: an
    /// activation request through foreign-toplevel management in our compositor.
    pub fn activate(&self, window: u32) -> Result<()> {
        x11_or!(self, OtherWindows, Ok(()), |xd| xd.activate(window))
    }

    /// Find an already running app window by its class and try to raise it.
    /// Return whether it was found, even if activation failed. Wayland: find
    /// the single instance through our socket and ask it to activate its window.
    pub fn raise_app_window(&self, class: &str) -> bool {
        let Backend::X11(x) = &self.backend else {
            // Wayland: our socket will find the single instance (not yet).
            return false;
        };
        let open = x
            .client_list()
            .into_iter()
            .find(|&w| x.window_info(w).is_some_and(|i| i.instance == class));
        if let Some(w) = open {
            let _ = x.activate(w);
            return true;
        }
        false
    }

    /// An ordinary titled window with a frame from the window manager
    /// (`class` names it for the window manager, `min` is its smallest
    /// size, both in points). Wayland: an `xdg_toplevel`.
    pub fn create_app_window(
        &self,
        class: &str,
        title: &str,
        size: (u32, u32),
        min: (u32, u32),
    ) -> Result<AppWindow> {
        Ok(match &self.backend {
            Backend::X11(x) => AppWindow::X11(x.create_app_window(class, title, size, min)?),
            Backend::Wayland(w) => {
                AppWindow::Wayland(w.create_app_window(class, title, size, min)?)
            }
        })
    }

    /// The primary output's area in points. Wayland: the primary output
    /// chosen by our compositor.
    pub fn primary_area(&self) -> (i32, i32, u32, u32) {
        match &self.backend {
            Backend::X11(x) => x.primary_area(),
            Backend::Wayland(w) => w.primary_area(),
        }
    }

    /// A popup for transient shell UI. Wayland: an `xdg_popup` or a
    /// layer-shell surface for panels and full-screen overlays.
    pub fn create_popup(&self, name: &str) -> Result<Popup> {
        Ok(match &self.backend {
            Backend::X11(x) => Popup::X11(x.create_popup(name)?),
            Backend::Wayland(w) => Popup::Wayland(w.create_popup(name)?),
        })
    }

    /// A strip at a screen edge. Wayland: a layer-shell surface.
    pub fn create_panel(&self, name: &str) -> Result<Panel> {
        Ok(match &self.backend {
            Backend::X11(x) => Panel::X11(x.create_dock(name)?),
            Backend::Wayland(w) => Panel::Wayland(w.create_panel(name)?),
        })
    }

    /// Whether an event concerns the whole screen / desktop state rather
    /// than one window. Wayland: desktop/output state identified by our compositor.
    pub fn is_screen(&self, window: u32) -> bool {
        x11_or!(self, ScreenArea, false, |x| window == x.root())
    }

    /// A program window's metadata. Wayland: ext-foreign-toplevel-list /
    /// wlr-foreign-toplevel-management, implemented by our compositor.
    pub fn window_info(&self, window: u32) -> Option<WindowInfo> {
        x11_or!(self, OtherWindows, None, |xd| xd.window_info(window))
    }

    /// List program windows. Wayland: ext-foreign-toplevel-list.
    pub fn program_windows(&self) -> Vec<u32> {
        x11_or!(self, OtherWindows, Vec::new(), |xd| xd.client_list())
    }

    /// A program window's title. Wayland: foreign-toplevel metadata.
    pub fn window_title(&self, window: u32) -> String {
        x11_or!(self, OtherWindows, String::new(), |xd| xd
            .window_title(window))
    }

    /// A program window's icon at the requested size in points.
    /// Wayland: foreign-toplevel icon data from our compositor.
    pub fn window_icon(&self, window: u32, size: u32) -> Option<tiny_skia::Pixmap> {
        x11_or!(self, OtherWindows, None, |xd| xd.window_icon(window, size))
    }

    /// A program window's rectangle on screen, including its frame, in points.
    /// Wayland: our compositor knows the foreign toplevel's frame rectangle.
    pub fn window_frame(&self, window: u32) -> Option<(i32, i32, u32, u32)> {
        x11_or!(self, OtherWindows, None, |xd| xd.frame_geometry(window))
    }

    /// Follow one program window's title, icon and state changes.
    /// Wayland: foreign-toplevel updates from our compositor.
    pub fn watch_window(&self, window: u32) -> Result<()> {
        x11_or!(self, OtherWindows, Ok(()), |xd| xd.watch_window(window))
    }

    /// Watch new program windows and their first frame.
    /// Wayland: our compositor tells the shell directly.
    pub fn watch_new_windows(&self) -> Result<()> {
        x11_or!(self, OtherWindows, Ok(()), |xd| xd.watch_new_windows())
    }

    /// How much longer to wait for a new window's first frame over a screen
    /// rectangle in pixels. Wayland: our compositor tells the shell directly.
    pub fn undrawn_over(&self, rect: (i32, i32, u32, u32)) -> Option<std::time::Duration> {
        x11_or!(self, OtherWindows, None, |xd| xd.undrawn_over(rect))
    }

    /// Close a program window. Wayland: foreign-toplevel management.
    pub fn close(&self, window: u32) -> Result<()> {
        x11_or!(self, OtherWindows, Ok(()), |xd| xd.close(window))
    }

    /// Minimize a program window. Wayland: foreign-toplevel management.
    pub fn minimize(&self, window: u32) -> Result<()> {
        x11_or!(self, OtherWindows, Ok(()), |xd| xd.minimize(window))
    }

    /// Read an app-menu address property by name. X11 only; None on Wayland,
    /// where GTK has no standard global menu (docs/wayland-plan.md).
    pub fn window_menu_property(&self, window: u32, name: &str) -> Option<String> {
        x11_or!(self, OtherWindows, None, |xd| xd.window_text(window, name))
    }

    /// Watch layout/group changes. Wayland: the compositor's xkb keymap.
    pub fn watch_keyboard(&self) -> Result<()> {
        x11_or!(self, KeyboardLayouts, Ok(()), |xd| xd.watch_keyboard())
    }

    /// Layout names. Wayland: the compositor's xkb keymap.
    pub fn keyboard_layouts(&self) -> Vec<String> {
        x11_or!(self, KeyboardLayouts, Vec::new(), |xd| xd
            .keyboard_layouts())
    }

    /// Current layout group. Wayland: the compositor's xkb keymap.
    pub fn keyboard_group(&self) -> u8 {
        x11_or!(self, KeyboardLayouts, 0, |xd| xd.keyboard_group())
    }

    /// Select a layout group. Wayland: the compositor's xkb keymap.
    pub fn set_keyboard_group(&self, group: u8) -> Result<()> {
        x11_or!(self, KeyboardLayouts, Ok(()), |xd| xd
            .set_keyboard_group(group))
    }

    /// Pointer position on screen, in points.
    /// Wayland: our compositor tells the shell the pointer's position.
    pub fn pointer_position(&self) -> Option<(i16, i16)> {
        x11_or!(self, PointerPosition, None, |xd| xd.pointer_position())
    }

    /// Keep pointer events during an icon drag. Wayland: an implicit grab
    /// during the press or an xdg_popup grab.
    pub fn grab_pointer_for_drag(&self, window: u32) -> Result<bool> {
        x11_or!(self, DragAndDrop, Ok(false), |xd| xd
            .grab_pointer_for_drag(window))
    }

    /// Release the drag's pointer grab. Wayland: finish the implicit or popup grab.
    pub fn ungrab_pointer(&self) -> Result<()> {
        x11_or!(self, DragAndDrop, Ok(()), |xd| xd.ungrab_pointer())
    }

    /// Hear about any window appearing or being restacked, and the screen
    /// changing size ([`Event::Mapped`], [`Event::Configure`]), so the lock
    /// screen can stay on top. Wayland: not needed — under
    /// `ext-session-lock-v1` nothing else is shown while locked.
    pub fn watch_stacking(&self) -> Result<()> {
        x11_or!(self, OtherWindows, Ok(()), |xd| xd.watch_stacking())
    }

    /// Give the keyboard back to the active window after the lock screen
    /// had it. Wayland: the compositor restores focus when the lock ends.
    pub fn restore_focus(&self) -> Result<()> {
        x11_or!(self, OtherWindows, Ok(()), |xd| xd.restore_focus())
    }

    /// A picture of the whole screen (the Apps window's blurred background).
    /// Wayland: our compositor renders it (screencopy).
    pub fn capture_screen(&self) -> Result<tiny_skia::Pixmap> {
        self.x11(Capability::ScreenCapture)?.capture_screen()
    }

    /// A picture of a window as it is on screen, for previews and tests.
    /// Wayland: our compositor renders the window (screencopy).
    pub fn capture_window(&self, window: u32) -> Option<tiny_skia::Pixmap> {
        x11_or!(self, ScreenCapture, None, |xd| xd.capture_window(window))
    }
}

/// A strip at a screen edge that reserves space. Positions and sizes are
/// in points. Wayland: a layer-shell surface with an exclusive zone.
pub enum Panel {
    X11(mimshel_x11::DockWindow),
    Wayland(mimshel_wayland::Panel),
}

impl Panel {
    pub fn id(&self) -> u32 {
        match self {
            Panel::X11(p) => p.id(),
            Panel::Wayland(p) => p.id(),
        }
    }

    pub fn show(&self, d: &Display) -> Result<()> {
        on_backend!(self, Panel, d, |p, x| p.show(x), p.show(x))
    }

    pub fn set_geometry(&self, d: &Display, x: i32, y: i32, w: u32, h: u32) -> Result<()> {
        on_backend!(
            self,
            Panel,
            d,
            |p, disp| p.set_geometry(disp, x, y, w, h),
            p.set_geometry(disp, x, y, w, h)
        )
    }

    /// Set the input region in panel-local points. Wayland: a surface input region.
    pub fn set_input(&self, d: &Display, rects: &[(i32, i32, u32, u32)]) -> Result<()> {
        on_backend!(
            self,
            Panel,
            d,
            |p, x| p.set_input(x, rects),
            p.set_input(x, rects)
        )
    }

    /// Reserve space from the screen's top over x_from..x_to, in points.
    /// Wayland: a layer-shell surface with an exclusive zone.
    pub fn reserve_top(&self, d: &Display, height: u32, x_from: u32, x_to: u32) -> Result<()> {
        on_backend!(
            self,
            Panel,
            d,
            |p, x| p.set_strut_top(x, height, x_from, x_to),
            p.reserve_top(x, height, x_from, x_to)
        )
    }

    /// Reserve space from the screen's bottom over x_from..x_to, in points.
    /// A height of 0 clears the reservation.
    /// Wayland: a layer-shell surface with an exclusive zone.
    pub fn reserve_bottom(&self, d: &Display, height: u32, x_from: u32, x_to: u32) -> Result<()> {
        on_backend!(
            self,
            Panel,
            d,
            |p, x| p.set_strut(x, height, x_from, x_to),
            p.reserve_bottom(x, height, x_from, x_to)
        )
    }

    /// Show only columns `x0..x1` of a whole-panel picture, in pixels.
    /// Wayland: update the surface buffer with damage for these columns.
    pub fn present_columns(&self, d: &Display, image: &impl Image, x0: u32, x1: u32) -> Result<()> {
        on_backend!(
            self,
            Panel,
            d,
            |p, x| p.present_columns(x, image, x0, x1),
            p.present_columns(x, image, x0, x1)
        )
    }

    /// Show a picture the size of the panel, in pixels.
    pub fn present(&self, d: &Display, image: &impl Image) -> Result<()> {
        on_backend!(
            self,
            Panel,
            d,
            |p, x| p.present(x, image),
            p.present(x, image)
        )
    }
}

/// An ordinary titled window ([`Display::create_app_window`]). Sizes and
/// positions are in points.
pub enum AppWindow {
    X11(mimshel_x11::AppWindow),
    Wayland(mimshel_wayland::AppWindow),
}

/// Calls the app window's own backend with the matching display; a window
/// and a display of different backends cannot meet (one display per
/// process), so that case answers `$none`.
macro_rules! on_window {
    ($w:expr, $d:expr, $none:expr, |$win:ident, $disp:ident| $body:expr) => {
        match ($w, &$d.backend) {
            (AppWindow::X11($win), Backend::X11($disp)) => $body,
            (AppWindow::Wayland($win), Backend::Wayland($disp)) => $body,
            _ => $none,
        }
    };
}

impl AppWindow {
    pub fn id(&self) -> u32 {
        match self {
            AppWindow::X11(w) => w.id(),
            AppWindow::Wayland(w) => w.id(),
        }
    }

    pub fn show(&self, d: &Display) -> Result<()> {
        on_window!(self, d, Err(NOT_ON_WAYLAND.into()), |w, x| w.show(x))
    }

    /// Let files be dropped on this window. Only a window whose part
    /// answers `Event::Dropped` asks for it: a drop nobody finishes leaves
    /// the source waiting and uses up one of the few pending-drop places.
    /// Wayland: nothing to switch on — a `wl_data_device` offer comes to
    /// any surface.
    pub fn accept_drops(&self, d: &Display) -> Result<()> {
        match (self, &d.backend) {
            (AppWindow::X11(w), Backend::X11(x)) => x.enable_drop(w.id()),
            _ => Ok(()),
        }
    }

    pub fn set_title(&self, d: &Display, title: &str) -> Result<()> {
        on_window!(self, d, Err(NOT_ON_WAYLAND.into()), |w, x| w
            .set_title(x, title))
    }

    /// Change the size, keeping the position.
    pub fn resize(&self, d: &Display, size: (u32, u32)) -> Result<()> {
        on_window!(self, d, Err(NOT_ON_WAYLAND.into()), |w, x| w
            .resize(x, size))
    }

    pub fn size(&self, d: &Display) -> Option<(u32, u32)> {
        on_window!(self, d, None, |w, x| w.size(x))
    }

    /// The window's top-left corner on screen, in points. On Wayland a client
    /// does not know its window's position; it is known only to our own compositor.
    pub fn origin(&self, d: &Display) -> Option<(i16, i16)> {
        match (self, &d.backend) {
            (AppWindow::X11(w), Backend::X11(x)) => w.origin(x),
            _ => None,
        }
    }

    /// Show a picture the size of the window, in pixels.
    pub fn present(&self, d: &Display, image: &impl Image) -> Result<()> {
        on_window!(self, d, Err(NOT_ON_WAYLAND.into()), |w, x| w
            .present(x, image))
    }

    /// Update only `(x0, y0, x1, y1)` pixels of a whole-window picture.
    /// Wayland: update the surface buffer with damage for this rectangle.
    pub fn present_rect(
        &self,
        d: &Display,
        image: &impl Image,
        rect: (u32, u32, u32, u32),
    ) -> Result<()> {
        on_window!(self, d, Err(NOT_ON_WAYLAND.into()), |w, x| w
            .present_rect(x, image, rect))
    }

    pub fn destroy(&self, d: &Display) -> Result<()> {
        on_window!(self, d, Err(NOT_ON_WAYLAND.into()), |w, x| w.destroy(x))
    }
}

/// A transient shell popup ([`Display::create_popup`]). Positions and sizes
/// are in points. Wayland: menus use `xdg_popup` with a grab; panels and
/// full-screen overlays use layer-shell surfaces.
pub enum Popup {
    X11(mimshel_x11::Popup),
    Wayland(mimshel_wayland::Popup),
}

impl Popup {
    pub fn id(&self) -> u32 {
        match self {
            Popup::X11(p) => p.id(),
            Popup::Wayland(p) => p.id(),
        }
    }

    /// Show and take keyboard and pointer input. Wayland: a popup grab
    /// or input focus for a layer-shell surface.
    pub fn show(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        on_backend!(
            self,
            Popup,
            d,
            |p, disp| p.show(disp, x, y, w, h),
            p.show(disp, x, y, w, h)
        )
    }

    /// Show above other windows without taking keyboard or pointer input.
    /// Wayland: a layer-shell surface without an input grab.
    pub fn show_passive(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        on_backend!(
            self,
            Popup,
            d,
            |p, disp| p.show_passive(disp, x, y, w, h),
            p.show_passive(disp, x, y, w, h)
        )
    }

    /// Show over the whole screen and take the keyboard and the pointer for
    /// good: the lock screen. `false` — the keyboard could not be taken; the
    /// lock screen must then exit so its guard starts it again (CLAUDE.md
    /// lessons 1, 21). Wayland: an `ext-session-lock-v1` surface.
    pub fn show_lock(&self, d: &Display, w: u16, h: u16) -> Result<bool> {
        match (self, &d.backend) {
            (Popup::X11(p), Backend::X11(x)) => p.show_lock(x, w, h),
            _ => Err(NOT_ON_WAYLAND.into()),
        }
    }

    /// Raise above every other window. Wayland: not needed under the
    /// session lock.
    pub fn raise(&self, d: &Display) -> Result<()> {
        match (self, &d.backend) {
            (Popup::X11(p), Backend::X11(x)) => p.raise(x),
            _ => Err(NOT_ON_WAYLAND.into()),
        }
    }

    /// Check the keyboard grab and take it back if another program took it;
    /// `true` — it is ours. Wayland: the session lock keeps input; always
    /// `true` there.
    pub fn regrab(&self, d: &Display) -> Result<bool> {
        match (self, &d.backend) {
            (Popup::X11(p), Backend::X11(x)) => p.regrab(x),
            _ => Err(NOT_ON_WAYLAND.into()),
        }
    }

    /// Hide and release keyboard and pointer input. Wayland: dismiss the
    /// popup or overlay and release its input focus.
    pub fn hide(&self, d: &Display) -> Result<()> {
        on_backend!(self, Popup, d, |p, x| p.hide(x), p.hide(x))
    }

    /// Hide without changing keyboard or pointer input. Wayland: unmap
    /// the surface without changing another surface's focus.
    pub fn hide_passive(&self, d: &Display) -> Result<()> {
        on_backend!(self, Popup, d, |p, x| p.hide_passive(x), p.hide_passive(x))
    }

    /// Change the position and size. Wayland: configure the popup or layer surface
    /// through our compositor.
    pub fn set_geometry(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        on_backend!(
            self,
            Popup,
            d,
            |p, disp| p.set_geometry(disp, x, y, w, h),
            p.set_geometry(disp, x, y, w, h)
        )
    }

    /// Let pointer input reach the surface below. Wayland: an empty input region.
    pub fn pass_through(&self, d: &Display) -> Result<()> {
        match (self, &d.backend) {
            (Popup::X11(p), Backend::X11(x)) => p.pass_through(x),
            _ => Err(NOT_ON_WAYLAND.into()),
        }
    }

    /// Update a picture at an offset in pixels. Wayland: surface buffer damage.
    pub fn present_at(&self, d: &Display, image: &impl Image, x: i32, y: i32) -> Result<()> {
        match (self, &d.backend) {
            (Popup::X11(p), Backend::X11(xd)) => p.present_at(xd, image, x, y),
            _ => Err(NOT_ON_WAYLAND.into()),
        }
    }

    /// Show a picture the size of the popup, in pixels.
    pub fn present(&self, d: &Display, image: &impl Image) -> Result<()> {
        on_backend!(
            self,
            Popup,
            d,
            |p, x| p.present(x, image),
            p.present(x, image)
        )
    }
}

/// Copy text to the clipboard, or the X11 PRIMARY selection when `primary`
/// is true. Wayland: `wl_data_device` or `zwp_primary_selection`, respectively.
pub fn copy_text(text: String, primary: bool) {
    mimshel_x11::copy_text(text, primary);
}

/// Read text from the clipboard, or the X11 PRIMARY selection when `primary`
/// is true. Wayland: `wl_data_device` or `zwp_primary_selection`, respectively.
pub fn clipboard_text(primary: bool) -> Option<String> {
    mimshel_x11::clipboard_text(primary)
}

/// Copy files. Wayland: `wl_data_device` with `text/uri-list` and
/// `x-special/gnome-copied-files` (including the cut flag).
pub fn copy_files(paths: Vec<std::path::PathBuf>, cut: bool) {
    mimshel_x11::copy_files(paths, cut);
}

/// Read files and the cut flag. Wayland: `wl_data_device` with `text/uri-list`
/// and `x-special/gnome-copied-files`.
pub fn clipboard_files() -> Option<(bool, Vec<std::path::PathBuf>)> {
    mimshel_x11::clipboard_files()
}

/// The wallpaper and icons layer. Wayland: layer-shell background layer.
pub struct Background(mimshel_x11::DesktopWindow);

impl Background {
    pub fn id(&self) -> u32 {
        self.0.id()
    }

    pub fn show(&self, d: &Display) -> Result<()> {
        self.0.show(d.x11(Capability::DesktopLayer)?)
    }

    /// Over other programs' desktop windows (our icons on a video
    /// wallpaper). Wayland: the compositor orders background layers.
    pub fn raise_over_others(&self, d: &Display) -> Result<()> {
        x11_or!(d, DesktopLayer, Ok(()), |xd| self.0.remap(xd))
    }

    pub fn set_wallpaper(&self, d: &Display, image: &tiny_skia::Pixmap) -> Result<()> {
        self.0
            .set_wallpaper(d.x11(Capability::DesktopLayer)?, image)
    }

    /// Update an area at an offset in pixels. Wayland: surface buffer damage.
    pub fn update(&self, d: &Display, image: &tiny_skia::Pixmap, x: i32, y: i32) -> Result<()> {
        self.0.update(d.x11(Capability::DesktopLayer)?, image, x, y)
    }
}

/// Rubber-band selection drawing over the desktop. X11: an XRender picture
/// with XFixes shaping; Wayland: a small overlay surface or compositor drawing.
pub struct SelectionOverlay(mimshel_x11::SelectionRenderer);

impl SelectionOverlay {
    /// Optional acceleration; the desktop retains its picture-based fallback.
    pub fn new(d: &Display, window: u32) -> Option<Self> {
        mimshel_x11::SelectionRenderer::new(d.x11(Capability::DesktopLayer).ok()?, window).map(Self)
    }

    pub fn paint(&mut self, d: &Display, rect: (f32, f32, f32, f32)) -> Result<()> {
        self.0.paint(d.x11(Capability::DesktopLayer)?, rect)
    }

    pub fn reset(&mut self, d: &Display) -> Result<()> {
        self.0.reset(d.x11(Capability::DesktopLayer)?)
    }
}

/// Points to pixels the system asks for, when the config does not say:
/// from `Xft.dpi` (xfsettingsd sets it from Xfce's settings), else 1.
/// Wayland: the output's scale from our compositor.
pub fn system_scale() -> f32 {
    mimshel_x11::xft_dpi().map_or(1.0, mimshel_x11::scale_for_dpi)
}

/// A quiet second connection that only waits for the display server to go
/// away (the session ended); the guard exits then (CLAUDE.md lesson 17).
/// Wayland: the compositor's socket closing.
pub struct ServerWatch(mimshel_x11::ServerWatch);

impl ServerWatch {
    pub fn connect() -> Result<Self> {
        Ok(Self(mimshel_x11::ServerWatch::connect()?))
    }

    /// Returns when the connection is gone.
    pub fn wait_closed(self) {
        self.0.wait_closed();
    }
}

#[cfg(test)]
mod tests {
    use super::{Capability, WAYLAND_LACKS};

    /// The capabilities named by the door calls that answer "nothing" on
    /// Wayland are exactly the ones [`WAYLAND_LACKS`] lists. Claiming a
    /// capability whose call does nothing would offer a dead control;
    /// leaving one listed after its calls got their Wayland side would hide
    /// a working one.
    #[test]
    fn wayland_lacks_what_its_calls_say() {
        let source = include_str!("platform.rs");
        let door = source.split("#[cfg(test)]").next().unwrap();
        let mut named: Vec<String> = Vec::new();
        let mut note = |rest: &str| {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if !named.contains(&name) {
                named.push(name);
            }
        };
        // `x11_or!(display, Capability, …)`: the second argument.
        for call in door.split("x11_or!(").skip(1) {
            let Some((first, rest)) = call.split_once(',') else {
                continue;
            };
            // The macro's own definition and its description.
            if first.contains('$') || first.contains('`') {
                continue;
            }
            note(rest.trim_start());
        }
        // `.x11(Capability::Name)`.
        for call in door.split(".x11(Capability::").skip(1) {
            note(call);
        }
        named.sort();
        let mut listed: Vec<String> = WAYLAND_LACKS.iter().map(|c| format!("{c:?}")).collect();
        listed.sort();
        assert_eq!(named, listed);
        for capability in WAYLAND_LACKS {
            assert!(Capability::ALL.contains(capability));
        }
    }

    /// Only this file reaches a backend, X11 or Wayland; every other file of
    /// the program goes through the door.
    #[test]
    fn only_the_door_names_the_backend() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs")
                && path.file_name().is_some_and(|n| n != "platform.rs")
            {
                let text = std::fs::read_to_string(&path).unwrap();
                if ["mimshel_x11", "mimshel_wayland"]
                    .iter()
                    .any(|backend| text.contains(backend))
                {
                    offenders.push(path);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a backend used past the door: {offenders:?}"
        );
    }
}
