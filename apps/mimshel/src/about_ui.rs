//! Окно «Об этом компьютере» (меню планеты), как «Об этом Mac»: слева наша
//! планета, справа название системы, её сборка и сведения строками. Рисует
//! служба; окно обычное, с рамкой xfwm4, закрывается крестиком, Esc или
//! Enter. Сведения собираются в потоке (lspci, glxinfo, df — доли секунды),
//! окно появляется, когда они готовы. Без службы — прежнее окно zenity.

use std::sync::mpsc::Sender;

use mimshel_icons::Icons;
use mimshel_render::Canvas;
use mimshel_setup::actions::About;
use mimshel_text::{Fonts, Style};
use mimshel_x11::{AppWindow, Display, Event, Key, Result};
use tiny_skia::{Color, PixmapPaint};

use crate::ui::Msg;

/// Класс окна — по нему строка меню и док узнают его.
const WINDOW_CLASS: &str = "mimshel-about";
const W: u32 = 700;
const PAD: f32 = 32.0;
const LOGO: u32 = 144;
/// Где начинается правая колонка и где в ней значения.
const TEXT_X: f32 = PAD + LOGO as f32 + 36.0;
const LABEL_W: f32 = 112.0;
const ROW: f32 = 22.0;

const BG: [u8; 3] = [0x1e, 0x1e, 0x1e];
const TITLE: Style = Style {
    size: 26.0,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 700,
};
const BUILD: Style = Style {
    size: 13.0,
    color: [0x8e, 0x8e, 0x93, 0xff],
    weight: 400,
};
const LABEL: Style = Style {
    size: 13.0,
    color: [0xf2, 0xf2, 0xf2, 0xff],
    weight: 600,
};
const VALUE: Style = Style {
    size: 13.0,
    color: [0xc7, 0xc7, 0xcc, 0xff],
    weight: 400,
};

pub struct AboutUi {
    win: Option<AppWindow>,
    info: Option<About>,
    dirty: bool,
}

impl AboutUi {
    pub fn new() -> AboutUi {
        AboutUi {
            win: None,
            info: None,
            dirty: false,
        }
    }

    /// Собрать сведения в потоке; готовые придут `Msg::About`.
    pub fn request(tx: Sender<Msg>) {
        std::thread::spawn(move || {
            let _ = tx.send(Msg::About(Box::new(mimshel_setup::actions::about())));
        });
    }

    /// Сведения готовы — показать окно (уже открыто — обновить и поднять).
    pub fn show(&mut self, d: &Display, info: About) -> Result<()> {
        let size = (W, height(&info));
        self.info = Some(info);
        self.dirty = true;
        match &self.win {
            Some(win) => d.activate(win.id())?,
            None => {
                let win = d.create_app_window(WINDOW_CLASS, "Об этом компьютере", size, size)?;
                win.show(d)?;
                self.win = Some(win);
            }
        }
        Ok(())
    }

    fn own(&self, window: u32) -> bool {
        self.win.as_ref().is_some_and(|w| w.id() == window)
    }

    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        match *ev {
            Event::Expose { window } | Event::Close { window } | Event::Configure { window } => {
                self.own(window)
            }
            Event::Key(_) => d.active_window().is_some_and(|w| self.own(w)),
            _ => false,
        }
    }

    pub fn event(&mut self, d: &Display, ev: Event) -> Result<()> {
        match ev {
            Event::Expose { .. } | Event::Configure { .. } => self.dirty = true,
            Event::Close { .. } => self.close(d)?,
            Event::Key(k) if matches!(k.key, Key::Escape | Key::Return) => self.close(d)?,
            _ => {}
        }
        Ok(())
    }

    fn close(&mut self, d: &Display) -> Result<()> {
        if let Some(win) = self.win.take() {
            win.destroy(d)?;
        }
        self.info = None;
        Ok(())
    }

    /// Перерисовать, если нужно.
    pub fn flush(&mut self, d: &Display, fonts: &mut Fonts, icons: &mut Icons) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        let (Some(win), Some(info)) = (&self.win, &self.info) else {
            return Ok(());
        };
        let size = win.size(d).unwrap_or((W, height(info)));
        let frame = render(info, size, fonts, icons);
        win.present(d, &frame)
    }
}

/// Высота окна под сведения: заголовок, сборка, строки — но не ниже планеты.
fn height(info: &About) -> u32 {
    let text = PAD + 40.0 + 24.0 + 18.0 + info.rows.len() as f32 * ROW + PAD;
    text.max(PAD * 2.0 + LOGO as f32) as u32
}

/// Окно целиком размером `(w, h)` точек.
pub fn render(info: &About, (w, h): (u32, u32), fonts: &mut Fonts, icons: &mut Icons) -> Canvas {
    let mut pm = Canvas::new(w.max(1), h.max(1)).expect("размер не нулевой");
    pm.fill(Color::from_rgba8(BG[0], BG[1], BG[2], 0xff));
    // Планета — по середине высоты левой части.
    if let Some(logo) = icons.get(mimshel_setup::parts::LOGO_NAME, LOGO) {
        let y = ((h as f32 - LOGO as f32) / 2.0).max(PAD);
        pm.draw_icon(logo.as_ref(), PAD, y, &PixmapPaint::default());
    }
    let mut y = PAD;
    fonts.draw(&mut pm, &info.name, &TITLE, TEXT_X, y, None);
    y += 40.0;
    fonts.draw(&mut pm, &info.build, &BUILD, TEXT_X, y, None);
    y += 24.0 + 18.0;
    let value_x = TEXT_X + LABEL_W;
    let value_w = w as f32 - PAD - value_x;
    for (label, value) in &info.rows {
        fonts.draw(&mut pm, label, &LABEL, TEXT_X, y, Some(LABEL_W - 8.0));
        fonts.draw(&mut pm, value, &VALUE, value_x, y, Some(value_w));
        y += ROW;
    }
    pm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_rows() {
        let info = About {
            name: "EndeavourOS".into(),
            build: "rolling".into(),
            rows: vec![
                ("Компьютер", "ded-gamingseries".into()),
                (
                    "Процессор",
                    "AMD Ryzen 9 7940H w/Radeon 780M Graphics (16 потоков)".into(),
                ),
                ("Графика", "NVIDIA GeForce RTX 4070 Max-Q / Mobile".into()),
                ("", "AMD Radeon 780M Graphics".into()),
            ],
        };
        let h = height(&info);
        assert!(h >= LOGO + 2 * PAD as u32, "планета влезает");
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::new("hicolor");
        let pm = render(&info, (W, h), &mut fonts, &mut icons);
        assert_eq!((pm.width(), pm.height()), (W, h));
        // Под строками сведений есть текст (светлые точки), фон — тёмный.
        let lit = (0..W)
            .filter(|&x| {
                pm.pixel(x, (PAD + 40.0 + 24.0 + 18.0 + 8.0) as u32)
                    .is_some_and(|p| p.red() > 120)
            })
            .count();
        assert!(lit > 0);
    }
}
