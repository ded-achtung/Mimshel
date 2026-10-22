//! Когда прятать док — по режиму из настроек.

use shell_core::config::DockHide;

/// Прямоугольник на экране.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w as i32
            && o.x < self.x + self.w as i32
            && self.y < o.y + o.h as i32
            && o.y < self.y + self.h as i32
    }
}

/// Прятать ли док. `bar` — где док стоит, когда виден; `active` — рамка
/// активного окна и развёрнуто ли оно; `pointer_inside` — указатель над доком
/// (тогда док не прячется никогда).
pub fn should_hide(
    mode: DockHide,
    bar: Rect,
    active: Option<(Rect, bool)>,
    pointer_inside: bool,
) -> bool {
    if pointer_inside {
        return false;
    }
    match mode {
        DockHide::Never => false,
        DockHide::Auto => true,
        DockHide::Smart => active.is_some_and(|(r, _)| r.overlaps(&bar)),
        DockHide::Maximized => active.is_some_and(|(_, maximized)| maximized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: Rect = Rect {
        x: 900,
        y: 1340,
        w: 700,
        h: 70,
    };

    fn win(y: i32, h: u32) -> Rect {
        Rect {
            x: 800,
            y,
            w: 900,
            h,
        }
    }

    #[test]
    fn modes() {
        let low = Some((win(900, 500), false)); // доходит до дока
        let high = Some((win(100, 500), false));
        let max = Some((win(0, 1413), true));
        assert!(should_hide(DockHide::Smart, BAR, low, false));
        assert!(!should_hide(DockHide::Smart, BAR, high, false));
        assert!(!should_hide(DockHide::Smart, BAR, None, false));
        assert!(!should_hide(DockHide::Smart, BAR, low, true));
        assert!(should_hide(DockHide::Auto, BAR, None, false));
        assert!(!should_hide(DockHide::Never, BAR, max, false));
        assert!(should_hide(DockHide::Maximized, BAR, max, false));
        assert!(!should_hide(DockHide::Maximized, BAR, low, false));
    }
}
