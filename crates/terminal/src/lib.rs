//! Свой терминал (`mimshel terminal`): разбор того, что присылают программы
//! (сотни команд xterm, второй экран, прокрутка, OSC 52), — ядро Alacritty
//! (`alacritty_terminal`, Apache-2.0), годами проверенное на vim, tmux и ssh.
//! Своё — всё, что видно и что под руками: вывод знаков (каждая буква
//! рисуется один раз и дальше только копируется), цвета, курсор, выделение,
//! клавиши, буфер обмена. Здесь нет X11: окно — в `apps/mimshel/src/term_ui.rs`.

pub mod glyphs;
pub mod keys;
pub mod links;
pub mod palette;
pub mod screen;
pub mod session;

pub use glyphs::Glyphs;
pub use keys::key_bytes;
pub use palette::Palette;
pub use session::{Session, TermEvent};
