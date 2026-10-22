//! Регистратор меню (`com.canonical.AppMenu.Registrar`): программы
//! сообщают, где меню их окна (`RegisterWindow`), а строка меню спрашивает
//! (`GetMenuForWindow`). Пока регистратор есть на шине, модуль
//! appmenu-gtk-module считает, что глобальное меню показывают, и прячет
//! строки меню внутри окон; не стало регистратора (служба остановилась) —
//! программы сами возвращают меню в окна.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::interface;
use zbus::message::Header;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

pub const NAME: &str = "com.canonical.AppMenu.Registrar";
pub const PATH: &str = "/com/canonical/AppMenu/Registrar";

/// Окно X11 → имя программы на шине и путь её меню (DBusMenu).
pub type Windows = Arc<Mutex<HashMap<u32, (String, String)>>>;

pub struct Registrar {
    pub windows: Windows,
}

#[interface(name = "com.canonical.AppMenu.Registrar")]
impl Registrar {
    fn register_window(
        &self,
        window_id: u32,
        menu_object_path: ObjectPath<'_>,
        #[zbus(header)] header: Header<'_>,
    ) {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        self.windows
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(window_id, (sender, menu_object_path.to_string()));
    }

    fn unregister_window(&self, window_id: u32) {
        self.windows
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&window_id);
    }

    fn get_menu_for_window(&self, window_id: u32) -> (String, OwnedObjectPath) {
        let windows = self.windows.lock().unwrap_or_else(|p| p.into_inner());
        let (bus, path) = windows
            .get(&window_id)
            .cloned()
            .unwrap_or_else(|| (String::new(), "/".into()));
        let path = OwnedObjectPath::try_from(path)
            .unwrap_or_else(|_| OwnedObjectPath::try_from("/").expect("корень — правильный путь"));
        (bus, path)
    }

    fn get_menus(&self) -> Vec<(u32, String, OwnedObjectPath)> {
        let windows = self.windows.lock().unwrap_or_else(|p| p.into_inner());
        windows
            .iter()
            .filter_map(|(w, (bus, path))| {
                Some((
                    *w,
                    bus.clone(),
                    OwnedObjectPath::try_from(path.as_str()).ok()?,
                ))
            })
            .collect()
    }
}
