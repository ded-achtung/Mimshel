//! What the shell's parts know about the screen, whatever draws it: the
//! events they react to, keys, errors. Today the X11 backend
//! (`mimshel-x11`) produces these; a Wayland backend will produce the same
//! (docs/platform.md, docs/wayland-plan.md phase 1). Nothing here knows
//! about a display protocol.
//!
//! Window ids (`window: u32`) are the backend's names for surfaces: X11
//! window ids today; a Wayland backend numbers its surfaces itself.

pub use mimshel_core::input::{Key, KeyPress};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// Which property of a window changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    /// The list of program windows.
    ClientList,
    /// The active window.
    ActiveWindow,
    WorkArea,
    /// Minimized, maximized and the like.
    State,
    Title,
    Icon,
    Class,
    Other,
}

/// The events the shell needs. Coordinates are relative to `window`; while
/// the pointer is grabbed, a click outside the window comes with
/// coordinates beyond its edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A new program window drew for the first time
    /// (once the backend watches new windows).
    Drawn {
        window: u32,
    },
    Key(KeyPress),
    Button {
        window: u32,
        x: i16,
        y: i16,
        button: u8,
    },
    /// A mouse button was released.
    Release {
        window: u32,
        x: i16,
        y: i16,
        button: u8,
    },
    Motion {
        window: u32,
        x: i16,
        y: i16,
    },
    /// The pointer entered the window (at `x`, `y`) / left it.
    Enter {
        window: u32,
        x: i16,
        y: i16,
    },
    Leave {
        window: u32,
    },
    /// The window needs redrawing.
    Expose {
        window: u32,
    },
    Property {
        window: u32,
        what: Property,
    },
    /// The window moved or changed size (the root window: the screen size
    /// changed).
    Configure {
        window: u32,
    },
    /// A window was mapped (after the backend was asked to watch stacking).
    Mapped {
        window: u32,
    },
    /// Files from another program are dragged over the window (at `x`, `y`).
    DropHover {
        window: u32,
        x: i16,
        y: i16,
    },
    DropLeave {
        window: u32,
    },
    /// Files were dropped on the window; `moving` comes from the drag
    /// protocol. The backend hands out the paths by `ticket`, then is told
    /// the drop is finished.
    Dropped {
        window: u32,
        x: i16,
        y: i16,
        moving: bool,
        ticket: u64,
    },
    /// The close button of an app window's frame was pressed, or the window
    /// was destroyed from outside.
    Close {
        window: u32,
    },
    /// The keyboard layout with this number (from 0) became active.
    KeyboardGroup(u8),
    /// The set of layouts changed (setxkbmap, for example).
    KeyboardLayouts,
    Other,
}

/// What is under the pointer during a file drag.
/// Wayland: target state from `wl_data_device` / `wl_data_source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragTarget {
    /// The source surface itself, where icons can be rearranged.
    #[default]
    Own,
    /// A program window that may accept files.
    App { accepts: bool },
    /// A surface that does not accept files.
    Nothing,
}
