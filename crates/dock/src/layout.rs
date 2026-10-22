//! Где что стоит в окне дока. Окно — во всю ширину экрана у нижнего края;
//! выше полосы дока — место для увеличенных значков и подписи.
//!
//! Увеличение как на Mac: значок под указателем растёт до `zoom`, соседние —
//! меньше, плавно сходя на нет примерно через три значка; полоса дока при
//! этом становится шире.

use crate::{Dock, Rect};

/// Отступ полосы от краёв значков по бокам.
pub(crate) const PAD_X: f32 = 8.0;
/// Промежуток между значками.
pub(crate) const GAP: f32 = 8.0;
/// Ширина разделителя между закреплёнными и остальными.
pub(crate) const SEPARATOR: f32 = 16.0;
/// Поля полосы над значками и под ними (снизу — место для точки).
pub(crate) const PAD_TOP: f32 = 8.0;
pub(crate) const PAD_BOTTOM: f32 = 12.0;
/// Зазор между полосой и краем экрана.
pub(crate) const MARGIN: f32 = 4.0;
/// Место над увеличенными значками для подписи.
pub(crate) const TOOLTIP_ROOM: f32 = 44.0;
/// На сколько значков в стороны распространяется увеличение.
const REACH: f32 = 3.0;

/// Значок на своём месте.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub item: usize,
    /// Левый край места значка и его ширина (с промежутками).
    pub x: f32,
    pub w: f32,
    /// Размер иконки.
    pub size: f32,
    /// Перед значком — разделитель (между закреплёнными и остальными).
    pub separator_before: bool,
}

impl Slot {
    pub fn center(&self) -> f32 {
        self.x + self.w / 2.0
    }
}

impl Dock {
    fn icon(&self) -> f32 {
        self.look.icon as f32
    }

    pub(crate) fn bar_height(&self) -> f32 {
        self.icon() + PAD_TOP + PAD_BOTTOM
    }

    /// Высота окна дока.
    pub fn window_height(&self) -> u32 {
        let grow = self.icon() * (self.look.zoom - 1.0).max(0.0);
        (MARGIN + self.bar_height() + grow + TOOLTIP_ROOM + self.icon() * 0.5).ceil() as u32
    }

    /// Самый большой размер иконки (при увеличении) — в нём иконки и хранятся.
    pub fn max_icon(&self) -> u32 {
        (self.icon() * self.look.zoom.max(1.0)).ceil() as u32
    }

    /// Места значков и полоса дока `(x, ширина)` при указателе `pointer`.
    pub(crate) fn slots_at(&self, pointer: Option<f32>) -> (Vec<Slot>, f32, f32) {
        let base = self.icon() + GAP;
        let (order, _) = self.arrangement();
        // Сначала без увеличения: где центры значков.
        let plain = self.lay_out(&order, &vec![1.0; order.len()]);
        let scales: Vec<f32> = match pointer {
            Some(px) if self.look.zoom > 1.0 => plain
                .0
                .iter()
                .map(|s| {
                    let d = (px - s.center()).abs() / base;
                    if d >= REACH {
                        1.0
                    } else {
                        let f = (1.0 + (std::f32::consts::PI * d / REACH).cos()) / 2.0;
                        1.0 + (self.look.zoom - 1.0) * f
                    }
                })
                .collect(),
            _ => vec![1.0; order.len()],
        };
        self.lay_out(&order, &scales)
    }

    /// Значки `order` (номер и разделитель перед ним) с увеличениями `scales`.
    pub(crate) fn lay_out(&self, order: &[(usize, bool)], scales: &[f32]) -> (Vec<Slot>, f32, f32) {
        let base = self.icon() + GAP;
        let width: f32 = order
            .iter()
            .zip(scales)
            .map(|(&(_, sep), s)| base * s + if sep { SEPARATOR } else { 0.0 })
            .sum::<f32>()
            + PAD_X * 2.0;
        let bar_x = ((self.screen_w - width) / 2.0).round();
        let mut x = bar_x + PAD_X;
        let mut slots = Vec::with_capacity(order.len());
        for (&(i, sep), s) in order.iter().zip(scales) {
            if sep {
                x += SEPARATOR;
            }
            slots.push(Slot {
                item: i,
                x,
                w: base * s,
                size: self.icon() * s,
                separator_before: sep,
            });
            x += base * s;
        }
        (slots, bar_x, width)
    }

    /// Низ полосы дока в окне высотой `h` (с учётом того, насколько спрятан).
    pub(crate) fn bar_bottom(&self, h: f32, hidden: f32) -> f32 {
        h - MARGIN + hidden * (self.bar_height() + MARGIN)
    }

    /// Где полоса дока на экране, когда он виден и не увеличен (для решения
    /// о скрытии и резерва места).
    pub fn bar_rect(&self, screen_h: u32) -> Rect {
        let (_, x, w) = self.slots_at(None);
        let h = self.bar_height();
        Rect {
            x: x as i32,
            y: (screen_h as f32 - MARGIN - h) as i32,
            w: w as u32,
            h: h as u32,
        }
    }

    /// Значок под точкой окна `(x, y)`.
    pub fn item_at(&self, x: f32, y: f32) -> Option<usize> {
        let h = self.window_height() as f32;
        let bottom = self.bar_bottom(h, 0.0);
        let (slots, ..) = self.slots_at(self.pointer);
        slots
            .iter()
            .find(|s| {
                x >= s.x
                    && x < s.x + s.w
                    && y <= bottom
                    && y >= bottom - PAD_BOTTOM - s.size - PAD_TOP
            })
            .map(|s| s.item)
    }

    /// Середина значка `index` и верх его иконки (в координатах окна, при
    /// нынешнем увеличении) — над ним открывается меню.
    pub fn anchor(&self, index: usize, hidden: f32) -> Option<(f32, f32)> {
        let h = self.window_height() as f32;
        let bottom = self.bar_bottom(h, hidden);
        let (slots, ..) = self.slots_at(self.pointer);
        let s = slots.iter().find(|s| s.item == index)?;
        Some((s.center(), bottom - PAD_BOTTOM - s.size))
    }

    /// Где окно принимает клики (в координатах окна). Спрятан — тонкая полоска
    /// у края экрана, чтобы док выехал, когда к ней подведут указатель.
    pub fn input_rects(&self, hidden: f32) -> Vec<(i32, i32, u32, u32)> {
        let h = self.window_height() as f32;
        let (slots, x, w) = self.slots_at(self.pointer);
        if hidden >= 1.0 {
            return vec![(x as i32, (h - 1.0) as i32, w as u32, 1)];
        }
        let bottom = self.bar_bottom(h, hidden);
        let tallest = slots.iter().map(|s| s.size).fold(self.icon(), f32::max);
        let top = if self.pointer.is_some() {
            bottom - PAD_BOTTOM - tallest - PAD_TOP
        } else {
            bottom - self.bar_height()
        };
        vec![(x as i32, top as i32, w as u32, (h - top).ceil() as u32)]
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::dock;

    #[test]
    fn centered_and_zoom_grows_neighbours_less() {
        let d = dock();
        let (slots, x, w) = d.slots_at(None);
        assert_eq!(slots.len(), 3);
        assert!((x + w / 2.0 - 800.0).abs() <= 1.0);
        assert!(slots.iter().all(|s| s.size == 48.0));

        let (zoomed, _, wz) = d.slots_at(Some(slots[1].center()));
        assert!((zoomed[1].size - 48.0 * 1.4).abs() < 0.5);
        assert!(zoomed[0].size > 48.0 && zoomed[0].size < zoomed[1].size);
        assert!(wz > w, "полоса шире при увеличении");
    }

    #[test]
    fn hit_testing_and_input() {
        let mut d = dock();
        let h = d.window_height() as f32;
        let (slots, ..) = d.slots_at(None);
        let y = h - 4.0 - 12.0 - 10.0; // над точкой, внутри значка
        assert_eq!(d.item_at(slots[2].center(), y), Some(2));
        assert_eq!(d.item_at(slots[2].center(), 5.0), None);
        assert_eq!(d.item_at(10.0, y), None);

        let plain = d.input_rects(0.0);
        d.set_pointer(Some(slots[1].center()));
        let hovered = d.input_rects(0.0);
        assert!(
            hovered[0].1 < plain[0].1,
            "при наведении — выше, под увеличенные значки"
        );
        let hidden = d.input_rects(1.0);
        assert_eq!(hidden[0].3, 1);
    }
}
