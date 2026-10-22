//! Перетаскивание значков, как на Mac: значок едет за мышью, остальные
//! раздвигаются, освобождая место. Отпустили среди закреплённых — значок
//! закреплён на этом месте; утащили вверх, прочь от дока, — откреплён.
//! Незакреплённые значки стоят в порядке запуска: передвинуть их нельзя,
//! только закрепить.

use crate::layout::SEPARATOR;
use crate::{Action, Dock, TRASH};

/// Значок, который тащат, и где сейчас указатель (координаты окна дока).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Drag {
    pub(crate) index: usize,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

impl Dock {
    /// Начать тащить значок `index` (указатель в `x`, `y`).
    pub fn start_drag(&mut self, index: usize, x: f32, y: f32) {
        // Корзина стоит на своём месте.
        if self.items.get(index).is_some_and(|i| i.key != TRASH) {
            self.drag = Some(Drag { index, x, y });
        }
    }

    pub fn drag_to(&mut self, x: f32, y: f32) {
        if let Some(d) = &mut self.drag {
            d.x = x;
            d.y = y;
        }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// Значок утащен вверх, прочь от дока (отпустить — убрать из дока).
    pub fn drag_out(&self) -> bool {
        self.drag.is_some_and(|d| self.is_out(d.y))
    }

    fn is_out(&self, y: f32) -> bool {
        let h = self.window_height() as f32;
        let top = self.bar_bottom(h, 0.0) - self.bar_height();
        y < top - self.look.icon as f32
    }

    /// Тащат закреплённый значок прочь — над ним подпись «Удалить».
    pub fn drag_removes(&self) -> bool {
        self.drag_out()
            && self
                .drag
                .is_some_and(|d| self.items.get(d.index).is_some_and(|i| i.pinned))
    }

    /// Значки в порядке на экране (с учётом перетаскивания), у кого перед
    /// собой разделитель, и сколько первых значков закреплено.
    pub(crate) fn arrangement(&self) -> (Vec<(usize, bool)>, usize) {
        let pinned_count = |order: &[usize], dragged: Option<(usize, bool)>| {
            order
                .iter()
                .filter(|&&i| match dragged {
                    Some((d, pinned)) if d == i => pinned,
                    _ => self.items[i].pinned,
                })
                .count()
        };
        let with_separator = |order: Vec<usize>, pinned: usize| {
            let seq = order
                .into_iter()
                .enumerate()
                .map(|(k, i)| (i, k > 0 && (k == pinned || self.items[i].key == TRASH)))
                .collect();
            (seq, pinned)
        };

        let Some(drag) = self.drag else {
            let order: Vec<usize> = (0..self.items.len()).collect();
            let pinned = pinned_count(&order, None);
            return with_separator(order, pinned);
        };
        let d = drag.index;
        let others: Vec<usize> = (0..self.items.len()).filter(|&i| i != d).collect();
        let pinned_others = pinned_count(&others, None);
        if self.is_out(drag.y) {
            return with_separator(others, pinned_others);
        }

        // Куда встанет: сколько других значков левее указателя — по обычной
        // раскладке (без перетаскивания и увеличения), чтобы место не
        // «плясало», пока остальные раздвигаются. Граница закреплённых —
        // разделитель.
        let all: Vec<usize> = (0..self.items.len()).collect();
        let plain = with_separator(all.clone(), pinned_count(&all, None)).0;
        let (slots, ..) = self.lay_out(&plain, &vec![1.0; plain.len()]);
        let at = slots
            .iter()
            .filter(|s| s.item != d && s.center() < drag.x)
            .count();
        let boundary = slots
            .iter()
            .find(|s| s.separator_before)
            .map_or(f32::INFINITY, |s| s.x - SEPARATOR / 2.0);

        let item = &self.items[d];
        let (at, pinned) = if item.app.is_none() {
            // Окно без ярлыка закрепить нельзя — остаётся на своём месте.
            (d, false)
        } else if at < pinned_others || (at == pinned_others && drag.x < boundary) {
            (at, true)
        } else if item.pinned {
            // Закреплённый, утащенный к запущенным, — последний из закреплённых.
            (pinned_others, true)
        } else {
            (d, false)
        };
        let mut order = others;
        order.insert(at.min(order.len()), d);
        let count = pinned_count(&order, Some((d, pinned)));
        with_separator(order, count)
    }

    /// Отпустили значок: новый список закреплённых или ничего.
    pub fn drop_drag(&mut self) -> Action {
        let Some(drag) = self.drag else {
            return Action::Nothing;
        };
        let (order, pinned_count) = self.arrangement();
        self.drag = None;
        let item = &self.items[drag.index];
        if item.app.is_none() {
            return Action::Nothing;
        }
        let stem = |key: &str| key.trim_end_matches(".desktop").to_string();
        // Запись в настройках для значка: как была (вдруг с «.desktop»), иначе имя.
        let entry = |key: &str| {
            self.pinned
                .iter()
                .find(|p| stem(p) == stem(key))
                .cloned()
                .unwrap_or_else(|| stem(key))
        };
        let mut list: Vec<String> = order[..pinned_count]
            .iter()
            .map(|&(i, _)| entry(&self.items[i].key))
            .collect();
        // Закреплённые, которых нет в доке (программа не установлена), — в конце.
        for p in &self.pinned {
            if !self.items.iter().any(|i| stem(&i.key) == stem(p)) {
                list.push(p.clone());
            }
        }
        if list == self.pinned {
            return Action::Nothing;
        }
        self.pinned = list;
        self.rebuild();
        Action::Pinned(self.pinned.clone())
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::{dock, window};
    use crate::{Action, Dock};

    fn keys(d: &Dock) -> Vec<&str> {
        d.items.iter().map(|i| i.key.as_str()).collect()
    }

    /// Середина `n`-го значка без увеличения и высота внутри полосы.
    fn spot(d: &Dock, n: usize) -> (f32, f32) {
        let (slots, ..) = d.slots_at(None);
        let h = d.window_height() as f32;
        (slots[n].center(), h - 30.0)
    }

    #[test]
    fn reorder_pinned() {
        let mut d = dock();
        let (x2, y) = spot(&d, 2);
        let (x0, _) = spot(&d, 0);
        // Блокнот (третий) — в начало.
        d.start_drag(2, x2, y);
        d.drag_to(x0 - 10.0, y);
        let (order, pinned) = d.arrangement();
        assert_eq!(order.iter().map(|&(i, _)| i).collect::<Vec<_>>(), [2, 0, 1]);
        assert_eq!(pinned, 3);
        assert_eq!(
            d.drop_drag(),
            Action::Pinned(vec![
                "org.xfce.mousepad".into(),
                "thunar".into(),
                "firefox".into()
            ])
        );
        assert_eq!(
            keys(&d),
            [
                "org.xfce.mousepad.desktop",
                "thunar.desktop",
                "firefox.desktop"
            ]
        );
        assert!(!d.dragging());

        // На то же место — ничего не меняется.
        let (x1, y) = spot(&d, 1);
        d.start_drag(1, x1, y);
        d.drag_to(x1 + 3.0, y);
        assert_eq!(d.drop_drag(), Action::Nothing);
    }

    #[test]
    fn drag_out_unpins_and_drag_in_pins() {
        let mut d = dock();
        d.set_windows(vec![
            (window(10, "xfce4-terminal", "Xfce4-terminal"), None),
            (window(12, "xeyes", "XEyes"), None),
        ]);
        // Firefox вверх, прочь от дока.
        let (x1, y) = spot(&d, 1);
        d.start_drag(1, x1, y);
        d.drag_to(x1, 0.0);
        assert!(d.drag_out() && d.drag_removes());
        let (order, pinned) = d.arrangement();
        assert_eq!(order.len(), 4, "пока утащен — места нет");
        assert_eq!(pinned, 2);
        assert!(order[2].1, "разделитель перед Терминалом");
        assert_eq!(
            d.drop_drag(),
            Action::Pinned(vec!["thunar".into(), "org.xfce.mousepad".into()])
        );

        // Запущенный Терминал — к закреплённым, между Проводником и Блокнотом.
        let (x0, y) = spot(&d, 0);
        let (x1, _) = spot(&d, 1);
        let (x2, _) = spot(&d, 2);
        assert_eq!(keys(&d)[2], "xfce4-terminal.desktop");
        d.start_drag(2, x2, y);
        d.drag_to((x0 + x1) / 2.0, y);
        assert!(!d.drag_removes(), "незакреплённый не «удаляют»");
        assert_eq!(
            d.drop_drag(),
            Action::Pinned(vec![
                "thunar".into(),
                "xfce4-terminal".into(),
                "org.xfce.mousepad".into()
            ])
        );

        // Окно без ярлыка закрепить нельзя.
        let n = keys(&d).iter().position(|k| *k == "window:12").unwrap();
        let (xn, y) = spot(&d, n);
        d.start_drag(n, xn, y);
        d.drag_to(x0 - 10.0, y);
        assert_eq!(d.drop_drag(), Action::Nothing);
    }

    #[test]
    fn unpinned_dragged_back_stays() {
        let mut d = dock();
        d.set_windows(vec![(window(10, "xfce4-terminal", "Xfce4-terminal"), None)]);
        let (x3, y) = spot(&d, 3);
        d.start_drag(3, x3, y);
        d.drag_to(x3 + 40.0, y);
        assert_eq!(d.drop_drag(), Action::Nothing);
    }
}
