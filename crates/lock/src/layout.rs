//! Положения на экране блокировки: часы сверху, внизу аватар, имя и поле
//! пароля с кольцом состояния справа. Одни и те же числа у фона и у кадра,
//! поэтому они совпадают до точки.

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
    /// Базовая линия даты и времени (положения привычны со времён i3lock).
    pub date_y: i64,
    pub time_y: i64,
    pub avatar_y: i64,
    pub name_y: i64,
    pub pill_x: i64,
    pub pill_y: i64,
    /// Центр кольца состояния (справа в поле).
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
}
