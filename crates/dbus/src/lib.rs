//! Связь с другими программами через D-Bus: трей (StatusNotifierItem и меню
//! значков DBusMenu), глобальное меню программ, уведомления и Bluetooth (BlueZ).

pub mod appmenu;
pub mod bluetooth;
pub mod menu;
pub mod notify;
pub mod registrar;
pub mod tray;

pub use appmenu::{AppMenu, AppMenuCmd, AppMenuUpdate, GtkPaths, Source};
pub use menu::MenuItem;
pub use tray::{Tray, TrayCmd, TrayItem, TrayUpdate};
