//! The shell's one door to the screen (docs/platform.md). Shell parts
//! (`*_ui.rs`) reach windows, input and outputs through this module only,
//! never through `mimshel_x11`. Every type here wraps the X11 backend
//! today; a Wayland backend becomes a second variant inside the same
//! types, so the parts do not change (docs/wayland-plan.md, phase 1).
//!
//! Methods are named for what a part needs, not for how X11 does it, and
//! each says what it maps to on Wayland when that is not obvious. Only
//! what a moved part uses is here: a part that moves adds its methods.
//! Parts move over one at a time; until then they use [`Display::x11`].

pub use mimshel_core::windows::WindowInfo;
pub use mimshel_platform::{DragTarget, Event, Key, KeyPress, Property, Result};
use mimshel_render::Image;

/// The connection to the screen: one per process.
pub struct Display {
    x11: mimshel_x11::Display,
}

impl Display {
    pub fn connect() -> Result<Self> {
        Ok(Self {
            x11: mimshel_x11::Display::connect()?,
        })
    }

    /// The X11 display, for parts not moved behind this module yet.
    pub fn x11(&self) -> &mimshel_x11::Display {
        &self.x11
    }

    /// The wallpaper and icons layer. Wayland: layer-shell background layer.
    pub fn create_background(&self) -> Result<Background> {
        Ok(Background(self.x11.create_desktop()?))
    }

    /// Output rectangles in points. Wayland: outputs from our compositor.
    pub fn monitors(&self) -> Vec<(i32, i32, u32, u32)> {
        self.x11.monitors()
    }

    /// Usable output area in points. Wayland: output minus exclusive zones.
    pub fn workarea(&self) -> Option<(i32, i32, u32, u32)> {
        self.x11.workarea()
    }

    /// Screen size in physical pixels. Wayland: compositor output bounds.
    pub fn screen_pixels(&self) -> (u16, u16) {
        self.x11.screen_pixels()
    }

    /// Begin offering files. Wayland: `wl_data_device` / `wl_data_source`.
    pub fn drag_begin(&self, source: u32, paths: &[std::path::PathBuf]) {
        self.x11.drag_begin(source, paths);
    }

    /// Update a file drag. Wayland: `wl_data_device` / `wl_data_source`.
    pub fn drag_motion(&self, x: i16, y: i16, copy: bool) -> DragTarget {
        self.x11.drag_motion(x, y, copy)
    }

    /// End a file drag. Wayland: `wl_data_device` / `wl_data_source`.
    pub fn drag_end(&self) -> DragTarget {
        self.x11.drag_end()
    }

    /// Take received file paths. Wayland: read the `wl_data_device` offer.
    pub fn take_drop(&self, ticket: u64) -> Vec<std::path::PathBuf> {
        self.x11.take_drop(ticket)
    }

    /// Report completion after the file operation. Wayland: finish the
    /// `wl_data_offer` with the accepted result.
    pub fn finish_drop(&self, ticket: u64, success: bool) {
        self.x11.finish_drop(ticket, success);
    }

    /// Wait for the next event (blocks).
    pub fn next_event(&self) -> Result<Event> {
        self.x11.next_event()
    }

    /// Points to pixels: 1 point is this many pixels.
    pub fn scale(&self) -> f32 {
        self.x11.scale()
    }

    /// Convert points to pixels at the display's scale.
    pub fn px(&self, points: u32) -> u32 {
        self.x11.px(points)
    }

    /// The whole screen in pixels. Wayland: the bounding box of the outputs.
    pub fn screen_size(&self) -> (u16, u16) {
        self.x11.screen_size()
    }

    /// Wait until everything sent so far has been handled. Wayland: a
    /// roundtrip.
    pub fn sync(&self) -> Result<()> {
        self.x11.sync()
    }

    /// The window that has keyboard focus, if it is one of ours or a
    /// program's. Wayland: foreign-toplevel state from our compositor;
    /// a part only compares it with its own windows.
    pub fn active_window(&self) -> Option<u32> {
        self.x11.active_window()
    }

    /// Subscribe to active-window, window-list, work-area and screen/monitor
    /// changes. Wayland: our compositor sends desktop state and output changes.
    pub fn watch_desktop(&self) -> Result<()> {
        self.x11.watch_root()
    }

    /// Whether Ctrl and Shift are pressed, in that order; false on query failure.
    /// Wayland: the compositor knows the modifier state.
    pub fn modifiers(&self) -> (bool, bool) {
        self.x11.modifiers()
    }

    /// Bring a window forward and give it keyboard focus. Wayland: an
    /// activation request through foreign-toplevel management in our compositor.
    pub fn activate(&self, window: u32) -> Result<()> {
        self.x11.activate(window)
    }

    /// Find an already running app window by its class and try to raise it.
    /// Return whether it was found, even if activation failed. Wayland: find
    /// the single instance through our socket and ask it to activate its window.
    pub fn raise_app_window(&self, class: &str) -> bool {
        let open = self
            .x11
            .client_list()
            .into_iter()
            .find(|&w| self.x11.window_info(w).is_some_and(|i| i.instance == class));
        if let Some(w) = open {
            let _ = self.x11.activate(w);
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
        Ok(AppWindow(
            self.x11.create_app_window(class, title, size, min)?,
        ))
    }

    /// The primary output's area in points. Wayland: the primary output
    /// chosen by our compositor.
    pub fn primary_area(&self) -> (i32, i32, u32, u32) {
        self.x11.primary_area()
    }

    /// A popup for transient shell UI. Wayland: an `xdg_popup` or a
    /// layer-shell surface for panels and full-screen overlays.
    pub fn create_popup(&self, name: &str) -> Result<Popup> {
        Ok(Popup(self.x11.create_popup(name)?))
    }

    /// A strip at a screen edge. Wayland: a layer-shell surface.
    pub fn create_panel(&self, name: &str) -> Result<Panel> {
        Ok(Panel(self.x11.create_dock(name)?))
    }

    /// Whether an event concerns the whole screen / desktop state rather
    /// than one window. Wayland: desktop/output state identified by our compositor.
    pub fn is_screen(&self, window: u32) -> bool {
        window == self.x11.root()
    }

    /// A program window's metadata. Wayland: ext-foreign-toplevel-list /
    /// wlr-foreign-toplevel-management, implemented by our compositor.
    pub fn window_info(&self, window: u32) -> Option<WindowInfo> {
        self.x11.window_info(window)
    }

    /// List program windows. Wayland: ext-foreign-toplevel-list.
    pub fn program_windows(&self) -> Vec<u32> {
        self.x11.client_list()
    }

    /// A program window's title. Wayland: foreign-toplevel metadata.
    pub fn window_title(&self, window: u32) -> String {
        self.x11.window_title(window)
    }

    /// A program window's icon at the requested size in points.
    /// Wayland: foreign-toplevel icon data from our compositor.
    pub fn window_icon(&self, window: u32, size: u32) -> Option<tiny_skia::Pixmap> {
        self.x11.window_icon(window, size)
    }

    /// A program window's rectangle on screen, including its frame, in points.
    /// Wayland: our compositor knows the foreign toplevel's frame rectangle.
    pub fn window_frame(&self, window: u32) -> Option<(i32, i32, u32, u32)> {
        self.x11.frame_geometry(window)
    }

    /// Follow one program window's title, icon and state changes.
    /// Wayland: foreign-toplevel updates from our compositor.
    pub fn watch_window(&self, window: u32) -> Result<()> {
        self.x11.watch_window(window)
    }

    /// Watch new program windows and their first frame.
    /// Wayland: our compositor tells the shell directly.
    pub fn watch_new_windows(&self) -> Result<()> {
        self.x11.watch_new_windows()
    }

    /// How much longer to wait for a new window's first frame over a screen
    /// rectangle in pixels. Wayland: our compositor tells the shell directly.
    pub fn undrawn_over(&self, rect: (i32, i32, u32, u32)) -> Option<std::time::Duration> {
        self.x11.undrawn_over(rect)
    }

    /// Close a program window. Wayland: foreign-toplevel management.
    pub fn close(&self, window: u32) -> Result<()> {
        self.x11.close(window)
    }

    /// Minimize a program window. Wayland: foreign-toplevel management.
    pub fn minimize(&self, window: u32) -> Result<()> {
        self.x11.minimize(window)
    }

    /// Read an app-menu address property by name. X11 only; None on Wayland,
    /// where GTK has no standard global menu (docs/wayland-plan.md).
    pub fn window_menu_property(&self, window: u32, name: &str) -> Option<String> {
        self.x11.window_text(window, name)
    }

    /// Watch layout/group changes. Wayland: the compositor's xkb keymap.
    pub fn watch_keyboard(&self) -> Result<()> {
        self.x11.watch_keyboard()
    }

    /// Layout names. Wayland: the compositor's xkb keymap.
    pub fn keyboard_layouts(&self) -> Vec<String> {
        self.x11.keyboard_layouts()
    }

    /// Current layout group. Wayland: the compositor's xkb keymap.
    pub fn keyboard_group(&self) -> u8 {
        self.x11.keyboard_group()
    }

    /// Select a layout group. Wayland: the compositor's xkb keymap.
    pub fn set_keyboard_group(&self, group: u8) -> Result<()> {
        self.x11.set_keyboard_group(group)
    }

    /// Pointer position on screen, in points.
    /// Wayland: our compositor tells the shell the pointer's position.
    pub fn pointer_position(&self) -> Option<(i16, i16)> {
        self.x11.pointer_position()
    }

    /// Keep pointer events during an icon drag. Wayland: an implicit grab
    /// during the press or an xdg_popup grab.
    pub fn grab_pointer_for_drag(&self, window: u32) -> Result<bool> {
        self.x11.grab_pointer_for_drag(window)
    }

    /// Release the drag's pointer grab. Wayland: finish the implicit or popup grab.
    pub fn ungrab_pointer(&self) -> Result<()> {
        self.x11.ungrab_pointer()
    }

    /// A picture of a window as it is on screen, for previews and tests.
    /// Wayland: our compositor renders the window (screencopy).
    pub fn capture_window(&self, window: u32) -> Option<tiny_skia::Pixmap> {
        self.x11.capture_window(window)
    }
}

/// A strip at a screen edge that reserves space. Positions and sizes are
/// in points. Wayland: a layer-shell surface with an exclusive zone.
pub struct Panel(mimshel_x11::DockWindow);

impl Panel {
    pub fn id(&self) -> u32 {
        self.0.id()
    }

    pub fn show(&self, d: &Display) -> Result<()> {
        self.0.show(&d.x11)
    }

    pub fn set_geometry(&self, d: &Display, x: i32, y: i32, w: u32, h: u32) -> Result<()> {
        self.0.set_geometry(&d.x11, x, y, w, h)
    }

    /// Set the input region in panel-local points. Wayland: a surface input region.
    pub fn set_input(&self, d: &Display, rects: &[(i32, i32, u32, u32)]) -> Result<()> {
        self.0.set_input(&d.x11, rects)
    }

    /// Reserve space from the screen's top over x_from..x_to, in points.
    /// Wayland: a layer-shell surface with an exclusive zone.
    pub fn reserve_top(&self, d: &Display, height: u32, x_from: u32, x_to: u32) -> Result<()> {
        self.0.set_strut_top(&d.x11, height, x_from, x_to)
    }

    /// Reserve space from the screen's bottom over x_from..x_to, in points.
    /// A height of 0 clears the reservation.
    /// Wayland: a layer-shell surface with an exclusive zone.
    pub fn reserve_bottom(&self, d: &Display, height: u32, x_from: u32, x_to: u32) -> Result<()> {
        self.0.set_strut(&d.x11, height, x_from, x_to)
    }

    /// Show only columns `x0..x1` of a whole-panel picture, in pixels.
    /// Wayland: update the surface buffer with damage for these columns.
    pub fn present_columns(&self, d: &Display, image: &impl Image, x0: u32, x1: u32) -> Result<()> {
        self.0.present_columns(&d.x11, image, x0, x1)
    }

    /// Show a picture the size of the panel, in pixels.
    pub fn present(&self, d: &Display, image: &impl Image) -> Result<()> {
        self.0.present(&d.x11, image)
    }
}

/// An ordinary titled window ([`Display::create_app_window`]). Sizes and
/// positions are in points.
pub struct AppWindow(mimshel_x11::AppWindow);

impl AppWindow {
    pub fn id(&self) -> u32 {
        self.0.id()
    }

    pub fn show(&self, d: &Display) -> Result<()> {
        self.0.show(&d.x11)
    }

    pub fn set_title(&self, d: &Display, title: &str) -> Result<()> {
        self.0.set_title(&d.x11, title)
    }

    /// Change the size, keeping the position.
    pub fn resize(&self, d: &Display, size: (u32, u32)) -> Result<()> {
        self.0.resize(&d.x11, size)
    }

    pub fn size(&self, d: &Display) -> Option<(u32, u32)> {
        self.0.size(&d.x11)
    }

    /// The window's top-left corner on screen, in points. On Wayland a client
    /// does not know its window's position; it is known only to our own compositor.
    pub fn origin(&self, d: &Display) -> Option<(i16, i16)> {
        self.0.origin(&d.x11)
    }

    /// Show a picture the size of the window, in pixels.
    pub fn present(&self, d: &Display, image: &impl Image) -> Result<()> {
        self.0.present(&d.x11, image)
    }

    /// Update only `(x0, y0, x1, y1)` pixels of a whole-window picture.
    /// Wayland: update the surface buffer with damage for this rectangle.
    pub fn present_rect(
        &self,
        d: &Display,
        image: &impl Image,
        rect: (u32, u32, u32, u32),
    ) -> Result<()> {
        self.0.present_rect(&d.x11, image, rect)
    }

    pub fn destroy(&self, d: &Display) -> Result<()> {
        self.0.destroy(&d.x11)
    }
}

/// A transient shell popup ([`Display::create_popup`]). Positions and sizes
/// are in points. Wayland: menus use `xdg_popup` with a grab; panels and
/// full-screen overlays use layer-shell surfaces.
pub struct Popup(mimshel_x11::Popup);

impl Popup {
    pub fn id(&self) -> u32 {
        self.0.id()
    }

    /// Show and take keyboard and pointer input. Wayland: a popup grab
    /// or input focus for a layer-shell surface.
    pub fn show(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        self.0.show(&d.x11, x, y, w, h)
    }

    /// Show above other windows without taking keyboard or pointer input.
    /// Wayland: a layer-shell surface without an input grab.
    pub fn show_passive(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        self.0.show_passive(&d.x11, x, y, w, h)
    }

    /// Hide and release keyboard and pointer input. Wayland: dismiss the
    /// popup or overlay and release its input focus.
    pub fn hide(&self, d: &Display) -> Result<()> {
        self.0.hide(&d.x11)
    }

    /// Hide without changing keyboard or pointer input. Wayland: unmap
    /// the surface without changing another surface's focus.
    pub fn hide_passive(&self, d: &Display) -> Result<()> {
        self.0.hide_passive(&d.x11)
    }

    /// Change the position and size. Wayland: configure the popup or layer surface
    /// through our compositor.
    pub fn set_geometry(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        self.0.set_geometry(&d.x11, x, y, w, h)
    }

    /// Let pointer input reach the surface below. Wayland: an empty input region.
    pub fn pass_through(&self, d: &Display) -> Result<()> {
        self.0.pass_through(&d.x11)
    }

    /// Update a picture at an offset in pixels. Wayland: surface buffer damage.
    pub fn present_at(&self, d: &Display, image: &impl Image, x: i32, y: i32) -> Result<()> {
        self.0.present_at(&d.x11, image, x, y)
    }

    /// Show a picture the size of the popup, in pixels.
    pub fn present(&self, d: &Display, image: &impl Image) -> Result<()> {
        self.0.present(&d.x11, image)
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
        self.0.show(&d.x11)
    }

    pub fn set_wallpaper(&self, d: &Display, image: &tiny_skia::Pixmap) -> Result<()> {
        self.0.set_wallpaper(&d.x11, image)
    }

    /// Update an area at an offset in pixels. Wayland: surface buffer damage.
    pub fn update(&self, d: &Display, image: &tiny_skia::Pixmap, x: i32, y: i32) -> Result<()> {
        self.0.update(&d.x11, image, x, y)
    }
}

/// Rubber-band selection drawing over the desktop. X11: an XRender picture
/// with XFixes shaping; Wayland: a small overlay surface or compositor drawing.
pub struct SelectionOverlay(mimshel_x11::SelectionRenderer);

impl SelectionOverlay {
    /// Optional acceleration; the desktop retains its picture-based fallback.
    pub fn new(d: &Display, window: u32) -> Option<Self> {
        mimshel_x11::SelectionRenderer::new(&d.x11, window).map(Self)
    }

    pub fn paint(&mut self, d: &Display, rect: (f32, f32, f32, f32)) -> Result<()> {
        self.0.paint(&d.x11, rect)
    }

    pub fn reset(&mut self, d: &Display) -> Result<()> {
        self.0.reset(&d.x11)
    }
}
