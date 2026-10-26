//! Установка и проверка системы (раньше — скрипт macos-look, с 24.10.2026
//! полностью здесь): проверка (`mimshel doctor`), включение частей и оформление
//! (`mimshel apply`), сторож блокировки (`mimshel lock`), пункты главного меню
//! (`mimshel action`), копии настроек и откат (`mimshel backup`, `mimshel restore`).

pub mod actions;
pub mod backup;
pub mod crash;
pub mod doctor;
pub mod greeter;
pub mod gtk_fixes;
pub mod gtk_links;
pub mod lock;
pub mod parts;
pub mod paths;
pub mod png_icons;
pub mod report;
mod sys;
pub mod terminal;
