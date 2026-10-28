//! Окошко Bluetooth, как в macOS: открывается из Пункта управления щелчком по
//! плитке «Bluetooth». Сверху — переключатель, ниже «Мои устройства»
//! (сопряжённые: щелчок — подключить или отключить) и «Другие устройства»
//! (найденные поиском, пока окошко открыто: щелчок — сопрячь и подключить),
//! внизу «Настройки Bluetooth…». Здесь состояние, мышь и рисование;
//! говорить с Bluetooth — дело службы.

use mimshel_core::input::Key;
use mimshel_dbus::bluetooth::BtState;
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{circle, paint, rounded};
use tiny_skia::{FillRule, Rect, Stroke, Transform};

/// Строка окошка, на которую можно навести или нажать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Power,
    /// Устройство с этим номером в `devices`.
    Device(usize),
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BluetoothAction {
    Nothing,
    Redraw,
    Close,
    Power(bool),
    /// Подключить устройство (путь BlueZ); не сопряжено — сопрячь.
    Connect(String),
    Disconnect(String),
    Settings,
}

pub struct BluetoothPanel {
    pub state: BtState,
    pub language: mimshel_core::i18n::Language,
    pub hover: Option<Row>,
}

const WIDTH: f32 = 300.0;
const PAD: f32 = 5.0;
const TITLE: f32 = 32.0;
const SEPARATOR: f32 = 11.0;
const ROW: f32 = 26.0;
const CAPTION: f32 = 22.0;
const INSET: f32 = 5.0;
const X0: f32 = 16.0;
const X1: f32 = WIDTH - 16.0;
/// Больше устройств в каждом списке не показываем — окошко не уйдёт за экран.
const MAX_ROWS: usize = 10;
/// Переключатель справа в заголовке.
const SWITCH_W: f32 = 36.0;
const SWITCH_H: f32 = 20.0;

const BG: [u8; 4] = [0x28, 0x28, 0x28, 0xf0];
const EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const LINE: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
const HOVER: [u8; 4] = [0x0a, 0x64, 0xd2, 0xff];
const ON: [u8; 4] = [0x0a, 0x84, 0xff, 0xff];
const OFF: [u8; 4] = [0xff, 0xff, 0xff, 0x40];
const KNOB: [u8; 4] = [0xff, 0xff, 0xff, 0xff];

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
const NOTE: Style = Style {
    size: 12.0,
    color: [0xa0, 0xa0, 0xa0, 0xff],
    weight: 400,
};
const ERROR: Style = Style {
    color: [0xff, 0x8a, 0x80, 0xff],
    ..NOTE
};

/// Что где в окошке (по высоте): считается одинаково для мыши и рисования.
struct Layout {
    /// Подписи групп: (верх, текст).
    captions: Vec<(f32, &'static str)>,
    /// Строки устройств: (верх, номер в `devices`).
    rows: Vec<(f32, usize)>,
    /// Строка-пояснение («Поиск устройств…», «Bluetooth выключен»).
    note: Option<(f32, String)>,
    error: Option<f32>,
    settings: f32,
    height: f32,
}

impl BluetoothPanel {
    pub fn new(state: BtState) -> BluetoothPanel {
        BluetoothPanel {
            state,
            language: mimshel_core::i18n::Language::Ru,
            hover: None,
        }
    }

    pub fn set_language(&mut self, lang: mimshel_core::i18n::Language) {
        self.language = lang;
    }

    fn layout(&self) -> Layout {
        let s = &self.state;
        let mut y = PAD + TITLE + SEPARATOR;
        let mut captions = Vec::new();
        let mut rows = Vec::new();
        let mut note = None;
        if s.powered {
            for (paired, caption) in [
                (true, mimshel_core::i18n::t(self.language, "bt.my_devices")),
                (
                    false,
                    mimshel_core::i18n::t(self.language, "bt.other_devices"),
                ),
            ] {
                let list: Vec<usize> = (0..s.devices.len())
                    .filter(|&i| s.devices[i].paired == paired)
                    .take(MAX_ROWS)
                    .collect();
                if paired && list.is_empty() {
                    continue;
                }
                captions.push((y, caption));
                y += CAPTION;
                for i in list {
                    rows.push((y, i));
                    y += ROW;
                }
                if !paired && rows.iter().all(|&(_, i)| s.devices[i].paired) {
                    let text = if s.discovering {
                        mimshel_core::i18n::t(self.language, "bt.searching")
                    } else {
                        mimshel_core::i18n::t(self.language, "bt.no_devices")
                    };
                    note = Some((y, text.to_string()));
                    y += ROW;
                }
            }
        } else {
            note = Some((
                y,
                mimshel_core::i18n::t(self.language, "bt.off").to_string(),
            ));
            y += ROW;
        }
        let error = s.error.as_ref().map(|_| {
            let top = y;
            y += ROW;
            top
        });
        let settings = y + SEPARATOR;
        Layout {
            captions,
            rows,
            note,
            error,
            settings,
            height: settings + ROW + PAD,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (WIDTH as u32, self.layout().height.ceil() as u32)
    }

    pub fn row_at(&self, x: f32, y: f32) -> Option<Row> {
        if !(0.0..WIDTH).contains(&x) {
            return None;
        }
        if (PAD..PAD + TITLE).contains(&y) && x >= X1 - SWITCH_W - 8.0 {
            return Some(Row::Power);
        }
        let l = self.layout();
        if let Some(&(_, i)) = l
            .rows
            .iter()
            .find(|(top, _)| (*top..top + ROW).contains(&y))
        {
            return Some(Row::Device(i));
        }
        (l.settings..l.settings + ROW)
            .contains(&y)
            .then_some(Row::Settings)
    }

    pub fn press(&mut self, x: f32, y: f32) -> BluetoothAction {
        let (_, h) = self.size();
        if !(0.0..WIDTH).contains(&x) || !(0.0..h as f32).contains(&y) {
            return BluetoothAction::Close;
        }
        match self.row_at(x, y) {
            Some(Row::Power) => {
                // Переключатель сразу встаёт в новое положение — не ждать BlueZ.
                self.state.powered = !self.state.powered;
                BluetoothAction::Power(self.state.powered)
            }
            Some(Row::Device(i)) => {
                if self.state.busy.is_some() {
                    return BluetoothAction::Nothing;
                }
                let Some(d) = self.state.devices.get(i) else {
                    return BluetoothAction::Nothing;
                };
                self.state.busy = Some(d.path.clone());
                self.state.error = None;
                if d.connected {
                    BluetoothAction::Disconnect(d.path.clone())
                } else {
                    BluetoothAction::Connect(d.path.clone())
                }
            }
            Some(Row::Settings) => BluetoothAction::Settings,
            None => BluetoothAction::Nothing,
        }
    }

    pub fn motion(&mut self, x: f32, y: f32) -> BluetoothAction {
        let hover = self.row_at(x, y).filter(|r| *r != Row::Power);
        if hover == self.hover {
            return BluetoothAction::Nothing;
        }
        self.hover = hover;
        BluetoothAction::Redraw
    }

    pub fn key(&mut self, key: Key) -> BluetoothAction {
        match key {
            Key::Escape => BluetoothAction::Close,
            _ => BluetoothAction::Nothing,
        }
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
        let line_y = |top: f32, height: f32, style: &Style| {
            top + ((height - style.line_height()) / 2.0).round()
        };
        let separator = |pm: &mut Canvas, y: f32| {
            if let Some(r) = Rect::from_xywh(INSET + 5.0, y.round(), wf - 2.0 * INSET - 10.0, 1.0) {
                pm.fill_rect(r, &paint(LINE), Transform::identity(), None);
            }
        };
        let s = &self.state;

        // Заголовок и переключатель.
        fonts.draw(
            &mut pm,
            "Bluetooth",
            &BOLD,
            X0,
            line_y(PAD, TITLE, &BOLD),
            None,
        );
        let (sx, sy) = (X1 - SWITCH_W, PAD + (TITLE - SWITCH_H) / 2.0);
        fill(
            &mut pm,
            rounded(sx, sy, SWITCH_W, SWITCH_H, SWITCH_H / 2.0),
            if s.powered { ON } else { OFF },
        );
        let kx = if s.powered {
            sx + SWITCH_W - SWITCH_H / 2.0
        } else {
            sx + SWITCH_H / 2.0
        };
        fill(
            &mut pm,
            circle(kx, sy + SWITCH_H / 2.0, SWITCH_H / 2.0 - 2.0),
            KNOB,
        );
        separator(&mut pm, PAD + TITLE + SEPARATOR / 2.0);

        let l = self.layout();
        for &(top, text) in &l.captions {
            fonts.draw(&mut pm, text, &DIM, X0, line_y(top, CAPTION, &DIM), None);
        }
        for &(top, i) in &l.rows {
            let d = &s.devices[i];
            if self.hover == Some(Row::Device(i)) {
                fill(
                    &mut pm,
                    rounded(INSET, top, wf - 2.0 * INSET, ROW, 4.0),
                    HOVER,
                );
            }
            // Кружок слева: синий — подключено.
            fill(
                &mut pm,
                circle(X0 + 5.0, top + ROW / 2.0, 5.0),
                if d.connected { ON } else { OFF },
            );
            let busy = s.busy.as_deref() == Some(d.path.as_str());
            let status = match (busy, d.connected) {
                (true, true) => mimshel_core::i18n::t(self.language, "bt.disconnecting"),
                (true, false) if d.paired => mimshel_core::i18n::t(self.language, "bt.connecting"),
                (true, false) => mimshel_core::i18n::t(self.language, "bt.pairing"),
                (false, true) => mimshel_core::i18n::t(self.language, "bt.connected"),
                (false, false) => "",
            };
            let sw = fonts.measure(status, &NOTE);
            fonts.draw(
                &mut pm,
                status,
                &NOTE,
                X1 - sw,
                line_y(top, ROW, &NOTE),
                None,
            );
            fonts.draw(
                &mut pm,
                &d.name,
                &TEXT,
                X0 + 18.0,
                line_y(top, ROW, &TEXT),
                Some(X1 - sw - X0 - 26.0),
            );
        }
        if let Some((top, text)) = &l.note {
            fonts.draw(&mut pm, text, &NOTE, X0, line_y(*top, ROW, &NOTE), None);
        }
        if let (Some(top), Some(err)) = (l.error, &s.error) {
            fonts.draw(
                &mut pm,
                err,
                &ERROR,
                X0,
                line_y(top, ROW, &ERROR),
                Some(X1 - X0),
            );
        }
        separator(&mut pm, l.settings - SEPARATOR / 2.0);
        if self.hover == Some(Row::Settings) {
            fill(
                &mut pm,
                rounded(INSET, l.settings, wf - 2.0 * INSET, ROW, 4.0),
                HOVER,
            );
        }
        fonts.draw(
            &mut pm,
            mimshel_core::i18n::t(self.language, "bt.settings"),
            &TEXT,
            X0,
            line_y(l.settings, ROW, &TEXT),
            None,
        );
        pm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mimshel_dbus::bluetooth::BtDevice;

    fn dev(name: &str, paired: bool, connected: bool) -> BtDevice {
        BtDevice {
            path: format!("/org/bluez/hci0/dev_{name}"),
            name: name.into(),
            icon: String::new(),
            paired,
            connected,
        }
    }

    fn panel() -> BluetoothPanel {
        BluetoothPanel::new(BtState {
            powered: true,
            discovering: true,
            devices: vec![
                dev("Наушники", true, true),
                dev("Мышь", true, false),
                dev("Телефон", false, false),
            ],
            busy: None,
            error: None,
        })
    }

    #[test]
    fn rows_and_actions() {
        let mut p = panel();
        let l = p.layout();
        assert_eq!(l.captions.len(), 2, "мои и другие устройства");
        assert_eq!(l.rows.len(), 3);
        assert!(l.note.is_none(), "найденные есть — «Поиск…» не нужен");
        let y = |i: usize| l.rows.iter().find(|r| r.1 == i).unwrap().0 + 5.0;
        // Подключённые наушники — отключить, телефон — сопрячь и подключить.
        assert_eq!(
            p.press(100.0, y(0)),
            BluetoothAction::Disconnect("/org/bluez/hci0/dev_Наушники".into())
        );
        assert_eq!(
            p.press(100.0, y(2)),
            BluetoothAction::Nothing,
            "пока одно устройство занято, другое не трогаем"
        );
        p.state.busy = None;
        assert_eq!(
            p.press(100.0, y(2)),
            BluetoothAction::Connect("/org/bluez/hci0/dev_Телефон".into())
        );
        assert_eq!(p.press(100.0, l.settings + 5.0), BluetoothAction::Settings);
        assert_eq!(p.press(-1.0, 10.0), BluetoothAction::Close, "мимо окошка");
        assert_eq!(
            p.press(X1 - 5.0, PAD + TITLE / 2.0),
            BluetoothAction::Power(false)
        );
        // Выключен — вместо списков пояснение.
        let l = p.layout();
        assert!(l.rows.is_empty());
        assert_eq!(l.note.unwrap().1, "Bluetooth выключен");
    }

    #[test]
    fn searching_note_and_render() {
        let mut p = panel();
        p.state.devices.retain(|d| d.paired);
        assert_eq!(p.layout().note.unwrap().1, "Поиск устройств…");
        p.state.error = Some("Не удалось подключить: Page Timeout".into());
        let mut fonts = Fonts::new("Inter");
        let pm = p.render(&mut fonts);
        assert_eq!((pm.width(), pm.height()), p.size());
    }
}
