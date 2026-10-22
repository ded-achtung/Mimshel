//! Установка и проверка системы — то, что делал скрипт macos-look
//! (~/macos-look), переносится сюда по частям: проверка (`shell doctor`),
//! включение частей (`shell apply`). Тему, обои, блокировку и меню яблока
//! пока применяет macos-look.

pub mod doctor;
pub mod look;
pub mod parts;
mod sys;
