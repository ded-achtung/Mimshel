//! Окно папки — свой Проводник: места слева, значки файлов строками,
//! переходы «назад/вперёд/вверх», выделение мышью и клавишами. Здесь —
//! состояние и картинка окна без X; окно на экране, слежение за папкой и
//! действия с файлами — в службе (`apps/mimshel/src/folder_ui.rs`).

pub mod places;
pub mod view;

pub use places::{
    Place, Section, automount_devices, gvfs_dir, phones, places, places_lang, special_folders,
};
pub use view::{Action, FolderView};
