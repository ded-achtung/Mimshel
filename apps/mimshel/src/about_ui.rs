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
const W: u32 = 770;
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
    language: mimshel_core::i18n::Language,
}

impl AboutUi {
    pub fn new(lang: mimshel_core::i18n::Language) -> AboutUi {
        AboutUi {
            win: None,
            info: None,
            dirty: false,
            language: lang,
        }
    }

    pub fn set_language(&mut self, lang: mimshel_core::i18n::Language) {
        self.language = lang;
        self.dirty = true;
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
                let win = d.create_app_window(
                    WINDOW_CLASS,
                    mimshel_core::i18n::t(self.language, "apple.about"),
                    size,
                    size,
                )?;
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
        let frame = render_lang(info, size, fonts, icons, self.language);
        win.present(d, &frame)?;
        // Вывод — только запрос в очереди: отправить сейчас, а не со
        // следующим чужим запросом (окно стояло бы пустым до движения мыши).
        d.sync()
    }
}

/// Высота окна под сведения: заголовок, сборка, строки — но не ниже планеты.
fn height(info: &About) -> u32 {
    let text = PAD + 40.0 + 24.0 + 18.0 + info.rows.len() as f32 * ROW + PAD;
    text.max(PAD * 2.0 + LOGO as f32) as u32
}

/// Окно целиком размером `(w, h)` точек (русский по умолчанию — тесты).
pub fn render(info: &About, (w, h): (u32, u32), fonts: &mut Fonts, icons: &mut Icons) -> Canvas {
    render_lang(info, (w, h), fonts, icons, mimshel_core::i18n::Language::Ru)
}

pub fn render_lang(
    info: &About,
    (w, h): (u32, u32),
    fonts: &mut Fonts,
    icons: &mut Icons,
    lang: mimshel_core::i18n::Language,
) -> Canvas {
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
        let label_t = match *label {
            "Компьютер" => mimshel_core::i18n::t(lang, "about.computer"),
            "Процессор" => mimshel_core::i18n::t(lang, "about.processor"),
            "Память" => mimshel_core::i18n::t(lang, "about.memory"),
            "Графика" => mimshel_core::i18n::t(lang, "about.graphics"),
            "Диск" => mimshel_core::i18n::t(lang, "about.disk"),
            "Оболочка" => mimshel_core::i18n::t(lang, "about.shell"),
            "Экран" => mimshel_core::i18n::t(lang, "about.screen"),
            "Ядро" => mimshel_core::i18n::t(lang, "about.kernel"),
            other => other,
        };
        let value_t = translate_about_value(lang, value);
        fonts.draw(&mut pm, label_t, &LABEL, TEXT_X, y, Some(LABEL_W - 8.0));
        fonts.draw(&mut pm, &value_t, &VALUE, value_x, y, Some(value_w));
        y += ROW;
    }
    pm
}

/// Перевод значений «Об этом компьютере», собранных по-русски:
/// диск «занято X из Y ГБ» и процессор «(N потоков)»; остальное —
/// имена собственные / уже английские, не трогаем.
fn translate_about_value(lang: mimshel_core::i18n::Language, value: &str) -> String {
    if lang.resolve() == mimshel_core::i18n::Language::Ru {
        return value.to_string();
    }
    if let Some(rest) = value.strip_prefix("занято ") {
        if let Some((used, size)) = rest.split_once(" из ") {
            let size = size.strip_suffix(" ГБ").unwrap_or(size);
            return match lang.resolve() {
                mimshel_core::i18n::Language::En => format!("Used {used} of {size} GB"),
                mimshel_core::i18n::Language::Zh => format!("已用 {used} / 共 {size} GB"),
                _ => format!("已用 {used} / 共 {size} GB"),
            };
        }
    }
    if let Some(n) = value.strip_suffix(" ГБ") {
        return format!("{n} GB");
    }
    if let Some(base) = value.strip_suffix(" потоков)") {
        if let Some((head, n)) = base.rsplit_once(" (") {
            return match lang.resolve() {
                mimshel_core::i18n::Language::En => format!("{head} ({n} threads)"),
                mimshel_core::i18n::Language::Zh => format!("{head}（{n} 个线程）"),
                _ => format!("{head}（{n} 個執行緒）"),
            };
        }
    }
    value.to_string()
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
