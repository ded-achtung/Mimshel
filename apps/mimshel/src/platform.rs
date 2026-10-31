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
pub use mimshel_platform::{Event, Key, Property, Result};
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

    /// Wait for the next event (blocks).
    pub fn next_event(&self) -> Result<Event> {
        self.x11.next_event()
    }

    /// Points to pixels: 1 point is this many pixels.
    pub fn scale(&self) -> f32 {
        self.x11.scale()
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

    /// Keep pointer events during an icon drag. Wayland: an implicit grab
    /// during the press or an xdg_popup grab.
    pub fn grab_pointer_for_drag(&self, window: u32) -> Result<bool> {
        self.x11.grab_pointer_for_drag(window)
    }

    /// Release the drag's pointer grab. Wayland: finish the implicit or popup grab.
    pub fn ungrab_pointer(&self) -> Result<()> {
        self.x11.ungrab_pointer()
    }

    /// A picture of one of our windows as it is on screen, for tests.
    #[cfg(test)]
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
