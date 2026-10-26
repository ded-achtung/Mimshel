//! Terminal window (`mimshel terminal [DIR]`) — a separate process, not the service:
//! if the terminal crashes, the dock and menu bar stay (lesson 12). Inside are
//! `mimshel_terminal` sessions (one per tab); this file is the window and input:
//! - tabs like macOS: a tab strip on top when there are two or more;
//!   Ctrl+Shift+T — new (in the current tab's folder), Ctrl+Shift+W — close,
//!   Ctrl+PageUp/PageDown and Ctrl+(Shift+)Tab — neighbour, Alt+1…9 — by number;
//!   mouse — click, the cross or middle button (close), «+» (new);
//! - keys go to the program; Ctrl+Shift+C/V — copy and paste,
//!   Shift+Insert — paste the selection, Shift+PageUp/PageDown — scrollback;
//! - mouse: drag to select (double click — word, triple — line), the
//!   selection goes straight to PRIMARY (middle-click paste), wheel —
//!   scrollback; programs that want the mouse (htop, mc) get their clicks;
//! - OSC 52 (Claude Code, tmux, vim over SSH) — to the clipboard;
//! - tab and window titles come from the program, the grid follows the window;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use mimshel_core::config::Osc52;
use mimshel_core::input::{Key, KeyPress, latin};
use mimshel_dbus::menu_export::MenuSpec;
use mimshel_render::Canvas;
use mimshel_terminal::screen::{PAD, cell_at, grid_size};
use mimshel_terminal::{Glyphs, Palette, Session, TermEvent, key_bytes};
use mimshel_text::{Fonts, Style};
use mimshel_x11::{Display, Event};
use tiny_skia::{Color, Paint, PathBuilder, PixmapPaint, Rect, Stroke, Transform};

/// Window class — the dock and menu bar recognise the terminal by it.
const WINDOW_CLASS: &str = "mimshel-terminal";
const TITLE: &str = "Терминал";
/// Font size in points and the grid of a new window.
const FONT_SIZE: f32 = 14.0;
const START: (usize, usize) = (100, 30);
/// Double and triple click — no longer than this between presses.
const MULTI_CLICK: Duration = Duration::from_millis(400);
/// Tab strip (in points): height, widest tab, the «+» button,
/// the close cross of a tab.
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
    /// The session of the tab with this id has something new.
    Term(u64),
    /// A menu item was chosen in the top menu bar (item id).
    Menu(i32),
    /// All system fonts have loaded (emoji, CJK) — redraw missing characters.
    Fonts,
    /// Cursor blink tick.
    Blink,
}

/// Cursor blink half-period.
const BLINK: Duration = Duration::from_millis(530);

/// What to open: `[DIR]` or `-e PROGRAM [ARGS…]` (also `-x`, `--`, as other
/// terminals accept — programs with `Terminal=true` are launched this way).
pub fn parse_args(args: &[String]) -> (Option<PathBuf>, Option<Vec<String>>) {
    match args.first().map(String::as_str) {
        Some("-e" | "-x" | "--") => (None, Some(args[1..].to_vec()).filter(|c| !c.is_empty())),
        Some(dir) => (Some(PathBuf::from(dir)), None),
        None => (None, None),
    }
}

/// Run a terminal window. Exit code 0: the shells finished or the window was closed.
pub fn run(dir: Option<PathBuf>, command: Option<Vec<String>>) -> i32 {
    match terminal(dir, command) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Терминал: {e}");
            1
        }
    }
}

/// Open a web address in the default browser (detached).
fn open_url(url: &str) {
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url);
    let _ = crate::ui::spawn_detached(cmd);
}

/// The system monospace font (`fc-match monospace`), else DejaVu Sans Mono.
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

/// What the left mouse button is doing now.
#[derive(Clone, Copy, PartialEq)]
enum Press {
    None,
    /// Selecting text.
    Selecting,
    /// Pressed over a program that wants the mouse itself.
    Program(u8),
    /// Dragging the scrollbar thumb; the number — where in the thumb it was
    /// grabbed (pixels from its top).
    Scrollbar(f32),
}

/// Scrollbar on the right of the terminal (pixels at scale 1): width when
/// idle and when hovered or dragged, and how far from the edge the mouse
/// still counts as «on the bar».
const SB_THIN: f32 = 4.0;
const SB_WIDE: f32 = 9.0;
const SB_HIT: f32 = 16.0;
const SB_MIN_THUMB: f32 = 24.0;

/// Scrollbar thumb in a track `top..top+len` (pixels): its top and height for
/// `history` lines above a `lines`-line screen scrolled up by `offset`.
fn thumb(top: f32, len: f32, history: usize, lines: usize, offset: usize) -> (f32, f32) {
    let total = (history + lines).max(1) as f32;
    let h = (len * lines as f32 / total).max(SB_MIN_THUMB).min(len);
    // offset 0 — the thumb at the bottom; offset = history — at the top.
    let free = len - h;
    let y = if history == 0 {
        top + free
    } else {
        top + free * (1.0 - offset as f32 / history as f32)
    };
    (y, h)
}

/// The reverse: the scroll offset for the thumb top at `y`.
fn offset_for(y: f32, top: f32, len: f32, thumb_h: f32, history: usize) -> usize {
    let free = (len - thumb_h).max(1.0);
    let pos = ((y - top) / free).clamp(0.0, 1.0);
    ((1.0 - pos) * history as f32).round() as usize
}

struct Tab {
    id: u64,
    session: Session,
    title: String,
}

/// Where the mouse is on the tab strip.
#[derive(Clone, Copy, PartialEq, Debug)]
enum BarHit {
    Tab(usize),
    Close(usize),
    Plus,
}

/// The window's tabs and how to draw them.
struct Tabs {
    list: Vec<Tab>,
    active: usize,
    next_id: u64,
    cell: (u32, u32),
    tx: Sender<Wake>,
    /// Whether programs may put text into the clipboard (OSC 52).
    osc52: Osc52,
    /// Tab label font (regular, not monospace) — loaded the first time
    /// there are two tabs.
    ui_fonts: Option<Fonts>,
    hover: Option<BarHit>,
    /// The mouse is over the scrollbar or drags it — draw it wide.
    sb_active: bool,
}

impl Tabs {
    /// Open a tab in folder `dir` with grid `grid` running the shell.
    fn open(&mut self, dir: Option<PathBuf>, grid: (usize, usize)) -> mimshel_x11::Result<()> {
        self.open_with(dir, grid, None)
    }

    /// Open a tab running `command` (program and arguments) instead of the shell.
    fn open_with(
        &mut self,
        dir: Option<PathBuf>,
        grid: (usize, usize),
        command: Option<Vec<String>>,
    ) -> mimshel_x11::Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        let wake = self.tx.clone();
        let title = command
            .as_ref()
            .and_then(|c| c.first())
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            .unwrap_or_else(|| TITLE.to_string());
        let session = Session::spawn(
            grid,
            self.cell,
            dir,
            command,
            Palette::default(),
            self.osc52,
            move || {
                let _ = wake.send(Wake::Term(id));
            },
        )
        .map_err(|e| format!("оболочка не запустилась: {e}"))?;
        self.list.push(Tab { id, session, title });
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

    /// Tab strip height in pixels (a single tab has no strip).
    fn bar_h(&self, scale: f32) -> u32 {
        if self.list.len() > 1 {
            (BAR_H * scale).round() as u32
        } else {
            0
        }
    }

    /// Tab width in pixels for window width `w`.
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

    /// Window frame `w`×`h` pixels: the tab strip and the active tab.
    fn render(
        &mut self,
        glyphs: &mut Glyphs,
        (w, h): (u32, u32),
        scale: f32,
        focused: bool,
        cursor_on: bool,
    ) -> Option<tiny_skia::Pixmap> {
        let bar = self.bar_h(scale);
        let session = &self.active().session;
        let mut term = session.render(glyphs, (w, h.saturating_sub(bar)), focused, cursor_on)?;
        // Scrollback scrollbar on the right (only when there is history).
        let (history, offset) = session.history();
        if history > 0 {
            let len = term.height() as f32 - 4.0 * scale;
            let (y, th) = thumb(2.0 * scale, len, history, session.size().1, offset);
            let bw = if self.sb_active { SB_WIDE } else { SB_THIN } * scale;
            let alpha = if self.sb_active || offset > 0 {
                0xb0
            } else {
                0x60
            };
            if let Some(r) = rounded_rect(w as f32 - bw - 3.0 * scale, y, bw, th, bw / 2.0) {
                let mut p = Paint::default();
                p.set_color(Color::from_rgba8(0xcd, 0xd6, 0xf4, alpha));
                p.anti_alias = true;
                term.fill_path(
                    &r,
                    &p,
                    tiny_skia::FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
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
            // Divider between tabs.
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
            // Close cross: on the active tab and the hovered one.
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
        // «+» — new tab.
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

/// Rounded rectangle path (the scrollbar thumb).
fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// What to do: the same from the top menu and from the keyboard.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cmd {
    NewWindow,
    NewTab,
    CloseTab,
    CloseWindow,
    Copy,
    Paste,
    SelectAll,
    ClearHistory,
    Bigger,
    Smaller,
    NormalSize,
    NextTab,
    PrevTab,
    SelectTab(usize),
    /// Scrollback: by lines (up — positive), to the top or bottom.
    ScrollLines(i32),
    ScrollTop,
    ScrollBottom,
}

/// The window menu in the top menu bar, like macOS Terminal. Item ids
/// come back on click ([`menu_cmd`]).
fn menus(lang: mimshel_core::i18n::Language) -> Vec<MenuSpec> {
    let tr = |k: &'static str| mimshel_core::i18n::t(lang, k);
    vec![
        MenuSpec::menu(
            1,
            tr("menu.shell"),
            vec![
                MenuSpec::item(101, tr("term.new_window")),
                MenuSpec::item(102, &format!("{}    Ctrl+Shift+T", tr("term.new_tab"))),
                MenuSpec::separator(103),
                MenuSpec::item(104, &format!("{}    Ctrl+Shift+W", tr("term.close_tab"))),
                MenuSpec::item(105, tr("term.close_window")),
            ],
        ),
        MenuSpec::menu(
            2,
            tr("menu.edit"),
            vec![
                MenuSpec::item(201, &format!("{}    Ctrl+Shift+C", tr("term.copy"))),
                MenuSpec::item(202, &format!("{}    Ctrl+Shift+V", tr("term.paste"))),
                MenuSpec::item(203, tr("term.select_all")),
                MenuSpec::separator(204),
                MenuSpec::item(205, tr("term.clear_history")),
            ],
        ),
        MenuSpec::menu(
            3,
            tr("menu.view"),
            vec![
                MenuSpec::item(301, &format!("{}    Ctrl+Shift+=", tr("term.zoom_in"))),
                MenuSpec::item(302, &format!("{}    Ctrl+Shift+−", tr("term.zoom_out"))),
                MenuSpec::item(303, &format!("{}    Ctrl+Shift+0", tr("term.zoom_reset"))),
            ],
        ),
        MenuSpec::menu(
            4,
            tr("menu.window"),
            vec![
                MenuSpec::item(401, &format!("{}    Ctrl+PageDown", tr("term.next_tab"))),
                MenuSpec::item(402, &format!("{}    Ctrl+PageUp", tr("term.prev_tab"))),
            ],
        ),
    ]
}

fn menu_cmd(id: i32) -> Option<Cmd> {
    Some(match id {
        101 => Cmd::NewWindow,
        102 => Cmd::NewTab,
        104 => Cmd::CloseTab,
        105 => Cmd::CloseWindow,
        201 => Cmd::Copy,
        202 => Cmd::Paste,
        203 => Cmd::SelectAll,
        205 => Cmd::ClearHistory,
        301 => Cmd::Bigger,
        302 => Cmd::Smaller,
        303 => Cmd::NormalSize,
        401 => Cmd::NextTab,
        402 => Cmd::PrevTab,
        _ => return None,
    })
}

/// A window shortcut (tabs, clipboard, font size) or `None` — the key is
/// for the program. Letters by key position (Russian layout too).
fn shortcut(k: &KeyPress) -> Option<Cmd> {
    let letter = match k.key {
        Key::Char(c) => Some(latin(c)),
        _ => None,
    };
    let ctrl_shift = k.ctrl && k.shift;
    match k.key {
        _ if ctrl_shift && letter == Some('t') => Some(Cmd::NewTab),
        _ if ctrl_shift && letter == Some('w') => Some(Cmd::CloseTab),
        _ if ctrl_shift && letter == Some('n') => Some(Cmd::NewWindow),
        _ if ctrl_shift && letter == Some('c') => Some(Cmd::Copy),
        _ if ctrl_shift && letter == Some('v') => Some(Cmd::Paste),
        // With Shift these keys give other characters: «+», «_», «)».
        _ if ctrl_shift && matches!(letter, Some('=' | '+')) => Some(Cmd::Bigger),
        _ if ctrl_shift && matches!(letter, Some('-' | '_')) => Some(Cmd::Smaller),
        _ if ctrl_shift && matches!(letter, Some('0' | ')')) => Some(Cmd::NormalSize),
        Key::Up if ctrl_shift => Some(Cmd::ScrollLines(1)),
        Key::Down if ctrl_shift => Some(Cmd::ScrollLines(-1)),
        Key::Home if ctrl_shift => Some(Cmd::ScrollTop),
        Key::End if ctrl_shift => Some(Cmd::ScrollBottom),
        Key::PageDown if k.ctrl && !k.shift => Some(Cmd::NextTab),
        Key::PageUp if k.ctrl && !k.shift => Some(Cmd::PrevTab),
        Key::Tab if k.ctrl => Some(if k.shift { Cmd::PrevTab } else { Cmd::NextTab }),
        Key::Char(c @ '1'..='9') if k.alt && !k.ctrl => {
            Some(Cmd::SelectTab(c as usize - '1' as usize))
        }
        _ => None,
    }
}

/// The whole terminal window: tabs, font, size, mouse.
struct Window {
    d: Arc<Display>,
    win: mimshel_x11::AppWindow,
    tabs: Tabs,
    glyphs: Glyphs,
    /// Cursor blink: wanted now (the ticker thread reads it) and which half.
    blink: Arc<AtomicBool>,
    cursor_on: bool,
    /// Font size in points (changed by «Bigger»/«Smaller»).
    font_size: f32,
    scale: f32,
    /// Window size in pixels.
    size: (u32, u32),
    dirty: bool,
    press: Press,
    last_click: Option<(Instant, (usize, usize), u8)>,
    shown_title: String,
}

impl Window {
    fn cell(&self) -> (u32, u32) {
        self.glyphs.cell
    }

    /// Grid for the area below the tab strip — for every tab.
    fn regrid(&mut self) {
        let bar = self.tabs.bar_h(self.scale);
        let grid = grid_size(self.size.0, self.size.1.saturating_sub(bar), self.cell());
        for t in &mut self.tabs.list {
            t.session.resize(grid);
        }
        self.dirty = true;
    }

    /// Window point (points) → cell and whether it is the cell's right half (below the strip).
    fn at(&self, x: i16, y: i16) -> ((usize, usize), bool) {
        let cell = self.cell();
        let bar = self.tabs.bar_h(self.scale) as f32;
        let (x, y) = (f32::from(x) * self.scale, f32::from(y) * self.scale - bar);
        let grid = self.tabs.active().session.size();
        let c = cell_at(x, y, cell, grid);
        let right = (x - PAD as f32) - (c.0 as f32 * cell.0 as f32) > cell.0 as f32 / 2.0;
        (c, right)
    }

    /// Scrollbar of the active tab in window pixels: track top, track length,
    /// thumb top and height, history length. `None` — no history, no bar.
    fn scrollbar(&self) -> Option<(f32, f32, f32, f32, usize)> {
        let session = &self.tabs.active().session;
        let (history, offset) = session.history();
        if history == 0 {
            return None;
        }
        let bar = self.tabs.bar_h(self.scale) as f32;
        let top = bar + 2.0 * self.scale;
        let len = self.size.1 as f32 - bar - 4.0 * self.scale;
        let (y, h) = thumb(top, len, history, session.size().1, offset);
        Some((top, len, y, h, history))
    }

    /// Is the window point (pixels) over the scrollbar strip?
    fn on_scrollbar(&self, x: f32, y: f32) -> bool {
        let bar = self.tabs.bar_h(self.scale) as f32;
        y >= bar && x >= self.size.0 as f32 - SB_HIT * self.scale && self.scrollbar().is_some()
    }

    fn new_tab(&mut self) -> mimshel_x11::Result<()> {
        let cwd = self.tabs.active().session.cwd();
        let grid = self.tabs.active().session.size();
        self.tabs.open(cwd, grid)?;
        self.regrid();
        Ok(())
    }

    /// Close tab `i`. `false` — no tabs left, close the window.
    fn close_tab(&mut self, i: usize) -> bool {
        self.tabs.close(i);
        if self.tabs.list.is_empty() {
            return false;
        }
        self.regrid();
        true
    }

    /// New font size: glyphs anew, a different cell — regrid every tab.
    fn set_font(&mut self, size: f32) {
        let size = size.clamp(8.0, 40.0);
        if (size - self.font_size).abs() < f32::EPSILON {
            return;
        }
        self.font_size = size;
        self.glyphs.set_size((size * self.scale).round());
        let cell = self.cell();
        self.tabs.cell = cell;
        let bar = self.tabs.bar_h(self.scale);
        let grid = grid_size(self.size.0, self.size.1.saturating_sub(bar), cell);
        for t in &mut self.tabs.list {
            t.session.set_cell(cell, grid);
        }
        self.dirty = true;
    }

    /// Run a command. `false` — close the window.
    fn cmd(&mut self, cmd: Cmd) -> mimshel_x11::Result<bool> {
        self.dirty = true;
        let session = &self.tabs.active().session;
        match cmd {
            Cmd::NewWindow => {
                // A separate process, in the current tab's folder.
                let mut c = std::process::Command::new(crate::guard::own_exe());
                c.arg("terminal");
                if let Some(dir) = session.cwd() {
                    c.arg(dir);
                }
                let _ = crate::ui::spawn_detached(c);
            }
            Cmd::NewTab => self.new_tab()?,
            Cmd::CloseTab => return Ok(self.close_tab(self.tabs.active)),
            Cmd::CloseWindow => return Ok(false),
            Cmd::Copy => {
                if let Some(text) = session.selection_text() {
                    mimshel_x11::copy_text(text, false);
                }
            }
            Cmd::Paste => {
                if let Some(text) = mimshel_x11::clipboard_text(false) {
                    session.paste(&text);
                }
            }
            Cmd::SelectAll => session.select_all(),
            Cmd::ClearHistory => session.clear_history(),
            Cmd::Bigger => self.set_font(self.font_size + 1.0),
            Cmd::Smaller => self.set_font(self.font_size - 1.0),
            Cmd::NormalSize => self.set_font(FONT_SIZE),
            Cmd::NextTab => self.tabs.active = (self.tabs.active + 1) % self.tabs.list.len(),
            Cmd::PrevTab => {
                let n = self.tabs.list.len();
                self.tabs.active = (self.tabs.active + n - 1) % n;
            }
            Cmd::SelectTab(i) => {
                if i < self.tabs.list.len() {
                    self.tabs.active = i;
                }
            }
            Cmd::ScrollLines(n) => session.scroll(n),
            Cmd::ScrollTop => session.scroll_end(true),
            Cmd::ScrollBottom => session.scroll_end(false),
        }
        Ok(true)
    }

    /// An X event. `false` — close the window.
    fn event(&mut self, ev: Event) -> mimshel_x11::Result<bool> {
        let me = self.win.id();
        let scale = self.scale;
        let bar = self.tabs.bar_h(scale);
        match ev {
            Event::Close { window } if window == me => return Ok(false),
            Event::Expose { window } if window == me => self.dirty = true,
            Event::Configure { window } if window == me => {
                if let Some((w, h)) = self.win.size(&self.d) {
                    self.size = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
                    self.regrid();
                }
            }
            Event::Key(k) => {
                if let Some(cmd) = shortcut(&k) {
                    return self.cmd(cmd);
                }
                let session = &self.tabs.active().session;
                if k.shift && k.key == Key::Insert {
                    if let Some(text) = mimshel_x11::clipboard_text(true) {
                        session.paste(&text);
                    }
                } else if k.shift && matches!(k.key, Key::PageUp | Key::PageDown) {
                    session.scroll_page(k.key == Key::PageUp);
                    self.dirty = true;
                } else if let Some(bytes) = key_bytes(&k, session.mode()) {
                    session.input(bytes);
                    // Typing shows the cursor at once, then the blink resumes.
                    self.cursor_on = true;
                    self.dirty = true;
                }
            }
            Event::Button {
                window,
                x,
                y,
                button,
            } if window == me => {
                self.dirty = true;
                // Tab strip.
                if f32::from(y) * scale < bar as f32 {
                    match (
                        self.tabs.bar_hit(f32::from(x) * scale, self.size.0, scale),
                        button,
                    ) {
                        (Some(BarHit::Tab(i)), 2) | (Some(BarHit::Close(i)), 1 | 2) => {
                            return Ok(self.close_tab(i));
                        }
                        (Some(BarHit::Tab(i)), 1) => self.tabs.active = i,
                        (Some(BarHit::Plus), 1) => self.new_tab()?,
                        _ => {}
                    }
                    return Ok(true);
                }
                let (px, py) = (f32::from(x) * scale, f32::from(y) * scale);
                if button == 1 && self.on_scrollbar(px, py) {
                    if let Some((top, len, ty, th, history)) = self.scrollbar() {
                        // On the thumb — grab it where pressed; on the track —
                        // jump so the thumb centre is under the pointer.
                        let grab = if (ty..ty + th).contains(&py) {
                            py - ty
                        } else {
                            th / 2.0
                        };
                        let session = &self.tabs.active().session;
                        session.scroll_to(offset_for(py - grab, top, len, th, history));
                        self.press = Press::Scrollbar(grab);
                        self.tabs.sb_active = true;
                    }
                    return Ok(true);
                }
                let (c, right) = self.at(x, y);
                let session = &self.tabs.active().session;
                // Shift + wheel always scrolls the terminal's own history, even
                // when the program took the mouse (Claude Code, mc, htop).
                let (ctrl, shift) = self.d.modifiers();
                // Ctrl + wheel — font size, as in a browser.
                if ctrl && matches!(button, 4 | 5) {
                    return self.cmd(if button == 4 {
                        Cmd::Bigger
                    } else {
                        Cmd::Smaller
                    });
                }
                // Ctrl + click on a web address — open it in the browser.
                if ctrl
                    && button == 1
                    && let Some(url) = session.url_at(c)
                {
                    open_url(&url);
                    return Ok(true);
                }
                match button {
                    4 | 5 if shift => session.scroll(if button == 4 { 3 } else { -3 }),
                    4 | 5 => session.wheel(button == 4, c),
                    b if session.mouse_wanted() => {
                        session.mouse_report(b - 1, c, true, false);
                        self.press = Press::Program(b - 1);
                    }
                    1 => {
                        let clicks = match self.last_click {
                            Some((t, at, n)) if t.elapsed() < MULTI_CLICK && at == c => n % 3 + 1,
                            _ => 1,
                        };
                        self.last_click = Some((Instant::now(), c, clicks));
                        session.select_start(c, clicks, right);
                        self.press = Press::Selecting;
                    }
                    2 => {
                        if let Some(text) = mimshel_x11::clipboard_text(true) {
                            session.paste(&text);
                        }
                    }
                    _ => {}
                }
            }
            Event::Motion { window, x, y } if window == me => {
                let hover = (f32::from(y) * scale < bar as f32)
                    .then(|| self.tabs.bar_hit(f32::from(x) * scale, self.size.0, scale))
                    .flatten();
                if hover != self.tabs.hover {
                    self.tabs.hover = hover;
                    self.dirty = true;
                }
                let (px, py) = (f32::from(x) * scale, f32::from(y) * scale);
                if let Press::Scrollbar(grab) = self.press {
                    if let Some((top, len, _, th, history)) = self.scrollbar() {
                        let session = &self.tabs.active().session;
                        session.scroll_to(offset_for(py - grab, top, len, th, history));
                        self.dirty = true;
                    }
                    return Ok(true);
                }
                let over = self.on_scrollbar(px, py);
                if over != self.tabs.sb_active {
                    self.tabs.sb_active = over;
                    self.dirty = true;
                }
                let (c, right) = self.at(x, y);
                let session = &self.tabs.active().session;
                match self.press {
                    Press::Selecting => {
                        session.select_update(c, right);
                        self.dirty = true;
                    }
                    Press::Program(b) => session.mouse_report(b, c, true, true),
                    Press::None | Press::Scrollbar(_) => {}
                }
            }
            Event::Leave { window } if window == me => {
                if self.tabs.hover.take().is_some() {
                    self.dirty = true;
                }
                if self.tabs.sb_active && !matches!(self.press, Press::Scrollbar(_)) {
                    self.tabs.sb_active = false;
                    self.dirty = true;
                }
            }
            Event::Release {
                window,
                x,
                y,
                button,
            } if window == me => {
                let (c, _) = self.at(x, y);
                let session = &self.tabs.active().session;
                match self.press {
                    Press::Selecting if button == 1 => {
                        // The selection goes straight to PRIMARY (middle-click paste).
                        match session.selection_text() {
                            Some(text) => mimshel_x11::copy_text(text, true),
                            // Just a click — no selection.
                            None => session.clear_selection(),
                        }
                        self.dirty = true;
                    }
                    Press::Program(b) => session.mouse_report(b, c, false, false),
                    Press::Scrollbar(_) => self.dirty = true,
                    _ => {}
                }
                self.press = Press::None;
            }
            _ => {}
        }
        Ok(true)
    }

    /// News from the session of tab `id`. `false` — close the window.
    fn term_event(&mut self, id: u64) -> bool {
        let Some(i) = self.tabs.find(id) else {
            return true;
        };
        for e in self.tabs.list[i].session.poll() {
            match e {
                TermEvent::Redraw => self.dirty |= i == self.tabs.active,
                TermEvent::Title(t) => {
                    self.tabs.list[i].title = t;
                    self.dirty = true;
                }
                TermEvent::ResetTitle => {
                    self.tabs.list[i].title = TITLE.to_string();
                    self.dirty = true;
                }
                TermEvent::Copy(text) => mimshel_x11::copy_text(text, false),
                TermEvent::Bell => {}
                // The tab's shell exited — close the tab.
                TermEvent::Exit => return self.close_tab(i),
            }
        }
        true
    }

    fn flush(&mut self) -> mimshel_x11::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        let title = self.tabs.active().title.clone();
        if title != self.shown_title {
            self.win.set_title(&self.d, &title)?;
            self.shown_title = title;
        }
        let focused = self.d.active_window() == Some(self.win.id());
        let blinking = focused && self.tabs.active().session.cursor_blinking();
        self.blink.store(blinking, Ordering::Relaxed);
        if !blinking {
            self.cursor_on = true;
        }
        if let Some(frame) = self.tabs.render(
            &mut self.glyphs,
            self.size,
            self.scale,
            focused,
            self.cursor_on,
        ) {
            self.win.present(&self.d, &frame)?;
            self.d.sync()?;
        }
        Ok(())
    }
}

fn terminal(dir: Option<PathBuf>, command: Option<Vec<String>>) -> mimshel_x11::Result<()> {
    let d = Arc::new(Display::connect()?);
    let scale = d.scale();
    let (tx, rx) = mpsc::channel::<Wake>();
    let fonts_tx = tx.clone();
    let glyphs = Glyphs::new(&mono_family(), (FONT_SIZE * scale).round(), move || {
        let _ = fonts_tx.send(Wake::Fonts);
    });
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
    let title = mimshel_core::i18n::t(crate::ui::load_config().language, "app.terminal");
    let win = d.create_app_window(WINDOW_CLASS, title, to_pt(px(START)), to_pt(px((20, 4))))?;

    let mut tabs = Tabs {
        list: Vec::new(),
        active: 0,
        next_id: 0,
        cell,
        tx: tx.clone(),
        osc52: crate::ui::load_config().osc52,
        ui_fonts: None,
        hover: None,
        sb_active: false,
    };
    tabs.open_with(dir, START, command)?;
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
    // Menu at the top: shown once the service's registrar is there, and
    // re-registered whenever the service restarts. Fails only without a
    // session bus.
    let menu_tx = tx.clone();
    let _menu = match mimshel_dbus::menu_export::export(
        win.id(),
        menus(crate::ui::load_config().language),
        move |id| {
            let _ = menu_tx.send(Wake::Menu(id));
        },
    ) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("Терминал: меню в строке меню не выставлено: {e}");
            None
        }
    };

    let mut w = Window {
        d,
        win,
        tabs,
        glyphs,
        blink: Arc::new(AtomicBool::new(false)),
        cursor_on: true,
        font_size: FONT_SIZE,
        scale,
        size: px(START),
        dirty: true,
        press: Press::None,
        last_click: None,
        shown_title: String::new(),
    };
    // Blink ticker: wakes the window only while the program wants blinking.
    {
        let blink = Arc::clone(&w.blink);
        let tx = tx.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(BLINK);
                if blink.load(Ordering::Relaxed) && tx.send(Wake::Blink).is_err() {
                    break;
                }
            }
        });
    }
    'run: while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(more) = rx.try_recv() {
            batch.push(more);
        }
        for wake in batch {
            let keep = match wake {
                Wake::X(ev) => w.event(ev)?,
                Wake::Term(id) => w.term_event(id),
                Wake::Menu(id) => match menu_cmd(id) {
                    Some(cmd) => w.cmd(cmd)?,
                    None => true,
                },
                Wake::Fonts => {
                    w.dirty |= w.glyphs.refresh();
                    true
                }
                Wake::Blink => {
                    w.cursor_on = !w.cursor_on;
                    w.dirty = true;
                    true
                }
            };
            if !keep {
                break 'run;
            }
        }
        w.flush()?;
    }
    w.win.destroy(&w.d)?;
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

    /// Two real tabs: the strip appears, the grid is below it, clicks on the
    /// strip land where they should. `MIMSHEL_TAB_PNG=file` saves the frame.
    #[test]
    fn two_tabs_render_with_bar() {
        let (tx, _rx) = mpsc::channel();
        let mut glyphs = Glyphs::new("DejaVu Sans Mono", 14.0, || {});
        let mut tabs = Tabs {
            list: Vec::new(),
            active: 0,
            next_id: 0,
            cell: glyphs.cell,
            tx,
            osc52: Osc52::Allow,
            ui_fonts: None,
            hover: None,
            sb_active: false,
        };
        tabs.open(None, (60, 10)).unwrap();
        assert_eq!(tabs.bar_h(1.0), 0, "one tab — no strip");
        tabs.open(None, (60, 10)).unwrap();
        tabs.list[0].title = "mc [ded@ded-gamingseries]:~".into();
        tabs.list[1].title = "ded@ded-gamingseries:~".into();
        assert_eq!(tabs.active, 1, "the new tab is active");
        let bar = tabs.bar_h(1.0);
        assert_eq!(bar, BAR_H as u32);
        let w = 700;
        assert_eq!(tabs.bar_hit(10.0, w, 1.0), Some(BarHit::Tab(0)));
        let tw = tabs.tab_w(w, 1.0);
        assert_eq!(tabs.bar_hit(tw - 5.0, w, 1.0), Some(BarHit::Close(0)));
        assert_eq!(tabs.bar_hit(2.0 * tw + 5.0, w, 1.0), Some(BarHit::Plus));
        tabs.hover = Some(BarHit::Tab(0));
        let frame = tabs.render(&mut glyphs, (w, 260), 1.0, true, true).unwrap();
        assert_eq!((frame.width(), frame.height()), (w, 260));
        if let Some(path) = std::env::var_os("MIMSHEL_TAB_PNG") {
            frame.save_png(path).unwrap();
        }
        tabs.close(0);
        assert_eq!((tabs.list.len(), tabs.active), (1, 0));
        assert_eq!(tabs.bar_h(1.0), 0, "one again — the strip is gone");
    }

    /// A real shell prints 300 lines: history appears and the scrollbar is
    /// drawn at the right edge. `MIMSHEL_SB_PNG=file` saves the frame.
    #[test]
    fn scrollbar_appears_with_history() {
        let (tx, _rx) = mpsc::channel();
        let mut glyphs = Glyphs::new("DejaVu Sans Mono", 14.0, || {});
        let mut tabs = Tabs {
            list: Vec::new(),
            active: 0,
            next_id: 0,
            cell: glyphs.cell,
            tx,
            osc52: Osc52::Allow,
            ui_fonts: None,
            hover: None,
            sb_active: false,
        };
        tabs.open(None, (50, 12)).unwrap();
        let session = &tabs.list[0].session;
        session.input(b"seq 1 300\r".to_vec());
        let start = Instant::now();
        while session.history().0 < 200 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            session.history().0 >= 200,
            "history: {:?}",
            session.history()
        );
        session.scroll(100);
        assert_eq!(session.history().1, 100);
        let (w, h) = (420, 240);
        let frame = tabs.render(&mut glyphs, (w, h), 1.0, true, true).unwrap();
        if let Some(path) = std::env::var_os("MIMSHEL_SB_PNG") {
            frame.save_png(path).unwrap();
        }
        // Somewhere in the right edge column there is the light thumb.
        let x = w - 4 - SB_THIN as u32 / 2;
        let lit = (0..h).any(|y| frame.pixel(x, y).is_some_and(|p| p.red() > 0x70));
        assert!(lit, "scrollbar thumb is drawn");
        tabs.list[0].session.scroll_end(false);
        assert_eq!(tabs.list[0].session.history().1, 0);
    }

    #[test]
    fn scrollbar_geometry() {
        // 100 lines of history, 25 on screen, track 400 px from y=10.
        let (y, h) = thumb(10.0, 400.0, 100, 25, 0);
        assert_eq!(h, 80.0, "thumb is the visible share of everything");
        assert_eq!(y, 330.0, "at the bottom when not scrolled");
        let (top_y, _) = thumb(10.0, 400.0, 100, 25, 100);
        assert_eq!(top_y, 10.0, "scrolled all the way up — at the top");
        // Back from the thumb position to the offset.
        assert_eq!(offset_for(330.0, 10.0, 400.0, h, 100), 0);
        assert_eq!(offset_for(10.0, 10.0, 400.0, h, 100), 100);
        assert_eq!(offset_for(170.0, 10.0, 400.0, h, 100), 50);
        // A huge history still leaves a thumb you can grab.
        assert_eq!(thumb(0.0, 400.0, 100_000, 25, 0).1, SB_MIN_THUMB);
        assert_eq!(
            shortcut(&key(Key::Up, true, true, false)),
            Some(Cmd::ScrollLines(1))
        );
        assert_eq!(
            shortcut(&key(Key::Home, true, true, false)),
            Some(Cmd::ScrollTop)
        );
    }

    #[test]
    fn command_line() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_args(&[]), (None, None));
        assert_eq!(
            parse_args(&s(&["/tmp"])),
            (Some(PathBuf::from("/tmp")), None)
        );
        assert_eq!(
            parse_args(&s(&["-e", "htop", "-d", "5"])),
            (None, Some(s(&["htop", "-d", "5"])))
        );
        assert_eq!(
            parse_args(&s(&["-e"])),
            (None, None),
            "-e without a program — the shell"
        );
    }

    #[test]
    fn tab_shortcuts() {
        assert!(matches!(
            shortcut(&key(Key::Char('T'), true, true, false)),
            Some(Cmd::NewTab)
        ));
        assert!(
            matches!(
                shortcut(&key(Key::Char('Е'), true, true, false)),
                Some(Cmd::NewTab)
            ),
            "Russian layout — by key position"
        );
        assert!(matches!(
            shortcut(&key(Key::PageDown, true, false, false)),
            Some(Cmd::NextTab)
        ));
        assert!(matches!(
            shortcut(&key(Key::Tab, true, true, false)),
            Some(Cmd::PrevTab)
        ));
        assert!(matches!(
            shortcut(&key(Key::Char('3'), false, false, true)),
            Some(Cmd::SelectTab(2))
        ));
        assert!(
            shortcut(&key(Key::Char('t'), true, false, false)).is_none(),
            "Ctrl+T without Shift goes to the program (bash: transpose chars)"
        );
        assert!(shortcut(&key(Key::PageUp, false, true, false)).is_none());
        // Font size: with Shift these keys give «+», «_», «)».
        assert_eq!(
            shortcut(&key(Key::Char('+'), true, true, false)),
            Some(Cmd::Bigger)
        );
        assert_eq!(
            shortcut(&key(Key::Char('_'), true, true, false)),
            Some(Cmd::Smaller)
        );
        assert_eq!(
            shortcut(&key(Key::Char(')'), true, true, false)),
            Some(Cmd::NormalSize)
        );
        assert_eq!(
            shortcut(&key(Key::Char('C'), true, true, false)),
            Some(Cmd::Copy)
        );
        // Menu and keys share the same commands.
        assert_eq!(menu_cmd(102), Some(Cmd::NewTab));
        assert_eq!(menu_cmd(103), None, "separator");
        let ids: Vec<i32> = menus(mimshel_core::i18n::Language::Ru)
            .iter()
            .flat_map(|m| m.children.iter().filter(|i| !i.separator).map(|i| i.id))
            .collect();
        assert!(
            ids.iter().all(|&id| menu_cmd(id).is_some()),
            "every item has a command"
        );
    }
}
