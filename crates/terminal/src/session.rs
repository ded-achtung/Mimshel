//! Сеанс терминала: оболочка в псевдотерминале (PTY), ядро Alacritty, которое
//! разбирает её вывод, и всё, что с ним делают: ввод, смена размера,
//! прокрутка истории, выделение, мышь для программ (htop, vim), ответы
//! программам (какой цвет, какой размер).
//!
//! Чтение PTY идёт в потоке Alacritty; он сообщает о событиях через
//! [`Proxy`]: событие — в очередь и `wake` (окно просыпается и зовёт
//! [`Session::poll`]).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::event::Notify;
use alacritty_terminal::event::{Event, EventListener, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use mimshel_core::config::Osc52;
use tiny_skia::Pixmap;

use crate::glyphs::Glyphs;
use crate::palette::Palette;

/// OSC 52 payloads above this many bytes are ignored: any program in the
/// terminal (including a remote one over SSH) can write to the clipboard,
/// so the data must not be unbounded.
const OSC52_MAX: usize = 1024 * 1024;

/// Accept a clipboard store from a program (setting + size limit)?
fn accept_osc52(osc52: Osc52, text: &str) -> bool {
    osc52 == Osc52::Allow && text.len() <= OSC52_MAX
}

/// Что сеанс сообщает окну.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// Появилось новое — перерисовать.
    Redraw,
    Title(String),
    ResetTitle,
    /// Программа просит положить текст в буфер обмена (OSC 52: Claude Code,
    /// tmux, vim по SSH). Читать буфер программам не даём.
    Copy(String),
    Bell,
    /// Оболочка закончила работу — окно закрыть.
    Exit,
}

/// Куда ядро Alacritty шлёт события.
#[derive(Clone)]
pub struct Proxy {
    tx: Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        let _ = self.tx.send(event);
        (self.wake)();
    }
}

pub struct Session {
    term: Arc<FairMutex<Term<Proxy>>>,
    notifier: Notifier,
    events: Receiver<Event>,
    cell: (u32, u32),
    size: (usize, usize),
    /// Palette — for «what colour are you» answers (OSC 10/11/4).
    palette: Palette,
    /// Whether programs may put text into the clipboard (OSC 52).
    osc52: Osc52,
    /// Shell process id — to find its current folder.
    shell_pid: u32,
}

impl Session {
    /// Start the user's shell (or `command` — program and arguments) in a
    /// `cols`×`rows` grid of `cell`-pixel cells, in folder `dir`.
    pub fn spawn(
        (cols, rows): (usize, usize),
        cell: (u32, u32),
        dir: Option<PathBuf>,
        command: Option<Vec<String>>,
        palette: Palette,
        osc52: Osc52,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> std::io::Result<Session> {
        let mut env = std::collections::HashMap::new();
        // Программы узнают по TERM, что умеет терминал: умеем всё, что xterm, и 24-битный цвет.
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        let shell = command.and_then(|mut c| {
            (!c.is_empty()).then(|| {
                let program = c.remove(0);
                tty::Shell::new(program, c)
            })
        });
        let options = tty::Options {
            shell,
            working_directory: dir,
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&options, window_size((cols, rows), cell), 0)?;
        let shell_pid = pty.child().id();
        let (tx, events) = mpsc::channel();
        let proxy = Proxy {
            tx,
            wake: Arc::new(wake),
        };
        let term = Term::new(Config::default(), &TermSize::new(cols, rows), proxy.clone());
        let term = Arc::new(FairMutex::new(term));
        let event_loop = EventLoop::new(term.clone(), proxy, pty, true, false)?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Session {
            term,
            notifier,
            events,
            cell,
            size: (cols, rows),
            palette,
            osc52,
            shell_pid,
        })
    }

    /// The shell's current folder (`cd` changes it) — a new tab
    /// opens there.
    pub fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{}/cwd", self.shell_pid)).ok()
    }

    /// Разобрать накопившиеся события ядра: на просьбы программ ответить
    /// сразу, остальное — окну.
    pub fn poll(&mut self) -> Vec<TermEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Wakeup | Event::MouseCursorDirty | Event::CursorBlinkingChange => {
                    out.push(TermEvent::Redraw);
                }
                Event::Title(t) => out.push(TermEvent::Title(t)),
                Event::ResetTitle => out.push(TermEvent::ResetTitle),
                // A program asks to store clipboard text: honour the setting
                // and the size limit (or anything, even over SSH, owns it).
                Event::ClipboardStore(_, text) => {
                    if accept_osc52(self.osc52, &text) {
                        out.push(TermEvent::Copy(text));
                    }
                }
                // Читать буфер обмена программе не даём (ядро и так не просит,
                // пока OSC 52 только на копирование) — на всякий случай пусто.
                Event::ClipboardLoad(..) => {}
                Event::PtyWrite(text) => self.write(text.into_bytes()),
                Event::ColorRequest(index, format) => {
                    let term = self.term.lock();
                    let rgb = self.palette.resolve(
                        alacritty_terminal::vte::ansi::Color::Indexed(index.min(255) as u8),
                        term.colors(),
                    );
                    let rgb = match index {
                        256 => self.palette.foreground,
                        257 => self.palette.background,
                        258 => self.palette.cursor,
                        _ => rgb,
                    };
                    drop(term);
                    let reply = format(alacritty_terminal::vte::ansi::Rgb {
                        r: rgb[0],
                        g: rgb[1],
                        b: rgb[2],
                    });
                    self.write(reply.into_bytes());
                }
                Event::TextAreaSizeRequest(format) => {
                    let reply = format(window_size(self.size, self.cell));
                    self.write(reply.into_bytes());
                }
                Event::Bell => out.push(TermEvent::Bell),
                Event::Exit | Event::ChildExit(_) => out.push(TermEvent::Exit),
            }
        }
        out
    }

    /// Отдать программе байты (нажатия, вставку).
    pub fn write(&self, bytes: Vec<u8>) {
        self.notifier.notify(bytes);
    }

    /// Нажатие или вставка: вернуться к последним строкам и снять выделение.
    pub fn input(&self, bytes: Vec<u8>) {
        {
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None;
        }
        self.write(bytes);
    }

    /// Вставить текст: в режиме «скобок» (bash, vim) — в скобках, чтобы
    /// программа не выполнила строки по одной.
    pub fn paste(&self, text: &str) {
        let bracketed = self.mode().contains(TermMode::BRACKETED_PASTE);
        self.input(paste_bytes(text, bracketed));
    }

    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    /// Окно сменило размер — сетка и PTY (программа получит SIGWINCH).
    pub fn resize(&mut self, size: (usize, usize)) {
        if size == self.size {
            return;
        }
        self.size = size;
        self.term.lock().resize(TermSize::new(size.0, size.1));
        self.notifier.on_resize(window_size(size, self.cell));
    }

    /// Прокрутить историю на `lines` строк (вверх — больше нуля).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    /// Scrollback length (lines above the screen) and how far up we are now
    /// (0 — at the bottom, the live screen).
    pub fn history(&self) -> (usize, usize) {
        let term = self.term.lock();
        let grid = term.grid();
        (grid.history_size(), grid.display_offset())
    }

    /// Scroll so that `offset` lines of history are above the bottom (clamped).
    pub fn scroll_to(&self, offset: usize) {
        let mut term = self.term.lock();
        let now = term.grid().display_offset() as i32;
        let target = offset.min(term.grid().history_size()) as i32;
        term.scroll_display(Scroll::Delta(target - now));
    }

    /// To the very top of the scrollback or back to the bottom.
    pub fn scroll_end(&self, top: bool) {
        self.term
            .lock()
            .scroll_display(if top { Scroll::Top } else { Scroll::Bottom });
    }

    pub fn scroll_page(&self, up: bool) {
        self.term
            .lock()
            .scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    /// Точка сетки под клеткой экрана (с учётом прокрутки истории).
    fn point(&self, col: usize, row: usize) -> Point {
        let offset = self.term.lock().grid().display_offset() as i32;
        Point::new(Line(row as i32 - offset), Column(col))
    }

    /// Начать выделение: `clicks` — 1 (знаки), 2 (слово), 3 (строки).
    pub fn select_start(&self, (col, row): (usize, usize), clicks: u8, right_half: bool) {
        let point = self.point(col, row);
        let kind = match clicks {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        let side = if right_half { Side::Right } else { Side::Left };
        self.term.lock().selection = Some(Selection::new(kind, point, side));
    }

    pub fn select_update(&self, (col, row): (usize, usize), right_half: bool) {
        let point = self.point(col, row);
        let side = if right_half { Side::Right } else { Side::Left };
        if let Some(sel) = &mut self.term.lock().selection {
            sel.update(point, side);
        }
    }

    /// Select everything: scrollback and screen.
    pub fn select_all(&self) {
        let mut term = self.term.lock();
        let history = term.grid().history_size() as i32;
        let (cols, lines) = (term.columns(), term.screen_lines() as i32);
        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(Line(-history), Column(0)),
            Side::Left,
        );
        sel.update(
            Point::new(Line(lines - 1), Column(cols.saturating_sub(1))),
            Side::Right,
        );
        term.selection = Some(sel);
    }

    /// Forget the scrollback (what scrolled off the top).
    pub fn clear_history(&self) {
        let mut term = self.term.lock();
        term.grid_mut().clear_history();
        term.scroll_display(Scroll::Bottom);
        term.selection = None;
    }

    /// New cell size (the font changed) and a grid to match.
    pub fn set_cell(&mut self, cell: (u32, u32), size: (usize, usize)) {
        self.cell = cell;
        self.size = (0, 0);
        self.resize(size);
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    /// Выделенный текст (пустое выделение — `None`).
    pub fn selection_text(&self) -> Option<String> {
        self.term
            .lock()
            .selection_to_string()
            .filter(|t| !t.is_empty())
    }

    /// Программа сама хочет мышь (htop, vim с `mouse=a`, mc).
    pub fn mouse_wanted(&self) -> bool {
        self.mode().intersects(TermMode::MOUSE_MODE)
    }

    /// Сообщить программе о мыши (SGR: `ESC [ < кнопка ; столбец ; строка M/m`).
    /// `button`: 0–2 — кнопки, 64/65 — колёсико; `drag` — движение с нажатой.
    pub fn mouse_report(&self, button: u8, (col, row): (usize, usize), pressed: bool, drag: bool) {
        let mode = self.mode();
        if !mode.contains(TermMode::SGR_MOUSE) {
            // Старый способ — только нажатия в пределах 223 клеток.
            if !pressed || col > 222 || row > 222 {
                return;
            }
            let b = button + if drag { 32 } else { 0 };
            self.write(vec![
                0x1b,
                b'[',
                b'M',
                32 + b,
                33 + col as u8,
                33 + row as u8,
            ]);
            return;
        }
        let b = button + if drag { 32 } else { 0 };
        let end = if pressed { 'M' } else { 'm' };
        self.write(format!("\x1b[<{b};{};{}{end}", col + 1, row + 1).into_bytes());
    }

    /// Колёсико: в истории — прокрутка; программе с мышью — сообщить; на
    /// втором экране (less, man) без мыши — стрелки, как в других терминалах.
    pub fn wheel(&self, up: bool, cell: (usize, usize)) {
        let mode = self.mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            self.mouse_report(if up { 64 } else { 65 }, cell, true, false);
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let arrow: &[u8] = match (up, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.write(arrow.repeat(3));
        } else {
            self.scroll(if up { 3 } else { -3 });
        }
    }

    /// A `w`×`h`-pixel frame. `cursor_on` — false in the «off» half of a blink.
    pub fn render(
        &self,
        glyphs: &mut Glyphs,
        size: (u32, u32),
        focused: bool,
        cursor_on: bool,
    ) -> Option<Pixmap> {
        let term = self.term.lock();
        crate::screen::render(&term, glyphs, &self.palette, size, focused, cursor_on)
    }

    /// The program asked for a blinking cursor (DECSCUSR, `CSI ?12h`).
    pub fn cursor_blinking(&self) -> bool {
        self.term.lock().cursor_style().blinking
    }

    /// The web address under screen cell `(col, row)`, if any (Ctrl+click
    /// opens it). Only within one screen row.
    pub fn url_at(&self, (col, row): (usize, usize)) -> Option<String> {
        let term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let line = Line(row as i32 - offset);
        let cols = term.columns();
        let chars: Vec<char> = (0..cols).map(|c| term.grid()[line][Column(c)].c).collect();
        drop(term);
        crate::links::url_at(&chars, col)
    }

    /// Сколько строк в сетке (для проверок).
    pub fn screen_lines(&self) -> usize {
        self.term.lock().screen_lines()
    }
}

fn window_size((cols, rows): (usize, usize), (cw, ch): (u32, u32)) -> WindowSize {
    WindowSize {
        num_lines: rows as u16,
        num_cols: cols as u16,
        cell_width: cw as u16,
        cell_height: ch as u16,
    }
}

/// The bytes a paste sends. In bracketed mode the text must not be able
/// to end the brackets early: a single pass removing `ESC [201~` is not
/// enough (`ESC [20` + `ESC [201~` + `1~` leaves a new one behind), so
/// every escape character goes, ESC and its one-byte form (C1 CSI).
/// Whatever followed a fake end would run in the shell as typed commands.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let safe: String = text
            .chars()
            .filter(|&c| c != '\x1b' && c != '\u{9b}')
            .collect();
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Настоящий сеанс: оболочка печатает — сетка это видит, OSC 52 доходит.
    #[test]
    fn paste_cannot_end_the_brackets() {
        let nested = "echo ok\x1b[20\x1b[201~1~\nrm -rf ~\n";
        let out = String::from_utf8(paste_bytes(nested, true)).unwrap();
        assert_eq!(out.matches("\x1b[201~").count(), 1, "{out:?}");
        assert!(out.ends_with("\x1b[201~"));
        assert!(!out.contains('\u{9b}'));
        let c1 = String::from_utf8(paste_bytes("a\u{9b}201~b", true)).unwrap();
        assert_eq!(c1, "\x1b[200~a201~b\x1b[201~");
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn shell_output_and_osc52() {
        let mut s = Session::spawn(
            (40, 5),
            (8, 18),
            None,
            None,
            Palette::default(),
            Osc52::Allow,
            || {},
        )
        .unwrap();
        s.input(b"printf 'privet\\033]52;c;0J/RgNC40LLQtdGC\\a'; echo\r".to_vec());
        let start = Instant::now();
        let mut copied = None;
        while start.elapsed() < Duration::from_secs(5) && copied.is_none() {
            for e in s.poll() {
                if let TermEvent::Copy(t) = e {
                    copied = Some(t);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(copied.as_deref(), Some("Привет"), "OSC 52 — в буфер обмена");
        let mut glyphs = Glyphs::new("DejaVu Sans Mono", 14.0, || {});
        let pm = s.render(&mut glyphs, (400, 120), true, true).unwrap();
        assert_eq!((pm.width(), pm.height()), (400, 120));
        s.resize((60, 8));
        assert_eq!(s.screen_lines(), 8);
    }

    #[test]
    fn osc52_size_limit_and_setting() {
        let big = "x".repeat(OSC52_MAX + 1);
        assert!(accept_osc52(Osc52::Allow, "small"));
        assert!(accept_osc52(Osc52::Allow, &"x".repeat(OSC52_MAX)));
        assert!(!accept_osc52(Osc52::Allow, &big));
        assert!(!accept_osc52(Osc52::Off, "small"));
    }

    /// Setting off: the OSC 52 request reaches the session but never
    /// becomes a clipboard copy.
    #[test]
    fn osc52_off_ignores_clipboard_store() {
        let mut s = Session::spawn(
            (40, 5),
            (8, 18),
            None,
            None,
            Palette::default(),
            Osc52::Off,
            || {},
        )
        .unwrap();
        // The marker must come only from the command's output, so it cannot
        // be already on screen in the echoed line as typed: `DO''NE` is
        // concatenated to `DONE` by the shell, but the echo shows the quotes.
        // The `\n` after the OSC puts the marker on a line of its own.
        s.input(b"printf 'privet\\033]52;c;0J/RgNC40LLQtdGC\\a\\n'; echo DO''NE\r".to_vec());
        let start = Instant::now();
        let mut done = false;
        while start.elapsed() < Duration::from_secs(5) && !done {
            for e in s.poll() {
                if let TermEvent::Copy(t) = e {
                    panic!("OSC 52 ignored the setting: {t}");
                }
            }
            s.select_all();
            done = s
                .selection_text()
                .is_some_and(|t| t.lines().any(|l| l.trim() == "DONE"));
            if !done {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(done, "the marker never appeared on the screen");
        // One more poll: the clipboard event is queued before the marker
        // reaches the grid, but it can arrive just after we saw the marker.
        for e in s.poll() {
            if let TermEvent::Copy(t) = e {
                panic!("OSC 52 ignored the setting: {t}");
            }
        }
    }
}
