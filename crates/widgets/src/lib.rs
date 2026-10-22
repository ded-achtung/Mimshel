//! Общие виджеты оболочки: меню и простые фигуры. Без X11 — состояние и
//! рисование в картинку.

pub mod menu;
pub mod shape;

pub use menu::{Menu, MenuEntry, MenuResult};
