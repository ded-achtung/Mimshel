//! Установка и проверка системы — то, что делал скрипт macos-look
//! (~/macos-look), переносится сюда по частям: проверка (`shell doctor`),
//! включение частей (`shell apply`), сторож блокировки (`shell lock`),
//! пункты меню яблока (`shell action`). Тему, обои, фон блокировки, скрытие и
//! переименование программ пока делает macos-look.

pub mod actions;
pub mod doctor;
pub mod lock;
pub mod look;
pub mod parts;
mod sys;
