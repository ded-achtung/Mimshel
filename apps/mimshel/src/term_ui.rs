//! Окно терминала (`mimshel terminal [ПАПКА]`) — отдельный процесс, не служба:
//! упадёт терминал — док и строка меню останутся (урок 12). Внутри — сеанс
//! `mimshel_terminal`, здесь — окно и руки:
//! - клавиши → программе; Ctrl+Shift+C/V — копировать и вставить,
//!   Shift+Insert — вставить выделенное, Shift+PageUp/PageDown — история;
//! - мышь: тянуть — выделить (двойной щелчок — слово, тройной — строку),
//!   выделенное сразу в «выделение» (вставка средней кнопкой), колёсико —
//!   история; программе с мышью (htop, mc) — её щелчки;
//! - OSC 52 (Claude Code, tmux, vim по SSH) — в буфер обмена;
//! - заголовок окна — от программы, размер сетки — по окну.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mimshel_core::input::{Key, latin};
use mimshel_terminal::screen::{PAD, cell_at, grid_size};
use mimshel_terminal::{Glyphs, Palette, Session, TermEvent, key_bytes};
use mimshel_x11::{Display, Event};

/// Класс окна — по нему док и строка меню узнают терминал.
const WINDOW_CLASS: &str = "mimshel-terminal";
const TITLE: &str = "Терминал";
/// Размер шрифта в точках и сетка нового окна.
const FONT_SIZE: f32 = 14.0;
const START: (usize, usize) = (100, 30);
/// Двойной и тройной щелчок — не дольше этого между нажатиями.
const MULTI_CLICK: Duration = Duration::from_millis(400);

enum Wake {
    X(Event),
    /// Сеанс сообщил что-то новое.
    Term,
}

/// Запустить окно терминала. Код выхода: 0 — оболочка закончила или окно закрыли.
pub fn run(dir: Option<PathBuf>) -> i32 {
    match terminal(dir) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Терминал: {e}");
            1
        }
    }
}

/// Моноширинный шрифт системы (`fc-match monospace`), иначе DejaVu Sans Mono.
fn mono_family() -> String {
    std::process::Command::new("fc-match")
        .args(["-f", "%{family}", "monospace"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split(',').next().map(|f| f.trim().to_string()))
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| "DejaVu Sans Mono".into())
}

/// Что сейчас делает левая кнопка мыши.
#[derive(Clone, Copy, PartialEq)]
enum Press {
    None,
    /// Выделяем текст.
    Selecting,
    /// Кнопку нажали над программой, которая сама хочет мышь.
    Program(u8),
}

fn terminal(dir: Option<PathBuf>) -> mimshel_x11::Result<()> {
    let d = Arc::new(Display::connect()?);
    let scale = d.scale();
    let mut glyphs = Glyphs::new(&mono_family(), (FONT_SIZE * scale).round());
    let cell = glyphs.cell;
    let px = |(cols, rows): (usize, usize)| {
        (
            cols as u32 * cell.0 + 2 * PAD,
            rows as u32 * cell.1 + 2 * PAD,
        )
    };
    let to_pt = |(w, h): (u32, u32)| {
        (
            (w as f32 / scale).ceil() as u32,
            (h as f32 / scale).ceil() as u32,
        )
    };
    let win = d.create_app_window(WINDOW_CLASS, TITLE, to_pt(px(START)), to_pt(px((20, 4))))?;

    let (tx, rx) = mpsc::channel::<Wake>();
    let wake = tx.clone();
    let mut session = Session::spawn(START, cell, dir, Palette::default(), move || {
        let _ = wake.send(Wake::Term);
    })
    .map_err(|e| format!("оболочка не запустилась: {e}"))?;
    {
        let d = Arc::clone(&d);
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = d.next_event() {
                if tx.send(Wake::X(ev)).is_err() {
                    break;
                }
            }
        });
    }
    win.show(&d)?;

    let me = win.id();
    let mut size = px(START);
    let mut dirty = true;
    let mut press = Press::None;
    let mut last_click: Option<(Instant, (usize, usize), u8)> = None;
    // Точка окна (точки) → клетка и правая ли половина клетки.
    let at = |x: i16, y: i16, grid: (usize, usize)| {
        let (x, y) = (f32::from(x) * scale, f32::from(y) * scale);
        let c = cell_at(x, y, cell, grid);
        let right = (x - PAD as f32) - (c.0 as f32 * cell.0 as f32) > cell.0 as f32 / 2.0;
        (c, right)
    };

    'run: while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        for wake in batch {
            match wake {
                Wake::Term => {
                    for e in session.poll() {
                        match e {
                            TermEvent::Redraw => dirty = true,
                            TermEvent::Title(t) => win.set_title(&d, &t)?,
                            TermEvent::ResetTitle => win.set_title(&d, TITLE)?,
                            TermEvent::Copy(text) => mimshel_x11::copy_text(text, false),
                            TermEvent::Bell => {}
                            TermEvent::Exit => break 'run,
                        }
                    }
                }
                Wake::X(Event::Close { window }) if window == me => break 'run,
                Wake::X(Event::Expose { window }) if window == me => dirty = true,
                Wake::X(Event::Configure { window }) if window == me => {
                    if let Some((w, h)) = win.size(&d) {
                        size = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
                        session.resize(grid_size(size.0, size.1, cell));
                        dirty = true;
                    }
                }
                Wake::X(Event::Key(k)) => {
                    let letter = match k.key {
                        Key::Char(c) => Some(latin(c)),
                        _ => None,
                    };
                    if k.ctrl && k.shift && letter == Some('c') {
                        if let Some(text) = session.selection_text() {
                            mimshel_x11::copy_text(text, false);
                        }
                    } else if k.ctrl && k.shift && letter == Some('v') {
                        if let Some(text) = mimshel_x11::clipboard_text(false) {
                            session.paste(&text);
                        }
                    } else if k.shift && k.key == Key::Insert {
                        if let Some(text) = mimshel_x11::clipboard_text(true) {
                            session.paste(&text);
                        }
                    } else if k.shift && matches!(k.key, Key::PageUp | Key::PageDown) {
                        session.scroll_page(k.key == Key::PageUp);
                        dirty = true;
                    } else if let Some(bytes) = key_bytes(&k, session.mode()) {
                        session.input(bytes);
                        dirty = true;
                    }
                }
                Wake::X(Event::Button {
                    window,
                    x,
                    y,
                    button,
                }) if window == me => {
                    let grid = session.size();
                    let (c, right) = at(x, y, grid);
                    match button {
                        4 | 5 => session.wheel(button == 4, c),
                        b if session.mouse_wanted() => {
                            session.mouse_report(b - 1, c, true, false);
                            press = Press::Program(b - 1);
                        }
                        1 => {
                            let clicks = match last_click {
                                Some((t, at, n)) if t.elapsed() < MULTI_CLICK && at == c => {
                                    n % 3 + 1
                                }
                                _ => 1,
                            };
                            last_click = Some((Instant::now(), c, clicks));
                            session.select_start(c, clicks, right);
                            press = Press::Selecting;
                        }
                        2 => {
                            if let Some(text) = mimshel_x11::clipboard_text(true) {
                                session.paste(&text);
                            }
                        }
                        _ => {}
                    }
                    dirty = true;
                }
                Wake::X(Event::Motion { window, x, y }) if window == me => match press {
                    Press::Selecting => {
                        let (c, right) = at(x, y, session.size());
                        session.select_update(c, right);
                        dirty = true;
                    }
                    Press::Program(b) => {
                        let (c, _) = at(x, y, session.size());
                        session.mouse_report(b, c, true, true);
                    }
                    Press::None => {}
                },
                Wake::X(Event::Release {
                    window,
                    x,
                    y,
                    button,
                }) if window == me => {
                    match press {
                        Press::Selecting if button == 1 => {
                            // Выделенное — сразу в «выделение» (вставка средней кнопкой).
                            match session.selection_text() {
                                Some(text) => mimshel_x11::copy_text(text, true),
                                // Просто щелчок — выделения нет.
                                None => session.clear_selection(),
                            }
                            dirty = true;
                        }
                        Press::Program(b) => {
                            let (c, _) = at(x, y, session.size());
                            session.mouse_report(b, c, false, false);
                        }
                        _ => {}
                    }
                    press = Press::None;
                }
                Wake::X(_) => {}
            }
        }
        if dirty {
            dirty = false;
            let focused = d.active_window() == Some(me);
            if let Some(frame) = session.render(&mut glyphs, size, focused) {
                win.present(&d, &frame)?;
                d.sync()?;
            }
        }
    }
    win.destroy(&d)?;
    Ok(())
}
