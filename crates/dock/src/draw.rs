//! Как выглядит док: тёмная полупрозрачная полоса со скруглёнными углами
//! (как тема WhiteSur-Dark для Plank), значки, точки под запущенными,
//! разделитель и подпись над значком под указателем.

use mimshel_render::Canvas;
use std::time::Instant;

use mimshel_icons::Icons;
use mimshel_text::{Fonts, Style};
use tiny_skia::{FillRule, FilterQuality, PixmapPaint, Rect, Stroke, Transform};

use crate::Dock;
use crate::layout::{PAD_BOTTOM, SEPARATOR};
use mimshel_widgets::shape::{circle, paint, rounded};

const BAR: [u8; 4] = [0x19, 0x19, 0x19, 0xbe];
const BAR_EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
const SEPARATOR_LINE: [u8; 4] = [0xff, 0xff, 0xff, 0x40];
const DOT: [u8; 4] = [0xf2, 0xf2, 0xf2, 0xd8];
const TIP: [u8; 4] = [0x28, 0x28, 0x28, 0xeb];
const RADIUS: f32 = 18.0;

const TIP_TEXT: Style = Style {
    size: 14.0,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 400,
};

impl Dock {
    /// Рисует окно дока (ширина экрана × [`Dock::window_height`]).
    pub fn render(&self, fonts: &mut Fonts, icons: &mut Icons, now: Instant) -> Canvas {
        let w = self.screen_w.max(1.0) as u32;
        let h = self.window_height();
        let mut pm = Canvas::new(w, h).expect("размер не нулевой");
        let hidden = self.hidden_at(now);
        if hidden >= 1.0 || self.items.is_empty() {
            return pm;
        }
        let hf = h as f32;
        let bottom = self.bar_bottom(hf, hidden);
        let top = bottom - self.bar_height();
        let (slots, bar_x, bar_w) = self.slots_at(self.pointer);

        if let Some(r) = rounded(bar_x, top, bar_w, self.bar_height(), RADIUS) {
            pm.fill_path(
                &r,
                &paint(BAR),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        if let Some(r) = rounded(
            bar_x + 0.5,
            top + 0.5,
            bar_w - 1.0,
            self.bar_height() - 1.0,
            RADIUS - 0.5,
        ) {
            let stroke = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&r, &paint(BAR_EDGE), &stroke, Transform::identity(), None);
        }

        let max = self.max_icon();
        let quality = PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        };
        let mut hovered = None;
        let dragged = self.drag.map(|d| d.index);
        for s in &slots {
            let item = &self.items[s.item];
            if s.separator_before {
                let x = (s.x - SEPARATOR / 2.0).round();
                fill_rect(
                    &mut pm,
                    x,
                    top + 10.0,
                    1.0,
                    self.bar_height() - 20.0,
                    SEPARATOR_LINE,
                );
            }
            // Перетаскиваемый значок едет за мышью в своём окне, здесь — пустое место.
            if Some(s.item) == dragged {
                continue;
            }
            let jump = self.bounce(&item.key, now) * self.look.icon as f32;
            let ix = s.center() - s.size / 2.0;
            let iy = bottom - PAD_BOTTOM - s.size - jump;
            let icon = match (&item.app, item.windows.first()) {
                (Some(app), _) => app.icon.as_deref().and_then(|n| icons.get(n, max)),
                (None, Some(&w)) => self.window_icon(w),
                (None, None) => None,
            };
            if let Some(icon) = icon {
                let k = s.size / icon.width() as f32;
                let t = Transform::from_scale(k, k).post_translate(ix, iy);
                pm.draw_pixmap(0, 0, icon.as_ref(), &quality, t, None);
            }
            if !item.windows.is_empty() {
                let dot = circle(s.center(), bottom - 5.0, 2.5);
                if let Some(dot) = dot {
                    pm.fill_path(
                        &dot,
                        &paint(DOT),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
            if let Some(px) = self.pointer
                && px >= s.x
                && px < s.x + s.w
            {
                hovered = Some((s, iy));
            }
        }

        // Подпись над значком под указателем (не при открытом меню).
        if let Some((s, icon_top)) = hovered.filter(|_| !self.popup_open && dragged.is_none()) {
            let name = &self.items[s.item].name;
            let (bw, bh) = tip_size(fonts, name);
            let bx = (s.center() - bw / 2.0)
                .clamp(4.0, self.screen_w - bw - 4.0)
                .round();
            let by = (icon_top - bh - 10.0).max(0.0).round();
            tip(&mut pm, fonts, name, bx, by);
        }
        pm
    }

    /// Картинка перетаскиваемого значка (для окна, которое едет за мышью) и
    /// где в ней середина иконки. Утащен прочь закреплённый — над ним «Удалить».
    pub fn drag_image(&self, fonts: &mut Fonts, icons: &mut Icons) -> Option<(Canvas, (f32, f32))> {
        let item = self.items.get(self.drag?.index)?;
        let size = self.max_icon() as f32;
        let label = self
            .drag_removes()
            .then_some(mimshel_core::i18n::t(self.language, "dock.remove"));
        let (tw, th) = label.map_or((0.0, 0.0), |t| tip_size(fonts, t));
        let top = if label.is_some() { th + 8.0 } else { 0.0 };
        let w = size.max(tw).ceil();
        let mut pm = Canvas::new(w as u32, (top + size).ceil() as u32)?;
        let icon = match (&item.app, item.windows.first()) {
            (Some(app), _) => app.icon.as_deref().and_then(|n| icons.get(n, size as u32)),
            (None, Some(&win)) => self.window_icon(win),
            (None, None) => None,
        };
        if let Some(icon) = icon {
            let k = size / icon.width() as f32;
            let t = Transform::from_scale(k, k).post_translate((w - size) / 2.0, top);
            let quality = PixmapPaint {
                quality: FilterQuality::Bicubic,
                ..PixmapPaint::default()
            };
            pm.draw_pixmap(0, 0, icon.as_ref(), &quality, t, None);
        }
        if let Some(text) = label {
            tip(&mut pm, fonts, text, ((w - tw) / 2.0).round(), 0.0);
        }
        Some((pm, (w / 2.0, top + size / 2.0)))
    }
}

/// Сколько длится «облачко», когда значок убрали из дока.
pub const POOF: std::time::Duration = std::time::Duration::from_millis(300);

/// Кадр «облачка» размером `size`: `t` от 0 до 1 — ход анимации. Клубы
/// разлетаются от середины, растут и тают.
pub fn poof_image(size: u32, t: f32) -> Canvas {
    let s = size as f32;
    let t = t.clamp(0.0, 1.0);
    let mut pm = Canvas::new(size.max(1), size.max(1)).expect("размер не нулевой");
    let alpha = ((1.0 - t) * 230.0) as u8;
    let spread = s * (0.10 + 0.22 * t);
    let r = s * (0.13 + 0.10 * t);
    let puffs = 7;
    let mut draw = |x: f32, y: f32, r: f32| {
        if let Some(c) = circle(x, y, r) {
            pm.fill_path(
                &c,
                &paint([0xee, 0xee, 0xee, alpha]),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    };
    for k in 0..puffs {
        let a = k as f32 * std::f32::consts::TAU / puffs as f32 + t * 0.6;
        draw(
            s / 2.0 + spread * a.cos(),
            s / 2.0 + 0.8 * spread * a.sin(),
            r,
        );
    }
    draw(s / 2.0, s / 2.0, r * 1.2);
    pm
}

/// Размер подписи (тёмной плашки с текстом).
fn tip_size(fonts: &mut Fonts, text: &str) -> (f32, f32) {
    let tw = fonts.measure(text, &TIP_TEXT).min(400.0);
    (tw + 20.0, TIP_TEXT.line_height() + 8.0)
}

/// Подпись с левым верхним углом в `(x, y)`.
fn tip(pm: &mut Canvas, fonts: &mut Fonts, text: &str, x: f32, y: f32) {
    let (bw, bh) = tip_size(fonts, text);
    if let Some(r) = rounded(x, y, bw, bh, 7.0) {
        pm.fill_path(
            &r,
            &paint(TIP),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some(r) = rounded(x + 0.5, y + 0.5, bw - 1.0, bh - 1.0, 6.5) {
        let stroke = Stroke {
            width: 1.0,
            ..Stroke::default()
        };
        pm.stroke_path(&r, &paint(BAR_EDGE), &stroke, Transform::identity(), None);
    }
    fonts.draw(pm, text, &TIP_TEXT, x + 10.0, y + 4.0, Some(bw - 19.0));
}

fn fill_rect(pm: &mut Canvas, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(c), Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{dock, window};

    #[test]
    fn poof_fades_out() {
        let lit = |pm: &Canvas| pm.pixels().iter().filter(|p| p.alpha() > 20).count();
        let start = poof_image(64, 0.0);
        let mid = poof_image(64, 0.5);
        assert!(lit(&start) > 500 && lit(&mid) > 500);
        assert_eq!(lit(&poof_image(64, 1.0)), 0, "в конце не видно");
    }

    #[test]
    fn renders_bar_dot_and_tooltip() {
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::with_dirs("none", vec![], vec![]);
        let now = Instant::now();
        let mut d = dock();
        d.set_windows(vec![(window(7, "firefox", "firefox"), None)]);
        let pm = d.render(&mut fonts, &mut icons, now);
        assert_eq!((pm.width(), pm.height()), (1600, d.window_height()));
        let bar = d.bar_rect(d.window_height());
        // Полоса тёмная полупрозрачная, над ней пусто.
        let inside = pm.pixel(bar.x as u32 + 30, bar.y as u32 + 30).unwrap();
        assert!(inside.alpha() > 150 && inside.red() < 60);
        assert_eq!(pm.pixel(bar.x as u32 + 30, 2).unwrap().alpha(), 0);
        // Точка под Firefox (второй значок).
        let (slots, ..) = d.slots_at(None);
        let dot = pm
            .pixel(slots[1].center() as u32, (bar.y + bar.h as i32 - 5) as u32)
            .unwrap();
        assert!(dot.red() > 200, "{}", dot.red());

        // Подпись появляется над значком под указателем.
        d.set_pointer(Some(slots[0].center()));
        let pm = d.render(&mut fonts, &mut icons, now);
        let lit = (0..bar.y as u32)
            .flat_map(|y| (0..1600).map(move |x| (x, y)))
            .filter(|&(x, y)| pm.pixel(x, y).unwrap().alpha() > 200)
            .count();
        assert!(lit > 500, "{lit}");

        // Спрятан — пусто.
        d.slide_to(true, now);
        d.settle(now + crate::SLIDE);
        let pm = d.render(&mut fonts, &mut icons, now + crate::SLIDE);
        assert!(pm.pixmap().data().iter().all(|&b| b == 0));
    }
}
