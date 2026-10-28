//! Пункт управления, как в macOS Sonoma: плитки под значком в строке меню.
//! Слева — «Сеть» и «Bluetooth» (кружок синий, когда включено), справа —
//! «Не беспокоить», ниже — «Дисплей» (brightness) and «Звук» sliders. Чего нет
//! в системе (Wi-Fi, адаптера Bluetooth), то показывается как недоступное;
//! no backlight (a desktop) or no sound — no slider. Здесь состояние,
//! мышь и рисование; включать и выключать — дело службы.

use mimshel_core::input::Key;
use mimshel_icons::Icons;
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{circle, paint, rounded};
use tiny_skia::{FillRule, FilterQuality, Pixmap, PixmapPaint, Stroke, Transform};

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
    /// Screen brightness, percent; `None` — no backlight.
    pub display: Option<u32>,
    pub sound: Option<Sound>,
}

/// Что на панели можно нажать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Network,
    Bluetooth,
    Dnd,
    /// The brightness slider.
    Display,
    /// The volume slider.
    Volume,
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
    /// Screen brightness, percent.
    SetBrightness(u32),
    NetworkSettings,
    /// Открыть список устройств Bluetooth (щелчок по названию плитки).
    OpenBluetooth,
    /// Открыть окошко сети (щелчок по названию плитки «Сеть»).
    OpenNetwork,
}

/// How a menu bar icon moved into the Control Center looks there.
#[derive(Debug, Clone)]
pub enum HiddenLook {
    /// A tray icon: the app's own picture.
    Picture(Pixmap),
    /// An icon from the theme (volume, battery, bell).
    Icon(String),
    /// The keyboard layout label.
    Text(String),
}

pub struct ControlPanel {
    pub state: ControlState,
    pub language: mimshel_core::i18n::Language,
    pub hover: Option<Tile>,
    /// The slider being dragged.
    dragging: Option<Tile>,
    /// Menu bar icons moved here, in bar order: a row at the bottom.
    pub hidden: Vec<HiddenLook>,
    /// The hidden icon under the pointer.
    pub hidden_hover: Option<usize>,
}

const WIDTH: f32 = 300.0;
const PAD: f32 = 10.0;
const GAP: f32 = 8.0;
const LEFT_W: f32 = 168.0;
const ROW: f32 = 50.0;
const TOP_H: f32 = 2.0 * ROW + 12.0;
/// Height of a slider tile (brightness, volume).
const SLIDER_H: f32 = 66.0;
const R: f32 = 15.0;
const TILE_R: f32 = 14.0;
/// The «Hidden icons» tile: title line and one cell per icon.
const HIDDEN_TITLE: f32 = 24.0;
const CELL: f32 = 34.0;

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
            language: mimshel_core::i18n::Language::Ru,
            hover: None,
            dragging: None,
            hidden: Vec::new(),
            hidden_hover: None,
        }
    }

    pub fn set_language(&mut self, lang: mimshel_core::i18n::Language) {
        self.language = lang;
    }

    pub fn size(&self) -> (u32, u32) {
        let mut h = PAD + TOP_H + self.sliders().len() as f32 * (GAP + SLIDER_H) + PAD;
        if !self.hidden.is_empty() {
            h += GAP + self.hidden_height();
        }
        (WIDTH as u32, h as u32)
    }

    /// Hidden icons per row.
    fn per_row() -> usize {
        ((WIDTH - 2.0 * PAD - 16.0) / CELL).floor() as usize
    }

    fn hidden_height(&self) -> f32 {
        let rows = self.hidden.len().div_ceil(Self::per_row());
        8.0 + HIDDEN_TITLE + rows as f32 * CELL + 6.0
    }

    /// Top of the «Hidden icons» tile.
    fn hidden_top(&self) -> f32 {
        PAD + TOP_H + self.sliders().len() as f32 * (GAP + SLIDER_H) + GAP
    }

    /// Centre of hidden icon `i`, in panel coordinates.
    pub fn hidden_center(&self, i: usize) -> (f32, f32) {
        let (row, col) = (i / Self::per_row(), i % Self::per_row());
        (
            PAD + 8.0 + col as f32 * CELL + CELL / 2.0,
            self.hidden_top() + 8.0 + HIDDEN_TITLE + row as f32 * CELL + CELL / 2.0,
        )
    }

    /// The hidden icon under `(x, y)`.
    pub fn hidden_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.hidden.len()).find(|&i| {
            let (cx, cy) = self.hidden_center(i);
            (x - cx).abs() < CELL / 2.0 && (y - cy).abs() < CELL / 2.0
        })
    }

    /// The sliders shown, top to bottom, with the top of each tile: display
    /// first, then sound (as in macOS).
    fn sliders(&self) -> Vec<(Tile, f32)> {
        let mut out = Vec::new();
        let mut top = PAD + TOP_H + GAP;
        for (tile, shown) in [
            (Tile::Display, self.state.display.is_some()),
            (Tile::Volume, self.state.sound.is_some()),
        ] {
            if shown {
                out.push((tile, top));
                top += SLIDER_H + GAP;
            }
        }
        out
    }

    /// A slider's track: left and right ends and the middle height.
    fn track(top: f32) -> (f32, f32, f32) {
        (PAD + 14.0, WIDTH - PAD - 14.0, top + SLIDER_H - 20.0)
    }

    fn track_of(&self, tile: Tile) -> Option<(f32, f32, f32)> {
        self.sliders()
            .into_iter()
            .find(|(t, _)| *t == tile)
            .map(|(_, top)| Self::track(top))
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
        self.sliders()
            .into_iter()
            .find(|(_, top)| inside(PAD, *top, WIDTH - 2.0 * PAD, SLIDER_H))
            .map(|(t, _)| t)
    }

    /// Percent under `x` on a slider's track.
    fn percent_at(x: f32, (x0, x1, _): (f32, f32, f32)) -> u32 {
        ((x - x0) / (x1 - x0) * 100.0).round().clamp(0.0, 100.0) as u32
    }

    /// Move slider `tile` to `x`.
    fn slide(&mut self, tile: Tile, x: f32) -> ControlAction {
        let Some(track) = self.track_of(tile) else {
            return ControlAction::Nothing;
        };
        let v = Self::percent_at(x, track);
        match tile {
            Tile::Display => self.set_brightness(v),
            _ => self.set_volume(v),
        }
    }

    fn set_brightness(&mut self, v: u32) -> ControlAction {
        match &mut self.state.display {
            Some(d) if *d != v => {
                *d = v;
                ControlAction::SetBrightness(v)
            }
            _ => ControlAction::Nothing,
        }
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
        self.dragging.is_some()
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
            // Как у Bluetooth: кружок — включить или выключить Wi-Fi, название
            // (и плитка без Wi-Fi) — окошко сети.
            Some(Tile::Network) => match s.network.wifi {
                Some(on) if x < Self::circle_right() => {
                    s.network.wifi = Some(!on);
                    ControlAction::SetWifi(!on)
                }
                _ => ControlAction::OpenNetwork,
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
            Some(t @ (Tile::Display | Tile::Volume)) => match self.track_of(t) {
                Some((x0, x1, cy))
                    if (cy - 14.0..cy + 14.0).contains(&y)
                        && (x0 - 10.0..x1 + 10.0).contains(&x) =>
                {
                    self.dragging = Some(t);
                    self.slide(t, x)
                }
                _ => ControlAction::Nothing,
            },
            None => ControlAction::Nothing,
        }
    }

    pub fn motion(&mut self, x: f32, y: f32) -> ControlAction {
        if let Some(t) = self.dragging {
            return self.slide(t, x);
        }
        let hover = self
            .tile_at(x, y)
            .filter(|t| !matches!(t, Tile::Display | Tile::Volume));
        let hidden = self.hidden_at(x, y);
        if hover == self.hover && hidden == self.hidden_hover {
            return ControlAction::Nothing;
        }
        self.hover = hover;
        self.hidden_hover = hidden;
        ControlAction::Redraw
    }

    pub fn release(&mut self) {
        self.dragging = None;
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
            (
                "network-wired-symbolic",
                mimshel_core::i18n::t(self.language, "control.network"),
            )
        };
        let net_status = match (&s.network.wifi, s.network.connection.as_str()) {
            (Some(false), _) => mimshel_core::i18n::t(self.language, "control.off").to_string(),
            (_, "") => mimshel_core::i18n::t(self.language, "control.not_connected").to_string(),
            (_, c) => c.to_string(),
        };
        let bt_status = match s.bluetooth {
            Some(true) => mimshel_core::i18n::t(self.language, "control.on"),
            Some(false) => mimshel_core::i18n::t(self.language, "control.off"),
            None => mimshel_core::i18n::t(self.language, "control.unavailable"),
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
        for (text, style, y) in [
            (
                mimshel_core::i18n::t(self.language, "control.dnd1"),
                &NAME,
                cy + 24.0,
            ),
            (
                mimshel_core::i18n::t(self.language, "control.dnd2"),
                &NAME,
                cy + 40.0,
            ),
        ] {
            let tw = fonts.measure(text, style);
            fonts.draw(&mut pm, text, style, cx - tw / 2.0, y, None);
        }

        // Sliders: display, sound.
        for (t, top) in self.sliders() {
            let (name, v, ic) = match (t, &s.display, &s.sound) {
                (Tile::Display, Some(b), _) => (
                    mimshel_core::i18n::t(self.language, "control.display"),
                    *b,
                    "display-brightness-symbolic",
                ),
                (_, _, Some(sound)) => (
                    mimshel_core::i18n::t(self.language, "control.sound"),
                    if sound.muted { 0 } else { sound.volume },
                    sound.icon(),
                ),
                _ => continue,
            };
            tile(&mut pm, PAD, top, WIDTH - 2.0 * PAD, SLIDER_H, false);
            fonts.draw(&mut pm, name, &NAME, PAD + 14.0, top + 10.0, None);
            let (x0, x1, cy) = Self::track(top);
            fill(&mut pm, rounded(x0, cy - 10.0, x1 - x0, 20.0, 10.0), TRACK);
            let kx = x0 + 20.0 + (x1 - x0 - 20.0) * v as f32 / 100.0;
            fill(&mut pm, rounded(x0, cy - 10.0, kx - x0, 20.0, 10.0), FILL);
            fill(
                &mut pm,
                circle(kx - 10.0, cy, 9.0),
                [0xff, 0xff, 0xff, 0xff],
            );
            icon(&mut pm, ic, x0 + 12.0, cy);
        }

        // Menu bar icons moved here.
        if !self.hidden.is_empty() {
            let top = self.hidden_top();
            tile(
                &mut pm,
                PAD,
                top,
                WIDTH - 2.0 * PAD,
                self.hidden_height(),
                false,
            );
            fonts.draw(
                &mut pm,
                mimshel_core::i18n::t(self.language, "control.hidden_icons"),
                &STATUS,
                PAD + 14.0,
                top + 8.0,
                None,
            );
            for (i, look) in self.hidden.iter().enumerate() {
                let (cx, cy) = self.hidden_center(i);
                if self.hidden_hover == Some(i) {
                    fill(&mut pm, circle(cx, cy, CELL / 2.0 - 2.0), TILE_HOVER);
                }
                draw_look(&mut pm, look, (cx, cy), fonts, icons);
            }
        }
        pm
    }
}

/// A hidden icon, 16 points, centred at `(cx, cy)`.
fn draw_look(
    pm: &mut Canvas,
    look: &HiddenLook,
    (cx, cy): (f32, f32),
    fonts: &mut Fonts,
    icons: &mut Icons,
) {
    let q = PixmapPaint {
        quality: FilterQuality::Bicubic,
        ..PixmapPaint::default()
    };
    let picture = |pm: &mut Canvas, img: &Pixmap| {
        let k = 16.0 / img.width().max(img.height()).max(1) as f32;
        let t = Transform::from_scale(k, k).post_translate(
            cx - img.width() as f32 * k / 2.0,
            cy - img.height() as f32 * k / 2.0,
        );
        pm.draw_pixmap(0, 0, img.as_ref(), &q, t, None);
    };
    match look {
        HiddenLook::Picture(img) => picture(pm, img),
        HiddenLook::Icon(name) => {
            if let Some(img) = icons.get(name, 16) {
                picture(pm, img);
            }
        }
        HiddenLook::Text(text) => {
            let tw = fonts.measure(text, &NAME);
            fonts.draw(pm, text, &NAME, cx - tw / 2.0, cy - 9.0, None);
        }
    }
}

/// What follows the pointer while a menu bar icon is dragged: the icon on
/// a small dark rounded square.
pub fn drag_image(look: &HiddenLook, fonts: &mut Fonts, icons: &mut Icons) -> Canvas {
    let size = DRAG_SIZE;
    let mut pm = Canvas::new(size, size).expect("размер не нулевой");
    let s = size as f32;
    if let Some(p) = rounded(0.0, 0.0, s, s, 8.0) {
        pm.fill_path(
            &p,
            &paint(BG),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    draw_look(&mut pm, look, (s / 2.0, s / 2.0), fonts, icons);
    pm
}

/// Side of [`drag_image`], points.
pub const DRAG_SIZE: u32 = 32;

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
            display: None,
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
    fn hidden_icons_row() {
        let mut panel = ControlPanel::new(state());
        let (w, h) = panel.size();
        panel.hidden = (0..10).map(|i| HiddenLook::Text(format!("{i}"))).collect();
        let (_, taller) = panel.size();
        assert!(taller > h + 2 * CELL as u32, "two rows of icons");
        for i in [0, 7, 8, 9] {
            let (x, y) = panel.hidden_center(i);
            assert_eq!(panel.hidden_at(x, y), Some(i));
            assert!(x > 0.0 && x < w as f32 && y < taller as f32);
        }
        assert_eq!(panel.hidden_at(5.0, 5.0), None);
        // Pressing a hidden icon is the menu bar's business, not a tile's.
        let (x, y) = panel.hidden_center(0);
        assert_eq!(panel.press(x, y), ControlAction::Nothing);
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::with_dirs("none", vec![], vec![]);
        let pm = panel.render(&mut fonts, &mut icons);
        assert_eq!((pm.width(), pm.height()), (w, taller));
    }

    #[test]
    fn tiles_and_actions() {
        let mut p = ControlPanel::new(state());
        // Проводная сеть (Wi-Fi нет) — щелчок открывает окошко сети, Bluetooth
        // без адаптера — ничего.
        assert_eq!(p.press(40.0, PAD + 6.0 + 20.0), ControlAction::OpenNetwork);
        assert_eq!(
            p.press(40.0, PAD + 6.0 + ROW + 20.0),
            ControlAction::Nothing
        );
        assert_eq!(
            p.press(WIDTH - 40.0, PAD + 40.0),
            ControlAction::SetDnd(true)
        );
        assert!(p.state.dnd);
        let (x0, x1, cy) = p.track_of(Tile::Volume).unwrap();
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
            p.press(LEFT_W - 20.0, PAD + 6.0 + 20.0),
            ControlAction::OpenNetwork,
            "по названию плитки — окошко сети"
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

    #[test]
    fn brightness_slider_above_sound() {
        let mut st = state();
        st.display = Some(80);
        let mut p = ControlPanel::new(st);
        let without = ControlPanel::new(state());
        assert_eq!(p.size().1 as f32, without.size().1 as f32 + GAP + SLIDER_H);
        let (bx0, bx1, by) = p.track_of(Tile::Display).unwrap();
        let (_, _, vy) = p.track_of(Tile::Volume).unwrap();
        assert!(by < vy, "display above sound");
        assert_eq!(p.tile_at(bx0 + 5.0, by), Some(Tile::Display));
        // Press and drag: brightness follows, sound does not move.
        assert_eq!(p.press(bx1, by), ControlAction::SetBrightness(100));
        assert!(p.dragging());
        let mid = bx0 + (bx1 - bx0) / 2.0;
        assert_eq!(p.motion(mid, vy), ControlAction::SetBrightness(50));
        assert_eq!(p.motion(mid, vy), ControlAction::Nothing, "same value");
        p.release();
        assert_eq!(p.state.display, Some(50));
        assert_eq!(p.state.sound.as_ref().unwrap().volume, 40);
        // No backlight and no sound: no sliders at all.
        let mut st = state();
        st.sound = None;
        let bare = ControlPanel::new(st);
        assert_eq!(bare.size().1 as f32, PAD + TOP_H + PAD);
        // Renders at the computed size with both sliders.
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::with_dirs("none", vec![], vec![]);
        let pm = p.render(&mut fonts, &mut icons);
        assert_eq!((pm.width(), pm.height()), p.size());
    }
}
