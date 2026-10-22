//! Меню значка по правому клику, как на Mac: окна программы (галочка —
//! у активного), «Открыть» (или «Новое окно»), «Оставить в Dock» с галочкой,
//! «Завершить»; у корзины — «Открыть» и «Очистить Корзину…». Само меню —
//! общее ([`shell_widgets::Menu`]), здесь — что делают его пункты.

/// Что выбрали в меню.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    /// Запустить программу (ещё одну копию, если уже запущена).
    Launch,
    /// Закрепить в доке или открепить.
    TogglePin,
    /// Закрыть все окна программы.
    Quit,
    /// Переключиться на окно.
    Activate(u32),
    /// Открыть корзину.
    OpenTrash,
    /// Очистить корзину (Проводник сначала спросит подтверждение).
    EmptyTrash,
}

pub type Menu = shell_widgets::Menu<MenuCommand>;
pub type MenuEntry = shell_widgets::MenuEntry<MenuCommand>;
pub type MenuResult = shell_widgets::MenuResult<MenuCommand>;
