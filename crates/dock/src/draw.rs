//! Как выглядит док: тёмная полупрозрачная полоса со скруглёнными углами
//! (как тема WhiteSur-Dark для Plank), значки, точки под запущенными,
//! разделитель и подпись над значком под указателем.

use std::time::Instant;

use shell_icons::Icons;
use shell_text::{Fonts, Style};
use tiny_skia::{
    Color, FillRule, FilterQuality, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke,
    Transform,
};

use crate::Dock;
use crate::layout::{PAD_BOTTOM, SEPARATOR};

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
    pub fn render(&self, fonts: &mut Fonts, icons: &mut Icons, now: Instant) -> Pixmap {
        let w = self.screen_w.max(1.0) as u32;
        let h = self.window_height();
        let mut pm = Pixmap::new(w, h).expect("размер не нулевой");
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
        for s in &slots {
            let item = &self.items[s.item];
            if item.separator_before {
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

        // Подпись над значком под указателем.
        if let Some((s, icon_top)) = hovered {
            let name = &self.items[s.item].name;
            let tw = fonts.measure(name, &TIP_TEXT).min(400.0);
            let (bw, bh) = (tw + 20.0, TIP_TEXT.line_height() + 8.0);
            let bx = (s.center() - bw / 2.0)
                .clamp(4.0, self.screen_w - bw - 4.0)
                .round();
            let by = (icon_top - bh - 10.0).max(0.0).round();
            if let Some(r) = rounded(bx, by, bw, bh, 7.0) {
                pm.fill_path(
                    &r,
                    &paint(TIP),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            if let Some(r) = rounded(bx + 0.5, by + 0.5, bw - 1.0, bh - 1.0, 6.5) {
                let stroke = Stroke {
                    width: 1.0,
                    ..Stroke::default()
                };
                pm.stroke_path(&r, &paint(BAR_EDGE), &stroke, Transform::identity(), None);
            }
            fonts.draw(
                &mut pm,
                name,
                &TIP_TEXT,
                bx + 10.0,
                by + 4.0,
                Some(tw + 1.0),
            );
        }
        pm
    }
}

fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], c[3]));
    p.anti_alias = true;
    p
}

fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(c), Transform::identity(), None);
    }
}

fn circle(x: f32, y: f32, r: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.push_circle(x, y, r);
    pb.finish()
}

/// Прямоугольник со скруглёнными углами.
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let k = r * 0.552_284_8;
    let (x1, y1) = (x + w, y + h);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + k, y, x1, y + r - k, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + k, x1 - r + k, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - k, y1, x, y1 - r + k, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{dock, window};

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
        assert!(pm.data().iter().all(|&b| b == 0));
    }
}
