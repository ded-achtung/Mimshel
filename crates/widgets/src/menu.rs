//! Меню, как на Mac: тёмное, со скруглёнными углами, синей подсветкой пункта,
//! галочками и разделителями. Одно на всех — меню значка дока, меню яблока в
//! строке меню. Что делает пункт, решает тот, кто меню открыл (команда `C`).
//! Здесь только состояние, наведение, клавиши и рисование в картинку; окно
//! для меню — у службы.

use shell_core::input::Key;
use shell_render::Canvas;
use shell_text::{Fonts, Style};
use tiny_skia::{FillRule, LineCap, LineJoin, PathBuilder, Stroke, Transform};

use crate::shape::{paint, rounded};

#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntry<C> {
    pub label: String,
    pub command: C,
    /// Галочка слева (у «Оставить в Dock», когда значок закреплён).
    pub checked: bool,
    /// Перед пунктом — разделительная линия.
    pub separator_before: bool,
    /// Неактивный пункт — серый, не подсвечивается и не выбирается.
    pub enabled: bool,
    /// С отступом (пункт подменю, показанного внутри меню).
    pub indent: bool,
}

/// Чем закончилось действие в меню.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuResult<C> {
    Nothing,
    /// Подсветка сдвинулась — перерисовать.
    Redraw,
    Close,
    Run(C),
}

/// Открытое меню.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu<C> {
    /// Для чего открыто меню (например, ключ значка дока).
    pub key: String,
    pub entries: Vec<MenuEntry<C>>,
    /// Пункт под указателем или выбранный стрелками.
    pub hover: Option<usize>,
    width: f32,
}

/// Поля сверху и снизу, высота пункта и разделителя.
const PAD: f32 = 5.0;
const ROW: f32 = 22.0;
const SEPARATOR: f32 = 11.0;
/// Слева — место под галочку, справа — поле.
const LEFT: f32 = 26.0;
const RIGHT: f32 = 20.0;
const MIN_WIDTH: f32 = 170.0;
/// Длинные заголовки окон обрезаются «…».
const MAX_WIDTH: f32 = 380.0;
/// Подсветка пункта отступает от краёв меню.
const INSET: f32 = 5.0;
const RADIUS: f32 = 8.0;

const BG: [u8; 4] = [0x28, 0x28, 0x28, 0xf0];
const EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const LINE: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
/// Синий цвет выделения macOS.
const HOVER: [u8; 4] = [0x0a, 0x64, 0xd2, 0xff];

const TEXT: Style = Style {
    size: 13.5,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 400,
};
const DISABLED: Style = Style {
    color: [0x8a, 0x8a, 0x8a, 0xff],
    ..TEXT
};
/// Отступ пунктов подменю.
const INDENT: f32 = 14.0;

impl<C: Copy> Menu<C> {
    pub fn new(key: String, entries: Vec<MenuEntry<C>>) -> Menu<C> {
        Menu {
            key,
            entries,
            hover: None,
            width: MIN_WIDTH,
        }
    }

    /// Размер меню в пикселях (по самой длинной надписи).
    pub fn measure(&mut self, fonts: &mut Fonts) -> (u32, u32) {
        let text = self
            .entries
            .iter()
            .map(|e| fonts.measure(&e.label, &TEXT) + if e.indent { INDENT } else { 0.0 })
            .fold(0.0, f32::max);
        self.width = (LEFT + text + RIGHT).clamp(MIN_WIDTH, MAX_WIDTH).ceil();
        (self.width as u32, self.height() as u32)
    }

    fn height(&self) -> f32 {
        self.rows()
            .last()
            .map_or(2.0 * PAD, |&(_, y)| y + ROW + PAD)
    }

    /// Пункты и верх каждого.
    fn rows(&self) -> Vec<(usize, f32)> {
        let mut y = PAD;
        let mut out = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.separator_before && i > 0 {
                y += SEPARATOR;
            }
            out.push((i, y));
            y += ROW;
        }
        out
    }

    /// Пункт в точке меню `(x, y)`.
    pub fn entry_at(&self, x: f32, y: f32) -> Option<usize> {
        if x < 0.0 || x >= self.width {
            return None;
        }
        self.rows()
            .into_iter()
            .find(|&(_, top)| y >= top && y < top + ROW)
            .map(|(i, _)| i)
    }

    /// Указатель сдвинулся (координаты меню). `true` — подсветка поменялась.
    pub fn motion(&mut self, x: f32, y: f32) -> bool {
        let hover = self.entry_at(x, y).filter(|&i| self.entries[i].enabled);
        let changed = hover != self.hover;
        self.hover = hover;
        changed
    }

    /// Клик в точке меню; за краями меню — закрыть.
    pub fn click(&self, x: f32, y: f32) -> MenuResult<C> {
        if let Some(i) = self.entry_at(x, y) {
            if !self.entries[i].enabled {
                return MenuResult::Nothing;
            }
            return MenuResult::Run(self.entries[i].command);
        }
        let inside = x >= 0.0 && x < self.width && y >= 0.0 && y < self.height();
        if inside {
            MenuResult::Nothing
        } else {
            MenuResult::Close
        }
    }

    /// Стрелки — выбор пункта по кругу, Enter — выполнить, Esc — закрыть.
    pub fn key(&mut self, key: Key) -> MenuResult<C> {
        let n = self.entries.len();
        if !self.entries.iter().any(|e| e.enabled) {
            return MenuResult::Close;
        }
        // Следующий доступный пункт по кругу (неактивные пропускаем).
        let step = |from: usize, forward: bool| {
            let mut i = from;
            loop {
                i = if forward {
                    (i + 1) % n
                } else {
                    (i + n - 1) % n
                };
                if self.entries[i].enabled {
                    return i;
                }
            }
        };
        match key {
            Key::Down => {
                self.hover = Some(step(self.hover.unwrap_or(n - 1), true));
                MenuResult::Redraw
            }
            Key::Up => {
                self.hover = Some(step(self.hover.unwrap_or(0), false));
                MenuResult::Redraw
            }
            Key::Return => match self.hover {
                Some(i) => MenuResult::Run(self.entries[i].command),
                None => MenuResult::Nothing,
            },
            Key::Escape => MenuResult::Close,
            _ => MenuResult::Nothing,
        }
    }

    /// Рисует меню (размер — из [`Menu::measure`]).
    pub fn render(&self, fonts: &mut Fonts) -> Canvas {
        let (w, h) = (self.width, self.height());
        let mut pm = Canvas::new(w as u32, h as u32).expect("размер не нулевой");
        if let Some(r) = rounded(0.0, 0.0, w, h, RADIUS) {
            pm.fill_path(
                &r,
                &paint(BG),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(r) = rounded(0.5, 0.5, w - 1.0, h - 1.0, RADIUS - 0.5) {
            let stroke = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&r, &paint(EDGE), &stroke, Transform::identity(), None);
        }
        for (i, top) in self.rows() {
            let e = &self.entries[i];
            if e.separator_before && i > 0 {
                let y = (top - SEPARATOR / 2.0).round();
                if let Some(r) =
                    tiny_skia::Rect::from_xywh(INSET + 5.0, y, w - 2.0 * INSET - 10.0, 1.0)
                {
                    pm.fill_rect(r, &paint(LINE), Transform::identity(), None);
                }
            }
            if self.hover == Some(i)
                && let Some(r) = rounded(INSET, top, w - 2.0 * INSET, ROW, 4.0)
            {
                pm.fill_path(
                    &r,
                    &paint(HOVER),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let left = if e.indent { LEFT + INDENT } else { LEFT };
            if e.checked {
                check(&mut pm, left - 15.0, top + ROW / 2.0);
            }
            let ty = top + ((ROW - TEXT.line_height()) / 2.0).round();
            let style = if e.enabled { TEXT } else { DISABLED };
            fonts.draw(&mut pm, &e.label, &style, left, ty, Some(w - left - 8.0));
        }
        pm
    }
}

/// Галочка с серединой по высоте `y`.
fn check(pm: &mut Canvas, x: f32, y: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + 3.5, y + 3.5);
    pb.line_to(x + 10.0, y - 4.0);
    let Some(path) = pb.finish() else {
        return;
    };
    let stroke = Stroke {
        width: 1.8,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pm.stroke_path(
        &path,
        &paint(TEXT.color),
        &stroke,
        Transform::identity(),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MenuCommand {
        Launch,
        TogglePin,
        Quit,
    }

    fn menu() -> Menu<MenuCommand> {
        let entry = |label: &str, command, separator_before| MenuEntry {
            label: label.into(),
            command,
            checked: command == MenuCommand::TogglePin,
            separator_before,
            enabled: true,
            indent: false,
        };
        Menu::new(
            "firefox.desktop".into(),
            vec![
                entry("Новое окно", MenuCommand::Launch, false),
                entry("Оставить в Dock", MenuCommand::TogglePin, true),
                entry("Завершить", MenuCommand::Quit, true),
            ],
        )
    }

    #[test]
    fn hit_testing_skips_separators() {
        let m = menu();
        assert_eq!(m.entry_at(50.0, PAD + 1.0), Some(0));
        // Между первым и вторым — разделитель.
        assert_eq!(m.entry_at(50.0, PAD + ROW + 2.0), None);
        assert_eq!(m.entry_at(50.0, PAD + ROW + SEPARATOR + 1.0), Some(1));
        assert_eq!(m.entry_at(-1.0, PAD + 1.0), None);

        assert_eq!(
            m.click(50.0, PAD + ROW + SEPARATOR + 1.0),
            MenuResult::Run(MenuCommand::TogglePin)
        );
        assert_eq!(m.click(50.0, PAD + ROW + 2.0), MenuResult::Nothing);
        assert_eq!(m.click(50.0, -20.0), MenuResult::Close, "клик мимо");
    }

    #[test]
    fn disabled_entries_are_skipped() {
        let mut m = menu();
        m.entries[1].enabled = false;
        // Ни подсветки, ни выбора у неактивного пункта.
        let y1 = PAD + ROW + SEPARATOR + 1.0;
        assert!(!m.motion(50.0, y1) || m.hover.is_none());
        assert_eq!(m.click(50.0, y1), MenuResult::Nothing);
        // Стрелки перескакивают через него.
        m.hover = Some(0);
        m.key(Key::Down);
        assert_eq!(m.hover, Some(2));
        m.key(Key::Up);
        assert_eq!(m.hover, Some(0));
    }

    #[test]
    fn keys_wrap_around() {
        let mut m = menu();
        assert_eq!(m.key(Key::Return), MenuResult::Nothing);
        m.key(Key::Up);
        assert_eq!(m.hover, Some(2));
        m.key(Key::Down);
        assert_eq!(m.hover, Some(0));
        m.key(Key::Down);
        assert_eq!(m.key(Key::Return), MenuResult::Run(MenuCommand::TogglePin));
        assert_eq!(m.key(Key::Escape), MenuResult::Close);
    }

    #[test]
    fn renders_with_hover() {
        let mut fonts = Fonts::new("Inter");
        let mut m = menu();
        let (w, h) = m.measure(&mut fonts);
        assert!(w >= MIN_WIDTH as u32);
        assert_eq!(h as f32, 2.0 * PAD + 3.0 * ROW + 2.0 * SEPARATOR);
        m.hover = Some(0);
        let pm = m.render(&mut fonts);
        assert_eq!((pm.width(), pm.height()), (w, h));
        // Под указателем — синяя подсветка.
        let p = pm.pixmap().pixel(w - 12, (PAD + 3.0) as u32).unwrap();
        assert!(p.blue() > 150 && p.red() < 40, "{p:?}");
    }
}
