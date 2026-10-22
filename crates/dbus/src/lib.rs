//! Связь с другими программами через D-Bus: трей (StatusNotifierItem и меню
//! значков DBusMenu) и глобальное меню программ.

pub mod appmenu;
pub mod menu;
pub mod tray;

pub use appmenu::{AppMenu, AppMenuCmd, AppMenuUpdate, GtkPaths, Source};
pub use menu::MenuItem;
pub use tray::{Tray, TrayCmd, TrayItem, TrayUpdate};
