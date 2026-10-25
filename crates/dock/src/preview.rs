//! Миниатюры окон над значком: задержали мышь на значке запущенной
//! программы — над ним карточки её окон (картинка и заголовок), клик по
//! карточке переключает на окно. Здесь раскладка, наведение и рисование;
//! снимки окон делает служба.

use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use tiny_skia::{FillRule, FilterQuality, IntSize, Pixmap, PixmapPaint, Rect, Stroke, Transform};

use mimshel_widgets::shape::{paint, rounded};

/// Окно в миниатюрах.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewCard {
    pub window: u32,
    pub title: String,
    /// Уменьшенный снимок окна (или иконка программы, если снимка нет —
    /// например, окно свёрнуто). Больше места под картинку — уменьшается.
    pub image: Option<Pixmap>,
}

/// Открытые миниатюры значка.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub key: String,
    pub cards: Vec<PreviewCard>,
    /// Карточка под указателем.
    pub hover: Option<usize>,
}

/// Место под картинку окна.
pub const THUMB_W: u32 = 200;
pub const THUMB_H: u32 = 125;
const PAD: f32 = 10.0;
const GAP: f32 = 10.0;
/// Между картинкой и заголовком.
const TITLE_GAP: f32 = 6.0;
const RADIUS: f32 = 10.0;

const BG: [u8; 4] = [0x28, 0x28, 0x28, 0xf0];
const EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const HOVER: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
const FRAME: [u8; 4] = [0xff, 0xff, 0xff, 0x30];

const TITLE: Style = Style {
    size: 12.5,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 400,
};

impl Preview {
    /// Сколько карточек влезет в ширину экрана `screen_w`.
    pub fn fits(screen_w: u32) -> usize {
        let room = screen_w as f32 - 2.0 * PAD - 8.0 + GAP;
        ((room / (THUMB_W as f32 + GAP)) as usize).max(1)
    }

    fn card_h() -> f32 {
        THUMB_H as f32 + TITLE_GAP + TITLE.line_height()
    }

    pub fn size(&self) -> (u32, u32) {
        let n = self.cards.len().max(1) as f32;
        let w = 2.0 * PAD + n * THUMB_W as f32 + (n - 1.0) * GAP;
        let h = 2.0 * PAD + Self::card_h();
        (w as u32, h.ceil() as u32)
    }

    /// Левый верхний угол карточки `i`.
    fn card_pos(i: usize) -> (f32, f32) {
        (PAD + i as f32 * (THUMB_W as f32 + GAP), PAD)
    }

    /// Карточка в точке `(x, y)` окна миниатюр.
    pub fn card_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.cards.len()).find(|&i| {
            let (cx, cy) = Self::card_pos(i);
            x >= cx - GAP / 2.0
                && x < cx + THUMB_W as f32 + GAP / 2.0
                && y >= cy
                && y < cy + Self::card_h()
        })
    }

    /// Указатель сдвинулся. `true` — подсветка поменялась.
    pub fn motion(&mut self, x: f32, y: f32) -> bool {
        let hover = self.card_at(x, y);
        let changed = hover != self.hover;
        self.hover = hover;
        changed
    }

    pub fn render(&self, fonts: &mut Fonts) -> Canvas {
        let (w, h) = self.size();
        let (wf, hf) = (w as f32, h as f32);
        let mut pm = Canvas::new(w, h).expect("размер не нулевой");
        if let Some(r) = rounded(0.0, 0.0, wf, hf, RADIUS) {
            pm.fill_path(
                &r,
                &paint(BG),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
        let stroke = Stroke {
            width: 1.0,
            ..Stroke::default()
        };
        if let Some(r) = rounded(0.5, 0.5, wf - 1.0, hf - 1.0, RADIUS - 0.5) {
            pm.stroke_path(&r, &paint(EDGE), &stroke, Transform::identity(), None);
        }
        let quality = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        for (i, card) in self.cards.iter().enumerate() {
            let (x, y) = Self::card_pos(i);
            if self.hover == Some(i)
                && let Some(r) = rounded(
                    x - 4.0,
                    y - 4.0,
                    THUMB_W as f32 + 8.0,
                    Self::card_h() + 6.0,
                    7.0,
                )
            {
                pm.fill_path(
                    &r,
                    &paint(HOVER),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            if let Some(img) = &card.image {
                // Вписать в место под картинку, по центру; маленькое (иконку)
                // не растягивать.
                let k = (THUMB_W as f32 / img.width() as f32)
                    .min(THUMB_H as f32 / img.height() as f32)
                    .min(1.0);
                let (iw, ih) = (img.width() as f32 * k, img.height() as f32 * k);
                let ix = (x + (THUMB_W as f32 - iw) / 2.0).round();
                let iy = (y + (THUMB_H as f32 - ih) / 2.0).round();
                let t = Transform::from_scale(k, k).post_translate(ix, iy);
                pm.draw_pixmap(0, 0, img.as_ref(), &quality, t, None);
                // Тонкая рамка вокруг снимка окна (у иконки не нужна).
                if iw > 96.0
                    && let Some(r) = Rect::from_xywh(ix + 0.5, iy + 0.5, iw - 1.0, ih - 1.0)
                {
                    let path = tiny_skia::PathBuilder::from_rect(r);
                    pm.stroke_path(&path, &paint(FRAME), &stroke, Transform::identity(), None);
                }
            }
            let tw = fonts.measure(&card.title, &TITLE).min(THUMB_W as f32);
            let tx = (x + (THUMB_W as f32 - tw) / 2.0).round();
            let ty = y + THUMB_H as f32 + TITLE_GAP;
            fonts.draw(&mut pm, &card.title, &TITLE, tx, ty, Some(THUMB_W as f32));
        }
        pm
    }
}

/// Уменьшенная копия картинки, вписанная в `max_w` × `max_h` (усреднение
/// пикселей — без «лесенки», в отличие от простой выборки).
pub fn thumbnail(src: &Pixmap, max_w: u32, max_h: u32) -> Pixmap {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let k = (max_w as f32 / sw as f32)
        .min(max_h as f32 / sh as f32)
        .min(1.0);
    let w = ((sw as f32 * k).round() as usize).max(1);
    let h = ((sh as f32 * k).round() as usize).max(1);
    let data = src.data();
    let mut out = vec![0u8; w * h * 4];
    for oy in 0..h {
        let (y0, y1) = (oy * sh / h, ((oy + 1) * sh / h).max(oy * sh / h + 1));
        for ox in 0..w {
            let (x0, x1) = (ox * sw / w, ((ox + 1) * sw / w).max(ox * sw / w + 1));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                let row = &data[(y * sw + x0) * 4..(y * sw + x1) * 4];
                for px in row.as_chunks::<4>().0 {
                    for c in 0..4 {
                        sum[c] += u32::from(px[c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (oy * w + ox) * 4;
            for c in 0..4 {
                out[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    Pixmap::from_vec(
        out,
        IntSize::from_wh(w as u32, h as u32).expect("размер не нулевой"),
    )
    .expect("размер данных совпадает")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(n: usize) -> Preview {
        Preview {
            key: "firefox.desktop".into(),
            cards: (0..n)
                .map(|i| PreviewCard {
                    window: 10 + i as u32,
                    title: format!("Окно {i}"),
                    image: None,
                })
                .collect(),
            hover: None,
        }
    }

    #[test]
    fn layout_and_hit_testing() {
        let p = preview(3);
        let (w, h) = p.size();
        assert_eq!(w, (2.0 * PAD + 3.0 * 200.0 + 2.0 * GAP) as u32);
        assert!(h > THUMB_H);
        let (x1, y1) = Preview::card_pos(1);
        assert_eq!(p.card_at(x1 + 50.0, y1 + 50.0), Some(1));
        assert_eq!(p.card_at(x1 + 50.0, 1.0), None, "поле сверху");
        assert_eq!(Preview::fits(1278), 6);
    }

    #[test]
    fn thumbnail_keeps_shape_and_averages() {
        // Полоски: чёрная и белая колонки через одну — в уменьшенной серое.
        let mut src = Pixmap::new(400, 250).unwrap();
        for (i, px) in src.data_mut().as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let v = if i % 2 == 0 { 0 } else { 255 };
            *px = [v, v, v, 255];
        }
        let t = thumbnail(&src, THUMB_W, THUMB_H);
        assert_eq!((t.width(), t.height()), (200, 125));
        let p = t.pixel(80, 60).unwrap();
        assert_eq!(p.red(), 127);
        // Маленькую не увеличивает.
        let small = Pixmap::new(64, 64).unwrap();
        assert_eq!(thumbnail(&small, THUMB_W, THUMB_H).width(), 64);
    }

    #[test]
    fn renders_hover() {
        let mut fonts = Fonts::new("Inter");
        let mut p = preview(2);
        let mut img = Pixmap::new(160, 100).unwrap();
        img.fill(tiny_skia::Color::from_rgba8(200, 30, 30, 255));
        p.cards[0].image = Some(img);
        p.hover = Some(1);
        let pm = p.render(&mut fonts);
        let (x0, y0) = Preview::card_pos(0);
        let red = pm.pixel((x0 + 100.0) as u32, (y0 + 62.0) as u32).unwrap();
        assert!(red.red() > 150 && red.green() < 60);
        // Подсветка второй карточки светлее фона.
        let (x1, y1) = Preview::card_pos(1);
        let lit = pm.pixel((x1 + 100.0) as u32, (y1 + 62.0) as u32).unwrap();
        let bg = pm.pixel((x0 + 100.0) as u32, (y0 + 2.0) as u32).unwrap();
        assert!(lit.red() > bg.red(), "{lit:?} {bg:?}");
    }
}
