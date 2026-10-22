//! Окна программ — как их видит док (без привязки к X11).

/// Окно программы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u32,
    /// Класс окна (`WM_CLASS`): «экземпляр» и «класс», например
    /// `("mousepad", "Mousepad")`.
    pub instance: String,
    pub class: String,
    pub title: String,
    /// Номер процесса (`_NET_WM_PID`), если программа его сообщила.
    pub pid: Option<u32>,
    pub minimized: bool,
    pub maximized: bool,
}
