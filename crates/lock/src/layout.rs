//! Положения на экране блокировки. Считаем сами: i3lock не умеет складывать
//! в координатах (урок 7). Те же числа — у фона, который мы рисуем, и у
//! команды i3lock (часы, кольцо ввода, подсказка), поэтому они совпадают.

/// Положения на экране блокировки (в точках экрана).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub w: i64,
    pub h: i64,
    /// Масштаб относительно экрана высотой 1080.
    pub s: f64,
    pub avatar: i64,
    pub pill_w: i64,
    pub pill_h: i64,
    pub date_y: i64,
    pub time_y: i64,
    pub avatar_y: i64,
    pub name_y: i64,
    pub pill_x: i64,
    pub pill_y: i64,
    pub ind_x: i64,
    pub ind_y: i64,
}

/// Округление «к чётному», как `round` в Python: так считал macos-look, и
/// фон, нарисованный им, должен совпасть с полем и кольцом до точки.
pub fn round(v: f64) -> i64 {
    v.round_ties_even() as i64
}

impl Layout {
    pub fn new(w: i64, h: i64) -> Layout {
        let s = h as f64 / 1080.0;
        let avatar = round(84.0 * s);
        let pill_w = round(230.0 * s);
        let pill_h = round(32.0 * s);
        let date_y = round(h as f64 * 0.12);
        let avatar_y = round(h as f64 * 0.70);
        let name_y = avatar_y + avatar + round(14.0 * s);
        let pill_x = (w - pill_w).div_euclid(2);
        let pill_y = name_y + round(40.0 * s);
        Layout {
            w,
            h,
            s,
            avatar,
            pill_w,
            pill_h,
            date_y,
            time_y: date_y + round(118.0 * s),
            avatar_y,
            name_y,
            pill_x,
            pill_y,
            ind_x: pill_x + pill_w - round(17.0 * s),
            ind_y: pill_y + pill_h.div_euclid(2),
        }
    }

    /// Размер `v` (для экрана высотой 1080) на этом экране, строкой — для i3lock.
    pub fn px(&self, v: f64) -> String {
        round(v * self.s).to_string()
    }

    /// Сколько точек ● помещается в поле: от левого отступа до кольца.
    /// Шаг точки (● и узкий пробел) ≈ 1,15 размера шрифта — замерено на экране.
    pub fn max_dots(&self) -> i64 {
        let room = self.pill_w - round(14.0 * self.s) - round(34.0 * self.s);
        let step = 1.15 * round(13.5 * self.s) as f64;
        ((room as f64 / step).floor() as i64).max(1)
    }
}
