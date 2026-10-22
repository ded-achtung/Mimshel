//! Связь с другими программами через D-Bus: трей (StatusNotifierItem и меню
//! значков DBusMenu).

pub mod menu;
pub mod tray;

pub use menu::MenuItem;
pub use tray::{Tray, TrayCmd, TrayItem, TrayUpdate};
