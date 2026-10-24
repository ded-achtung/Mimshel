//! Рабочий стол: части без окон X — состояние и рисование в картинку.
//! Окно стола и показ этих частей — в службе (`apps/shell/src/desktop_ui.rs`).

pub mod files;
pub mod icon_grid;
pub mod wallpaper_picker;

pub use wallpaper_picker::{PickerResult, Tile, WallpaperPicker};
