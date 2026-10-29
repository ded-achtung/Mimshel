//! The System Settings window: pages of Mimshel's own settings, plus links
//! to the Xfce dialogs for the rest (displays, keyboard, mouse…).
//!
//! No X11 here: what the pages hold ([`model`]), the picture and what a
//! click or a key means ([`view`]). `apps/mimshel/src/settings_ui.rs` puts
//! it in a window and writes the changes the way `mimshel set` does.

pub mod model;
pub mod view;

pub use model::{Control, Env, Group, LINKS, Monitor, Page, PageKind, Row, pages, values};
pub use view::{Action, SettingsView};
