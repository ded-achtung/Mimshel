//! Окошко сети, как меню Wi-Fi в macOS: открывается щелчком по значку сети в
//! строке меню и по названию плитки «Сеть» в Пункте управления. Сверху —
//! переключатель Wi-Fi, ниже — проводное подключение (если есть) и сети Wi-Fi
//! с силой сигнала и замком у защищённых: щелчок — подключиться (новой сети
//! с паролем пароль спросит окно nm-applet), внизу «Настройки сети…».
//! Здесь — состояние, разбор вывода `nmcli`, мышь и рисование; говорить с
//! NetworkManager — дело службы.

use mimshel_core::input::Key;
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{circle, paint, rounded};
use tiny_skia::{FillRule, PathBuilder, Rect, Stroke, Transform};

/// Сеть Wi-Fi (все её точки доступа вместе).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiNet {
    pub ssid: String,
    /// Сила сигнала лучшей точки, 0–100.
    pub signal: u8,
    /// С паролем.
    pub secure: bool,
    /// Подключены к ней сейчас.
    pub active: bool,
    /// Есть сохранённое подключение — пароль уже известен.
    pub known: bool,
}

/// Сеть целиком.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetState {
    /// Wi-Fi: `None` — нет оборудования, иначе включён ли.
    pub wifi: Option<bool>,
    /// Проводное подключение: есть ли разъём (`Some`), подключено ли и как называется.
    pub wired: Option<(bool, String)>,
    /// Сети Wi-Fi: подключённая, известные, остальные — по силе сигнала.
    pub networks: Vec<WifiNet>,
    /// Ищем сети.
    pub scanning: bool,
    /// Сеть, к которой сейчас подключаемся.
    pub busy: Option<String>,
    pub error: Option<String>,
}

/// Поля строки `nmcli -t` (двоеточие внутри значения — `\:`).
pub fn split_terse(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let (Some(next), Some(s)) = (chars.next(), out.last_mut()) {
                    s.push(next);
                }
            }
            ':' => out.push(String::new()),
            c => {
                if let Some(s) = out.last_mut() {
                    s.push(c);
                }
            }
        }
    }
    out
}

/// Сети из `nmcli -t -f IN-USE,SIGNAL,SECURITY,SSID device wifi list`:
/// точки одной сети (разные диапазоны, ретрансляторы) — одной строкой,
/// скрытые сети без имени не показываем. `known` — имена сохранённых
/// подключений Wi-Fi.
pub fn parse_wifi_list(text: &str, known: &[String]) -> Vec<WifiNet> {
    let mut out: Vec<WifiNet> = Vec::new();
    for line in text.lines() {
        let f = split_terse(line);
        let [in_use, signal, security, ssid] = f.as_slice() else {
            continue;
        };
        if ssid.is_empty() {
            continue;
        }
        let signal = signal.parse::<u8>().unwrap_or(0).min(100);
        let active = in_use == "*";
        let secure = !security.is_empty() && security != "--";
        match out.iter_mut().find(|n| n.ssid == *ssid) {
            Some(n) => {
                n.signal = n.signal.max(signal);
                n.active |= active;
            }
            None => out.push(WifiNet {
                ssid: ssid.clone(),
                signal,
                secure,
                active,
                known: known.contains(ssid),
            }),
        }
    }
    sort_networks(&mut out);
    out
}

/// Подключённая — первой, потом известные, внутри — по силе сигнала.
fn sort_networks(list: &mut [WifiNet]) {
    list.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.known.cmp(&a.known))
            .then(b.signal.cmp(&a.signal))
            .then_with(|| a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase()))
    });
}

/// Имена сохранённых подключений Wi-Fi из `nmcli -t -f NAME,TYPE connection show`.
pub fn parse_known(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let f = split_terse(l);
            (f.get(1).map(String::as_str) == Some("802-11-wireless")).then(|| f[0].clone())
        })
        .collect()
}

/// Проводное подключение из `nmcli -t -f TYPE,STATE,CONNECTION device`: есть ли
/// разъём, подключено ли, название.
pub fn parse_wired(text: &str) -> Option<(bool, String)> {
    let mut found = None;
    for l in text.lines() {
        let f = split_terse(l);
        if f.first().map(String::as_str) != Some("ethernet") {
            continue;
        }
        let connected = f.get(1).map(String::as_str) == Some("connected");
        let name = f.get(2).cloned().unwrap_or_default();
        if connected {
            return Some((true, name));
        }
        found.get_or_insert((false, String::new()));
    }
    found
}

/// Строка окошка, на которую можно навести или нажать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRow {
    Power,
    /// Сеть с этим номером в `networks`.
    Net(usize),
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkAction {
    Nothing,
    Redraw,
    Close,
    Wifi(bool),
    /// Подключиться к сети (имя, есть ли сохранённое подключение).
    Connect(String, bool),
    Settings,
}

pub struct NetworkPanel {
    pub state: NetState,
    pub hover: Option<NetRow>,
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
/// Больше сетей не показываем — окошко не уйдёт за экран.
const MAX_ROWS: usize = 12;
const SWITCH_W: f32 = 36.0;
const SWITCH_H: f32 = 20.0;

const BG: [u8; 4] = [0x28, 0x28, 0x28, 0xf0];
const EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x30];
const LINE: [u8; 4] = [0xff, 0xff, 0xff, 0x26];
const HOVER: [u8; 4] = [0x0a, 0x64, 0xd2, 0xff];
const ON: [u8; 4] = [0x0a, 0x84, 0xff, 0xff];
const OFF: [u8; 4] = [0xff, 0xff, 0xff, 0x40];
const BAR_ON: [u8; 4] = [0xf2, 0xf2, 0xf2, 0xff];
const BAR_OFF: [u8; 4] = [0xff, 0xff, 0xff, 0x38];
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

/// Что где в окошке по высоте — одинаково для мыши и рисования.
struct Layout {
    captions: Vec<(f32, &'static str)>,
    /// Проводное подключение: верх строки.
    wired: Option<f32>,
    rows: Vec<(f32, usize)>,
    note: Option<(f32, &'static str)>,
    error: Option<f32>,
    settings: f32,
    height: f32,
}

impl NetworkPanel {
    pub fn new(state: NetState) -> NetworkPanel {
        NetworkPanel { state, hover: None }
    }

    fn layout(&self) -> Layout {
        let s = &self.state;
        let mut y = PAD + TITLE + SEPARATOR;
        let mut captions = Vec::new();
        let mut wired = None;
        let mut rows = Vec::new();
        let mut note = None;
        if s.wired.is_some() {
            captions.push((y, "Проводная сеть"));
            y += CAPTION;
            wired = Some(y);
            y += ROW;
        }
        match s.wifi {
            Some(true) => {
                captions.push((y, "Сети Wi-Fi"));
                y += CAPTION;
                for i in 0..s.networks.len().min(MAX_ROWS) {
                    rows.push((y, i));
                    y += ROW;
                }
                if rows.is_empty() {
                    note = Some((
                        y,
                        if s.scanning {
                            "Поиск сетей…"
                        } else {
                            "Сетей рядом не найдено"
                        },
                    ));
                    y += ROW;
                }
            }
            Some(false) => {
                note = Some((y, "Wi-Fi выключен"));
                y += ROW;
            }
            None if s.wired.is_none() => {
                note = Some((y, "Нет сетевых устройств"));
                y += ROW;
            }
            None => {}
        }
        let error = s.error.as_ref().map(|_| {
            let top = y;
            y += ROW;
            top
        });
        let settings = y + SEPARATOR;
        Layout {
            captions,
            wired,
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

    pub fn row_at(&self, x: f32, y: f32) -> Option<NetRow> {
        if !(0.0..WIDTH).contains(&x) {
            return None;
        }
        if self.state.wifi.is_some() && (PAD..PAD + TITLE).contains(&y) && x >= X1 - SWITCH_W - 8.0
        {
            return Some(NetRow::Power);
        }
        let l = self.layout();
        if let Some(&(_, i)) = l
            .rows
            .iter()
            .find(|(top, _)| (*top..top + ROW).contains(&y))
        {
            return Some(NetRow::Net(i));
        }
        (l.settings..l.settings + ROW)
            .contains(&y)
            .then_some(NetRow::Settings)
    }

    pub fn press(&mut self, x: f32, y: f32) -> NetworkAction {
        let (_, h) = self.size();
        if !(0.0..WIDTH).contains(&x) || !(0.0..h as f32).contains(&y) {
            return NetworkAction::Close;
        }
        match self.row_at(x, y) {
            Some(NetRow::Power) => {
                let on = !self.state.wifi.unwrap_or(false);
                self.state.wifi = Some(on);
                if !on {
                    self.state.networks.clear();
                }
                NetworkAction::Wifi(on)
            }
            Some(NetRow::Net(i)) => {
                let Some(n) = self.state.networks.get(i) else {
                    return NetworkAction::Nothing;
                };
                // К подключённой — ничего (отключать щелчком опасно: можно
                // остаться без сети); пока подключаемся к одной — ждём.
                if n.active || self.state.busy.is_some() {
                    return NetworkAction::Nothing;
                }
                self.state.busy = Some(n.ssid.clone());
                self.state.error = None;
                NetworkAction::Connect(n.ssid.clone(), n.known)
            }
            Some(NetRow::Settings) => NetworkAction::Settings,
            None => NetworkAction::Nothing,
        }
    }

    pub fn motion(&mut self, x: f32, y: f32) -> NetworkAction {
        let hover = self.row_at(x, y).filter(|r| *r != NetRow::Power);
        if hover == self.hover {
            return NetworkAction::Nothing;
        }
        self.hover = hover;
        NetworkAction::Redraw
    }

    pub fn key(&mut self, key: Key) -> NetworkAction {
        match key {
            Key::Escape => NetworkAction::Close,
            _ => NetworkAction::Nothing,
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

        // Заголовок и переключатель Wi-Fi.
        let title = if s.wifi.is_some() {
            "Wi-Fi"
        } else {
            "Сеть"
        };
        fonts.draw(&mut pm, title, &BOLD, X0, line_y(PAD, TITLE, &BOLD), None);
        if let Some(on) = s.wifi {
            let (sx, sy) = (X1 - SWITCH_W, PAD + (TITLE - SWITCH_H) / 2.0);
            fill(
                &mut pm,
                rounded(sx, sy, SWITCH_W, SWITCH_H, SWITCH_H / 2.0),
                if on { ON } else { OFF },
            );
            let kx = if on {
                sx + SWITCH_W - SWITCH_H / 2.0
            } else {
                sx + SWITCH_H / 2.0
            };
            fill(
                &mut pm,
                circle(kx, sy + SWITCH_H / 2.0, SWITCH_H / 2.0 - 2.0),
                KNOB,
            );
        }
        separator(&mut pm, PAD + TITLE + SEPARATOR / 2.0);

        let l = self.layout();
        for &(top, text) in &l.captions {
            fonts.draw(&mut pm, text, &DIM, X0, line_y(top, CAPTION, &DIM), None);
        }
        // Кружок слева: синий — подключено.
        let dot = |pm: &mut Canvas, top: f32, on: bool| {
            fill(
                pm,
                circle(X0 + 5.0, top + ROW / 2.0, 5.0),
                if on { ON } else { OFF },
            );
        };
        if let (Some(top), Some((connected, name))) = (l.wired, &s.wired) {
            dot(&mut pm, top, *connected);
            let label = if name.is_empty() {
                "Кабель не подключён"
            } else {
                name.as_str()
            };
            let status = if *connected {
                "Подключено"
            } else {
                ""
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
                label,
                &TEXT,
                X0 + 18.0,
                line_y(top, ROW, &TEXT),
                Some(X1 - sw - X0 - 26.0),
            );
        }
        for &(top, i) in &l.rows {
            let n = &s.networks[i];
            if self.hover == Some(NetRow::Net(i)) && !n.active {
                fill(
                    &mut pm,
                    rounded(INSET, top, wf - 2.0 * INSET, ROW, 4.0),
                    HOVER,
                );
            }
            dot(&mut pm, top, n.active);
            // Справа: сила сигнала (четыре столбика) и замок у защищённых.
            let bars_x = X1 - 18.0;
            signal_bars(&mut pm, bars_x, top + ROW / 2.0 + 6.0, n.signal);
            let mut right = bars_x - 6.0;
            if n.secure {
                lock(&mut pm, right - 9.0, top + ROW / 2.0);
                right -= 16.0;
            }
            let busy = s.busy.as_deref() == Some(n.ssid.as_str());
            let status = if busy {
                "Подключение…"
            } else {
                ""
            };
            let sw = fonts.measure(status, &NOTE);
            fonts.draw(
                &mut pm,
                status,
                &NOTE,
                right - sw,
                line_y(top, ROW, &NOTE),
                None,
            );
            let style = if n.active { &BOLD } else { &TEXT };
            fonts.draw(
                &mut pm,
                &n.ssid,
                style,
                X0 + 18.0,
                line_y(top, ROW, style),
                Some(right - sw - X0 - 26.0),
            );
        }
        if let Some((top, text)) = l.note {
            fonts.draw(&mut pm, text, &NOTE, X0, line_y(top, ROW, &NOTE), None);
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
        if self.hover == Some(NetRow::Settings) {
            fill(
                &mut pm,
                rounded(INSET, l.settings, wf - 2.0 * INSET, ROW, 4.0),
                HOVER,
            );
        }
        fonts.draw(
            &mut pm,
            "Настройки сети…",
            &TEXT,
            X0,
            line_y(l.settings, ROW, &TEXT),
            None,
        );
        pm
    }
}

/// Сила сигнала — четыре столбика растущей высоты, левый нижний угол `(x, bottom)`.
fn signal_bars(pm: &mut Canvas, x: f32, bottom: f32, signal: u8) {
    let lit = match signal {
        0..=19 => 1,
        20..=44 => 2,
        45..=69 => 3,
        _ => 4,
    };
    for k in 0..4 {
        let h = 3.0 + 3.0 * k as f32;
        if let Some(r) = rounded(x + k as f32 * 4.5, bottom - h, 3.0, h, 1.0) {
            pm.fill_path(
                &r,
                &paint(if k < lit { BAR_ON } else { BAR_OFF }),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// Замок: дужка и корпус, середина по высоте — `cy`.
fn lock(pm: &mut Canvas, x: f32, cy: f32) {
    if let Some(r) = rounded(x, cy - 1.0, 9.0, 7.0, 1.5) {
        pm.fill_path(
            &r,
            &paint(BAR_ON),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let mut pb = PathBuilder::new();
    pb.move_to(x + 2.0, cy - 1.0);
    pb.line_to(x + 2.0, cy - 3.5);
    pb.cubic_to(x + 2.0, cy - 7.0, x + 7.0, cy - 7.0, x + 7.0, cy - 3.5);
    pb.line_to(x + 7.0, cy - 1.0);
    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width: 1.5,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(BAR_ON), &stroke, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terse_fields_with_escaped_colons() {
        assert_eq!(
            split_terse(r"*:85:WPA2:Дом\:5G"),
            ["*", "85", "WPA2", "Дом:5G"]
        );
        assert_eq!(split_terse(":0::"), ["", "0", "", ""]);
    }

    #[test]
    fn wifi_list_grouped_and_sorted() {
        // Как отдаёт ноутбук: одна сеть с двух точек, скрытые без имени.
        let text = " :100:WPA2:\n :100:WPA2:TP-Link_A704\n :100:WPA2 WPA3:NetCam\n\
                    *:85:WPA2:TP-Link_A704\n :40:--:Кафе\n :60:WPA2:Сосед\n";
        let known = parse_known("TP-Link_A704:802-11-wireless\nlo:loopback\n");
        assert_eq!(known, ["TP-Link_A704"]);
        let list = parse_wifi_list(text, &known);
        let names: Vec<&str> = list.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, ["TP-Link_A704", "NetCam", "Сосед", "Кафе"]);
        assert!(list[0].active && list[0].known);
        assert_eq!(list[0].signal, 100, "лучшая из точек сети");
        assert!(!list[3].secure, "открытая сеть без замка");
    }

    #[test]
    fn wired_state() {
        assert_eq!(
            parse_wired("wifi:connected:Дом\nethernet:unavailable:\n"),
            Some((false, String::new()))
        );
        assert_eq!(
            parse_wired("ethernet:connected:Проводное подключение 1\n"),
            Some((true, "Проводное подключение 1".into()))
        );
        assert_eq!(parse_wired("wifi:connected:Дом\n"), None);
    }

    fn panel() -> NetworkPanel {
        let known = vec!["Дом".to_string()];
        NetworkPanel::new(NetState {
            wifi: Some(true),
            wired: Some((false, String::new())),
            networks: parse_wifi_list("*:90:WPA2:Дом\n :50:WPA2:Сосед\n :30:--:Кафе\n", &known),
            scanning: false,
            busy: None,
            error: None,
        })
    }

    #[test]
    fn clicks() {
        let mut p = panel();
        let l = p.layout();
        let y = |i: usize| l.rows.iter().find(|r| r.1 == i).unwrap().0 + 5.0;
        assert_eq!(
            p.press(100.0, y(0)),
            NetworkAction::Nothing,
            "подключённая сеть — щелчок ничего не рвёт"
        );
        assert_eq!(
            p.press(100.0, y(1)),
            NetworkAction::Connect("Сосед".into(), false)
        );
        assert_eq!(
            p.press(100.0, y(2)),
            NetworkAction::Nothing,
            "пока подключаемся к одной — ждём"
        );
        assert_eq!(p.press(100.0, l.settings + 5.0), NetworkAction::Settings);
        assert_eq!(
            p.press(X1 - 5.0, PAD + TITLE / 2.0),
            NetworkAction::Wifi(false)
        );
        assert_eq!(p.layout().note.unwrap().1, "Wi-Fi выключен");
        assert_eq!(p.press(-1.0, 5.0), NetworkAction::Close);
    }

    #[test]
    fn renders() {
        let mut p = panel();
        p.state.error = Some("Не удалось подключиться: неверный пароль".into());
        let mut fonts = Fonts::new("Inter");
        let pm = p.render(&mut fonts);
        assert_eq!((pm.width(), pm.height()), p.size());
    }
}
