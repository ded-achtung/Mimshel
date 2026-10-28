//! Окошко громкости под значком динамика, как в macOS: «Звук» и проценты,
//! ползунок (щелчок или перетаскивание), громкость каждой программы, которая
//! сейчас выводит звук (как микшер Windows), устройства вывода (галочка — у
//! нынешнего) и «Настройки звука…». Здесь состояние, мышь и рисование;
//! менять громкость — дело службы.

use mimshel_core::input::Key;
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{circle, paint, rounded};
use tiny_skia::{FillRule, LineCap, LineJoin, PathBuilder, Rect, Stroke, Transform};

use crate::status::Sound;

/// Строка окошка, на которую можно навести или нажать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Slider,
    /// Ползунок программы с этим номером в `apps`.
    App(usize),
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
    /// Громкость программы с этим номером в `apps`.
    SetAppVolume(usize, u32),
    /// Сделать устройство с этим номером в `sinks` устройством по умолчанию.
    SetSink(usize),
    Settings,
}

pub struct SoundPanel {
    pub sound: Sound,
    pub language: mimshel_core::i18n::Language,
    pub hover: Option<Row>,
    /// Какой ползунок тащат: общий (`Row::Slider`) или программы.
    dragging: Option<Row>,
}

const WIDTH: f32 = 280.0;
const PAD: f32 = 5.0;
const TITLE: f32 = 26.0;
const SLIDER: f32 = 30.0;
const SEPARATOR: f32 = 11.0;
const ROW: f32 = 22.0;
/// Программа: строка с названием и процентами и под ней ползунок.
const APP_SLIDER: f32 = 22.0;
const APP_H: f32 = ROW + APP_SLIDER;
/// Больше программ не показываем — окошко не должно уходить за экран.
const MAX_APPS: usize = 8;
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
            language: mimshel_core::i18n::Language::Ru,
            hover: None,
            dragging: None,
        }
    }

    pub fn set_language(&mut self, lang: mimshel_core::i18n::Language) {
        self.language = lang;
    }

    fn slider_top() -> f32 {
        PAD + TITLE
    }

    /// Сколько программ показано.
    fn app_count(&self) -> usize {
        self.sound.apps.len().min(MAX_APPS)
    }

    /// Верх строки «Программы» (подпись над программами).
    fn apps_caption_top() -> f32 {
        Self::slider_top() + SLIDER + SEPARATOR
    }

    fn app_top(i: usize) -> f32 {
        Self::apps_caption_top() + ROW + i as f32 * APP_H
    }

    /// Середина ползунка программы `i` по высоте.
    fn app_slider_cy(i: usize) -> f32 {
        Self::app_top(i) + ROW + APP_SLIDER / 2.0 - 2.0
    }

    /// Верх строки «Выход» (подпись над устройствами). Программ нет — сразу
    /// под общим ползунком.
    fn caption_top(&self) -> f32 {
        match self.app_count() {
            0 => Self::slider_top() + SLIDER + SEPARATOR,
            n => Self::app_top(n) + SEPARATOR,
        }
    }

    fn sink_top(&self, i: usize) -> f32 {
        self.caption_top() + ROW + i as f32 * ROW
    }

    fn settings_top(&self) -> f32 {
        self.sink_top(self.sound.sinks.len()) + SEPARATOR
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
        if let Some(i) = (0..self.app_count())
            .find(|&i| (Self::app_top(i)..Self::app_top(i) + APP_H).contains(&y))
        {
            return Some(Row::App(i));
        }
        if let Some(i) = (0..self.sound.sinks.len())
            .find(|&i| (self.sink_top(i)..self.sink_top(i) + ROW).contains(&y))
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
                self.dragging = Some(Row::Slider);
                self.set_volume(Self::volume_at(x))
            }
            Some(Row::App(i)) => {
                self.dragging = Some(Row::App(i));
                self.set_app_volume(i, Self::volume_at(x))
            }
            Some(Row::Sink(i)) => SoundAction::SetSink(i),
            Some(Row::Settings) => SoundAction::Settings,
            None => SoundAction::Nothing,
        }
    }

    pub fn motion(&mut self, x: f32, y: f32) -> SoundAction {
        match self.dragging {
            Some(Row::App(i)) => return self.set_app_volume(i, Self::volume_at(x)),
            Some(_) => return self.set_volume(Self::volume_at(x)),
            None => {}
        }
        let hover = self
            .row_at(x, y)
            .filter(|r| !matches!(r, Row::Slider | Row::App(_)));
        if hover == self.hover {
            return SoundAction::Nothing;
        }
        self.hover = hover;
        SoundAction::Redraw
    }

    /// Тащат ли сейчас ползунок.
    pub fn dragging(&self) -> bool {
        self.dragging.is_some()
    }

    pub fn release(&mut self) {
        self.dragging = None;
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

    /// То же для программы `i`.
    fn set_app_volume(&mut self, i: usize, v: u32) -> SoundAction {
        let Some(app) = self.sound.apps.get_mut(i) else {
            return SoundAction::Nothing;
        };
        if v == app.volume && !app.muted {
            return SoundAction::Nothing;
        }
        app.volume = v;
        app.muted = false;
        SoundAction::SetAppVolume(i, v)
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
        fonts.draw(
            &mut pm,
            mimshel_core::i18n::t(self.language, "sound.title"),
            &BOLD,
            TRACK_X0,
            ty,
            None,
        );
        let pct = if self.sound.muted {
            mimshel_core::i18n::t(self.language, "sound.off").to_string()
        } else {
            format!("{} %", self.sound.volume)
        };
        let pw = fonts.measure(&pct, &TEXT);
        fonts.draw(&mut pm, &pct, &TEXT, TRACK_X1 - pw, ty, None);

        // Ползунок: дорожка, заполненная часть и кружок (`knob` — его радиус).
        let slider = |pm: &mut Canvas, cy: f32, v: u32, knob: f32| {
            fill(
                pm,
                rounded(TRACK_X0, cy - 3.0, TRACK_X1 - TRACK_X0, 6.0, 3.0),
                TRACK,
            );
            let kx = TRACK_X0 + (TRACK_X1 - TRACK_X0) * v as f32 / 100.0;
            fill(
                pm,
                rounded(TRACK_X0, cy - 3.0, (kx - TRACK_X0).max(6.0), 6.0, 3.0),
                FILL,
            );
            fill(pm, circle(kx, cy, knob + 1.0), [0, 0, 0, 0x40]);
            fill(pm, circle(kx, cy, knob), FILL);
        };
        let shown = |v: u32, muted: bool| if muted { 0 } else { v };
        slider(
            &mut pm,
            Self::slider_top() + SLIDER / 2.0,
            shown(self.sound.volume, self.sound.muted),
            8.0,
        );

        let separator = |pm: &mut Canvas, y: f32| {
            if let Some(r) = Rect::from_xywh(INSET + 5.0, y.round(), wf - 2.0 * INSET - 10.0, 1.0) {
                pm.fill_rect(r, &paint(LINE), Transform::identity(), None);
            }
        };
        separator(&mut pm, Self::slider_top() + SLIDER + SEPARATOR / 2.0);

        // Программы: название и проценты, под ними — свой ползунок.
        if self.app_count() > 0 {
            fonts.draw(
                &mut pm,
                mimshel_core::i18n::t(self.language, "sound.apps"),
                &DIM,
                TRACK_X0,
                line_y(Self::apps_caption_top(), &DIM),
                None,
            );
            for (i, app) in self.sound.apps.iter().take(MAX_APPS).enumerate() {
                let top = Self::app_top(i);
                let pct = if app.muted {
                    mimshel_core::i18n::t(self.language, "sound.off").to_string()
                } else {
                    format!("{} %", app.volume)
                };
                let pw = fonts.measure(&pct, &TEXT);
                fonts.draw(
                    &mut pm,
                    &pct,
                    &TEXT,
                    TRACK_X1 - pw,
                    line_y(top, &TEXT),
                    None,
                );
                fonts.draw(
                    &mut pm,
                    &app.name,
                    &TEXT,
                    TRACK_X0,
                    line_y(top, &TEXT),
                    Some(TRACK_X1 - pw - TRACK_X0 - 8.0),
                );
                slider(
                    &mut pm,
                    Self::app_slider_cy(i),
                    shown(app.volume, app.muted),
                    6.5,
                );
            }
            separator(&mut pm, self.caption_top() - SEPARATOR / 2.0);
        }

        fonts.draw(
            &mut pm,
            mimshel_core::i18n::t(self.language, "sound.output"),
            &DIM,
            TRACK_X0,
            line_y(self.caption_top(), &DIM),
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
                self.sink_top(i),
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
            mimshel_core::i18n::t(self.language, "sound.settings"),
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
    use crate::status::{AppSound, Sink};

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
            apps: vec![],
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
        assert_eq!(p.motion(100.0, p.sink_top(1) + 3.0), SoundAction::Redraw);
        assert_eq!(p.hover, Some(Row::Sink(1)));
        assert_eq!(p.press(100.0, p.sink_top(1) + 3.0), SoundAction::SetSink(1));
        assert_eq!(
            p.press(100.0, p.settings_top() + 3.0),
            SoundAction::Settings
        );
        assert_eq!(p.press(100.0, -5.0), SoundAction::Close, "мимо окошка");
        assert_eq!(p.key(Key::Left), SoundAction::SetVolume(95));
    }

    #[test]
    fn app_sliders() {
        let mut p = panel();
        let sinks_before = p.sink_top(0);
        p.sound.apps = vec![
            AppSound {
                name: "Firefox".into(),
                inputs: vec![89, 91],
                volume: 100,
                muted: false,
            },
            AppSound {
                name: "mpv".into(),
                inputs: vec![90],
                volume: 50,
                muted: true,
            },
        ];
        // Программы встают между общим ползунком и устройствами.
        assert!(p.sink_top(0) > sinks_before);
        let cy = SoundPanel::app_slider_cy(1);
        assert_eq!(p.row_at(100.0, cy), Some(Row::App(1)));
        assert_eq!(p.press(TRACK_X0, cy), SoundAction::SetAppVolume(1, 0));
        assert_eq!(
            p.motion(TRACK_X1, cy + 40.0),
            SoundAction::SetAppVolume(1, 100),
            "тащат — ползунок свой, куда бы ни ушла мышь"
        );
        p.release();
        assert_eq!(p.sound.apps[1].volume, 100);
        assert!(!p.sound.apps[1].muted);
        // Общий ползунок не тронут.
        assert_eq!(p.sound.volume, 40);
        let mut fonts = Fonts::new("Inter");
        let pm = p.render(&mut fonts);
        assert_eq!((pm.width(), pm.height()), p.size());
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
