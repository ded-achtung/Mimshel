//! Окно Spotlight на экране: служба (`shell daemon`) или разовый запуск.
//!
//! Служба заранее загружает шрифты, иконки и список программ и ждёт команды
//! через сокет `$XDG_RUNTIME_DIR/shell.sock`; поэтому окно открывается сразу.
//! Без службы `shell spotlight` делает всё сам — медленнее, но работает.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::Instant;

use shell_apps::{App, Settings, Watcher};
use shell_core::config::Config;
use shell_icons::Icons;
use shell_launcher::{Action, Spotlight};
use shell_text::Fonts;
use shell_x11::{Display, Event, Popup};
use tiny_skia::Pixmap;

type Result<T> = shell_x11::Result<T>;

/// Иконки программ в списке — такого размера (см. оформление Spotlight).
const ROW_ICON: u32 = 26;
/// Окно стоит на этой доле высоты экрана сверху, как в macOS.
const TOP: f32 = 0.22;

enum Msg {
    X(Event),
    /// Открыть или закрыть окно.
    Toggle,
    /// Список программ изменился.
    Apps(Vec<App>),
}

/// Где служба ждёт команды.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("shell.sock"),
        None => std::env::temp_dir().join(format!("shell-{}.sock", std::process::id())),
    }
}

/// Просит запущенную службу открыть Spotlight. `false` — службы нет.
pub fn ask_daemon() -> bool {
    UnixStream::connect(socket_path())
        .and_then(|mut s| s.write_all(b"spotlight\n"))
        .is_ok()
}

/// Запускает окно. `daemon` — работать службой и ждать команд;
/// иначе открыть окно один раз и выйти после закрытия.
pub fn run(daemon: bool) -> Result<()> {
    let start = Instant::now();
    let config = Config::default();
    let (tx, rx) = mpsc::channel();

    if daemon {
        listen(tx.clone())?;
    }
    let display = Arc::new(Display::connect()?);
    let popup = display.create_popup("spotlight")?;
    {
        let display = display.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = display.next_event() {
                if tx.send(Msg::X(ev)).is_err() {
                    break;
                }
            }
        });
    }

    let settings = Settings::from_env();
    let apps = shell_apps::load(&settings);
    let fonts = Fonts::new(&config.font);
    let mut icons = Icons::new(&config.icon_theme);
    if daemon {
        watch_apps(settings, tx.clone());
        // Иконки всех программ — заранее, чтобы первое открытие было мгновенным.
        for name in apps.iter().filter_map(|a| a.icon.as_deref()) {
            icons.get(name, ROW_ICON);
        }
        eprintln!("Служба готова за {:?}", start.elapsed());
    }

    let mut window = Window {
        display: &display,
        popup,
        spotlight: Spotlight::new(apps),
        fonts,
        icons,
        frame: None,
        visible: false,
        terminal: config.terminal,
    };
    if !daemon {
        window.toggle()?;
        eprintln!("Окно открыто за {:?}", start.elapsed());
    }

    for msg in rx {
        match msg {
            Msg::Toggle => window.toggle()?,
            Msg::Apps(apps) => {
                window.spotlight.set_apps(apps);
                if window.visible {
                    window.redraw()?;
                }
            }
            Msg::X(ev) if window.visible => window.event(ev)?,
            Msg::X(_) => {}
        }
        if !daemon && !window.visible {
            break;
        }
    }
    Ok(())
}

struct Window<'a> {
    display: &'a Display,
    popup: Popup,
    spotlight: Spotlight,
    fonts: Fonts,
    icons: Icons,
    /// Последняя нарисованная картинка — чтобы быстро повторить её по Expose.
    frame: Option<Pixmap>,
    visible: bool,
    terminal: Vec<String>,
}

impl Window<'_> {
    fn toggle(&mut self) -> Result<()> {
        if self.visible {
            return self.hide();
        }
        self.spotlight.reset();
        let frame = self.spotlight.render(&mut self.fonts, &mut self.icons);
        let (x, y) = self.position(frame.width());
        self.popup.show(
            self.display,
            x,
            y,
            frame.width() as u16,
            frame.height() as u16,
        )?;
        self.popup.present(self.display, &frame)?;
        self.frame = Some(frame);
        self.visible = true;
        Ok(())
    }

    fn hide(&mut self) -> Result<()> {
        self.popup.hide(self.display)?;
        self.visible = false;
        Ok(())
    }

    /// Левый верхний угол окна: по центру, на 22 % высоты экрана.
    fn position(&self, width: u32) -> (i16, i16) {
        let (sw, sh) = self.display.screen_size();
        let x = (i32::from(sw) - width as i32) / 2;
        let y = (f32::from(sh) * TOP) as i32;
        (x as i16, y as i16)
    }

    fn redraw(&mut self) -> Result<()> {
        let frame = self.spotlight.render(&mut self.fonts, &mut self.icons);
        let old = self.frame.as_ref().map(|f| (f.width(), f.height()));
        if old != Some((frame.width(), frame.height())) {
            let (x, y) = self.position(frame.width());
            self.popup.set_geometry(
                self.display,
                x,
                y,
                frame.width() as u16,
                frame.height() as u16,
            )?;
        }
        self.popup.present(self.display, &frame)?;
        self.frame = Some(frame);
        Ok(())
    }

    fn event(&mut self, ev: Event) -> Result<()> {
        let action = match ev {
            Event::Key(k) => self.spotlight.key(k),
            Event::Button { x, y, button } => {
                self.spotlight.click(i32::from(x), i32::from(y), button)
            }
            Event::Motion { x, y } => self.spotlight.motion(i32::from(x), i32::from(y)),
            Event::Expose => {
                if let Some(frame) = &self.frame {
                    self.popup.present(self.display, frame)?;
                }
                Action::Nothing
            }
            Event::Other => Action::Nothing,
        };
        match action {
            Action::Nothing => {}
            Action::Redraw => self.redraw()?,
            Action::Close => self.hide()?,
            Action::Launch(app) => {
                self.hide()?;
                if let Err(e) = app.launch(&self.terminal) {
                    eprintln!("Не удалось запустить «{}»: {e}", app.name);
                }
            }
        }
        Ok(())
    }
}

/// Принимает команды через сокет в отдельном потоке.
fn listen(tx: Sender<Msg>) -> Result<()> {
    let path = socket_path();
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(&path).is_ok() {
                return Err("служба уже запущена".into());
            }
            // Файл остался от упавшей службы — убираем и занимаем заново.
            std::fs::remove_file(&path)?;
            UnixListener::bind(&path)?
        }
        Err(e) => return Err(e.into()),
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            for line in BufReader::new(stream).lines().map_while(|l| l.ok()) {
                let msg = match line.trim() {
                    "spotlight" => Msg::Toggle,
                    other => {
                        eprintln!("Неизвестная команда: {other}");
                        continue;
                    }
                };
                if tx.send(msg).is_err() {
                    return;
                }
            }
        }
    });
    Ok(())
}

/// Следит за ярлыками и присылает новый список программ.
fn watch_apps(settings: Settings, tx: Sender<Msg>) {
    let mut watcher = match Watcher::new(&settings) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Не удалось следить за ярлыками: {e}");
            return;
        }
    };
    std::thread::spawn(move || {
        while watcher.wait().is_ok() {
            if tx.send(Msg::Apps(shell_apps::load(&settings))).is_err() {
                break;
            }
        }
    });
}

/// Рисует Spotlight с набранным текстом в PNG — без экрана.
pub fn snapshot(text: &str, file: &str) -> Result<()> {
    let config = Config::default();
    let start = Instant::now();
    let mut fonts = Fonts::new(&config.font);
    let mut icons = Icons::new(&config.icon_theme);
    let mut spotlight = Spotlight::new(shell_apps::load(&Settings::from_env()));
    eprintln!("Загрузка: {:?}", start.elapsed());
    for c in text.chars() {
        spotlight.key(shell_x11::KeyPress::plain(shell_x11::Key::Char(c)));
    }
    let start = Instant::now();
    let frame = spotlight.render(&mut fonts, &mut icons);
    eprintln!("Рисование: {:?}", start.elapsed());
    frame.save_png(file)?;
    Ok(())
}
