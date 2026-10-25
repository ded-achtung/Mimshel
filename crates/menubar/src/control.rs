//! Пункт управления, как в macOS Sonoma: плитки под значком в строке меню.
//! Слева — «Сеть» и «Bluetooth» (кружок синий, когда включено), справа —
//! «Не беспокоить», ниже — «Звук» с ползунком. Чего нет в системе (Wi-Fi,
//! адаптера Bluetooth), то показывается как недоступное. Здесь состояние,
//! мышь и рисование; включать и выключать — дело службы.

use mimshel_core::input::Key;
use mimshel_icons::Icons;
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{circle, paint, rounded};
use tiny_skia::{FillRule, FilterQuality, PixmapPaint, Stroke, Transform};

use crate::status::Sound;

/// Сеть сейчас.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Network {
    /// Название подключения («Проводное подключение 1»); пусто — не подключено.
    pub connection: String,
    pub wired: bool,
    /// Wi-Fi: `None` — нет оборудования, иначе включено ли.
    pub wifi: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ControlState {
    pub network: Network,
    /// `None` — адаптера нет, иначе включён ли.
    pub bluetooth: Option<bool>,
    pub dnd: bool,
    pub sound: Option<Sound>,
}

/// Что на панели можно нажать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Network,
    Bluetooth,
    Dnd,
    Slider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlAction {
    Nothing,
    Redraw,
    Close,
    SetWifi(bool),
    SetBluetooth(bool),
    SetDnd(bool),
    SetVolume(u32),
    NetworkSettings,
    /// Открыть список устройств Bluetooth (щелчок по названию плитки).
    OpenBluetooth,
}

pub struct ControlPanel {
    pub state: ControlState,
    pub hover: Option<Tile>,
    dragging: bool,
}

const WIDTH: f32 = 300.0;
const PAD: f32 = 10.0;
const GAP: f32 = 8.0;
const LEFT_W: f32 = 168.0;
const ROW: f32 = 50.0;
const TOP_H: f32 = 2.0 * ROW + 12.0;
const SOUND_H: f32 = 66.0;
const R: f32 = 15.0;
const TILE_R: f32 = 14.0;

const BG: [u8; 4] = [0x24, 0x24, 0x26, 0xf2];
const EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const TILE: [u8; 4] = [0xff, 0xff, 0xff, 0x14];
const TILE_HOVER: [u8; 4] = [0xff, 0xff, 0xff, 0x22];
const ON: [u8; 4] = [0x0a, 0x84, 0xff, 0xff];
const OFF: [u8; 4] = [0xff, 0xff, 0xff, 0x2e];
const TRACK: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const FILL: [u8; 4] = [0xf2, 0xf2, 0xf2, 0xff];

const NAME: Style = Style {
    size: 13.0,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 700,
};
const STATUS: Style = Style {
    size: 11.5,
    color: [0xa8, 0xa8, 0xad, 0xff],
    weight: 400,
};

impl ControlPanel {
    pub fn new(state: ControlState) -> ControlPanel {
        ControlPanel {
            state,
            hover: None,
            dragging: false,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        let h = PAD
            + TOP_H
            + if self.state.sound.is_some() {
                GAP + SOUND_H
            } else {
                0.0
            }
            + PAD;
        (WIDTH as u32, h as u32)
    }

    /// Ползунок звука: края дорожки и середина по высоте.
    fn track() -> (f32, f32, f32) {
        let top = PAD + TOP_H + GAP;
        (PAD + 14.0, WIDTH - PAD - 14.0, top + SOUND_H - 20.0)
    }

    pub fn tile_at(&self, x: f32, y: f32) -> Option<Tile> {
        let inside =
            |x0: f32, y0: f32, w: f32, h: f32| x >= x0 && x < x0 + w && y >= y0 && y < y0 + h;
        if inside(PAD, PAD + 6.0, LEFT_W, ROW) {
            return Some(Tile::Network);
        }
        if inside(PAD, PAD + 6.0 + ROW, LEFT_W, ROW) {
            return Some(Tile::Bluetooth);
        }
        if inside(
            PAD + LEFT_W + GAP,
            PAD,
            WIDTH - 2.0 * PAD - LEFT_W - GAP,
            TOP_H,
        ) {
            return Some(Tile::Dnd);
        }
        if self.state.sound.is_some() && inside(PAD, PAD + TOP_H + GAP, WIDTH - 2.0 * PAD, SOUND_H)
        {
            return Some(Tile::Slider);
        }
        None
    }

    fn volume_at(x: f32) -> u32 {
        let (x0, x1, _) = Self::track();
        ((x - x0) / (x1 - x0) * 100.0).round().clamp(0.0, 100.0) as u32
    }

    fn set_volume(&mut self, v: u32) -> ControlAction {
        let Some(sound) = &mut self.state.sound else {
            return ControlAction::Nothing;
        };
        if sound.volume == v && !sound.muted {
            return ControlAction::Nothing;
        }
        sound.volume = v;
        sound.muted = false;
        ControlAction::SetVolume(v)
    }

    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// Правый край кружка со значком у плиток слева (с запасом для пальца).
    fn circle_right() -> f32 {
        PAD + 8.0 + 2.0 * R + 2.0 + 4.0
    }

    pub fn press(&mut self, x: f32, y: f32) -> ControlAction {
        let (w, h) = self.size();
        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
            return ControlAction::Close;
        }
        let tile = self.tile_at(x, y);
        let s = &mut self.state;
        match tile {
            Some(Tile::Network) => match s.network.wifi {
                Some(on) => {
                    s.network.wifi = Some(!on);
                    ControlAction::SetWifi(!on)
                }
                None => ControlAction::NetworkSettings,
            },
            // Кружок со значком — включить или выключить, название — список
            // устройств (как в macOS: стрелка у плитки).
            Some(Tile::Bluetooth) => match s.bluetooth {
                Some(on) if x < Self::circle_right() => {
                    s.bluetooth = Some(!on);
                    ControlAction::SetBluetooth(!on)
                }
                Some(_) => ControlAction::OpenBluetooth,
                None => ControlAction::Nothing,
            },
            Some(Tile::Dnd) => {
                s.dnd = !s.dnd;
                ControlAction::SetDnd(s.dnd)
            }
            Some(Tile::Slider) => {
                let (x0, x1, cy) = Self::track();
                if (cy - 14.0..cy + 14.0).contains(&y) && (x0 - 10.0..x1 + 10.0).contains(&x) {
                    self.dragging = true;
                    self.set_volume(Self::volume_at(x))
                } else {
                    ControlAction::Nothing
                }
            }
            None => ControlAction::Nothing,
        }
    }

    pub fn motion(&mut self, x: f32, y: f32) -> ControlAction {
        if self.dragging {
            return self.set_volume(Self::volume_at(x));
        }
        let hover = self.tile_at(x, y).filter(|t| *t != Tile::Slider);
        if hover == self.hover {
            return ControlAction::Nothing;
        }
        self.hover = hover;
        ControlAction::Redraw
    }

    pub fn release(&mut self) {
        self.dragging = false;
    }

    pub fn key(&mut self, key: Key) -> ControlAction {
        match key {
            Key::Escape => ControlAction::Close,
            _ => ControlAction::Nothing,
        }
    }

    pub fn render(&self, fonts: &mut Fonts, icons: &mut Icons) -> Canvas {
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
        fill(&mut pm, rounded(0.0, 0.0, wf, hf, 16.0), BG);
        if let Some(r) = rounded(0.5, 0.5, wf - 1.0, hf - 1.0, 15.5) {
            let s = Stroke {
                width: 1.0,
                ..Stroke::default()
            };
            pm.stroke_path(&r, &paint(EDGE), &s, Transform::identity(), None);
        }
        let tile = |pm: &mut Canvas, x: f32, y: f32, w: f32, h: f32, hover: bool| {
            fill(
                pm,
                rounded(x, y, w, h, TILE_R),
                if hover { TILE_HOVER } else { TILE },
            );
        };
        let mut icon = |pm: &mut Canvas, name: &str, cx: f32, cy: f32| {
            if let Some(img) = icons.get(name, 16) {
                let k = 16.0 / img.width().max(img.height()) as f32;
                let t = Transform::from_scale(k, k).post_translate(cx - 8.0, cy - 8.0);
                let q = PixmapPaint {
                    quality: FilterQuality::Bicubic,
                    ..PixmapPaint::default()
                };
                pm.draw_pixmap(0, 0, img.as_ref(), &q, t, None);
            }
        };

        // Слева: сеть и Bluetooth.
        tile(&mut pm, PAD, PAD, LEFT_W, TOP_H, false);
        let s = &self.state;
        let net_on = !s.network.connection.is_empty() || s.network.wifi == Some(true);
        let (net_icon, net_name) = if s.network.wifi.is_some() {
            ("network-wireless-symbolic", "Wi-Fi")
        } else {
            ("network-wired-symbolic", "Сеть")
        };
        let net_status = match (&s.network.wifi, s.network.connection.as_str()) {
            (Some(false), _) => "Выключен".to_string(),
            (_, "") => "Не подключено".to_string(),
            (_, c) => c.to_string(),
        };
        let bt_status = match s.bluetooth {
            Some(true) => "Включён",
            Some(false) => "Выключен",
            None => "Недоступен",
        };
        let rows = [
            (Tile::Network, net_icon, net_name, net_status, net_on),
            (
                Tile::Bluetooth,
                "bluetooth-active-symbolic",
                "Bluetooth",
                bt_status.to_string(),
                s.bluetooth == Some(true),
            ),
        ];
        for (k, (t, ic, name, status, on)) in rows.iter().enumerate() {
            let top = PAD + 6.0 + k as f32 * ROW;
            if self.hover == Some(*t) {
                fill(
                    &mut pm,
                    rounded(PAD + 4.0, top, LEFT_W - 8.0, ROW, 10.0),
                    TILE,
                );
            }
            let (cx, cy) = (PAD + 8.0 + R + 2.0, top + ROW / 2.0);
            fill(&mut pm, circle(cx, cy, R), if *on { ON } else { OFF });
            icon(&mut pm, ic, cx, cy);
            let tx = cx + R + 10.0;
            let tw = LEFT_W - (tx - PAD) - 8.0;
            fonts.draw(&mut pm, name, &NAME, tx, cy - 17.0, Some(tw));
            fonts.draw(&mut pm, status, &STATUS, tx, cy + 1.0, Some(tw));
        }

        // Справа: «Не беспокоить».
        let rx = PAD + LEFT_W + GAP;
        let rw = WIDTH - PAD - rx;
        tile(&mut pm, rx, PAD, rw, TOP_H, self.hover == Some(Tile::Dnd));
        let (cx, cy) = (rx + rw / 2.0, PAD + 36.0);
        fill(
            &mut pm,
            circle(cx, cy, R + 2.0),
            if s.dnd { ON } else { OFF },
        );
        icon(&mut pm, "weather-clear-night-symbolic", cx, cy);
        for (text, style, y) in [("Не", &NAME, cy + 24.0), ("беспокоить", &NAME, cy + 40.0)]
        {
            let tw = fonts.measure(text, style);
            fonts.draw(&mut pm, text, style, cx - tw / 2.0, y, None);
        }

        // Звук.
        if let Some(sound) = &s.sound {
            let top = PAD + TOP_H + GAP;
            tile(&mut pm, PAD, top, WIDTH - 2.0 * PAD, SOUND_H, false);
            fonts.draw(&mut pm, "Звук", &NAME, PAD + 14.0, top + 10.0, None);
            let (x0, x1, cy) = Self::track();
            fill(&mut pm, rounded(x0, cy - 10.0, x1 - x0, 20.0, 10.0), TRACK);
            let v = if sound.muted { 0 } else { sound.volume };
            let kx = x0 + 20.0 + (x1 - x0 - 20.0) * v as f32 / 100.0;
            fill(&mut pm, rounded(x0, cy - 10.0, kx - x0, 20.0, 10.0), FILL);
            fill(
                &mut pm,
                circle(kx - 10.0, cy, 9.0),
                [0xff, 0xff, 0xff, 0xff],
            );
            icon(&mut pm, sound.icon(), x0 + 12.0, cy);
        }
        pm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> ControlState {
        ControlState {
            network: Network {
                connection: "Проводное подключение 1".into(),
                wired: true,
                wifi: None,
            },
            bluetooth: None,
            dnd: false,
            sound: Some(Sound {
                volume: 40,
                muted: false,
                sinks: vec![],
                default: String::new(),
                apps: vec![],
            }),
        }
    }

    #[test]
    fn tiles_and_actions() {
        let mut p = ControlPanel::new(state());
        // Проводная сеть — щелчок открывает параметры, Bluetooth без адаптера — ничего.
        assert_eq!(
            p.press(40.0, PAD + 6.0 + 20.0),
            ControlAction::NetworkSettings
        );
        assert_eq!(
            p.press(40.0, PAD + 6.0 + ROW + 20.0),
            ControlAction::Nothing
        );
        assert_eq!(
            p.press(WIDTH - 40.0, PAD + 40.0),
            ControlAction::SetDnd(true)
        );
        assert!(p.state.dnd);
        let (x0, x1, cy) = ControlPanel::track();
        assert_eq!(p.press(x1, cy), ControlAction::SetVolume(100));
        assert_eq!(p.motion(x0 - 30.0, cy), ControlAction::SetVolume(0));
        p.release();
        assert_eq!(p.press(-5.0, 10.0), ControlAction::Close);

        // Есть Wi-Fi и Bluetooth — переключаются.
        let mut st = state();
        st.network.wifi = Some(true);
        st.bluetooth = Some(false);
        let mut p = ControlPanel::new(st);
        assert_eq!(
            p.press(40.0, PAD + 6.0 + 20.0),
            ControlAction::SetWifi(false)
        );
        assert_eq!(
            p.press(40.0, PAD + 6.0 + ROW + 20.0),
            ControlAction::SetBluetooth(true)
        );
        // По названию плитки Bluetooth — список устройств.
        assert_eq!(
            p.press(LEFT_W - 20.0, PAD + 6.0 + ROW + 20.0),
            ControlAction::OpenBluetooth
        );
    }

    #[test]
    fn renders_and_hides_sound_without_it() {
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::with_dirs("none", vec![], vec![]);
        let p = ControlPanel::new(state());
        let pm = p.render(&mut fonts, &mut icons);
        assert_eq!((pm.width(), pm.height()), p.size());
        let mut st = state();
        st.sound = None;
        let small = ControlPanel::new(st);
        assert!(small.size().1 < p.size().1);
    }
}
