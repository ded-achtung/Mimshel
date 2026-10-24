//! Рамка выделения рабочего стола: XRender обновляет только пиксели, у
//! которых изменился цвет. CPU не создаёт RGBA-кадры для каждого движения.
//!
//! Окно прозрачно для мыши благодаря XFixes InputShape. Если расширение или
//! формат ARGB32 недоступны, DesktopUi использует прежний tiny-skia путь.

use x11rb::connection::Connection;
use x11rb::protocol::render::{Color, ConnectionExt as _, CreatePictureAux, PictOp};
use x11rb::protocol::shape::SK;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::Rectangle;

use crate::{Display, Result};

const FILL: Color = Color {
    red: 0xffff,
    green: 0xffff,
    blue: 0xffff,
    alpha: 0x2828,
};
const EDGE: Color = Color {
    red: 0xffff,
    green: 0xffff,
    blue: 0xffff,
    alpha: 0x9090,
};
const CLEAR: Color = Color {
    red: 0,
    green: 0,
    blue: 0,
    alpha: 0,
};

/// Координаты в физических пикселях, полуоткрытые [x0, x1) × [y0, y1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl Rect {
    fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Option<Self> {
        (x0 < x1 && y0 < y1).then_some(Self { x0, y0, x1, y1 })
    }

    fn intersection(self, b: Self) -> Option<Self> {
        Self::new(
            self.x0.max(b.x0),
            self.y0.max(b.y0),
            self.x1.min(b.x1),
            self.y1.min(b.y1),
        )
    }

    /// Четыре максимум непересекающихся остатка A \ B.
    fn minus(self, b: Self) -> Vec<Self> {
        let Some(i) = self.intersection(b) else {
            return vec![self];
        };
        [
            Self::new(self.x0, self.y0, self.x1, i.y0),
            Self::new(self.x0, i.y1, self.x1, self.y1),
            Self::new(self.x0, i.y0, i.x0, i.y1),
            Self::new(i.x1, i.y0, self.x1, i.y1),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn x11(self) -> Option<Rectangle> {
        Some(Rectangle {
            x: self.x0.try_into().ok()?,
            y: self.y0.try_into().ok()?,
            width: (self.x1 - self.x0).try_into().ok()?,
            height: (self.y1 - self.y0).try_into().ok()?,
        })
    }

    /// Два координатных пространства: мышь работает в точках, Render — в px.
    fn from_points(rect: (f32, f32, f32, f32), scale: f32, screen: (u16, u16)) -> Option<Self> {
        let (x, y, w, h) = rect;
        let edge = |n: f32, ceil: bool, max: u16| {
            let n = n * scale;
            let n = if ceil { n.ceil() } else { n.floor() };
            n.clamp(0.0, f32::from(max)) as i32
        };
        let x0 = edge(x, false, screen.0);
        let y0 = edge(y, false, screen.1);
        let x1 = edge(x + w.max(1.0), true, screen.0);
        let y1 = edge(y + h.max(1.0), true, screen.1);
        Self::new(x0, y0, x1, y1)
    }

    /// Сплошной внутренний прямоугольник, не содержащий обводку.
    fn inside(self, stroke: i32) -> Vec<Self> {
        Self::new(
            self.x0 + stroke,
            self.y0 + stroke,
            self.x1 - stroke,
            self.y1 - stroke,
        )
        .into_iter()
        .collect()
    }

    /// Непересекающиеся полосы обводки толщиной stroke.
    fn border(self, stroke: i32) -> Vec<Self> {
        let n = stroke.min((self.x1 - self.x0 + 1) / 2).min((self.y1 - self.y0 + 1) / 2);
        [
            Self::new(self.x0, self.y0, self.x1, (self.y0 + n).min(self.y1)),
            Self::new(self.x0, (self.y1 - n).max(self.y0 + n), self.x1, self.y1),
            Self::new(self.x0, self.y0 + n, self.x0 + n, self.y1 - n),
            Self::new(self.x1 - n, self.y0 + n, self.x1, self.y1 - n),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// Часть новых прямоугольников за вычетом всех старых.
/// Для рамки наборы очень малы (не больше четырёх полос).
fn delta(new: &[Rect], old: &[Rect]) -> Vec<Rect> {
    let mut out = Vec::new();
    for &r in new {
        let mut pieces = vec![r];
        for &prev in old {
            pieces = pieces.into_iter().flat_map(|piece| piece.minus(prev)).collect();
        }
        out.extend(pieces);
    }
    out
}

fn shape(rect: Option<Rect>, stroke: i32) -> (Vec<Rect>, Vec<Rect>) {
    match rect {
        None => (Vec::new(), Vec::new()),
        Some(r) => (r.inside(stroke), r.border(stroke)),
    }
}

pub struct SelectionRenderer {
    /// Картинка ARGB, созданная для окна desktop-band. Живёт как окно Shell.
    picture: u32,
    previous: Option<Rect>,
}

impl SelectionRenderer {
    /// Расширения пробуем при инициализации, но отсутствие Render/XFixes не
    /// мешает запуску всего Shell.
    pub fn new(d: &Display, window: u32) -> Option<Self> {
        // XFixes >= 2 поддерживает InputShape, чтобы рамка не закрывала стол.
        let fixes = d.conn.xfixes_query_version(4, 0).ok()?.reply().ok()?;
        if fixes.major_version < 2 {
            return None;
        }
        let formats = d.conn.render_query_pict_formats().ok()?.reply().ok()?;
        let format = formats
            .screens
            .iter()
            .flat_map(|s| &s.depths)
            .flat_map(|depth| &depth.visuals)
            .find(|v| v.visual == d.visual)?
            .format;
        // Сначала проверяем InputShape. При ошибке переходим на прежний путь.
        let region = d.conn.generate_id().ok()?;
        d.conn.xfixes_create_region(region, &[]).ok()?.check().ok()?;
        let pass = d
            .conn
            .xfixes_set_window_shape_region(window, SK::INPUT, 0, 0, region)
            .ok()
            .and_then(|c| c.check().ok())
            .is_some();
        let _ = d.conn.xfixes_destroy_region(region);
        if !pass {
            return None;
        }
        let picture = d.conn.generate_id().ok()?;
        if d
            .conn
            .render_create_picture(picture, window, format, &CreatePictureAux::default())
            .ok()?
            .check()
            .is_err()
        {
            // Не оставляем полноэкранное окно прозрачным для ввода, если
            // пришлось отказаться от Render и вернуть tiny-skia fallback.
            // Ввод перехватывает только видимый слой во время выделения.
            return None;
        }
        Some(Self {
            picture,
            previous: None,
        })
    }

    fn fill(&self, d: &Display, color: Color, areas: Vec<Rect>) -> Result<()> {
        let rectangles: Vec<Rectangle> = areas.into_iter().filter_map(Rect::x11).collect();
        if !rectangles.is_empty() {
            d.conn
                .render_fill_rectangles(PictOp::SRC, self.picture, color, &rectangles)?;
        }
        Ok(())
    }

    /// Каждый переход обновляет лишь отличающиеся участки заливки и границы.
    pub fn paint(&mut self, d: &Display, rect: (f32, f32, f32, f32)) -> Result<()> {
        let next = Rect::from_points(rect, d.scale(), d.screen_pixels());
        if next == self.previous {
            return Ok(());
        }
        let stroke = (d.scale().round() as i32).max(1);
        let (old_fill, old_edge) = shape(self.previous, stroke);
        let (new_fill, new_edge) = shape(next, stroke);
        // Старая площадь снаружи новой становится прозрачной.
        let cleared = match (self.previous, next) {
            (Some(a), Some(b)) => a.minus(b),
            (Some(a), None) => vec![a],
            _ => Vec::new(),
        };
        self.fill(d, CLEAR, cleared)?;
        self.fill(d, FILL, delta(&new_fill, &old_fill))?;
        self.fill(d, EDGE, delta(&new_edge, &old_edge))?;
        d.conn.flush()?;
        self.previous = next;
        Ok(())
    }

    /// При окончании выделения очищаем слой перед следующим показом.
    pub fn reset(&mut self, d: &Display) -> Result<()> {
        let (w, h) = d.screen_pixels();
        if let Some(all) = Rect::new(0, 0, i32::from(w), i32::from(h)) {
            self.fill(d, CLEAR, vec![all])?;
        }
        d.conn.flush()?;
        self.previous = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(rect: Option<Rect>, x: i32, y: i32) -> u8 {
        let (fill, edge) = shape(rect, 1);
        let covered = |a: &Rect| a.x0 <= x && x < a.x1 && a.y0 <= y && y < a.y1;
        if edge.iter().any(covered) {
            2
        } else if fill.iter().any(covered) {
            1
        } else {
            0
        }
    }

    #[test]
    fn difference_updates_exactly_the_changed_pixels() {
        let positions = [
            None,
            Rect::new(2, 2, 10, 9),
            Rect::new(3, 2, 11, 9),
            Rect::new(2, 3, 10, 10),
            Rect::new(2, 2, 6, 5),
            Rect::new(1, 1, 13, 12),
            Rect::new(0, 0, 1, 1),
        ];
        for &old in &positions {
            for &new in &positions {
                let (old_fill, old_edge) = shape(old, 1);
                let (new_fill, new_edge) = shape(new, 1);
                let clear = match (old, new) {
                    (Some(a), Some(b)) => a.minus(b),
                    (Some(a), None) => vec![a],
                    _ => vec![],
                };
                let fills = delta(&new_fill, &old_fill);
                let edges = delta(&new_edge, &old_edge);
                let covered = |r: &Rect, x: i32, y: i32| {
                    r.x0 <= x && x < r.x1 && r.y0 <= y && y < r.y1
                };
                for y in 0..16 {
                    for x in 0..16 {
                        let mut color = pixel(old, x, y);
                        if clear.iter().any(|r| covered(r, x, y)) {
                            color = 0;
                        }
                        if fills.iter().any(|r| covered(r, x, y)) {
                            color = 1;
                        }
                        if edges.iter().any(|r| covered(r, x, y)) {
                            color = 2;
                        }
                        assert_eq!(color, pixel(new, x, y), "old={old:?} new={new:?} x={x} y={y}");
                    }
                }
            }
        }
    }

    #[test]
    fn physical_coordinates_clipped_and_scaled() {
        assert_eq!(
            Rect::from_points((4.0, 5.0, 10.0, 20.0), 2.0, (80, 90)),
            Rect::new(8, 10, 28, 50)
        );
        assert_eq!(
            Rect::from_points((-5.0, -5.0, 7.0, 7.0), 1.0, (80, 90)),
            Rect::new(0, 0, 2, 2)
        );
    }
}
