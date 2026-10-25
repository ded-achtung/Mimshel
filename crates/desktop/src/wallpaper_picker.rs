//! Окно «Обои» (правый щелчок по столу → «Изменить обои…»): обои плитками с
//! миниатюрами, как в настройках Mac. Щелчок по плитке сразу ставит обои,
//! выбранные обведены; последняя плитка — «Выбрать картинку…» (свой файл).
//! Esc или щелчок мимо окна — закрыть.

use mimshel_core::input::Key;
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{paint, rounded};
use tiny_skia::{FillRule, FilterQuality, Paint, Pattern, Pixmap, SpreadMode, Stroke, Transform};

/// Миниатюра (в точках).
pub const THUMB_W: f32 = 200.0;
pub const THUMB_H: f32 = 125.0;
/// Плиток в ряду.
const COLUMNS: usize = 3;
const PAD: f32 = 20.0;
const GAP: f32 = 18.0;
/// Высота заголовка «Обои».
const TITLE_H: f32 = 44.0;
/// Подпись под миниатюрой.
const CAPTION_H: f32 = 30.0;
const RADIUS: f32 = 14.0;
const THUMB_RADIUS: f32 = 8.0;

const BACKGROUND: [u8; 4] = [0x1e, 0x1e, 0x1e, 0xf0];
const BORDER: [u8; 4] = [0xff, 0xff, 0xff, 0x1f];
const SELECTED: [u8; 4] = [0x0a, 0x84, 0xff, 0xff];
const HOVER: [u8; 4] = [0xff, 0xff, 0xff, 0x60];
/// Пустая плитка (ещё нет миниатюры) и плитка «Выбрать картинку…».
const EMPTY: [u8; 4] = [0xff, 0xff, 0xff, 0x14];
const PLUS: [u8; 4] = [0xd0, 0xd0, 0xd0, 0xff];

const TITLE: Style = Style {
    size: 17.0,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 600,
};
const CAPTION: Style = Style {
    size: 14.0,
    color: [0xd8, 0xd8, 0xd8, 0xff],
    weight: 400,
};

/// Плитка окна.
#[derive(Debug, Clone, PartialEq)]
pub enum Tile {
    /// Обои: значение настройки `look.wallpaper` (имя своих или путь к
    /// картинке) и подпись.
    Wallpaper { value: String, label: String },
    /// «Выбрать картинку…».
    Pick,
}

/// Что сделать после события.
#[derive(Debug, Clone, PartialEq)]
pub enum PickerResult {
    Nothing,
    Redraw,
    /// Поставить обои с этим значением настройки.
    Apply(String),
    /// Выбрать свою картинку (окно выбора файла).
    PickFile,
    Close,
}

pub struct WallpaperPicker {
    tiles: Vec<Tile>,
    /// Миниатюры (в пикселях экрана) по значению настройки.
    thumbs: Vec<(String, Pixmap)>,
    /// Нынешние обои (значение настройки).
    current: String,
    hover: Option<usize>,
}

impl WallpaperPicker {
    /// `builtin` — свои обои: (имя, подпись); `current` — нынешние. Если они —
    /// своя картинка, у неё отдельная плитка.
    pub fn new(builtin: &[(&str, &str)], current: &str) -> WallpaperPicker {
        let mut tiles: Vec<Tile> = builtin
            .iter()
            .map(|(value, label)| Tile::Wallpaper {
                value: (*value).to_string(),
                label: (*label).to_string(),
            })
            .collect();
        if !current.is_empty() && !builtin.iter().any(|(v, _)| *v == current) {
            tiles.push(Tile::Wallpaper {
                value: current.to_string(),
                label: "Своя картинка".into(),
            });
        }
        tiles.push(Tile::Pick);
        WallpaperPicker {
            tiles,
            thumbs: Vec::new(),
            current: current.to_string(),
            hover: None,
        }
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    /// Значения обоев, которым нужны миниатюры.
    pub fn values(&self) -> Vec<String> {
        self.tiles
            .iter()
            .filter_map(|t| match t {
                Tile::Wallpaper { value, .. } => Some(value.clone()),
                Tile::Pick => None,
            })
            .collect()
    }

    /// Миниатюра готова.
    pub fn set_thumb(&mut self, value: String, image: Pixmap) {
        self.thumbs.retain(|(v, _)| *v != value);
        self.thumbs.push((value, image));
    }

    /// Обои поставлены (выделение — на них).
    pub fn set_current(&mut self, value: &str) {
        self.current = value.to_string();
    }

    /// Размер окна в точках.
    pub fn size(&self) -> (u32, u32) {
        let rows = self.tiles.len().div_ceil(COLUMNS);
        let cols = self.tiles.len().min(COLUMNS) as f32;
        let w = 2.0 * PAD + cols * THUMB_W + (cols - 1.0) * GAP;
        let h = TITLE_H + rows as f32 * (THUMB_H + CAPTION_H) + (rows as f32 - 1.0) * GAP + PAD;
        (w.ceil() as u32, h.ceil() as u32)
    }

    /// Левый верхний угол миниатюры плитки `i`.
    fn tile_pos(i: usize) -> (f32, f32) {
        let (col, row) = ((i % COLUMNS) as f32, (i / COLUMNS) as f32);
        (
            PAD + col * (THUMB_W + GAP),
            TITLE_H + row * (THUMB_H + CAPTION_H + GAP),
        )
    }

    /// Плитка под точкой (миниатюра или подпись под ней).
    fn tile_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.tiles.len()).find(|&i| {
            let (tx, ty) = Self::tile_pos(i);
            x >= tx && x < tx + THUMB_W && y >= ty && y < ty + THUMB_H + CAPTION_H
        })
    }

    pub fn motion(&mut self, x: f32, y: f32) -> bool {
        let hover = self.tile_at(x, y);
        let changed = hover != self.hover;
        self.hover = hover;
        changed
    }

    pub fn click(&self, x: f32, y: f32) -> PickerResult {
        let (w, h) = self.size();
        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
            return PickerResult::Close;
        }
        match self.tile_at(x, y).map(|i| &self.tiles[i]) {
            Some(Tile::Wallpaper { value, .. }) if *value != self.current => {
                PickerResult::Apply(value.clone())
            }
            Some(Tile::Pick) => PickerResult::PickFile,
            _ => PickerResult::Nothing,
        }
    }

    pub fn key(&self, key: Key) -> PickerResult {
        match key {
            Key::Escape => PickerResult::Close,
            _ => PickerResult::Nothing,
        }
    }

    pub fn render(&self, fonts: &mut Fonts) -> Canvas {
        let (w, h) = self.size();
        let mut pm = Canvas::new(w, h).expect("размер не нулевой");
        let (wf, hf) = (w as f32, h as f32);
        let id = Transform::identity();
        if let Some(r) = rounded(0.0, 0.0, wf, hf, RADIUS) {
            pm.fill_path(&r, &paint(BACKGROUND), FillRule::Winding, id, None);
        }
        if let Some(r) = rounded(0.5, 0.5, wf - 1.0, hf - 1.0, RADIUS - 0.5) {
            let stroke = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&r, &paint(BORDER), &stroke, id, None);
        }
        let ty = ((TITLE_H - TITLE.line_height()) / 2.0).round();
        fonts.draw(&mut pm, "Обои", &TITLE, PAD, ty, None);

        for (i, tile) in self.tiles.iter().enumerate() {
            let (x, y) = Self::tile_pos(i);
            let selected = matches!(tile, Tile::Wallpaper { value, .. } if *value == self.current);
            // Обводка: выбранные — синяя, под указателем — светлая.
            let ring = if selected {
                Some(SELECTED)
            } else if self.hover == Some(i) {
                Some(HOVER)
            } else {
                None
            };
            if let Some(c) = ring
                && let Some(r) = rounded(x - 3.0, y - 3.0, THUMB_W + 6.0, THUMB_H + 6.0, 10.0)
            {
                let stroke = Stroke {
                    width: 3.0,
                    ..Stroke::default()
                };
                pm.stroke_path(&r, &paint(c), &stroke, id, None);
            }
            let Some(frame) = rounded(x, y, THUMB_W, THUMB_H, THUMB_RADIUS) else {
                continue;
            };
            let (label, thumb) = match tile {
                Tile::Wallpaper { value, label } => (
                    label.as_str(),
                    self.thumbs.iter().find(|(v, _)| v == value).map(|(_, p)| p),
                ),
                Tile::Pick => ("Выбрать картинку…", None),
            };
            match thumb {
                // Картинка — заливкой со скруглёнными углами: растянута на
                // миниатюру (она уже нужного размера, в пикселях экрана).
                Some(img) => {
                    let k = THUMB_W / img.width() as f32;
                    let shader = Pattern::new(
                        img.as_ref(),
                        SpreadMode::Pad,
                        FilterQuality::Bicubic,
                        1.0,
                        Transform::from_scale(k, k).post_translate(x, y),
                    );
                    let mut p = Paint {
                        shader,
                        ..Paint::default()
                    };
                    p.anti_alias = true;
                    pm.fill_path(&frame, &p, FillRule::Winding, id, None);
                }
                None => {
                    pm.fill_path(&frame, &paint(EMPTY), FillRule::Winding, id, None);
                    if *tile == Tile::Pick {
                        plus(&mut pm, x + THUMB_W / 2.0, y + THUMB_H / 2.0);
                    }
                }
            }
            let tw = fonts.measure(label, &CAPTION).min(THUMB_W);
            let cy = y + THUMB_H + ((CAPTION_H - CAPTION.line_height()) / 2.0).round();
            fonts.draw(
                &mut pm,
                label,
                &CAPTION,
                (x + (THUMB_W - tw) / 2.0).round(),
                cy,
                Some(THUMB_W),
            );
        }
        pm
    }
}

/// Крупный «+» на плитке «Выбрать картинку…».
fn plus(pm: &mut Canvas, cx: f32, cy: f32) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(cx - 16.0, cy);
    pb.line_to(cx + 16.0, cy);
    pb.move_to(cx, cy - 16.0);
    pb.line_to(cx, cy + 16.0);
    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width: 3.0,
            line_cap: tiny_skia::LineCap::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(PLUS), &stroke, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILTIN: &[(&str, &str)] = &[
        ("space", "Космос"),
        ("waves", "Волны"),
        ("aurora", "Сияние"),
    ];

    fn center(i: usize) -> (f32, f32) {
        let (x, y) = WallpaperPicker::tile_pos(i);
        (x + THUMB_W / 2.0, y + THUMB_H / 2.0)
    }

    #[test]
    fn tiles_and_own_picture() {
        let p = WallpaperPicker::new(BUILTIN, "space");
        assert_eq!(p.tiles().len(), 4, "три своих и «Выбрать картинку…»");
        assert_eq!(p.values(), ["space", "waves", "aurora"]);
        let p = WallpaperPicker::new(BUILTIN, "/home/ded/Картинки/горы.jpg");
        assert_eq!(p.tiles().len(), 5);
        assert_eq!(p.values().last().unwrap(), "/home/ded/Картинки/горы.jpg");
        assert_eq!(
            p.size(),
            (
                2 * 20 + 3 * 200 + 2 * 18,
                (44.0 + 2.0 * 155.0 + 18.0 + 20.0) as u32
            )
        );
    }

    #[test]
    fn clicks() {
        let p = WallpaperPicker::new(BUILTIN, "space");
        let (x, y) = center(1);
        assert_eq!(p.click(x, y), PickerResult::Apply("waves".into()));
        let (x, y) = center(0);
        assert_eq!(p.click(x, y), PickerResult::Nothing, "уже стоят");
        let (x, y) = center(3);
        assert_eq!(p.click(x, y), PickerResult::PickFile);
        assert_eq!(p.click(-5.0, 10.0), PickerResult::Close, "мимо окна");
        assert_eq!(p.click(5.0, 5.0), PickerResult::Nothing, "по заголовку");
        assert_eq!(p.key(Key::Escape), PickerResult::Close);
    }

    #[test]
    fn renders_thumb_and_selection() {
        let mut fonts = Fonts::new("Inter");
        let mut p = WallpaperPicker::new(BUILTIN, "waves");
        let mut red = Pixmap::new(THUMB_W as u32, THUMB_H as u32).unwrap();
        red.fill(tiny_skia::Color::from_rgba8(220, 20, 20, 255));
        p.set_thumb("space".into(), red);
        let pm = p.render(&mut fonts);
        let (x, y) = center(0);
        let px = pm.pixel(x as u32, y as u32).unwrap();
        assert!(px.red() > 200 && px.green() < 40, "миниатюра: {px:?}");
        // Обводка выбранных «Волн» — синяя.
        let (x1, y1) = WallpaperPicker::tile_pos(1);
        let ring = pm
            .pixel((x1 + THUMB_W / 2.0) as u32, (y1 - 3.0) as u32)
            .unwrap();
        assert!(ring.blue() > 200 && ring.red() < 40, "обводка: {ring:?}");
    }
}
