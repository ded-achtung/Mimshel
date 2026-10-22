//! Уведомления на экране, как в macOS Sonoma: баннеры справа сверху под
//! строкой меню — иконка программы, её название, заголовок, до двух строк
//! текста и кнопки действий; крестик при наведении. Здесь — что показывать,
//! когда убрать, раскладка и рисование в картинку; окно и время — у службы.

use std::time::{Duration, Instant};

pub use shell_dbus::notify::{CloseReason, Notification};
use shell_text::{Fonts, Style};
use shell_widgets::shape::{circle, paint, rounded};
use tiny_skia::{
    FillRule, FilterQuality, LineCap, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform,
};

/// Ширина баннера и окна под них.
pub const WIDTH: f32 = 344.0;
/// Обычное уведомление исчезает через столько (если программа не попросила иначе).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// Больше баннеров сразу не показываем — старые уходят в историю.
pub const MAX_BANNERS: usize = 3;

const PAD: f32 = 12.0;
const ICON: f32 = 32.0;
/// Между баннерами.
const GAP: f32 = 8.0;
const RADIUS: f32 = 14.0;
const TEXT_X: f32 = PAD + ICON + 10.0;
const LINE: f32 = 18.0;
const BUTTON_H: f32 = 26.0;
/// Крестик «закрыть» в левом верхнем углу (при наведении).
const CLOSE_R: f32 = 9.0;
const CLOSE_C: (f32, f32) = (4.0 + CLOSE_R, 4.0 + CLOSE_R);

const BG: [u8; 4] = [0x2c, 0x2c, 0x2e, 0xeb];
const EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
const BUTTON: [u8; 4] = [0xff, 0xff, 0xff, 0x1f];
const BUTTON_HOVER: [u8; 4] = [0xff, 0xff, 0xff, 0x38];

const APP: Style = Style {
    size: 11.5,
    color: [0xa8, 0xa8, 0xad, 0xff],
    weight: 500,
};
const TITLE: Style = Style {
    size: 13.5,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 700,
};
const BODY: Style = Style {
    size: 13.5,
    color: [0xe6, 0xe6, 0xe6, 0xff],
    weight: 400,
};
const BUTTON_TEXT: Style = Style {
    size: 12.5,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 500,
};

/// Уведомление на экране.
pub struct Banner {
    pub n: Notification,
    /// Иконка программы или картинка уведомления (подобрала служба).
    pub icon: Option<Pixmap>,
    pub shown_at: Instant,
}

impl Banner {
    /// Когда убрать сам. `None` — пока не закроют (срочное или так просила программа).
    pub fn expires(&self) -> Option<Instant> {
        if self.n.urgency >= 2 {
            return None;
        }
        match self.n.expire_timeout {
            0 => None,
            t if t > 0 => Some(self.shown_at + Duration::from_millis(t as u64)),
            _ => Some(self.shown_at + DEFAULT_TIMEOUT),
        }
    }

    /// Кнопки — действия, кроме `default` (оно — щелчок по самому баннеру).
    fn buttons(&self) -> Vec<&(String, String)> {
        self.n
            .actions
            .iter()
            .filter(|(k, l)| k != "default" && !l.is_empty())
            .take(3)
            .collect()
    }
}

/// Куда пришёлся щелчок по баннеру.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// Сам баннер — действие `default` (если есть) и закрыть.
    Body(u32),
    Close(u32),
    /// Кнопка действия: id уведомления и ключ действия.
    Action(u32, String),
}

/// Стопка баннеров: новые — сверху.
#[derive(Default)]
pub struct Banners {
    pub items: Vec<Banner>,
    /// Баннер под указателем (номер) и кнопка под ним.
    pub hover: Option<usize>,
    pub hover_button: Option<usize>,
}

impl Banners {
    /// Показать уведомление: с тем же id — заменить на месте, иначе сверху.
    /// Ответ — уведомления, которые не влезли (уходят в историю).
    pub fn show(
        &mut self,
        n: Notification,
        icon: Option<Pixmap>,
        now: Instant,
    ) -> Vec<Notification> {
        let banner = Banner {
            n,
            icon,
            shown_at: now,
        };
        if let Some(old) = self.items.iter_mut().find(|b| b.n.id == banner.n.id) {
            *old = banner;
            return Vec::new();
        }
        self.items.insert(0, banner);
        let mut gone = Vec::new();
        while self.items.len() > MAX_BANNERS {
            if let Some(b) = self.items.pop() {
                gone.push(b.n);
            }
        }
        gone
    }

    pub fn remove(&mut self, id: u32) -> Option<Notification> {
        let i = self.items.iter().position(|b| b.n.id == id)?;
        if self.hover == Some(i) {
            self.hover = None;
        }
        Some(self.items.remove(i).n)
    }

    /// Убрать те, чьё время вышло (кроме баннера под указателем — пока на
    /// него смотрят, он не исчезает). Ответ — убранные.
    pub fn expire(&mut self, now: Instant) -> Vec<Notification> {
        let hover = self.hover.and_then(|i| self.items.get(i)).map(|b| b.n.id);
        let mut gone = Vec::new();
        self.items.retain(|b| {
            let keep = Some(b.n.id) == hover || b.expires().is_none_or(|t| t > now);
            if !keep {
                gone.push(b.n.clone());
            }
            keep
        });
        if !gone.is_empty() {
            self.hover = None;
        }
        gone
    }

    /// Ближайший момент, когда что-то исчезнет (разбудить службу).
    pub fn next_expiry(&self) -> Option<Instant> {
        self.items.iter().filter_map(Banner::expires).min()
    }

    /// Строки текста баннера (до двух, по словам).
    fn body_lines(fonts: &mut Fonts, b: &Banner) -> Vec<String> {
        wrap(fonts, &b.n.body, &BODY, WIDTH - TEXT_X - PAD, 2)
    }

    fn banner_height(fonts: &mut Fonts, b: &Banner) -> f32 {
        let lines = Self::body_lines(fonts, b).len() as f32;
        let text = PAD + 16.0 + LINE + lines * LINE + PAD;
        let buttons = if b.buttons().is_empty() {
            0.0
        } else {
            BUTTON_H + 8.0
        };
        (text + buttons).max(ICON + 2.0 * PAD).ceil()
    }

    /// Верх и высота каждого баннера.
    pub fn layout(&self, fonts: &mut Fonts) -> Vec<(f32, f32)> {
        let mut y = 0.0;
        self.items
            .iter()
            .map(|b| {
                let h = Self::banner_height(fonts, b);
                let at = (y, h);
                y += h + GAP;
                at
            })
            .collect()
    }

    /// Размер окна под все баннеры.
    pub fn size(&self, fonts: &mut Fonts) -> (u32, u32) {
        let h = self.layout(fonts).last().map_or(0.0, |&(y, h)| y + h);
        (WIDTH as u32, h.ceil() as u32)
    }

    /// Кнопки баннера: левый край и ширина (в координатах баннера).
    fn button_rects(fonts: &mut Fonts, b: &Banner, h: f32) -> Vec<(f32, f32, f32)> {
        let mut x = TEXT_X;
        let y = h - PAD - BUTTON_H + 4.0;
        b.buttons()
            .iter()
            .map(|(_, label)| {
                let w = fonts.measure(label, &BUTTON_TEXT) + 24.0;
                let r = (x, y, w);
                x += w + 6.0;
                r
            })
            .collect()
    }

    /// Указатель в точке окна. `true` — надо перерисовать.
    pub fn motion(&mut self, fonts: &mut Fonts, x: f32, y: f32) -> bool {
        let layout = self.layout(fonts);
        let hover = layout.iter().position(|&(top, h)| y >= top && y < top + h);
        let button = hover.and_then(|i| {
            let (top, h) = layout[i];
            Self::button_rects(fonts, &self.items[i], h)
                .iter()
                .position(|&(bx, by, bw)| {
                    x >= bx && x < bx + bw && y - top >= by && y - top < by + BUTTON_H
                })
        });
        let changed = hover != self.hover || button != self.hover_button;
        self.hover = hover;
        self.hover_button = button;
        changed
    }

    pub fn leave(&mut self) -> bool {
        let changed = self.hover.is_some();
        self.hover = None;
        self.hover_button = None;
        changed
    }

    pub fn hit(&self, fonts: &mut Fonts, x: f32, y: f32) -> Option<Hit> {
        let layout = self.layout(fonts);
        let i = layout
            .iter()
            .position(|&(top, h)| y >= top && y < top + h)?;
        let (top, h) = layout[i];
        let b = &self.items[i];
        let (ly, id) = (y - top, b.n.id);
        let (cx, cy) = CLOSE_C;
        if (x - cx).powi(2) + (ly - cy).powi(2) <= (CLOSE_R + 3.0).powi(2) {
            return Some(Hit::Close(id));
        }
        for (k, &(bx, by, bw)) in Self::button_rects(fonts, b, h).iter().enumerate() {
            if x >= bx && x < bx + bw && ly >= by && ly < by + BUTTON_H {
                return Some(Hit::Action(id, b.buttons()[k].0.clone()));
            }
        }
        Some(Hit::Body(id))
    }

    pub fn render(&self, fonts: &mut Fonts) -> Pixmap {
        let (w, h) = self.size(fonts);
        let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("размер не нулевой");
        let layout = self.layout(fonts);
        for (i, (b, &(top, bh))) in self.items.iter().zip(&layout).enumerate() {
            self.draw_banner(&mut pm, fonts, b, top, bh, self.hover == Some(i));
        }
        pm
    }

    fn draw_banner(
        &self,
        pm: &mut Pixmap,
        fonts: &mut Fonts,
        b: &Banner,
        top: f32,
        h: f32,
        hover: bool,
    ) {
        let fill = |pm: &mut Pixmap, path: Option<tiny_skia::Path>, c: [u8; 4]| {
            if let Some(p) = path {
                pm.fill_path(
                    &p,
                    &paint(c),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        };
        fill(pm, rounded(0.0, top, WIDTH, h, RADIUS), BG);
        if let Some(r) = rounded(0.5, top + 0.5, WIDTH - 1.0, h - 1.0, RADIUS - 0.5) {
            let stroke = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&r, &paint(EDGE), &stroke, Transform::identity(), None);
        }
        if let Some(icon) = &b.icon {
            let k = ICON / icon.width().max(icon.height()) as f32;
            let t = Transform::from_scale(k, k).post_translate(PAD, top + PAD + 2.0);
            let q = PixmapPaint {
                quality: FilterQuality::Bicubic,
                ..PixmapPaint::default()
            };
            pm.draw_pixmap(0, 0, icon.as_ref(), &q, t, None);
        }
        let text_w = WIDTH - TEXT_X - PAD;
        let mut y = top + PAD - 2.0;
        let now = "сейчас";
        let now_w = fonts.measure(now, &APP);
        let app = b.n.app_name.to_uppercase();
        fonts.draw(pm, &app, &APP, TEXT_X, y, Some(text_w - now_w - 8.0));
        fonts.draw(pm, now, &APP, WIDTH - PAD - now_w, y, None);
        y += 16.0;
        if !b.n.summary.is_empty() {
            fonts.draw(pm, &b.n.summary, &TITLE, TEXT_X, y, Some(text_w));
            y += LINE;
        }
        for line in Self::body_lines(fonts, b) {
            fonts.draw(pm, &line, &BODY, TEXT_X, y, Some(text_w));
            y += LINE;
        }
        for (k, &(bx, by, bw)) in Self::button_rects(fonts, b, h).iter().enumerate() {
            let c = if hover && self.hover_button == Some(k) {
                BUTTON_HOVER
            } else {
                BUTTON
            };
            fill(pm, rounded(bx, top + by, bw, BUTTON_H, 7.0), c);
            let label = &b.buttons()[k].1;
            let tw = fonts.measure(label, &BUTTON_TEXT);
            let ty = top + by + ((BUTTON_H - BUTTON_TEXT.line_height()) / 2.0).round();
            fonts.draw(pm, label, &BUTTON_TEXT, bx + (bw - tw) / 2.0, ty, None);
        }
        if hover {
            let (cx, cy) = CLOSE_C;
            fill(pm, circle(cx, top + cy, CLOSE_R), [0x55, 0x55, 0x58, 0xf5]);
            let mut pb = PathBuilder::new();
            let d = 3.5;
            pb.move_to(cx - d, top + cy - d);
            pb.line_to(cx + d, top + cy + d);
            pb.move_to(cx + d, top + cy - d);
            pb.line_to(cx - d, top + cy + d);
            if let Some(p) = pb.finish() {
                let s = Stroke {
                    width: 1.6,
                    line_cap: LineCap::Round,
                    ..Stroke::default()
                };
                pm.stroke_path(
                    &p,
                    &paint([0xf2, 0xf2, 0xf2, 0xff]),
                    &s,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

/// Текст по словам в строки шириной `width`, не больше `max` строк (в
/// последнюю — всё оставшееся; лишнее при рисовании заменится «…»).
pub fn wrap(fonts: &mut Fonts, text: &str, style: &Style, width: f32, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if line.is_empty()
                || fonts.measure(&candidate, style) <= width
                || lines.len() + 1 >= max
            {
                line = candidate;
            } else {
                lines.push(std::mem::take(&mut line));
                line = word.to_string();
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
        if lines.len() >= max {
            break;
        }
    }
    lines.truncate(max);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u32, body: &str) -> Notification {
        Notification {
            id,
            app_name: "Firefox".into(),
            app_icon: String::new(),
            summary: "Загрузка завершена".into(),
            body: body.into(),
            actions: vec![],
            image: None,
            desktop_entry: None,
            urgency: 1,
            expire_timeout: -1,
            transient: false,
        }
    }

    #[test]
    fn stack_newest_first_and_overflow_to_history() {
        let now = Instant::now();
        let mut b = Banners::default();
        for id in 1..=4 {
            let gone = b.show(note(id, ""), None, now);
            assert_eq!(gone.len(), usize::from(id == 4));
        }
        let ids: Vec<u32> = b.items.iter().map(|x| x.n.id).collect();
        assert_eq!(ids, [4, 3, 2]);
        // Замена с тем же id — на месте.
        b.show(note(3, "новый текст"), None, now);
        assert_eq!(b.items[1].n.body, "новый текст");
    }

    #[test]
    fn expiry_respects_hover_and_urgency() {
        let now = Instant::now();
        let mut b = Banners::default();
        b.show(note(1, ""), None, now);
        let mut urgent = note(2, "");
        urgent.urgency = 2;
        b.show(urgent, None, now);
        b.show(note(3, ""), None, now);
        b.hover = Some(0); // id 3 под указателем
        let gone = b.expire(now + DEFAULT_TIMEOUT + Duration::from_millis(1));
        assert_eq!(gone.iter().map(|n| n.id).collect::<Vec<_>>(), [1]);
        let left: Vec<u32> = b.items.iter().map(|x| x.n.id).collect();
        assert_eq!(left, [3, 2]);
        assert_eq!(b.next_expiry(), Some(now + DEFAULT_TIMEOUT));
    }

    #[test]
    fn wrap_and_hits() {
        let mut fonts = Fonts::new("Inter");
        let long =
            "Файл «отчёт за третий квартал.pdf» загружен в папку Загрузки и готов к открытию";
        let lines = wrap(&mut fonts, long, &BODY, 200.0, 2);
        assert_eq!(lines.len(), 2);
        assert!(fonts.measure(&lines[0], &BODY) <= 200.0);

        let now = Instant::now();
        let mut b = Banners::default();
        let mut n = note(7, long);
        n.actions = vec![
            ("default".into(), "".into()),
            ("open".into(), "Открыть".into()),
        ];
        b.show(n, None, now);
        let (top, h) = b.layout(&mut fonts)[0];
        assert_eq!(
            b.hit(&mut fonts, CLOSE_C.0, top + CLOSE_C.1),
            Some(Hit::Close(7))
        );
        assert_eq!(b.hit(&mut fonts, 200.0, top + 30.0), Some(Hit::Body(7)));
        let by = top + h - PAD - BUTTON_H + 4.0 + 5.0;
        assert_eq!(
            b.hit(&mut fonts, TEXT_X + 5.0, by),
            Some(Hit::Action(7, "open".into()))
        );
        let pm = b.render(&mut fonts);
        assert_eq!((pm.width(), pm.height()), b.size(&mut fonts));
    }
}
