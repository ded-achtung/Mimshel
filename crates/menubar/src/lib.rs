//! Строка меню, как в macOS: сверху во всю ширину экрана, полупрозрачная.
//! Слева — яблоко (меню яблока), жирное название активной программы и её
//! меню («Файл», «Правка»… — глобальное меню),
//! справа — раскладка, громкость, батарея, колокольчик уведомлений и часы. Здесь нет X11: состояние, где что
//! стоит, и рисование в картинку.

mod apple;
mod control;
mod sound;
pub mod status;

pub use apple::{AppleItem, apple_menu, parse_menu};
pub use control::{ControlAction, ControlPanel, ControlState, Network, Tile};
pub use sound::{Row, SoundAction, SoundPanel};

use shell_icons::Icons;
use shell_text::{Fonts, Style};
use shell_widgets::shape::{paint, rounded};
use tiny_skia::{FillRule, FilterQuality, Pixmap, PixmapPaint, Rect, Transform};

/// Часть строки, по которой можно щёлкнуть.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Apple,
    App,
    /// Значок трея (номер в [`Bar::tray`]).
    Tray(usize),
    /// Меню программы (номер в [`Bar::menus`]).
    Menu(usize),
    Layout,
    Volume,
    Battery,
    Bell,
    /// Пункт управления.
    Control,
    Clock,
}

/// Строка меню: что в ней показано.
#[derive(Debug, Clone)]
pub struct Bar {
    pub width: u32,
    pub height: u32,
    /// Иконка меню яблока (имя в теме).
    pub icon: String,
    /// Название активной программы.
    pub app_name: String,
    /// Её меню: «Файл», «Правка»…
    pub menus: Vec<String>,
    /// Подпись раскладки («en», «ru»); `None` — раскладка одна, не показываем.
    pub layout: Option<String>,
    /// Иконка громкости; `None` — звука нет.
    pub volume: Option<String>,
    /// Иконка батареи и проценты; `None` — батареи нет.
    pub battery: Option<(String, String)>,
    /// Иконка колокольчика (обычный или «Не беспокоить»).
    pub bell: Option<String>,
    /// Значки трея — готовые картинки (размер любой, рисуются 16×16).
    pub tray: Vec<Pixmap>,
    /// Показывать значок Пункта управления.
    pub control: bool,
    pub clock: String,
    /// Подсвеченная часть — та, чьё меню открыто.
    pub open: Option<Part>,
}

/// Отступ от краёв экрана.
const EDGE: f32 = 6.0;
/// Поля вокруг надписи или иконки: столько же занимает подсветка.
const ITEM_PAD: f32 = 9.0;
const ICON: u32 = 16;
/// Между иконкой батареи и процентами.
const BATTERY_GAP: f32 = 4.0;

/// Полупрозрачная полоса: чуть светлее обоев, как строка меню Sonoma.
const BG: [u8; 4] = [0xff, 0xff, 0xff, 0x14];
/// Подсветка части с открытым меню.
const HIGHLIGHT: [u8; 4] = [0xff, 0xff, 0xff, 0x38];

const TEXT: Style = Style {
    size: 13.5,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 400,
};
const BOLD: Style = Style {
    weight: 700,
    ..TEXT
};

impl Bar {
    pub fn new(width: u32, height: u32, icon: &str) -> Bar {
        Bar {
            width,
            height,
            icon: icon.to_string(),
            app_name: String::new(),
            menus: Vec::new(),
            layout: None,
            volume: None,
            battery: None,
            bell: None,
            tray: Vec::new(),
            control: false,
            clock: String::new(),
            open: None,
        }
    }

    /// Части строки: какая, левый край и ширина (вместе с полями).
    pub fn parts(&self, fonts: &mut Fonts) -> Vec<(Part, f32, f32)> {
        let mut out = Vec::new();
        let apple_w = ICON as f32 + 2.0 * ITEM_PAD;
        out.push((Part::Apple, EDGE, apple_w));
        let mut left = EDGE + apple_w;
        if !self.app_name.is_empty() {
            let w = fonts.measure(&self.app_name, &BOLD).min(400.0) + 2.0 * ITEM_PAD;
            out.push((Part::App, left, w));
            left += w;
        }
        let menus_from = out.len();
        for (i, title) in self.menus.iter().enumerate() {
            let w = fonts.measure(title, &TEXT) + 2.0 * ITEM_PAD;
            out.push((Part::Menu(i), left, w));
            left += w;
        }
        let mut right = self.width as f32 - EDGE;
        if !self.clock.is_empty() {
            let w = fonts.measure(&self.clock, &TEXT) + 2.0 * ITEM_PAD;
            right -= w;
            out.push((Part::Clock, right, w));
        }
        let icon_w = ICON as f32 + 2.0 * ITEM_PAD;
        if self.control {
            right -= icon_w;
            out.push((Part::Control, right, icon_w));
        }
        if self.bell.is_some() {
            right -= icon_w;
            out.push((Part::Bell, right, icon_w));
        }
        if let Some((_, text)) = &self.battery {
            let w = icon_w + BATTERY_GAP + fonts.measure(text, &TEXT);
            right -= w;
            out.push((Part::Battery, right, w));
        }
        if self.volume.is_some() {
            right -= icon_w;
            out.push((Part::Volume, right, icon_w));
        }
        if let Some(layout) = &self.layout {
            let w = fonts.measure(layout, &BOLD) + 2.0 * ITEM_PAD;
            right -= w;
            out.push((Part::Layout, right, w));
        }
        for i in (0..self.tray.len()).rev() {
            right -= icon_w;
            out.push((Part::Tray(i), right, icon_w));
        }
        // Меню программы, которые не влезли до значков справа, не показываем.
        let mut i = menus_from;
        while i < out.len() {
            if matches!(out[i].0, Part::Menu(_)) && out[i].1 + out[i].2 > right - 8.0 {
                out.remove(i);
            } else {
                i += 1;
            }
        }
        out
    }

    /// Часть под точкой `x` (и её левый край — под ним открывается меню).
    pub fn part_at(&self, fonts: &mut Fonts, x: f32) -> Option<(Part, f32)> {
        self.parts(fonts)
            .into_iter()
            .find(|&(_, px, w)| x >= px && x < px + w)
            .map(|(p, px, _)| (p, px))
    }

    pub fn render(&self, fonts: &mut Fonts, icons: &mut Icons) -> Pixmap {
        let (w, h) = (self.width.max(1), self.height.max(1));
        let hf = h as f32;
        let mut pm = Pixmap::new(w, h).expect("размер не нулевой");
        if let Some(r) = Rect::from_xywh(0.0, 0.0, w as f32, hf) {
            pm.fill_rect(r, &paint(BG), Transform::identity(), None);
        }
        let text_y = ((hf - TEXT.line_height()) / 2.0).round();
        for (part, x, pw) in self.parts(fonts) {
            if self.open == Some(part)
                && let Some(r) = rounded(x, 3.0, pw, hf - 6.0, 5.0)
            {
                pm.fill_path(
                    &r,
                    &paint(HIGHLIGHT),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            let tx = x + ITEM_PAD;
            let mut icon = |pm: &mut Pixmap, name: &str| {
                if let Some(icon) = icons.get(name, ICON) {
                    let k = ICON as f32 / icon.width() as f32;
                    let iy = ((hf - ICON as f32) / 2.0).round();
                    let t = Transform::from_scale(k, k).post_translate(tx, iy);
                    let quality = PixmapPaint {
                        quality: FilterQuality::Bicubic,
                        ..PixmapPaint::default()
                    };
                    pm.draw_pixmap(0, 0, icon.as_ref(), &quality, t, None);
                }
            };
            match part {
                Part::Apple => icon(&mut pm, &self.icon),
                Part::Tray(i) => {
                    if let Some(img) = self.tray.get(i) {
                        let k = ICON as f32 / img.width().max(img.height()) as f32;
                        let iy = ((hf - img.height() as f32 * k) / 2.0).round();
                        let t = Transform::from_scale(k, k).post_translate(tx, iy);
                        let quality = PixmapPaint {
                            quality: FilterQuality::Bicubic,
                            ..PixmapPaint::default()
                        };
                        pm.draw_pixmap(0, 0, img.as_ref(), &quality, t, None);
                    }
                }
                Part::Volume => {
                    if let Some(name) = &self.volume {
                        icon(&mut pm, name);
                    }
                }
                Part::Control => control_icon(&mut pm, tx, hf / 2.0),
                Part::Bell => {
                    if let Some(name) = &self.bell {
                        icon(&mut pm, name);
                    }
                }
                Part::Battery => {
                    if let Some((name, text)) = &self.battery {
                        icon(&mut pm, name);
                        let px = tx + ICON as f32 + BATTERY_GAP;
                        fonts.draw(&mut pm, text, &TEXT, px, text_y, None);
                    }
                }
                Part::App => {
                    let max = pw - 2.0 * ITEM_PAD + 1.0;
                    fonts.draw(&mut pm, &self.app_name, &BOLD, tx, text_y, Some(max));
                }
                Part::Layout => {
                    if let Some(layout) = &self.layout {
                        fonts.draw(&mut pm, layout, &BOLD, tx, text_y, None);
                    }
                }
                Part::Clock => fonts.draw(&mut pm, &self.clock, &TEXT, tx, text_y, None),
                Part::Menu(i) => {
                    if let Some(title) = self.menus.get(i) {
                        fonts.draw(&mut pm, title, &TEXT, tx, text_y, None);
                    }
                }
            }
        }
        pm
    }
}

/// Значок Пункта управления — два переключателя-«таблетки»: у верхнего
/// кружок справа, у нижнего слева (как в macOS). `x` — левый край, `cy` —
/// середина по высоте.
fn control_icon(pm: &mut Pixmap, x: f32, cy: f32) {
    let stroke = tiny_skia::Stroke {
        width: 1.3,
        ..tiny_skia::Stroke::default()
    };
    let color = paint([0xf2, 0xf2, 0xf2, 0xff]);
    for (dy, knob_right) in [(-3.6, true), (3.6, false)] {
        let y = cy + dy;
        if let Some(r) = rounded(x + 1.0, y - 3.0, 14.0, 6.0, 3.0) {
            pm.stroke_path(&r, &color, &stroke, Transform::identity(), None);
        }
        let kx = if knob_right { x + 12.0 } else { x + 4.0 };
        if let Some(c) = shell_widgets::shape::circle(kx, y, 2.2) {
            pm.fill_path(&c, &color, FillRule::Winding, Transform::identity(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> Bar {
        let mut b = Bar::new(1280, 28, "start-here");
        b.app_name = "Проводник".into();
        b.layout = Some("en".into());
        b.clock = "Чт 22 окт  11:49".into();
        b
    }

    #[test]
    fn app_menus_after_name_and_cut_before_right_side() {
        let mut fonts = Fonts::new("Inter");
        let mut b = bar();
        b.menus = vec!["Файл".into(), "Правка".into(), "Вид".into()];
        let parts = b.parts(&mut fonts);
        let x = |p: Part| parts.iter().find(|q| q.0 == p).map(|q| q.1);
        assert!(x(Part::App).unwrap() < x(Part::Menu(0)).unwrap());
        assert!(x(Part::Menu(0)).unwrap() < x(Part::Menu(2)).unwrap());
        // Узкий экран — меню, что не влезли, пропадают, а не налезают на часы.
        b.width = 420;
        b.menus = (0..10).map(|i| format!("Меню {i}")).collect();
        let parts = b.parts(&mut fonts);
        let menus = parts
            .iter()
            .filter(|p| matches!(p.0, Part::Menu(_)))
            .count();
        assert!(menus < 10);
        let clock = parts.iter().find(|p| p.0 == Part::Clock).unwrap().1;
        assert!(
            parts
                .iter()
                .all(|p| !matches!(p.0, Part::Menu(_)) || p.1 + p.2 <= clock)
        );
    }

    #[test]
    fn status_icons_between_layout_and_clock() {
        let mut fonts = Fonts::new("Inter");
        let mut b = bar();
        b.volume = Some("audio-volume-medium-symbolic".into());
        b.battery = Some(("battery-full-charged-symbolic".into(), "100%".into()));
        b.bell = Some("notification-symbolic".into());
        let parts = b.parts(&mut fonts);
        let mut right: Vec<(Part, f32)> = parts.iter().skip(2).map(|p| (p.0, p.1)).collect();
        right.sort_by(|a, b| a.1.total_cmp(&b.1));
        let order: Vec<Part> = right.iter().map(|p| p.0).collect();
        assert_eq!(
            order,
            [
                Part::Layout,
                Part::Volume,
                Part::Battery,
                Part::Bell,
                Part::Clock
            ]
        );
        // Трей — левее раскладки, по порядку.
        b.tray = vec![Pixmap::new(22, 22).unwrap(), Pixmap::new(16, 16).unwrap()];
        let parts = b.parts(&mut fonts);
        let x = |p: Part| parts.iter().find(|q| q.0 == p).unwrap().1;
        assert!(x(Part::Tray(0)) < x(Part::Tray(1)));
        assert!(x(Part::Tray(1)) < x(Part::Layout));
    }

    #[test]
    fn parts_left_and_right() {
        let mut fonts = Fonts::new("Inter");
        let b = bar();
        let parts = b.parts(&mut fonts);
        let order: Vec<Part> = parts.iter().map(|p| p.0).collect();
        assert_eq!(order, [Part::Apple, Part::App, Part::Clock, Part::Layout]);
        let (_, cx, cw) = parts[2];
        assert!(
            (cx + cw - (1280.0 - EDGE)).abs() < 0.5,
            "часы у правого края"
        );
        let (_, lx, lw) = parts[3];
        assert!(
            (lx + lw - cx).abs() < 0.5,
            "раскладка вплотную слева от часов"
        );
        assert_eq!(b.part_at(&mut fonts, 10.0), Some((Part::Apple, EDGE)));
        assert_eq!(b.part_at(&mut fonts, 640.0), None);
        assert_eq!(
            b.part_at(&mut fonts, lx + 2.0).map(|p| p.0),
            Some(Part::Layout)
        );
    }

    #[test]
    fn renders_text_and_highlight() {
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::with_dirs("none", vec![], vec![]);
        let mut b = bar();
        let plain = b.render(&mut fonts, &mut icons);
        assert_eq!((plain.width(), plain.height()), (1280, 28));
        // Полоса полупрозрачная, текст — светлый и непрозрачный.
        assert_eq!(plain.pixel(640, 14).unwrap().alpha(), BG[3]);
        let lit = |pm: &Pixmap| pm.pixels().iter().filter(|p| p.alpha() > 200).count();
        assert!(lit(&plain) > 100);

        b.open = Some(Part::Apple);
        let open = b.render(&mut fonts, &mut icons);
        let x = (EDGE + 2.0) as u32;
        assert!(open.pixel(x, 14).unwrap().alpha() > plain.pixel(x, 14).unwrap().alpha());
    }
}
