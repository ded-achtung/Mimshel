//! Связь с другими программами через D-Bus: трей (StatusNotifierItem и меню
//! значков DBusMenu), глобальное меню программ, уведомления, Bluetooth (BlueZ)
//! and screen brightness (logind).

pub mod appmenu;
pub mod backlight;
pub mod bluetooth;
pub mod menu;
pub mod menu_export;
pub mod notify;
pub mod registrar;
pub mod tray;

/// How long a call to another program may take. zbus waits forever by
/// default, and each of our services has one worker thread: a single hung
/// app (a frozen GTK window, a tray icon that never answers) would freeze
/// the global menu or the whole tray.
pub const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The longest a «wait until it is quiet» pause may last in all. Each of
/// our watchers gathers a burst of signals before re-reading, waiting for a
/// short silence; a program that never falls silent (an animated tray icon,
/// Bluetooth scanning among many devices) must not hold the re-read and the
/// user's commands back for ever.
pub const QUIET_LIMIT: std::time::Duration = std::time::Duration::from_millis(300);

/// A system bus connection whose method calls give up after `timeout`
/// (pairing a Bluetooth device takes much longer than a property read).
pub fn system(timeout: std::time::Duration) -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::system()?
        .method_timeout(timeout)
        .build()
}

/// A session bus connection whose method calls give up after
/// [`CALL_TIMEOUT`].
pub fn session() -> zbus::Result<zbus::blocking::Connection> {
    zbus::blocking::connection::Builder::session()?
        .method_timeout(CALL_TIMEOUT)
        .build()
}

pub use appmenu::{AppMenu, AppMenuCmd, AppMenuUpdate, GtkPaths, Source};
pub use menu::MenuItem;
pub use tray::{Tray, TrayCmd, TrayItem, TrayUpdate};
