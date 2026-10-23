//! Календарь по щелчку на часах: сверху — сегодняшняя дата словами, под ней
//! месяц со стрелками ‹ ›, дни недели и сетка чисел; сегодня — синий кружок.
//! Названия месяцев и дней приходят снаружи (из языка системы), здесь —
//! только счёт дней, мышь и рисование.

use shell_core::input::Key;
use shell_text::{Fonts, Style};
use shell_widgets::shape::{circle, paint, rounded};
use tiny_skia::{FillRule, LineCap, LineJoin, PathBuilder, Pixmap, Stroke, Transform};

/// День: год, месяц (1–12), число.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

/// Подписи на языке системы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    /// Месяцы с января, как в заголовке («Октябрь»).
    pub months: [String; 12],
    /// Дни недели с понедельника, коротко («Пн»).
    pub weekdays: [String; 7],
    /// Неделя начинается с понедельника (иначе — с воскресенья).
    pub monday_first: bool,
}

impl Default for Names {
    fn default() -> Names {
        let months = [
            "Январь",
            "Февраль",
            "Март",
            "Апрель",
            "Май",
            "Июнь",
            "Июль",
            "Август",
            "Сентябрь",
            "Октябрь",
            "Ноябрь",
            "Декабрь",
        ];
        let weekdays = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
        Names {
            months: months.map(String::from),
            weekdays: weekdays.map(String::from),
            monday_first: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarAction {
    Nothing,
    Redraw,
    Close,
}

/// Куда можно нажать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spot {
    Today,
    Prev,
    Next,
}

pub struct CalendarPanel {
    today: Date,
    /// Сегодняшняя дата словами («Пятница, 23 октября»).
    today_text: String,
    names: Names,
    /// Показанный месяц.
    year: i32,
    month: u32,
    hover: Option<Spot>,
}

const WIDTH: f32 = 268.0;
const PAD: f32 = 14.0;
const CELL: f32 = (WIDTH - 2.0 * PAD) / 7.0;
const ROW: f32 = 30.0;
const TODAY_H: f32 = 30.0;
const HEADER_H: f32 = 30.0;
const WEEK_H: f32 = 22.0;
const ARROW: f32 = 26.0;

const BG: [u8; 4] = [0x24, 0x24, 0x26, 0xf2];
const EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const LINE: [u8; 4] = [0xff, 0xff, 0xff, 0x1f];
const HOVER: [u8; 4] = [0xff, 0xff, 0xff, 0x22];
const ACCENT: [u8; 4] = [0x0a, 0x84, 0xff, 0xff];

const BIG: Style = Style {
    size: 15.0,
    color: [255, 255, 255, 255],
    weight: 600,
};
const TITLE: Style = Style {
    size: 13.0,
    color: [255, 255, 255, 235],
    weight: 600,
};
const WEEK: Style = Style {
    size: 11.0,
    color: [255, 255, 255, 120],
    weight: 600,
};
const DAY: Style = Style {
    size: 13.0,
    color: [255, 255, 255, 235],
    weight: 400,
};

/// Високосный ли год.
fn leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        2 if leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// День недели: 0 — понедельник … 6 — воскресенье.
pub fn weekday(y: i32, m: u32, d: u32) -> u32 {
    // Способ Сакамото: 0 — воскресенье.
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    let sunday0 = (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d as i32).rem_euclid(7);
    ((sunday0 + 6) % 7) as u32
}

/// Сетка месяца: 6 недель по 7 дней. Каждый день — (дата, этого ли месяца).
pub fn month_grid(y: i32, m: u32, monday_first: bool) -> Vec<(Date, bool)> {
    let first = weekday(y, m, 1);
    // Сколько дней прошлого месяца показать перед первым числом.
    let lead = if monday_first { first } else { (first + 1) % 7 };
    let (py, pm) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let prev_days = days_in_month(py, pm);
    let days = days_in_month(y, m);
    (0..42)
        .map(|i| {
            let i = i as i64 - i64::from(lead);
            if i < 0 {
                let day = (i64::from(prev_days) + i + 1) as u32;
                (
                    Date {
                        year: py,
                        month: pm,
                        day,
                    },
                    false,
                )
            } else if i < i64::from(days) {
                let day = (i + 1) as u32;
                (
                    Date {
                        year: y,
                        month: m,
                        day,
                    },
                    true,
                )
            } else {
                let day = (i - i64::from(days) + 1) as u32;
                (
                    Date {
                        year: ny,
                        month: nm,
                        day,
                    },
                    false,
                )
            }
        })
        .collect()
}

impl CalendarPanel {
    pub fn new(today: Date, today_text: String, names: Names) -> CalendarPanel {
        CalendarPanel {
            today,
            today_text,
            names,
            year: today.year,
            month: today.month,
            hover: None,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        let h = PAD + TODAY_H + HEADER_H + WEEK_H + 6.0 * ROW + PAD / 2.0;
        (WIDTH as u32, h.ceil() as u32)
    }

    /// Показанный месяц: (год, месяц).
    pub fn shown(&self) -> (i32, u32) {
        (self.year, self.month)
    }

    fn header_y(&self) -> f32 {
        PAD + TODAY_H
    }

    fn arrows(&self) -> [(Spot, f32); 2] {
        let x = WIDTH - PAD - 2.0 * ARROW;
        [(Spot::Prev, x), (Spot::Next, x + ARROW)]
    }

    fn spot_at(&self, x: f32, y: f32) -> Option<Spot> {
        if (PAD..PAD + TODAY_H).contains(&y) {
            return Some(Spot::Today);
        }
        let hy = self.header_y();
        if (hy..hy + HEADER_H).contains(&y) {
            for (spot, ax) in self.arrows() {
                if (ax..ax + ARROW).contains(&x) {
                    return Some(spot);
                }
            }
        }
        None
    }

    /// Пролистать на `delta` месяцев.
    fn flip(&mut self, delta: i32) {
        let index = self.year * 12 + self.month as i32 - 1 + delta;
        self.year = index.div_euclid(12);
        self.month = index.rem_euclid(12) as u32 + 1;
    }

    pub fn press(&mut self, x: f32, y: f32) -> CalendarAction {
        match self.spot_at(x, y) {
            Some(Spot::Prev) => self.flip(-1),
            Some(Spot::Next) => self.flip(1),
            Some(Spot::Today) if (self.year, self.month) != (self.today.year, self.today.month) => {
                self.year = self.today.year;
                self.month = self.today.month;
            }
            _ => return CalendarAction::Nothing,
        }
        CalendarAction::Redraw
    }

    pub fn motion(&mut self, x: f32, y: f32) -> CalendarAction {
        let hover = self.spot_at(x, y);
        if hover == self.hover {
            return CalendarAction::Nothing;
        }
        self.hover = hover;
        CalendarAction::Redraw
    }

    pub fn key(&mut self, key: Key) -> CalendarAction {
        match key {
            Key::Escape => CalendarAction::Close,
            Key::Left | Key::Up => {
                self.flip(-1);
                CalendarAction::Redraw
            }
            Key::Right | Key::Down => {
                self.flip(1);
                CalendarAction::Redraw
            }
            _ => CalendarAction::Nothing,
        }
    }

    pub fn render(&self, fonts: &mut Fonts) -> Pixmap {
        let (w, h) = self.size();
        let (wf, hf) = (w as f32, h as f32);
        let mut pm = Pixmap::new(w, h).expect("размер не нулевой");
        let id = Transform::identity();
        let fill = |pm: &mut Pixmap, path: Option<tiny_skia::Path>, c: [u8; 4]| {
            if let Some(p) = path {
                pm.fill_path(&p, &paint(c), FillRule::Winding, id, None);
            }
        };
        fill(&mut pm, rounded(0.0, 0.0, wf, hf, 16.0), BG);
        if let Some(r) = rounded(0.5, 0.5, wf - 1.0, hf - 1.0, 15.5) {
            let s = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&r, &paint(EDGE), &s, id, None);
        }

        // Сегодня словами; не на текущем месяце — подсказка «вернуться».
        if self.hover == Some(Spot::Today) && self.shown() != (self.today.year, self.today.month) {
            fill(
                &mut pm,
                rounded(PAD - 6.0, PAD, wf - 2.0 * PAD + 12.0, TODAY_H - 4.0, 8.0),
                HOVER,
            );
        }
        let ty = PAD + ((TODAY_H - 4.0 - BIG.line_height()) / 2.0).round();
        fonts.draw(
            &mut pm,
            &self.today_text,
            &BIG,
            PAD,
            ty,
            Some(wf - 2.0 * PAD),
        );

        // Месяц и год, стрелки.
        let hy = self.header_y();
        let line = PathBuilder::from_rect(
            tiny_skia::Rect::from_xywh(PAD, hy - 1.0, wf - 2.0 * PAD, 1.0).expect("не пустой"),
        );
        pm.fill_path(&line, &paint(LINE), FillRule::Winding, id, None);
        let title = format!(
            "{} {}",
            self.names.months[(self.month - 1) as usize],
            self.year
        );
        let tty = hy + ((HEADER_H - TITLE.line_height()) / 2.0).round();
        fonts.draw(
            &mut pm,
            &title,
            &TITLE,
            PAD,
            tty,
            Some(wf - 2.0 * PAD - 2.0 * ARROW),
        );
        for (spot, ax) in self.arrows() {
            let (cx, cy) = (ax + ARROW / 2.0, hy + HEADER_H / 2.0);
            if self.hover == Some(spot) {
                fill(&mut pm, circle(cx, cy, ARROW / 2.0 - 1.0), HOVER);
            }
            let dx = if spot == Spot::Prev { 2.0 } else { -2.0 };
            let mut pb = PathBuilder::new();
            pb.move_to(cx + dx, cy - 5.0);
            pb.line_to(cx - dx, cy);
            pb.line_to(cx + dx, cy + 5.0);
            if let Some(p) = pb.finish() {
                let s = Stroke {
                    width: 1.8,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Stroke::default()
                };
                pm.stroke_path(&p, &paint([255, 255, 255, 220]), &s, id, None);
            }
        }

        // Дни недели.
        let wy = hy + HEADER_H;
        for col in 0..7 {
            let i = if self.names.monday_first {
                col
            } else {
                (col + 6) % 7
            };
            let name = &self.names.weekdays[i];
            let nw = fonts.measure(name, &WEEK);
            let x = PAD + col as f32 * CELL + (CELL - nw) / 2.0;
            fonts.draw(&mut pm, name, &WEEK, x, wy + 2.0, None);
        }

        // Числа.
        let gy = wy + WEEK_H;
        for (i, (date, this_month)) in month_grid(self.year, self.month, self.names.monday_first)
            .into_iter()
            .enumerate()
        {
            let (row, col) = ((i / 7) as f32, (i % 7) as f32);
            let (cx, cy) = (PAD + col * CELL + CELL / 2.0, gy + row * ROW + ROW / 2.0);
            let is_today = date == self.today;
            let weekend = weekday(date.year, date.month, date.day) >= 5;
            let mut style = DAY;
            if is_today {
                fill(&mut pm, circle(cx, cy, ROW / 2.0 - 2.0), ACCENT);
                style.weight = 600;
                style.color = [255, 255, 255, 255];
            } else if !this_month {
                style.color[3] = 70;
            } else if weekend {
                style.color[3] = 150;
            }
            let text = date.day.to_string();
            let tw = fonts.measure(&text, &style);
            let y = cy - (style.line_height() / 2.0).round();
            fonts.draw(&mut pm, &text, &style, (cx - tw / 2.0).round(), y, None);
        }
        pm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn dates() {
        assert_eq!(weekday(2026, 10, 23), 4, "23.10.2026 — пятница");
        assert_eq!(weekday(2024, 1, 1), 0, "1.01.2024 — понедельник");
        assert_eq!(weekday(2000, 2, 29), 1, "29.02.2000 — вторник");
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(2026, 4), 30);
    }

    #[test]
    fn grid_starts_on_the_right_day() {
        // Октябрь 2026 начинается в четверг: с понедельника — три дня сентября.
        let g = month_grid(2026, 10, true);
        assert_eq!(g.len(), 42);
        assert_eq!(g[0], (d(2026, 9, 28), false));
        assert_eq!(g[3], (d(2026, 10, 1), true));
        assert_eq!(g[33], (d(2026, 10, 31), true));
        assert_eq!(g[34], (d(2026, 11, 1), false));
        // С воскресенья — четыре дня сентября.
        assert_eq!(month_grid(2026, 10, false)[4], (d(2026, 10, 1), true));
        // Январь: перед ним — декабрь прошлого года.
        assert_eq!(month_grid(2027, 1, true)[0].0, d(2026, 12, 28));
    }

    #[test]
    fn flip_and_clicks() {
        let today = d(2026, 12, 31);
        let mut c = CalendarPanel::new(today, "Четверг, 31 декабря".into(), Names::default());
        c.key(Key::Right);
        assert_eq!(c.shown(), (2027, 1));
        c.key(Key::Left);
        c.key(Key::Left);
        assert_eq!(c.shown(), (2026, 11));
        // Стрелка «следующий» в заголовке.
        let [_, (_, nx)] = c.arrows();
        let y = c.header_y() + HEADER_H / 2.0;
        assert_eq!(c.press(nx + 5.0, y), CalendarAction::Redraw);
        assert_eq!(c.shown(), (2026, 12));
        c.press(nx + 5.0, y);
        // Щелчок по сегодняшней дате — обратно к текущему месяцу.
        assert_eq!(c.press(40.0, PAD + 5.0), CalendarAction::Redraw);
        assert_eq!(c.shown(), (2026, 12));
        assert_eq!(c.key(Key::Escape), CalendarAction::Close);

        let mut fonts = Fonts::new("Inter");
        let pm = c.render(&mut fonts);
        assert_eq!((pm.width(), pm.height()), c.size());
    }
}
