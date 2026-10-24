//! Общие виджеты оболочки: меню и простые фигуры. Без X11 — состояние и
//! рисование в картинку.

pub mod field;
pub mod menu;
pub mod shape;

pub use field::{Edit, TextField};
pub use menu::{Menu, MenuEntry, MenuResult};
