//! Окно терминала (`mimshel terminal [ПАПКА]`) — отдельный процесс, не служба:
//! упадёт терминал — док и строка меню останутся (урок 12). Внутри — сеансы
//! `mimshel_terminal` (по одному на вкладку), здесь — окно и руки:
//! - вкладки, как в macOS: полоска сверху, когда их две и больше;
//!   Ctrl+Shift+T — новая (в папке текущей), Ctrl+Shift+W — закрыть,
//!   Ctrl+PageUp/PageDown и Ctrl+(Shift+)Tab — соседняя, Alt+1…9 — по номеру;
//!   мышью — щелчок, крестик или средняя кнопка (закрыть), «+» (новая);
//! - клавиши → программе; Ctrl+Shift+C/V — копировать и вставить,
//!   Shift+Insert — вставить выделенное, Shift+PageUp/PageDown — история;
//! - мышь: тянуть — выделить (двойной щелчок — слово, тройной — строку),
//!   выделенное сразу в «выделение» (вставка средней кнопкой), колёсико —
//!   история; программе с мышью (htop, mc) — её щелчки;
//! - OSC 52 (Claude Code, tmux, vim по SSH) — в буфер обмена;
//! - заголовок вкладки и окна — от программы, размер сетки — по окну.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use mimshel_core::input::{Key, KeyPress, latin};
use mimshel_render::Canvas;
use mimshel_terminal::screen::{PAD, cell_at, grid_size};
use mimshel_terminal::{Glyphs, Palette, Session, TermEvent, key_bytes};
use mimshel_text::{Fonts, Style};
use mimshel_x11::{Display, Event};
use tiny_skia::{Color, Paint, PathBuilder, PixmapPaint, Rect, Stroke, Transform};

/// Класс окна — по нему док и строка меню узнают терминал.
const WINDOW_CLASS: &str = "mimshel-terminal";
const TITLE: &str = "Терминал";
/// Размер шрифта в точках и сетка нового окна.
const FONT_SIZE: f32 = 14.0;
const START: (usize, usize) = (100, 30);
/// Двойной и тройной щелчок — не дольше этого между нажатиями.
const MULTI_CLICK: Duration = Duration::from_millis(400);
/// Полоска вкладок (в точках): высота, наибольшая ширина вкладки, кнопка «+»,
/// крестик у вкладки.
const BAR_H: f32 = 30.0;
const TAB_MAX_W: f32 = 240.0;
const PLUS_W: f32 = 34.0;
const CLOSE_W: f32 = 22.0;

const BAR_BG: [u8; 3] = [0x18, 0x18, 0x25];
const TAB_BG: [u8; 3] = [0x1e, 0x1e, 0x2e];
const TAB_HOVER: [u8; 3] = [0x26, 0x26, 0x38];
const LINE: [u8; 3] = [0x31, 0x32, 0x44];
const TAB_TEXT: [u8; 4] = [0xcd, 0xd6, 0xf4, 0xff];
const TAB_DIM: [u8; 4] = [0x8c, 0x8f, 0xa8, 0xff];

enum Wake {
    X(Event),
    /// Сеанс вкладки с этим номером сообщил что-то новое.
    Term(u64),
}

/// Запустить окно терминала. Код выхода: 0 — оболочки закончили или окно закрыли.
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

struct Tab {
    id: u64,
    session: Session,
    title: String,
}

/// Куда попала мышь на полоске вкладок.
#[derive(Clone, Copy, PartialEq, Debug)]
enum BarHit {
    Tab(usize),
    Close(usize),
    Plus,
}

/// Вкладки окна и как их рисовать.
struct Tabs {
    list: Vec<Tab>,
    active: usize,
    next_id: u64,
    cell: (u32, u32),
    tx: Sender<Wake>,
    /// Шрифт подписей вкладок (обычный, не моноширинный) — грузится, когда
    /// вкладок впервые стало две.
    ui_fonts: Option<Fonts>,
    hover: Option<BarHit>,
}

impl Tabs {
    /// Открыть вкладку в папке `dir` с сеткой `grid`.
    fn open(&mut self, dir: Option<PathBuf>, grid: (usize, usize)) -> mimshel_x11::Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        let wake = self.tx.clone();
        let session = Session::spawn(grid, self.cell, dir, Palette::default(), move || {
            let _ = wake.send(Wake::Term(id));
        })
        .map_err(|e| format!("оболочка не запустилась: {e}"))?;
        self.list.push(Tab {
            id,
            session,
            title: TITLE.to_string(),
        });
        self.active = self.list.len() - 1;
        Ok(())
    }

    fn active(&self) -> &Tab {
        &self.list[self.active]
    }

    fn find(&mut self, id: u64) -> Option<usize> {
        self.list.iter().position(|t| t.id == id)
    }

    fn close(&mut self, i: usize) {
        if i < self.list.len() {
            self.list.remove(i);
            if self.active >= self.list.len() || self.active > i {
                self.active = self.active.saturating_sub(1);
            }
        }
    }

    /// Высота полоски вкладок в пикселях (одна вкладка — полоски нет).
    fn bar_h(&self, scale: f32) -> u32 {
        if self.list.len() > 1 {
            (BAR_H * scale).round() as u32
        } else {
            0
        }
    }

    /// Ширина вкладки в пикселях при ширине окна `w`.
    fn tab_w(&self, w: u32, scale: f32) -> f32 {
        let room = w as f32 - PLUS_W * scale;
        (room / self.list.len().max(1) as f32).min(TAB_MAX_W * scale)
    }

    fn bar_hit(&self, x: f32, w: u32, scale: f32) -> Option<BarHit> {
        let tw = self.tab_w(w, scale);
        let i = (x / tw).floor() as usize;
        if i < self.list.len() {
            let in_tab = x - i as f32 * tw;
            return Some(if in_tab >= tw - CLOSE_W * scale {
                BarHit::Close(i)
            } else {
                BarHit::Tab(i)
            });
        }
        let plus = self.list.len() as f32 * tw;
        (x >= plus && x < plus + PLUS_W * scale).then_some(BarHit::Plus)
    }

    /// Кадр окна `w`×`h` пикселей: полоска вкладок и активная вкладка.
    fn render(
        &mut self,
        glyphs: &mut Glyphs,
        (w, h): (u32, u32),
        scale: f32,
        focused: bool,
    ) -> Option<tiny_skia::Pixmap> {
        let bar = self.bar_h(scale);
        let term = self
            .active()
            .session
            .render(glyphs, (w, h.saturating_sub(bar)), focused)?;
        if bar == 0 {
            return Some(term);
        }
        let mut canvas = Canvas::with_scale(w, h, 1.0)?;
        let fill = |c: &mut Canvas, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]| {
            if let Some(r) = Rect::from_xywh(x, y, w, h) {
                let mut p = Paint::default();
                p.set_color(Color::from_rgba8(rgb[0], rgb[1], rgb[2], 0xff));
                c.fill_rect(r, &p, Transform::identity(), None);
            }
        };
        let bh = bar as f32;
        fill(&mut canvas, 0.0, 0.0, w as f32, bh, BAR_BG);
        canvas.draw_pixmap(
            0,
            bar as i32,
            term.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        let fonts = self
            .ui_fonts
            .get_or_insert_with(|| Fonts::quick("Inter").unwrap_or_else(|| Fonts::new("Inter")));
        let tw = (w as f32 - PLUS_W * scale) / self.list.len() as f32;
        let tw = tw.min(TAB_MAX_W * scale);
        let style = |active: bool| Style {
            size: 12.5 * scale,
            color: if active { TAB_TEXT } else { TAB_DIM },
            weight: if active { 600 } else { 400 },
        };
        for (i, tab) in self.list.iter().enumerate() {
            let x = i as f32 * tw;
            let active = i == self.active;
            let hovered = matches!(self.hover, Some(BarHit::Tab(j) | BarHit::Close(j)) if j == i);
            if active {
                fill(&mut canvas, x, 0.0, tw, bh, TAB_BG);
            } else if hovered {
                fill(&mut canvas, x, 0.0, tw, bh, TAB_HOVER);
            }
            // Черта между вкладками.
            fill(
                &mut canvas,
                x + tw - 1.0,
                6.0 * scale,
                1.0,
                bh - 12.0 * scale,
                LINE,
            );
            let st = style(active);
            let text_w = tw - (12.0 + CLOSE_W) * scale;
            let ty = ((bh - st.line_height()) / 2.0).round();
            fonts.draw(
                &mut canvas,
                &tab.title,
                &st,
                x + 12.0 * scale,
                ty,
                Some(text_w),
            );
            // Крестик: у активной и у той, над которой мышь.
            if active || hovered {
                let cx = x + tw - CLOSE_W * scale / 2.0 - 4.0 * scale;
                let cy = bh / 2.0;
                let r = 3.5 * scale;
                let mut pb = PathBuilder::new();
                pb.move_to(cx - r, cy - r);
                pb.line_to(cx + r, cy + r);
                pb.move_to(cx + r, cy - r);
                pb.line_to(cx - r, cy + r);
                if let Some(path) = pb.finish() {
                    let mut p = Paint::default();
                    let c = if self.hover == Some(BarHit::Close(i)) {
                        TAB_TEXT
                    } else {
                        TAB_DIM
                    };
                    p.set_color(Color::from_rgba8(c[0], c[1], c[2], c[3]));
                    p.anti_alias = true;
                    let stroke = Stroke {
                        width: 1.4 * scale,
                        ..Stroke::default()
                    };
                    canvas.stroke_path(&path, &p, &stroke, Transform::identity(), None);
                }
            }
        }
        // «+» — новая вкладка.
        let px = self.list.len() as f32 * tw + PLUS_W * scale / 2.0;
        if self.hover == Some(BarHit::Plus) {
            fill(
                &mut canvas,
                self.list.len() as f32 * tw,
                0.0,
                PLUS_W * scale,
                bh,
                TAB_HOVER,
            );
        }
        let mut pb = PathBuilder::new();
        let r = 5.0 * scale;
        pb.move_to(px - r, bh / 2.0);
        pb.line_to(px + r, bh / 2.0);
        pb.move_to(px, bh / 2.0 - r);
        pb.line_to(px, bh / 2.0 + r);
        if let Some(path) = pb.finish() {
            let mut p = Paint::default();
            p.set_color(Color::from_rgba8(TAB_DIM[0], TAB_DIM[1], TAB_DIM[2], 0xff));
            p.anti_alias = true;
            let stroke = Stroke {
                width: 1.5 * scale,
                ..Stroke::default()
            };
            canvas.stroke_path(&path, &p, &stroke, Transform::identity(), None);
        }
        Some(canvas.into_pixmap())
    }
}

/// Сочетание для вкладок — что сделать (или `None`: нажатие для программы).
enum TabKey {
    New,
    Close,
    Next,
    Prev,
    Select(usize),
}

fn tab_key(k: &KeyPress) -> Option<TabKey> {
    let letter = match k.key {
        Key::Char(c) => Some(latin(c)),
        _ => None,
    };
    match k.key {
        _ if k.ctrl && k.shift && letter == Some('t') => Some(TabKey::New),
        _ if k.ctrl && k.shift && letter == Some('w') => Some(TabKey::Close),
        Key::PageDown if k.ctrl && !k.shift => Some(TabKey::Next),
        Key::PageUp if k.ctrl && !k.shift => Some(TabKey::Prev),
        Key::Tab if k.ctrl => Some(if k.shift { TabKey::Prev } else { TabKey::Next }),
        Key::Char(c @ '1'..='9') if k.alt && !k.ctrl => {
            Some(TabKey::Select(c as usize - '1' as usize))
        }
        _ => None,
    }
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
    let mut tabs = Tabs {
        list: Vec::new(),
        active: 0,
        next_id: 0,
        cell,
        tx: tx.clone(),
        ui_fonts: None,
        hover: None,
    };
    tabs.open(dir, START)?;
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
    // Сетка под область без полоски вкладок.
    let grid_for = |size: (u32, u32), bar: u32| grid_size(size.0, size.1.saturating_sub(bar), cell);
    // Точка окна (точки) → клетка и правая ли половина клетки (под полоской).
    let at = |x: i16, y: i16, grid: (usize, usize), bar: u32| {
        let (x, y) = (f32::from(x) * scale, f32::from(y) * scale - bar as f32);
        let c = cell_at(x, y, cell, grid);
        let right = (x - PAD as f32) - (c.0 as f32 * cell.0 as f32) > cell.0 as f32 / 2.0;
        (c, right)
    };
    // Число вкладок стало 1 или 2 — полоска появилась или ушла: сетка у всех.
    let regrid = |tabs: &mut Tabs, size: (u32, u32)| {
        let grid = grid_for(size, tabs.bar_h(scale));
        for t in &mut tabs.list {
            t.session.resize(grid);
        }
    };
    let mut shown_title = String::new();

    'run: while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        for wake in batch {
            let bar = tabs.bar_h(scale);
            match wake {
                Wake::Term(id) => {
                    let Some(i) = tabs.find(id) else {
                        continue;
                    };
                    for e in tabs.list[i].session.poll() {
                        match e {
                            TermEvent::Redraw => dirty |= i == tabs.active,
                            TermEvent::Title(t) => {
                                tabs.list[i].title = t;
                                dirty = true;
                            }
                            TermEvent::ResetTitle => {
                                tabs.list[i].title = TITLE.to_string();
                                dirty = true;
                            }
                            TermEvent::Copy(text) => mimshel_x11::copy_text(text, false),
                            TermEvent::Bell => {}
                            TermEvent::Exit => {
                                tabs.close(i);
                                if tabs.list.is_empty() {
                                    break 'run;
                                }
                                regrid(&mut tabs, size);
                                dirty = true;
                                break;
                            }
                        }
                    }
                }
                Wake::X(Event::Close { window }) if window == me => break 'run,
                Wake::X(Event::Expose { window }) if window == me => dirty = true,
                Wake::X(Event::Configure { window }) if window == me => {
                    if let Some((w, h)) = win.size(&d) {
                        size = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
                        regrid(&mut tabs, size);
                        dirty = true;
                    }
                }
                Wake::X(Event::Key(k)) => {
                    if let Some(action) = tab_key(&k) {
                        match action {
                            TabKey::New => {
                                let cwd = tabs.active().session.cwd();
                                let grid = tabs.active().session.size();
                                tabs.open(cwd, grid)?;
                                regrid(&mut tabs, size);
                            }
                            TabKey::Close => {
                                tabs.close(tabs.active);
                                if tabs.list.is_empty() {
                                    break 'run;
                                }
                                regrid(&mut tabs, size);
                            }
                            TabKey::Next => tabs.active = (tabs.active + 1) % tabs.list.len(),
                            TabKey::Prev => {
                                tabs.active = (tabs.active + tabs.list.len() - 1) % tabs.list.len();
                            }
                            TabKey::Select(n) => {
                                if n < tabs.list.len() {
                                    tabs.active = n;
                                }
                            }
                        }
                        dirty = true;
                        continue;
                    }
                    let session = &tabs.active().session;
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
                    // Полоска вкладок.
                    if f32::from(y) * scale < bar as f32 {
                        match (tabs.bar_hit(f32::from(x) * scale, size.0, scale), button) {
                            (Some(BarHit::Tab(i)), 2) | (Some(BarHit::Close(i)), 1 | 2) => {
                                tabs.close(i);
                                if tabs.list.is_empty() {
                                    break 'run;
                                }
                                regrid(&mut tabs, size);
                            }
                            (Some(BarHit::Tab(i)), 1) => tabs.active = i,
                            (Some(BarHit::Plus), 1) => {
                                let cwd = tabs.active().session.cwd();
                                let grid = tabs.active().session.size();
                                tabs.open(cwd, grid)?;
                                regrid(&mut tabs, size);
                            }
                            _ => {}
                        }
                        dirty = true;
                        continue;
                    }
                    let session = &tabs.active().session;
                    let grid = session.size();
                    let (c, right) = at(x, y, grid, bar);
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
                Wake::X(Event::Motion { window, x, y }) if window == me => {
                    let hover = (f32::from(y) * scale < bar as f32)
                        .then(|| tabs.bar_hit(f32::from(x) * scale, size.0, scale))
                        .flatten();
                    if hover != tabs.hover {
                        tabs.hover = hover;
                        dirty = true;
                    }
                    let session = &tabs.active().session;
                    match press {
                        Press::Selecting => {
                            let (c, right) = at(x, y, session.size(), bar);
                            session.select_update(c, right);
                            dirty = true;
                        }
                        Press::Program(b) => {
                            let (c, _) = at(x, y, session.size(), bar);
                            session.mouse_report(b, c, true, true);
                        }
                        Press::None => {}
                    }
                }
                Wake::X(Event::Leave { window }) if window == me => {
                    if tabs.hover.take().is_some() {
                        dirty = true;
                    }
                }
                Wake::X(Event::Release {
                    window,
                    x,
                    y,
                    button,
                }) if window == me => {
                    let session = &tabs.active().session;
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
                            let (c, _) = at(x, y, session.size(), bar);
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
            let title = tabs.active().title.clone();
            if title != shown_title {
                win.set_title(&d, &title)?;
                shown_title = title;
            }
            let focused = d.active_window() == Some(me);
            if let Some(frame) = tabs.render(&mut glyphs, size, scale, focused) {
                win.present(&d, &frame)?;
                d.sync()?;
            }
        }
    }
    win.destroy(&d)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, ctrl: bool, shift: bool, alt: bool) -> KeyPress {
        KeyPress {
            key,
            ctrl,
            shift,
            alt,
            logo: false,
        }
    }

    #[test]
    fn tab_shortcuts() {
        assert!(matches!(
            tab_key(&key(Key::Char('T'), true, true, false)),
            Some(TabKey::New)
        ));
        assert!(
            matches!(
                tab_key(&key(Key::Char('Е'), true, true, false)),
                Some(TabKey::New)
            ),
            "в русской раскладке — по месту клавиши"
        );
        assert!(matches!(
            tab_key(&key(Key::PageDown, true, false, false)),
            Some(TabKey::Next)
        ));
        assert!(matches!(
            tab_key(&key(Key::Tab, true, true, false)),
            Some(TabKey::Prev)
        ));
        assert!(matches!(
            tab_key(&key(Key::Char('3'), false, false, true)),
            Some(TabKey::Select(2))
        ));
        assert!(
            tab_key(&key(Key::Char('t'), true, false, false)).is_none(),
            "Ctrl+T без Shift — программе (bash: поменять буквы местами)"
        );
        assert!(tab_key(&key(Key::PageUp, false, true, false)).is_none());
    }
}
