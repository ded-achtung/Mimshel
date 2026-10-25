//! Экран блокировки (`mimshel lock-screen`): окно на весь экран, клавиатура и
//! мышь захвачены, пароль проверяет PAM (та же цепочка `system-local-login`,
//! что у входа в систему и была у i3lock). Процесс запускает сторож
//! `mimshel lock` и перезапускает, если экран вышел без верного пароля
//! (урок 1 в CLAUDE.md) — поэтому любой сбой здесь возвращает код ≠ 0.
//!
//! Порядок старта важен: сначала экран закрывается тёмным окном и
//! захватывается клавиатура (до долей секунды, пока грузятся шрифты и фон),
//! и только потом рисуется полный вид.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mimshel_core::config::Config;
use mimshel_lock::{Frame, Layout, Phase};
use mimshel_text::Fonts;
use mimshel_x11::{Display, Event, Key, KeyPress, Popup};
use tiny_skia::Pixmap;

use mimshel_setup::lock::log;

/// Цепочка PAM: как у `i3lock` на Arch (его файл включает эту же).
const PAM_SERVICE: &str = "system-local-login";
/// Шаг громкости клавишами, как в строке меню.
const VOLUME_STEP: &str = "5%";
/// Сколько показывать «Неверный пароль».
const WRONG_SHOW: Duration = Duration::from_millis(900);
/// Как часто проверять, что захват клавиатуры ещё наш.
const REGRAB: Duration = Duration::from_secs(2);
/// Предел длины пароля (точки всё равно влезают не все).
const MAX_PASSWORD: usize = 128;
/// Шаг ожидания событий: по нему же обновляем часы и следим за захватом.
const TICK: Duration = Duration::from_millis(400);

/// Запустить экран блокировки. 0 — разблокирован верным паролем.
pub fn run() -> i32 {
    match locked_run() {
        Ok(code) => code,
        Err(e) => {
            log(&format!("экран блокировки: {e}"));
            1
        }
    }
}

fn locked_run() -> mimshel_x11::Result<i32> {
    let config = crate::ui::load_config();
    let display = Arc::new(Display::connect()?);
    let (w, h) = display.screen_size();
    let layout = Layout::new(i64::from(w), i64::from(h));

    // 1. Закрыть экран и забрать ввод — раньше всего остального.
    let popup = display.create_popup("lock-screen")?;
    let dark = dark_frame(u32::from(w), u32::from(h));
    // Захват не вышел (клавиатуру держит другая программа) — не уходим:
    // окно уже закрыло экран, захват повторяется каждый шаг цикла.
    let grabbed = popup.show_lock(&display, w, h)?;
    if !grabbed {
        log("клавиатура или мышь заняты другой программой — повторяю захват");
    }
    popup.present(&display, &dark)?;
    // Следить за окнами, появляющимися поверх (уведомления, всплывающие
    // окна), и за сменой размера экрана (подключили монитор).
    display.watch_stacking()?;

    // 2. Фон (кеш или рисуется заново) и шрифты — экран уже закрыт.
    let mut ui = LockUi::new(Arc::clone(&display), &config, &layout, &popup, dark);
    ui.grabbed = grabbed;
    ui.redraw()?;

    // 3. События — в отдельном потоке; основной цикл по таймауту обновляет
    //    часы, возвращает захват и гасит «Неверный пароль».
    let (tx, rx) = mpsc::channel();
    {
        let display = Arc::clone(&display);
        std::thread::spawn(move || {
            while let Ok(ev) = display.next_event() {
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
    }
    log("экран блокировки показан");
    loop {
        match rx.recv_timeout(TICK) {
            Ok(ev) => {
                if let Some(code) = ui.event(ev)? {
                    return Ok(code);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // X-соединение оборвалось (выход из сеанса) — блокировать нечего.
                return Ok(1);
            }
        }
        if let Some(code) = ui.tick()? {
            return Ok(code);
        }
    }
}

struct LockUi<'a> {
    display: Arc<Display>,
    popup: &'a Popup,
    layout: Layout,
    fonts: Fonts,
    /// Готовый фон (обои, аватар, имя, поле) — копируется в каждый кадр.
    bg: Pixmap,
    /// Последний показанный кадр (для Expose).
    frame: Option<Pixmap>,
    password: Vec<char>,
    phase: Phase,
    wrong_since: Option<Instant>,
    time: String,
    date: String,
    minute: u64,
    placeholder: String,
    last_regrab: Instant,
    /// Клавиатура и мышь сейчас у нас.
    grabbed: bool,
    /// Обои из настроек — нарисовать фон заново, если сменится размер экрана.
    wallpaper: String,
    user: String,
}

impl<'a> LockUi<'a> {
    fn new(
        display: Arc<Display>,
        config: &Config,
        layout: &Layout,
        popup: &'a Popup,
        dark: Pixmap,
    ) -> LockUi<'a> {
        let bg = background(&config.wallpaper, layout).unwrap_or(dark);
        let user = std::env::var("USER")
            .ok()
            .filter(|u| !u.is_empty())
            .or_else(|| {
                std::env::var("HOME").ok().and_then(|home| {
                    std::path::Path::new(&home)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
            })
            .unwrap_or_default();
        let mut ui = LockUi {
            display,
            popup,
            layout: *layout,
            fonts: Fonts::new(&config.font),
            bg,
            frame: None,
            password: Vec::new(),
            phase: Phase::Idle,
            wrong_since: None,
            time: String::new(),
            date: String::new(),
            minute: u64::MAX,
            placeholder: config.lock_placeholder.clone(),
            last_regrab: Instant::now(),
            grabbed: true,
            wallpaper: config.wallpaper.clone(),
            user,
        };
        ui.update_clock();
        ui
    }

    /// Кадр из текущего состояния → в окно.
    fn redraw(&mut self) -> mimshel_x11::Result<()> {
        let state = Frame {
            time: &self.time,
            date: &self.date,
            placeholder: &self.placeholder,
            typed: self.password.len(),
            phase: self.phase,
        };
        let Some(frame) = mimshel_lock::frame(&self.layout, &self.bg, &state, &mut self.fonts)
        else {
            return Ok(());
        };
        self.popup.present(&self.display, &frame)?;
        self.frame = Some(frame);
        Ok(())
    }

    /// `Some(код)` — пора выходить (0 — верный пароль).
    fn event(&mut self, ev: Event) -> mimshel_x11::Result<Option<i32>> {
        let me = self.popup.id();
        let root = self.display.root();
        match ev {
            Event::Key(k) => self.key(k),
            Event::Expose { .. } => {
                if let Some(frame) = &self.frame {
                    self.popup.present(&self.display, frame)?;
                }
                Ok(None)
            }
            // Сменился размер экрана (подключили монитор) — закрыть его весь.
            Event::Configure { window } if window == root => {
                self.resize()?;
                Ok(None)
            }
            // Другое окно появилось или переставлено — наше снова наверх:
            // иначе уведомление или всплывающее окно показалось бы поверх
            // блокировки вместе со своим текстом.
            Event::Mapped { window } | Event::Configure { window } if window != me => {
                self.popup.raise(&self.display)?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Экран сменил размер: окно на весь новый экран, фон и раскладка заново.
    fn resize(&mut self) -> mimshel_x11::Result<()> {
        let (w, h) = self.display.screen_size();
        let layout = Layout::new(i64::from(w), i64::from(h));
        if layout.w == self.layout.w && layout.h == self.layout.h {
            return Ok(());
        }
        log(&format!("экран блокировки: новый размер {w}x{h}"));
        // Сначала закрыть весь экран тёмным, потом — полный вид.
        let dark = dark_frame(u32::from(w), u32::from(h));
        self.popup.set_geometry(&self.display, 0, 0, w, h)?;
        self.popup.present(&self.display, &dark)?;
        self.popup.raise(&self.display)?;
        self.layout = layout;
        self.bg = background(&self.wallpaper, &self.layout).unwrap_or(dark);
        self.redraw()
    }

    fn key(&mut self, k: KeyPress) -> mimshel_x11::Result<Option<i32>> {
        if k.logo {
            return Ok(None);
        }
        match k.key {
            Key::Char(c) if !k.ctrl && !k.alt => {
                if self.phase == Phase::Wrong {
                    self.phase = Phase::Idle;
                    self.wrong_since = None;
                }
                if self.password.len() < MAX_PASSWORD {
                    self.password.push(c);
                    self.redraw()?;
                }
            }
            Key::BackSpace | Key::Delete => {
                if self.password.pop().is_some() {
                    self.redraw()?;
                }
            }
            Key::Escape => {
                if !self.password.is_empty() {
                    self.clear_password();
                    self.redraw()?;
                }
            }
            Key::Char('u') if k.ctrl => {
                if !self.password.is_empty() {
                    self.clear_password();
                    self.redraw()?;
                }
            }
            Key::Return => {
                // Пустой пароль не проверяем (как --ignore-empty-password).
                if !self.password.is_empty() && self.phase != Phase::Verifying {
                    return self.verify();
                }
            }
            Key::VolumeUp => volume(&[
                "set-sink-volume",
                "@DEFAULT_SINK@",
                &format!("+{VOLUME_STEP}"),
            ]),
            Key::VolumeDown => volume(&[
                "set-sink-volume",
                "@DEFAULT_SINK@",
                &format!("-{VOLUME_STEP}"),
            ]),
            Key::VolumeMute => volume(&["set-sink-mute", "@DEFAULT_SINK@", "toggle"]),
            _ => {}
        }
        Ok(None)
    }

    /// Проверить пароль. `Some(0)` — верный, экран закрывается; `None` —
    /// неверный, остаёмся и показываем «Неверный пароль».
    fn verify(&mut self) -> mimshel_x11::Result<Option<i32>> {
        self.phase = Phase::Verifying;
        self.redraw()?;
        self.display.sync()?;
        let password: String = self.password.iter().collect();
        let ok = authenticate(&self.user, &password);
        // Копию тоже стереть: память строки вернётся в общий запас программы.
        let mut bytes = password.into_bytes();
        bytes.fill(0);
        drop(bytes);
        self.clear_password();
        if ok {
            log("пароль верный — разблокировано");
            // Фокус — активному окну: иначе клавиатура осталась бы «нигде»
            // (на время блокировки фокус уходил в корневое окно).
            let _ = self.display.restore_focus();
            self.popup.hide(&self.display)?;
            return Ok(Some(0));
        }
        self.phase = Phase::Wrong;
        self.wrong_since = Some(Instant::now());
        self.redraw()?;
        Ok(None)
    }

    /// Таймер: часы, возврат «Неверный пароль» в покой, проверка захвата.
    /// `Some(код)` — пора выходить.
    fn tick(&mut self) -> mimshel_x11::Result<Option<i32>> {
        let mut redraw = false;
        let minute = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() / 60)
            .unwrap_or(0);
        if minute != self.minute {
            self.update_clock();
            redraw = true;
        }
        if self.phase == Phase::Wrong && self.wrong_since.is_some_and(|t| t.elapsed() >= WRONG_SHOW)
        {
            self.phase = Phase::Idle;
            self.wrong_since = None;
            redraw = true;
        }
        if redraw {
            self.redraw()?;
        }
        // На всякий случай — наверх и каждый шаг, не только по событиям.
        self.popup.raise(&self.display)?;
        // Захват мог снять другой процесс: вернуть себе. Не выходит — НЕ
        // уходить (окно ушло бы с экрана, и стол открылся бы до перезапуска),
        // а держать экран закрытым и пробовать каждый шаг.
        if !self.grabbed || self.last_regrab.elapsed() >= REGRAB {
            self.last_regrab = Instant::now();
            let now = self.popup.regrab(&self.display)?;
            if now != self.grabbed {
                log(if now {
                    "захват клавиатуры и мыши снова наш"
                } else {
                    "захват клавиатуры или мыши потерян — экран остаётся, пробую вернуть"
                });
            }
            self.grabbed = now;
        }
        Ok(None)
    }

    fn update_clock(&mut self) {
        self.minute = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() / 60)
            .unwrap_or(0);
        let (time, date) = clock();
        self.time = time;
        self.date = date;
    }

    /// Пароль стирается из памяти сразу после проверки.
    fn clear_password(&mut self) {
        self.password.fill('\0');
        self.password.clear();
    }
}

impl Drop for LockUi<'_> {
    fn drop(&mut self) {
        self.clear_password();
    }
}

/// Время и дата одной командой `date` (язык — из окружения сеанса).
fn clock() -> (String, String) {
    let out = Command::new("date")
        .arg("+%H:%M\n%A, %-d %B")
        .stdin(Stdio::null())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let mut lines = out.lines();
    (
        lines.next().unwrap_or("").to_string(),
        lines.next().unwrap_or("").to_string(),
    )
}

/// Проверка пароля через PAM (цепочка та же, что при входе в систему).
fn authenticate(user: &str, password: &str) -> bool {
    match pam::Client::with_password(PAM_SERVICE) {
        Ok(mut client) => {
            client.conversation_mut().set_credentials(user, password);
            client.authenticate().is_ok()
        }
        Err(e) => {
            // Не «неверный пароль», а сломалась сама проверка — в журнал.
            log(&format!("PAM не начал проверку: {e}"));
            false
        }
    }
}

/// Громкость клавишами — как в строке меню (pactl, не блокируя экран).
fn volume(args: &[&str]) {
    let mut cmd = Command::new("setsid");
    cmd.arg("-f").arg("pactl").args(args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = cmd.status();
}

/// Фон экрана блокировки под этот размер (обои, аватар, имя, поле).
fn background(wallpaper: &str, layout: &Layout) -> Option<Pixmap> {
    let (w, h) = (u32::try_from(layout.w).ok()?, u32::try_from(layout.h).ok()?);
    mimshel_setup::lock::background(wallpaper, layout.w, layout.h)
        .and_then(|path| mimshel_icons::render_file(&path, w, h, mimshel_icons::Fit::Cover))
}

/// Ровный тёмный кадр — закрыть экран, пока грузится остальное.
fn dark_frame(w: u32, h: u32) -> Pixmap {
    let mut pm =
        Pixmap::new(w, h).unwrap_or_else(|| Pixmap::new(1, 1).expect("картинка 1×1 создаётся"));
    pm.fill(tiny_skia::Color::from_rgba8(28, 28, 32, 255));
    pm
}

/// `mimshel snapshot lock ТЕКСТ ФАЙЛ.png` — нарисовать экран блокировки
/// (ТЕКСТ — «набранный пароль»; пустой — подсказка в поле; «!» в начале —
/// состояние «Неверный пароль»).
pub fn snapshot(text: &str, file: &str) -> mimshel_x11::Result<()> {
    let config = crate::ui::load_config();
    let display = Display::connect().ok();
    let (w, h) = display.map_or((1920, 1080), |d| {
        let (w, h) = d.screen_size();
        (u32::from(w), u32::from(h))
    });
    let layout = Layout::new(i64::from(w), i64::from(h));
    let bg = background(&config.wallpaper, &layout).unwrap_or_else(|| dark_frame(w, h));
    let mut fonts = Fonts::new(&config.font);
    let (time, date) = clock();
    let wrong = text.starts_with('!');
    let typed = text.strip_prefix('!').unwrap_or(text);
    let state = Frame {
        time: &time,
        date: &date,
        placeholder: &config.lock_placeholder,
        typed: typed.chars().count(),
        phase: if wrong { Phase::Wrong } else { Phase::Idle },
    };
    let frame =
        mimshel_lock::frame(&layout, &bg, &state, &mut fonts).ok_or("экран не нарисован")?;
    frame.save_png(file)?;
    Ok(())
}
