//! Список установленных программ из файлов .desktop (стандарт freedesktop
//! «Desktop Entry»). Отсюда его берут Spotlight, Launchpad и док.
//! [`Watcher`] сообщает, когда список пора перечитать.

pub mod entry;
pub mod exec;
mod index;
#[cfg(test)]
mod test_util;
mod watch;

pub use entry::Locale;
pub use index::{App, Settings, load};
pub use watch::Watcher;
