//! Установка и проверка системы (раньше — скрипт macos-look, с 24.10.2026
//! полностью здесь): проверка (`shell doctor`), включение частей и оформление
//! (`shell apply`), сторож блокировки (`shell lock`), пункты главного меню
//! (`shell action`), копии настроек и откат (`shell backup`, `shell restore`).

pub mod actions;
pub mod backup;
pub mod doctor;
pub mod gtk_fixes;
pub mod gtk_links;
pub mod lock;
pub mod parts;
pub mod paths;
pub mod png_icons;
mod sys;
