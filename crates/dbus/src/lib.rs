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
