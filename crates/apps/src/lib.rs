//! Список установленных программ из файлов .desktop (стандарт freedesktop
//! «Desktop Entry»). Отсюда его берут Spotlight, Launchpad и док.

pub mod entry;
pub mod exec;
mod index;

pub use entry::Locale;
pub use index::{App, Settings, load};
