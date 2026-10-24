//! Окошко громкости под значком динамика, как в macOS: «Звук» и проценты,
//! ползунок (щелчок или перетаскивание), устройства вывода (галочка — у
//! нынешнего) и «Настройки звука…». Здесь состояние, мышь и рисование;
//! менять громкость — дело службы.

use shell_core::input::Key;
use shell_render::Canvas;
use shell_text::{Fonts, Style};
use shell_widgets::shape::{circle, paint, rounded};
use tiny_skia::{FillRule, LineCap, LineJoin, PathBuilder, Rect, Stroke, Transform};

use crate::status::Sound;

/// Строка окошка, на которую можно навести или нажать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Slider,
    Sink(usize),
    Settings,
}

/// Что сделать после нажатия или движения мыши.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundAction {
    Nothing,
    Redraw,
    Close,
    SetVolume(u32),
    /// Сделать устройство с этим номером в `sinks` устройством по умолчанию.
    SetSink(usize),
    Settings,
}

pub struct SoundPanel {
    pub sound: Sound,
    pub hover: Option<Row>,
    /// Тащат ползунок.
    dragging: bool,
}

const WIDTH: f32 = 280.0;
const PAD: f32 = 5.0;
const TITLE: f32 = 26.0;
const SLIDER: f32 = 30.0;
const SEPARATOR: f32 = 11.0;
const ROW: f32 = 22.0;
/// Слева у строк — место под галочку.
const LEFT: f32 = 26.0;
const INSET: f32 = 5.0;
/// Края дорожки ползунка.
const TRACK_X0: f32 = 16.0;
const TRACK_X1: f32 = WIDTH - 16.0;

const BG: [u8; 4] = [0x28, 0x28, 0x28, 0xf0];
const EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const LINE: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
const HOVER: [u8; 4] = [0x0a, 0x64, 0xd2, 0xff];
const TRACK: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const FILL: [u8; 4] = [0xf2, 0xf2, 0xf2, 0xff];

const TEXT: Style = Style {
    size: 13.5,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 400,
};
const BOLD: Style = Style {
    weight: 700,
    ..TEXT
};
const DIM: Style = Style {
    size: 12.0,
    color: [0xa0, 0xa0, 0xa0, 0xff],
    weight: 600,
};

impl SoundPanel {
    pub fn new(sound: Sound) -> SoundPanel {
        SoundPanel {
            sound,
            hover: None,
            dragging: false,
        }
    }

    fn slider_top() -> f32 {
        PAD + TITLE
    }

    /// Верх строки «Выход» (подпись над устройствами).
    fn caption_top() -> f32 {
        Self::slider_top() + SLIDER + SEPARATOR
    }

    fn sink_top(i: usize) -> f32 {
        Self::caption_top() + ROW + i as f32 * ROW
    }

    fn settings_top(&self) -> f32 {
        Self::sink_top(self.sound.sinks.len()) + SEPARATOR
    }

    pub fn size(&self) -> (u32, u32) {
        (
            WIDTH as u32,
            (self.settings_top() + ROW + PAD).ceil() as u32,
        )
    }

    pub fn row_at(&self, x: f32, y: f32) -> Option<Row> {
        if !(0.0..WIDTH).contains(&x) {
            return None;
        }
        let top = Self::slider_top();
        if (top..top + SLIDER).contains(&y) {
            return Some(Row::Slider);
        }
        if let Some(i) = (0..self.sound.sinks.len())
            .find(|&i| (Self::sink_top(i)..Self::sink_top(i) + ROW).contains(&y))
        {
            return Some(Row::Sink(i));
        }
        let s = self.settings_top();
        (s..s + ROW).contains(&y).then_some(Row::Settings)
    }

    /// Громкость по точке на ползунке.
    fn volume_at(x: f32) -> u32 {
        ((x - TRACK_X0) / (TRACK_X1 - TRACK_X0) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u32
    }

    pub fn press(&mut self, x: f32, y: f32) -> SoundAction {
        let (_, h) = self.size();
        if !(0.0..WIDTH).contains(&x) || !(0.0..h as f32).contains(&y) {
            return SoundAction::Close;
        }
        match self.row_at(x, y) {
            Some(Row::Slider) => {
                self.dragging = true;
                self.set_volume(Self::volume_at(x))
            }
            Some(Row::Sink(i)) => SoundAction::SetSink(i),
            Some(Row::Settings) => SoundAction::Settings,
            None => SoundAction::Nothing,
        }
    }

    pub fn motion(&mut self, x: f32, y: f32) -> SoundAction {
        if self.dragging {
            return self.set_volume(Self::volume_at(x));
        }
        let hover = self.row_at(x, y).filter(|r| *r != Row::Slider);
        if hover == self.hover {
            return SoundAction::Nothing;
        }
        self.hover = hover;
        SoundAction::Redraw
    }

    /// Тащат ли сейчас ползунок.
    pub fn dragging(&self) -> bool {
        self.dragging
    }

    pub fn release(&mut self) {
        self.dragging = false;
    }

    pub fn key(&mut self, key: Key) -> SoundAction {
        match key {
            Key::Escape => SoundAction::Close,
            Key::Left | Key::Down => self.set_volume(self.sound.volume.saturating_sub(5)),
            Key::Right | Key::Up => self.set_volume((self.sound.volume + 5).min(100)),
            _ => SoundAction::Nothing,
        }
    }

    /// Ползунок сразу встаёт на новое место — не ждать ответа звуковой системы.
    fn set_volume(&mut self, v: u32) -> SoundAction {
        if v == self.sound.volume && !self.sound.muted {
            return SoundAction::Nothing;
        }
        self.sound.volume = v;
        self.sound.muted = false;
        SoundAction::SetVolume(v)
    }

    pub fn render(&self, fonts: &mut Fonts) -> Canvas {
        let (w, h) = self.size();
        let (wf, hf) = (w as f32, h as f32);
        let mut pm = Canvas::new(w, h).expect("размер не нулевой");
        let fill = |pm: &mut Canvas, path: Option<tiny_skia::Path>, c: [u8; 4]| {
            if let Some(p) = path {
                pm.fill_path(
                    &p,
                    &paint(c),
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        };
        fill(&mut pm, rounded(0.0, 0.0, wf, hf, 8.0), BG);
        if let Some(r) = rounded(0.5, 0.5, wf - 1.0, hf - 1.0, 7.5) {
            let stroke = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&r, &paint(EDGE), &stroke, Transform::identity(), None);
        }
        let line_y = |row: f32, style: &Style| row + ((ROW - style.line_height()) / 2.0).round();

        // «Звук» и проценты.
        let ty = PAD + ((TITLE - BOLD.line_height()) / 2.0).round();
        fonts.draw(&mut pm, "Звук", &BOLD, TRACK_X0, ty, None);
        let pct = if self.sound.muted {
            "выкл.".to_string()
        } else {
            format!("{} %", self.sound.volume)
        };
        let pw = fonts.measure(&pct, &TEXT);
        fonts.draw(&mut pm, &pct, &TEXT, TRACK_X1 - pw, ty, None);

        // Ползунок: дорожка, заполненная часть и кружок.
        let cy = Self::slider_top() + SLIDER / 2.0;
        fill(
            &mut pm,
            rounded(TRACK_X0, cy - 3.0, TRACK_X1 - TRACK_X0, 6.0, 3.0),
            TRACK,
        );
        let v = if self.sound.muted {
            0
        } else {
            self.sound.volume
        };
        let kx = TRACK_X0 + (TRACK_X1 - TRACK_X0) * v as f32 / 100.0;
        fill(
            &mut pm,
            rounded(TRACK_X0, cy - 3.0, (kx - TRACK_X0).max(6.0), 6.0, 3.0),
            FILL,
        );
        fill(&mut pm, circle(kx, cy, 9.0), [0, 0, 0, 0x40]);
        fill(&mut pm, circle(kx, cy, 8.0), FILL);

        let separator = |pm: &mut Canvas, y: f32| {
            if let Some(r) = Rect::from_xywh(INSET + 5.0, y.round(), wf - 2.0 * INSET - 10.0, 1.0) {
                pm.fill_rect(r, &paint(LINE), Transform::identity(), None);
            }
        };
        separator(&mut pm, Self::slider_top() + SLIDER + SEPARATOR / 2.0);
        fonts.draw(
            &mut pm,
            "Выход",
            &DIM,
            TRACK_X0,
            line_y(Self::caption_top(), &DIM),
            None,
        );

        let row = |pm: &mut Canvas,
                   fonts: &mut Fonts,
                   top: f32,
                   text: &str,
                   hover: bool,
                   checked: bool| {
            if hover {
                fill(pm, rounded(INSET, top, wf - 2.0 * INSET, ROW, 4.0), HOVER);
            }
            if checked {
                check(pm, 11.0, top + ROW / 2.0);
            }
            fonts.draw(
                pm,
                text,
                &TEXT,
                LEFT,
                line_y(top, &TEXT),
                Some(wf - LEFT - 10.0),
            );
        };
        for (i, sink) in self.sound.sinks.iter().enumerate() {
            row(
                &mut pm,
                fonts,
                Self::sink_top(i),
                &sink.description,
                self.hover == Some(Row::Sink(i)),
                sink.name == self.sound.default,
            );
        }
        separator(&mut pm, self.settings_top() - SEPARATOR / 2.0);
        row(
            &mut pm,
            fonts,
            self.settings_top(),
            "Настройки звука…",
            self.hover == Some(Row::Settings),
            false,
        );
        pm
    }
}

/// Галочка с серединой по высоте `y`.
fn check(pm: &mut Canvas, x: f32, y: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + 3.5, y + 3.5);
    pb.line_to(x + 10.0, y - 4.0);
    let Some(path) = pb.finish() else {
        return;
    };
    let stroke = Stroke {
        width: 1.8,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pm.stroke_path(
        &path,
        &paint(TEXT.color),
        &stroke,
        Transform::identity(),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::Sink;

    fn panel() -> SoundPanel {
        SoundPanel::new(Sound {
            volume: 40,
            muted: false,
            sinks: vec![
                Sink {
                    name: "a".into(),
                    description: "Встроенное аудио".into(),
                },
                Sink {
                    name: "b".into(),
                    description: "HDMI".into(),
                },
            ],
            default: "a".into(),
        })
    }

    #[test]
    fn slider_drag_and_rows() {
        let mut p = panel();
        let cy = SoundPanel::slider_top() + SLIDER / 2.0;
        assert_eq!(p.press(TRACK_X0, cy), SoundAction::SetVolume(0));
        assert_eq!(p.motion(TRACK_X1 + 50.0, cy), SoundAction::SetVolume(100));
        assert_eq!(p.motion(TRACK_X1, cy), SoundAction::Nothing, "уже 100");
        p.release();
        assert_eq!(
            p.motion(100.0, SoundPanel::sink_top(1) + 3.0),
            SoundAction::Redraw
        );
        assert_eq!(p.hover, Some(Row::Sink(1)));
        assert_eq!(
            p.press(100.0, SoundPanel::sink_top(1) + 3.0),
            SoundAction::SetSink(1)
        );
        assert_eq!(
            p.press(100.0, p.settings_top() + 3.0),
            SoundAction::Settings
        );
        assert_eq!(p.press(100.0, -5.0), SoundAction::Close, "мимо окошка");
        assert_eq!(p.key(Key::Left), SoundAction::SetVolume(95));
    }

    #[test]
    fn renders() {
        let mut fonts = Fonts::new("Inter");
        let p = panel();
        let pm = p.render(&mut fonts);
        assert_eq!((pm.width(), pm.height()), p.size());
        // Заполненная часть дорожки (40 %) светлая, дальше — тёмная.
        let cy = (SoundPanel::slider_top() + SLIDER / 2.0) as u32;
        let at = |pct: f32| {
            pm.pixel((TRACK_X0 + (TRACK_X1 - TRACK_X0) * pct) as u32, cy)
                .unwrap()
        };
        assert!(at(0.2).red() > 200);
        assert!(at(0.8).red() < 120);
    }
}
