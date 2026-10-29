//! Transfer progress and drawing, independent of X11 and file operations.

use std::time::Duration;

use mimshel_core::i18n::{Language, t};
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{paint, rounded};
use tiny_skia::{Color, FillRule, Transform};

pub const WIDTH: u32 = 620;
pub const ROW_HEIGHT: u32 = 126;
pub const DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Amount {
    pub bytes: u64,
    pub files: u64,
}

impl Amount {
    pub fn plus(self, other: Self) -> Self {
        Self {
            bytes: self.bytes.saturating_add(other.bytes),
            files: self.files.saturating_add(other.files),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Counting,
    Copying,
    Removing,
    Finishing,
    Stopping,
    Done,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub name: String,
    pub folder: String,
    pub moving: bool,
    pub phase: Phase,
    pub total: Option<Amount>,
    pub arrived: Amount,
    pub elapsed: Duration,
    pub copying_for: Duration,
}

impl Row {
    pub fn visible(&self) -> bool {
        self.elapsed > DELAY && self.phase != Phase::Done
    }

    pub fn stoppable(&self) -> bool {
        matches!(self.phase, Phase::Counting | Phase::Copying)
    }

    pub fn fraction(&self) -> f64 {
        self.total.map_or(0.0, |total| {
            let (done, all) = if total.bytes > 0 {
                (self.arrived.bytes, total.bytes)
            } else {
                (self.arrived.files, total.files)
            };
            if all == 0 {
                0.0
            } else {
                (done as f64 / all as f64).clamp(0.0, 1.0)
            }
        })
    }

    pub fn remaining(&self) -> Option<Duration> {
        let fraction = self.fraction();
        (self.phase == Phase::Copying
            && self.copying_for >= Duration::from_secs(4)
            && fraction > 0.0
            && fraction < 1.0)
            .then(|| {
                Duration::from_secs(
                    (self.copying_for.as_secs_f64() * (1.0 - fraction) / fraction)
                        .min(365.0 * 86400.0)
                        .round() as u64,
                )
            })
    }

    pub fn title(&self, lang: Language) -> String {
        t(
            lang,
            if self.moving {
                "transfer.moving"
            } else {
                "transfer.copying"
            },
        )
        .replace("{name}", &self.name)
        .replace("{folder}", &self.folder)
    }

    pub fn detail(&self, lang: Language) -> String {
        match self.phase {
            Phase::Counting => return t(lang, "transfer.counting").into(),
            Phase::Finishing => return t(lang, "transfer.finishing").into(),
            Phase::Removing => return t(lang, "transfer.removing").into(),
            Phase::Stopping => return t(lang, "transfer.stopping").into(),
            _ => {}
        }
        let Some(total) = self.total else {
            return t(lang, "transfer.measuring").into();
        };
        let mut text = t(lang, "transfer.amount")
            .replace("{done}", &bytes(self.arrived.bytes.min(total.bytes), lang))
            .replace("{total}", &bytes(total.bytes, lang))
            .replace("{files}", &self.arrived.files.min(total.files).to_string())
            .replace("{all_files}", &total.files.to_string());
        if let Some(time) = self.remaining() {
            let minutes = time.as_secs().div_ceil(60).max(1);
            text.push_str(
                &t(lang, "transfer.remaining").replace("{minutes}", &minutes.to_string()),
            );
        }
        text
    }
}

fn bytes(n: u64, lang: Language) -> String {
    let units = [
        "transfer.bytes",
        "transfer.kb",
        "transfer.mb",
        "transfer.gb",
        "transfer.tb",
    ];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < units.len() {
        value /= 1000.0;
        unit += 1;
    }
    let number = if unit == 0 {
        n.to_string()
    } else {
        format!("{value:.1}")
    };
    let number = if lang.resolve() == Language::Ru {
        number.replace('.', ",")
    } else {
        number
    };
    format!(
        "{number} {}",
        t(lang, units.get(unit).copied().unwrap_or("transfer.tb"))
    )
}

pub fn height(rows: usize) -> u32 {
    24u32.saturating_add(ROW_HEIGHT.saturating_mul(u32::try_from(rows.max(1)).unwrap_or(u32::MAX)))
}

pub fn stop_hit(index: usize, width: u32, x: f32, y: f32) -> bool {
    let top = 12.0 + index as f32 * ROW_HEIGHT as f32;
    x >= width as f32 - 144.0 && x < width as f32 - 28.0 && y >= top + 76.0 && y < top + 102.0
}

pub fn render(rows: &[Row], size: (u32, u32), fonts: &mut Fonts, lang: Language) -> Option<Canvas> {
    let mut frame = Canvas::new(size.0.max(1), size.1.max(1))?;
    frame.fill(Color::from_rgba8(0x1e, 0x1e, 0x20, 255));
    let text = Style {
        size: 13.0,
        color: [0xf2, 0xf2, 0xf7, 255],
        weight: 600,
    };
    let small = Style {
        size: 11.0,
        color: [0x98, 0x98, 0x9f, 255],
        weight: 400,
    };
    for (i, row) in rows.iter().enumerate() {
        let y = 12.0 + i as f32 * ROW_HEIGHT as f32;
        let w = size.0 as f32;
        box_fill(
            &mut frame,
            (12.0, y, w - 24.0, 114.0),
            10.0,
            [255, 255, 255, 13],
        );
        fonts.draw(
            &mut frame,
            &row.title(lang),
            &text,
            28.0,
            y + 14.0,
            Some(w - 56.0),
        );
        box_fill(
            &mut frame,
            (28.0, y + 43.0, w - 56.0, 6.0),
            3.0,
            [255, 255, 255, 30],
        );
        let progress = if row.phase == Phase::Counting {
            0.15
        } else {
            row.fraction() as f32
        };
        if progress > 0.0 {
            box_fill(
                &mut frame,
                (28.0, y + 43.0, (w - 56.0) * progress, 6.0),
                3.0,
                [10, 132, 255, 255],
            );
        }
        fonts.draw(
            &mut frame,
            &row.detail(lang),
            &small,
            28.0,
            y + 56.0,
            Some(w - 56.0),
        );
        let enabled = row.stoppable();
        box_fill(
            &mut frame,
            (w - 144.0, y + 76.0, 116.0, 26.0),
            6.0,
            [255, 255, 255, if enabled { 28 } else { 10 }],
        );
        let button = Style {
            size: 12.0,
            color: if enabled { text.color } else { small.color },
            weight: 400,
        };
        fonts.draw(
            &mut frame,
            t(lang, "transfer.stop"),
            &button,
            w - 132.0,
            y + 81.0,
            Some(96.0),
        );
    }
    Some(frame)
}

fn box_fill(frame: &mut Canvas, (x, y, w, h): (f32, f32, f32, f32), radius: f32, color: [u8; 4]) {
    if let Some(path) = rounded(x, y, w, h, radius) {
        frame.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

pub fn example() -> Row {
    Row {
        name: "DCIM".into(),
        folder: "Desktop".into(),
        moving: false,
        phase: Phase::Copying,
        total: Some(Amount {
            bytes: 3_400_000_000,
            files: 1200,
        }),
        arrived: Amount {
            bytes: 1_360_000_000,
            files: 480,
        },
        elapsed: Duration::from_secs(80),
        copying_for: Duration::from_secs(80),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_canvas_dimensions_fail_without_panicking() {
        let mut fonts = Fonts::new("DejaVu Sans");
        assert!(render(&[], (u32::MAX, u32::MAX), &mut fonts, Language::En).is_none());
        assert_eq!(height(usize::MAX), u32::MAX);
    }

    #[test]
    fn threshold_and_finished_rows() {
        let mut row = example();
        for (elapsed, expected) in [(999, false), (1000, false), (1001, true)] {
            row.elapsed = Duration::from_millis(elapsed);
            assert_eq!(row.visible(), expected);
        }
        row.phase = Phase::Done;
        assert!(!row.visible());
    }

    #[test]
    fn progress_and_estimate() {
        let mut row = example();
        assert!((row.fraction() - 0.4).abs() < 0.001);
        assert_eq!(row.remaining(), Some(Duration::from_secs(120)));
        row.copying_for = Duration::from_secs(3);
        assert_eq!(row.remaining(), None);
        row.arrived.bytes = u64::MAX;
        assert_eq!(row.fraction(), 1.0);
        assert_eq!(row.remaining(), None);
        row.total = Some(Amount {
            bytes: 0,
            files: 10,
        });
        row.arrived.files = 4;
        assert_eq!(row.fraction(), 0.4);
        row.total = Some(Amount::default());
        assert_eq!(row.fraction(), 0.0);
        row.total = None;
        assert_eq!(row.fraction(), 0.0);
    }

    #[test]
    fn localized_rows_and_phases() {
        let mut row = example();
        row.folder = "Рабочий стол".into();
        assert_eq!(
            row.title(Language::Ru),
            "Копирование «DCIM» в «Рабочий стол»"
        );
        assert!(row.detail(Language::Ru).contains("1,4 ГБ из 3,4 ГБ"));
        assert!(row.detail(Language::Ru).contains("осталось около 2 мин"));
        row.folder = "Desktop".into();
        row.moving = true;
        assert_eq!(row.title(Language::En), "Moving “DCIM” to “Desktop”");
        assert!(row.detail(Language::En).contains("480 of 1200 files"));
        row.phase = Phase::Counting;
        assert_eq!(row.detail(Language::Ru), "Подсчёт…");
        row.phase = Phase::Removing;
        assert!(!row.stoppable());
        assert_eq!(row.detail(Language::Ru), "Удаление исходных…");
        assert_eq!(row.detail(Language::En), "Removing originals…");
    }

    #[test]
    fn drawing_and_button_bounds() {
        let row = example();
        let frame = render(
            &[row],
            (WIDTH, height(1)),
            &mut Fonts::new("DejaVu Sans"),
            Language::En,
        )
        .unwrap();
        assert_eq!(frame.width(), WIDTH);
        assert!(stop_hit(0, WIDTH, 520.0, 90.0));
        assert!(!stop_hit(1, WIDTH, 520.0, 90.0));
        assert!(!stop_hit(0, WIDTH, 20.0, 90.0));
    }
}
