//! Transfer progress and drawing, independent of X11 and file operations.

use std::time::Duration;

use mimshel_core::i18n::{Language, t};
use mimshel_icons::Icons;
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{paint, rounded};
use tiny_skia::{Color, FillRule, Transform};

/// The window as macOS Finder's copy window: a row per transfer — the
/// item's icon, the title, a thin bar with a round stop button beside it,
/// the amount and the time left under it.
pub const WIDTH: u32 = 460;
pub const ROW_HEIGHT: u32 = 72;
/// Above the first row and below the last one.
const EDGE: f32 = 6.0;
const ICON: u32 = 40;
const TEXT_X: f32 = 16.0 + ICON as f32 + 12.0;
/// The stop button: a circle this far left of the right edge, its radius.
const STOP_RIGHT: f32 = 26.0;
const STOP_R: f32 = 9.0;
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
    /// The item is a folder (its icon in the row).
    pub is_dir: bool,
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
    let rows = u32::try_from(rows.max(1)).unwrap_or(u32::MAX);
    ((2.0 * EDGE) as u32).saturating_add(ROW_HEIGHT.saturating_mul(rows))
}

/// Top of row `index`.
fn row_top(index: usize) -> f32 {
    EDGE + index as f32 * ROW_HEIGHT as f32
}

/// The bar's middle, in row coordinates (the stop button sits on it).
const BAR_Y: f32 = 34.0;

/// The middle of row `index`'s stop button in a window `width` wide.
pub fn stop_center(index: usize, width: u32) -> (f32, f32) {
    (width as f32 - STOP_RIGHT, row_top(index) + BAR_Y)
}

pub fn stop_hit(index: usize, width: u32, x: f32, y: f32) -> bool {
    let (cx, cy) = stop_center(index, width);
    // A little more than the circle: easy to hit.
    (x - cx).powi(2) + (y - cy).powi(2) <= (STOP_R + 4.0).powi(2)
}

pub fn render(
    rows: &[Row],
    size: (u32, u32),
    fonts: &mut Fonts,
    icons: &mut Icons,
    lang: Language,
) -> Option<Canvas> {
    let mut frame = Canvas::new(size.0.max(1), size.1.max(1))?;
    frame.fill(Color::from_rgba8(0x26, 0x26, 0x29, 255));
    let title = Style {
        size: 13.0,
        color: [0xf2, 0xf2, 0xf7, 255],
        weight: 400,
    };
    let small = Style {
        size: 11.0,
        color: [0x98, 0x98, 0x9f, 255],
        weight: 400,
    };
    let w = size.0 as f32;
    let bar_w = (w - TEXT_X - STOP_RIGHT - STOP_R - 12.0).max(40.0);
    for (i, row) in rows.iter().enumerate() {
        let y = row_top(i);
        if i > 0 {
            box_fill(
                &mut frame,
                (TEXT_X, y, w - TEXT_X, 1.0),
                0.0,
                [255, 255, 255, 20],
            );
        }
        let icon = if row.is_dir {
            "folder"
        } else {
            "text-x-generic"
        };
        if let Some(pm) = icons.get(icon, ICON) {
            frame.draw_icon(
                pm.as_ref(),
                16.0,
                y + (ROW_HEIGHT as f32 - ICON as f32) / 2.0,
                &tiny_skia::PixmapPaint::default(),
            );
        }
        fonts.draw(
            &mut frame,
            &row.title(lang),
            &title,
            TEXT_X,
            y + 10.0,
            Some(w - TEXT_X - 16.0),
        );
        // A thin bar on a grey track, as in Finder.
        let bar_y = y + BAR_Y - 2.5;
        box_fill(
            &mut frame,
            (TEXT_X, bar_y, bar_w, 5.0),
            2.5,
            [255, 255, 255, 34],
        );
        let progress = if row.phase == Phase::Counting {
            0.0
        } else {
            row.fraction() as f32
        };
        if progress > 0.0 {
            box_fill(
                &mut frame,
                (TEXT_X, bar_y, (bar_w * progress).max(5.0), 5.0),
                2.5,
                [10, 132, 255, 255],
            );
        }
        stop_button(&mut frame, (w - STOP_RIGHT, y + BAR_Y), row.stoppable());
        fonts.draw(
            &mut frame,
            &row.detail(lang),
            &small,
            TEXT_X,
            y + 44.0,
            Some(w - TEXT_X - 16.0),
        );
    }
    Some(frame)
}

/// The round grey stop button with a cross; dimmer when stopping is not
/// possible (the originals are being removed).
fn stop_button(frame: &mut Canvas, (cx, cy): (f32, f32), enabled: bool) {
    let (bg, cross) = if enabled {
        ([255, 255, 255, 46], [0xd8, 0xd8, 0xdc, 255])
    } else {
        ([255, 255, 255, 16], [0x70, 0x70, 0x75, 255])
    };
    let mut pb = tiny_skia::PathBuilder::new();
    pb.push_circle(cx, cy, STOP_R);
    if let Some(path) = pb.finish() {
        frame.fill_path(
            &path,
            &paint(bg),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let d = 3.2;
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(cx - d, cy - d);
    pb.line_to(cx + d, cy + d);
    pb.move_to(cx + d, cy - d);
    pb.line_to(cx - d, cy + d);
    if let Some(path) = pb.finish() {
        let stroke = tiny_skia::Stroke {
            width: 1.6,
            line_cap: tiny_skia::LineCap::Round,
            ..tiny_skia::Stroke::default()
        };
        frame.stroke_path(&path, &paint(cross), &stroke, Transform::identity(), None);
    }
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
        is_dir: true,
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
        assert!(
            render(
                &[],
                (u32::MAX, u32::MAX),
                &mut fonts,
                &mut Icons::new("hicolor"),
                Language::En
            )
            .is_none()
        );
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
        assert!(row.detail(Language::En).starts_with("1.4 GB of 3.4 GB"));
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
            &mut Icons::new("hicolor"),
            Language::En,
        )
        .unwrap();
        assert_eq!(frame.width(), WIDTH);
        // The round button beside the bar of each row.
        let (x, y) = (WIDTH as f32 - STOP_RIGHT, row_top(0) + BAR_Y);
        assert!(stop_hit(0, WIDTH, x, y));
        assert!(stop_hit(0, WIDTH, x + STOP_R, y));
        assert!(!stop_hit(1, WIDTH, x, y));
        assert!(stop_hit(1, WIDTH, x, y + ROW_HEIGHT as f32));
        assert!(!stop_hit(0, WIDTH, 20.0, y));
    }
}
