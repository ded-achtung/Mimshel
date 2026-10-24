//! Список установленных программ из файлов .desktop (стандарт freedesktop
//! «Desktop Entry»). Отсюда его берут «Поиск», «Программы» и док.
//! [`Watcher`] сообщает, когда список пора перечитать.

pub mod entry;
pub mod exec;
mod index;
mod launch;
pub mod process;
#[cfg(test)]
mod test_util;
mod watch;

pub use entry::Locale;
pub use index::{App, Settings, load, load_all};
pub use watch::Watcher;
