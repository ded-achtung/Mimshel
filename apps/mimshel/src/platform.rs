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

pub use mimshel_platform::{Event, Key, Result};
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
    /// program's. Wayland: our compositor knows; a part only compares it
    /// with its own windows.
    pub fn active_window(&self) -> Option<u32> {
        self.x11.active_window()
    }

    /// Bring a window forward and give it keyboard focus. Wayland: an
    /// activation request handled by our compositor.
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
    /// layer-shell overlay for notifications.
    pub fn create_popup(&self, name: &str) -> Result<Popup> {
        Ok(Popup(self.x11.create_popup(name)?))
    }

    /// A picture of one of our windows as it is on screen, for tests.
    #[cfg(test)]
    pub fn capture_window(&self, window: u32) -> Option<tiny_skia::Pixmap> {
        self.x11.capture_window(window)
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
/// are in points; our Wayland compositor places notification overlays.
pub struct Popup(mimshel_x11::Popup);

impl Popup {
    pub fn id(&self) -> u32 {
        self.0.id()
    }

    /// Show and take keyboard and pointer input. Wayland: a popup grab
    /// or input focus for the notification-center overlay.
    pub fn show(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        self.0.show(&d.x11, x, y, w, h)
    }

    /// Show above other windows without taking keyboard or pointer input.
    /// Wayland: a notification overlay without an input grab.
    pub fn show_passive(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        self.0.show_passive(&d.x11, x, y, w, h)
    }

    /// Hide and release keyboard and pointer input. Wayland: dismiss the
    /// popup or overlay and release its input focus.
    pub fn hide(&self, d: &Display) -> Result<()> {
        self.0.hide(&d.x11)
    }

    /// Hide without changing keyboard or pointer input. Wayland: unmap
    /// the notification overlay without changing another surface's focus.
    pub fn hide_passive(&self, d: &Display) -> Result<()> {
        self.0.hide_passive(&d.x11)
    }

    /// Change the position and size. Wayland: configure the overlay
    /// through our compositor.
    pub fn set_geometry(&self, d: &Display, x: i16, y: i16, w: u16, h: u16) -> Result<()> {
        self.0.set_geometry(&d.x11, x, y, w, h)
    }

    /// Show a picture the size of the popup, in pixels.
    pub fn present(&self, d: &Display, image: &impl Image) -> Result<()> {
        self.0.present(&d.x11, image)
    }
}
