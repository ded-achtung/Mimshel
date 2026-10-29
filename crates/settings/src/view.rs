//! The settings window's picture and its input: a sidebar of pages on the
//! left, the chosen page on the right — rows in rounded boxes, each with a
//! popup button, a switch, a slider or a text field, as in macOS System
//! Settings. Everything in points; the window glue turns clicks into
//! [`Action`]s here and writes the settings.

use std::collections::HashMap;

use mimshel_core::i18n::{Language, t};
use mimshel_core::input::{Key, KeyPress};
use mimshel_icons::Icons;
use mimshel_render::Canvas;
use mimshel_text::{Fonts, Style};
use mimshel_widgets::shape::{circle, paint, rounded};
use mimshel_widgets::{Edit, TextField};
use tiny_skia::{Color, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

use crate::model::{Control, Env, Page, PageKind, Row, pages};

/// The window's size when it opens, and the smallest it can get.
pub const SIZE: (u32, u32) = (820, 580);
pub const MIN_SIZE: (u32, u32) = (680, 440);

const SIDEBAR_W: f32 = 230.0;
const SIDE_PAD: f32 = 10.0;
const SIDE_ROW: f32 = 32.0;
const SIDE_ICON: u32 = 20;
const SIDE_HEADER: f32 = 28.0;
const PAD: f32 = 24.0;
const TITLE_H: f32 = 56.0;
const ROW_H: f32 = 44.0;
const HINT_H: f32 = 16.0;
const GROUP_GAP: f32 = 16.0;
const GROUP_TITLE_H: f32 = 26.0;
const CONTROL_H: f32 = 26.0;
const SWITCH_W: f32 = 40.0;
const SWITCH_H: f32 = 24.0;
const SLIDER_W: f32 = 180.0;
const FIELD_W: f32 = 240.0;
const OPTION_H: f32 = 26.0;
const THUMB: (u32, u32) = (168, 105);
const THUMB_GAP: f32 = 18.0;
const SCROLL_STEP: f32 = 48.0;

const BG: [u8; 4] = [0x1e, 0x1e, 0x20, 0xff];
const SIDEBAR: [u8; 4] = [0x26, 0x26, 0x29, 0xff];
const BOX: [u8; 4] = [0xff, 0xff, 0xff, 0x0d];
const LINE: [u8; 4] = [0xff, 0xff, 0xff, 0x14];
const HOVER: [u8; 4] = [0xff, 0xff, 0xff, 0x10];
const ACCENT: [u8; 4] = [0x0a, 0x84, 0xff, 0xff];
const BUTTON: [u8; 4] = [0xff, 0xff, 0xff, 0x1c];
const BUTTON_HOVER: [u8; 4] = [0xff, 0xff, 0xff, 0x2a];
const OFF: [u8; 4] = [0xff, 0xff, 0xff, 0x29];
const KNOB: [u8; 4] = [0xf5, 0xf5, 0xf7, 0xff];
const FIELD: [u8; 4] = [0x00, 0x00, 0x00, 0x40];
const FIELD_EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x24];
const MENU: [u8; 4] = [0x2e, 0x2e, 0x31, 0xfa];
const MENU_EDGE: [u8; 4] = [0xff, 0xff, 0xff, 0x2a];
const WHITE: [u8; 4] = [0xf2, 0xf2, 0xf7, 0xff];

const fn style(size: f32, color: [u8; 4], weight: u16) -> Style {
    Style {
        size,
        color,
        weight,
    }
}
const TEXT: Style = style(13.0, WHITE, 400);
const TEXT_SEL: Style = style(13.0, [0xff, 0xff, 0xff, 0xff], 500);
const SMALL: Style = style(11.0, [0x98, 0x98, 0x9f, 0xff], 400);
const HEADER: Style = style(11.0, [0x8e, 0x8e, 0x93, 0xff], 600);
const TITLE: Style = style(20.0, WHITE, 700);
const VALUE: Style = style(13.0, [0xc7, 0xc7, 0xcc, 0xff], 400);
const STATUS: Style = style(12.0, [0x98, 0x98, 0x9f, 0xff], 400);

/// What a click or a key asks the window to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Nothing,
    Redraw,
    /// Write `key = value` (as `mimshel set` takes it) and apply it.
    Set(&'static str, String),
    /// Ask for a picture file for the wallpaper.
    PickWallpaper,
    /// Run another program's settings window.
    Open(Vec<String>),
    Close,
}

/// A place in the window that reacts to the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hit {
    Page(usize),
    /// The control of a row (index into the page's rows, in order).
    Control(usize),
    /// An option of the open popup.
    Option(usize),
    Wallpaper(usize),
    Pick,
    Open,
}

/// A rectangle `(x, y, w, h)` in window points.
type Rect = (f32, f32, f32, f32);

fn inside((x, y, w, h): Rect, px: f32, py: f32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

pub struct SettingsView {
    lang: Language,
    values: HashMap<String, String>,
    env: Env,
    pages: Vec<Page>,
    /// The page shown.
    page: usize,
    size: (u32, u32),
    /// How far the page is scrolled down.
    scroll: f32,
    /// The page's height last drawn (to keep the scroll in range).
    content_h: f32,
    hover: Option<Hit>,
    /// The row whose popup is open.
    open: Option<usize>,
    /// The row being typed into.
    editing: Option<(usize, TextField)>,
    /// The slider being dragged and its value now.
    drag: Option<(usize, u32)>,
    /// Where things were drawn — for the next click.
    hits: Vec<(Rect, Hit)>,
    /// Slider tracks: row → `(x, w)` of the track.
    tracks: HashMap<usize, (f32, f32)>,
    /// Wallpaper thumbnails by value, in pixels.
    thumbs: HashMap<String, Pixmap>,
    /// One line at the bottom: «Applying…», «Applied», an error.
    status: Option<String>,
}

impl SettingsView {
    pub fn new(lang: Language, values: HashMap<String, String>, env: Env) -> SettingsView {
        let pages = pages(lang, &values, &env);
        SettingsView {
            lang,
            values,
            env,
            pages,
            page: 0,
            size: SIZE,
            scroll: 0.0,
            content_h: 0.0,
            hover: None,
            open: None,
            editing: None,
            drag: None,
            hits: Vec::new(),
            tracks: HashMap::new(),
            thumbs: HashMap::new(),
            status: None,
        }
    }

    /// The settings as the file has them now (after a write, or another
    /// program changed them), and the language they are shown in.
    pub fn set_values(&mut self, lang: Language, values: HashMap<String, String>) {
        let id = self.pages.get(self.page).map(|p| p.id);
        self.lang = lang;
        self.values = values;
        self.pages = pages(lang, &self.values, &self.env);
        self.page = id
            .and_then(|id| self.pages.iter().position(|p| p.id == id))
            .unwrap_or(0);
        self.open = None;
    }

    pub fn language(&self) -> Language {
        self.lang
    }

    pub fn set_status(&mut self, status: Option<String>) {
        self.status = status;
    }

    /// Open on the page with this id (`mimshel settings dock`).
    pub fn show_page(&mut self, id: &str) {
        if let Some(i) = self.pages.iter().position(|p| p.id == id) {
            self.select(i);
        }
    }

    /// The wallpapers the page shows, to make thumbnails for.
    pub fn wallpapers(&self) -> Vec<String> {
        self.pages
            .iter()
            .find_map(|p| match &p.kind {
                PageKind::Wallpaper(list) => Some(list.iter().map(|w| w.0.clone()).collect()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// A thumbnail `thumb_size` pixels for wallpaper `value`.
    pub fn set_thumb(&mut self, value: String, pixmap: Pixmap) {
        self.thumbs.insert(value, pixmap);
    }

    pub fn has_thumb(&self, value: &str) -> bool {
        self.thumbs.contains_key(value)
    }

    /// Thumbnail size in points.
    pub fn thumb_size() -> (u32, u32) {
        THUMB
    }

    fn rows(&self) -> Vec<&Row> {
        match self.pages.get(self.page).map(|p| &p.kind) {
            Some(PageKind::Rows(groups)) => groups.iter().flat_map(|g| &g.rows).collect(),
            _ => Vec::new(),
        }
    }

    fn value(&self, key: &str) -> &str {
        self.values.get(key).map_or("", String::as_str)
    }

    fn select(&mut self, page: usize) {
        self.page = page;
        self.scroll = 0.0;
        self.open = None;
        self.editing = None;
        self.drag = None;
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        // The open popup is on top of everything and drawn last.
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| inside(*r, x, y))
            .map(|(_, h)| *h)
    }

    /// Something changed now: remember it right away, so the window shows
    /// it before the file is read back.
    fn set(&mut self, key: &'static str, value: String) -> Action {
        if self.value(key) == value {
            return Action::Redraw;
        }
        self.values.insert(key.to_string(), value.clone());
        let id = self.pages[self.page].id;
        self.pages = pages(self.lang, &self.values, &self.env);
        self.page = self.pages.iter().position(|p| p.id == id).unwrap_or(0);
        Action::Set(key, value)
    }

    /// The text being typed, written if it changed.
    fn commit_field(&mut self) -> Action {
        let Some((row, field)) = self.editing.take() else {
            return Action::Nothing;
        };
        let Some(key) = self.rows().get(row).map(|r| r.key) else {
            return Action::Redraw;
        };
        let text = field.text().trim().to_string();
        if text.is_empty() {
            return Action::Redraw;
        }
        self.set(key, text)
    }

    pub fn press(&mut self, x: f32, y: f32) -> Action {
        let hit = self.hit(x, y);
        if let Some(row) = self.open.take() {
            if let Some(Hit::Option(i)) = hit {
                let rows = self.rows();
                let picked = rows.get(row).and_then(|r| match &r.control {
                    Control::Choice(options) => options.get(i).map(|o| (r.key, o.0.clone())),
                    _ => None,
                });
                if let Some((key, value)) = picked {
                    return self.set(key, value);
                }
            }
            // A click beside the popup only closes it.
            return Action::Redraw;
        }
        if self.editing.is_some() && hit != self.editing.as_ref().map(|e| Hit::Control(e.0)) {
            let done = self.commit_field();
            if done != Action::Nothing && done != Action::Redraw {
                return done;
            }
        }
        match hit {
            Some(Hit::Page(i)) => match &self.pages[i].kind {
                PageKind::Link(command) => Action::Open(command.clone()),
                _ => {
                    self.select(i);
                    Action::Redraw
                }
            },
            Some(Hit::Control(i)) => {
                let Some(row) = self.rows().get(i).map(|r| (*r).clone()) else {
                    return Action::Nothing;
                };
                match row.control {
                    Control::Choice(_) => {
                        self.open = Some(i);
                        Action::Redraw
                    }
                    Control::Switch { on, off } => {
                        let value = if self.value(row.key) == off { on } else { off };
                        self.set(row.key, value)
                    }
                    Control::Slider { min, max } => {
                        let v = self.slider_at(i, x, min, max);
                        self.drag = Some((i, v));
                        Action::Redraw
                    }
                    Control::Text => {
                        if self.editing.as_ref().is_none_or(|e| e.0 != i) {
                            let text = self.value(row.key).to_string();
                            let n = text.chars().count();
                            self.editing = Some((i, TextField::with_text(&text, n)));
                        }
                        Action::Redraw
                    }
                }
            }
            Some(Hit::Wallpaper(i)) => {
                let value = self.wallpapers().get(i).cloned();
                value.map_or(Action::Nothing, |v| self.set("look.wallpaper", v))
            }
            Some(Hit::Pick) => Action::PickWallpaper,
            Some(Hit::Open) => match &self.pages[self.page].kind {
                PageKind::Link(command) => Action::Open(command.clone()),
                _ => Action::Nothing,
            },
            Some(Hit::Option(_)) | None => Action::Nothing,
        }
    }

    fn slider_at(&self, row: usize, x: f32, min: u32, max: u32) -> u32 {
        let Some(&(x0, w)) = self.tracks.get(&row) else {
            return min;
        };
        let k = ((x - x0) / w).clamp(0.0, 1.0);
        min + ((max - min) as f32 * k).round() as u32
    }

    pub fn motion(&mut self, x: f32, y: f32) -> Action {
        if let Some((row, old)) = self.drag {
            let range = self.rows().get(row).and_then(|r| match r.control {
                Control::Slider { min, max } => Some((min, max)),
                _ => None,
            });
            if let Some((min, max)) = range {
                let v = self.slider_at(row, x, min, max);
                if v != old {
                    self.drag = Some((row, v));
                    return Action::Redraw;
                }
            }
            return Action::Nothing;
        }
        let hit = self.hit(x, y);
        if hit != self.hover {
            self.hover = hit;
            return Action::Redraw;
        }
        Action::Nothing
    }

    pub fn leave(&mut self) -> Action {
        if self.hover.take().is_some() {
            return Action::Redraw;
        }
        Action::Nothing
    }

    pub fn release(&mut self) -> Action {
        let Some((row, v)) = self.drag.take() else {
            return Action::Nothing;
        };
        match self.rows().get(row).map(|r| r.key) {
            Some(key) => self.set(key, v.to_string()),
            None => Action::Redraw,
        }
    }

    /// Mouse wheel: `down` scrolls the page down.
    pub fn wheel(&mut self, down: bool) -> Action {
        let max = (self.content_h - self.size.1 as f32).max(0.0);
        let to = (self.scroll + if down { SCROLL_STEP } else { -SCROLL_STEP }).clamp(0.0, max);
        if to == self.scroll {
            return Action::Nothing;
        }
        self.scroll = to;
        self.open = None;
        Action::Redraw
    }

    pub fn key(&mut self, k: KeyPress) -> Action {
        if let Some((_, field)) = &mut self.editing {
            return match k.key {
                Key::Return => {
                    let done = self.commit_field();
                    if done == Action::Nothing {
                        Action::Redraw
                    } else {
                        done
                    }
                }
                Key::Escape => {
                    self.editing = None;
                    Action::Redraw
                }
                _ => match field.key(k) {
                    Edit::Ignored => Action::Nothing,
                    _ => Action::Redraw,
                },
            };
        }
        if self.open.is_some() && k.key == Key::Escape {
            self.open = None;
            return Action::Redraw;
        }
        match k.key {
            Key::Char('w' | 'q') if k.ctrl => Action::Close,
            Key::Up | Key::Down => {
                let n = self.pages.len();
                let mut i = self.page;
                // Skip the pages that open another window.
                for _ in 0..n {
                    i = if k.key == Key::Up {
                        (i + n - 1) % n
                    } else {
                        (i + 1) % n
                    };
                    if !matches!(self.pages[i].kind, PageKind::Link(_)) {
                        break;
                    }
                }
                self.select(i);
                Action::Redraw
            }
            _ => Action::Nothing,
        }
    }

    /// The whole window, `(w, h)` points.
    pub fn render(&mut self, (w, h): (u32, u32), fonts: &mut Fonts, icons: &mut Icons) -> Canvas {
        self.size = (w.max(1), h.max(1));
        self.hits.clear();
        self.tracks.clear();
        let mut pm = Canvas::new(self.size.0, self.size.1).expect("size is not zero");
        pm.fill(color(BG));
        self.draw_sidebar(&mut pm, fonts, icons);
        let cw = (w as f32 - SIDEBAR_W).max(1.0) as u32;
        let mut content = Canvas::new(cw, self.size.1).expect("size is not zero");
        content.fill(color(BG));
        let popup = self.draw_page(&mut content, fonts);
        if let Some((row, x, y, bw)) = popup {
            self.draw_popup(&mut content, fonts, row, (x, y, bw));
        }
        pm.draw_canvas(SIDEBAR_W, 0.0, &content, &PixmapPaint::default());
        pm
    }

    fn draw_sidebar(&mut self, pm: &mut Canvas, fonts: &mut Fonts, icons: &mut Icons) {
        let h = self.size.1 as f32;
        fill(pm, (0.0, 0.0, SIDEBAR_W, h), SIDEBAR, 0.0);
        let mut y = 12.0;
        let mut was_link = None;
        for i in 0..self.pages.len() {
            let link = matches!(self.pages[i].kind, PageKind::Link(_));
            if was_link != Some(link) {
                let title = if link {
                    t(self.lang, "settings.group.system")
                } else {
                    "Mimshel"
                };
                if was_link.is_some() {
                    y += 8.0;
                }
                fonts.draw(pm, title, &HEADER, SIDE_PAD + 10.0, y + 8.0, None);
                y += SIDE_HEADER;
                was_link = Some(link);
            }
            let r = (SIDE_PAD, y, SIDEBAR_W - 2.0 * SIDE_PAD, SIDE_ROW);
            let selected = i == self.page;
            if selected {
                fill(pm, r, ACCENT, 7.0);
            } else if self.hover == Some(Hit::Page(i)) {
                fill(pm, r, HOVER, 7.0);
            }
            let page = &self.pages[i];
            let icon_y = y + (SIDE_ROW - SIDE_ICON as f32) / 2.0;
            if let Some(icon) = icons.get(page.icon, SIDE_ICON) {
                pm.draw_icon(
                    icon.as_ref(),
                    SIDE_PAD + 8.0,
                    icon_y,
                    &PixmapPaint::default(),
                );
            }
            let text = if selected { &TEXT_SEL } else { &TEXT };
            let tx = SIDE_PAD + 8.0 + SIDE_ICON as f32 + 10.0;
            let ty = y + (SIDE_ROW - text.line_height()) / 2.0;
            fonts.draw(
                pm,
                &page.title,
                text,
                tx,
                ty,
                Some(r.2 - (tx - SIDE_PAD) - 8.0),
            );
            self.hits.push((r, Hit::Page(i)));
            y += SIDE_ROW + 2.0;
        }
    }

    /// Draws the page (scrolled); returns where the open popup goes:
    /// `(row, x, y, button width)` in content points.
    fn draw_page(&mut self, pm: &mut Canvas, fonts: &mut Fonts) -> Option<(usize, f32, f32, f32)> {
        let page = self.pages.get(self.page).cloned()?;
        let w = pm.width() as f32;
        let top = -self.scroll;
        fonts.draw(
            pm,
            &page.title,
            &TITLE,
            PAD,
            top + 18.0,
            Some(w - 2.0 * PAD),
        );
        let mut y = top + TITLE_H;
        let mut popup = None;
        match &page.kind {
            PageKind::Rows(groups) => {
                let mut index = 0;
                for group in groups {
                    if let Some(title) = &group.title {
                        fonts.draw(pm, title, &SMALL, PAD + 4.0, y, Some(w - 2.0 * PAD));
                        y += GROUP_TITLE_H - 8.0;
                    }
                    let heights: Vec<f32> = group.rows.iter().map(row_height).collect();
                    let box_h: f32 = heights.iter().sum();
                    fill(pm, (PAD, y, w - 2.0 * PAD, box_h), BOX, 10.0);
                    for (n, row) in group.rows.iter().enumerate() {
                        if n > 0 {
                            fill(pm, (PAD + 14.0, y, w - 2.0 * PAD - 28.0, 1.0), LINE, 0.0);
                        }
                        if let Some(p) = self.draw_row(pm, fonts, row, index, y, w) {
                            popup = Some(p);
                        }
                        y += heights[n];
                        index += 1;
                    }
                    y += GROUP_GAP;
                }
            }
            PageKind::Wallpaper(list) => {
                y = self.draw_wallpapers(pm, fonts, list, y, w);
            }
            PageKind::Link(_) => {
                let text = t(self.lang, "settings.link_text");
                fonts.draw(pm, text, &VALUE, PAD, y, Some(w - 2.0 * PAD));
                y += 32.0;
                let label = t(self.lang, "settings.open");
                let bw = fonts.measure(label, &TEXT) + 32.0;
                let r = (PAD, y, bw, CONTROL_H + 2.0);
                self.button(pm, fonts, r, label, Hit::Open, true);
                y += 40.0;
            }
        }
        self.content_h = y + self.scroll + PAD;
        if let Some(status) = &self.status {
            let h = pm.height() as f32;
            fill(pm, (0.0, h - 30.0, w, 30.0), BG, 0.0);
            fonts.draw(pm, status, &STATUS, PAD, h - 24.0, Some(w - 2.0 * PAD));
        }
        popup
    }

    fn draw_row(
        &mut self,
        pm: &mut Canvas,
        fonts: &mut Fonts,
        row: &Row,
        index: usize,
        y: f32,
        w: f32,
    ) -> Option<(usize, f32, f32, f32)> {
        let x0 = PAD + 14.0;
        let right = w - PAD - 14.0;
        let ly = y + (ROW_H - TEXT.line_height()) / 2.0;
        let control_y = y + (ROW_H - CONTROL_H) / 2.0;
        let label_w = (right - x0 - 200.0).max(120.0);
        fonts.draw(pm, &row.label, &TEXT, x0, ly, Some(label_w));
        if let Some(hint) = &row.hint {
            fonts.draw(pm, hint, &SMALL, x0, y + ROW_H - 10.0, Some(right - x0));
        }
        let value = self.value(row.key).to_string();
        let hover = self.hover == Some(Hit::Control(index));
        let content_x = SIDEBAR_W;
        let mut popup = None;
        match &row.control {
            Control::Choice(options) => {
                let label = options
                    .iter()
                    .find(|o| o.0 == value)
                    .map_or(value.as_str(), |o| o.1.as_str())
                    .to_string();
                let bw = (fonts.measure(&label, &TEXT) + 40.0).clamp(110.0, 280.0);
                let r = (right - bw, control_y, bw, CONTROL_H);
                fill(pm, r, if hover { BUTTON_HOVER } else { BUTTON }, 6.0);
                fonts.draw(pm, &label, &TEXT, r.0 + 10.0, r.1 + 4.0, Some(bw - 34.0));
                chevrons(pm, r.0 + bw - 16.0, r.1 + CONTROL_H / 2.0);
                self.hits
                    .push(((content_x + r.0, r.1, r.2, r.3), Hit::Control(index)));
                if self.open == Some(index) {
                    popup = Some((index, r.0, r.1 + CONTROL_H + 4.0, bw));
                }
            }
            Control::Switch { off, .. } => {
                let on = value != *off;
                let r = (
                    right - SWITCH_W,
                    y + (ROW_H - SWITCH_H) / 2.0,
                    SWITCH_W,
                    SWITCH_H,
                );
                fill(pm, r, if on { ACCENT } else { OFF }, SWITCH_H / 2.0);
                let kx = if on {
                    r.0 + SWITCH_W - SWITCH_H / 2.0
                } else {
                    r.0 + SWITCH_H / 2.0
                };
                if let Some(c) = circle(kx, r.1 + SWITCH_H / 2.0, SWITCH_H / 2.0 - 2.0) {
                    pm.fill_path(
                        &c,
                        &paint(KNOB),
                        tiny_skia::FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                self.hits
                    .push(((content_x + r.0, r.1, r.2, r.3), Hit::Control(index)));
            }
            Control::Slider { min, max } => {
                let v = match self.drag {
                    Some((i, v)) if i == index => v,
                    _ => value.parse().unwrap_or(*min),
                };
                let tx = right - SLIDER_W;
                let cy = y + ROW_H / 2.0;
                fill(pm, (tx, cy - 2.0, SLIDER_W, 4.0), OFF, 2.0);
                let k = (v.saturating_sub(*min)) as f32 / (*max - *min).max(1) as f32;
                let kx = tx + SLIDER_W * k.clamp(0.0, 1.0);
                fill(pm, (tx, cy - 2.0, kx - tx, 4.0), ACCENT, 2.0);
                if let Some(c) = circle(kx, cy, if hover { 10.0 } else { 9.0 }) {
                    pm.fill_path(
                        &c,
                        &paint(KNOB),
                        tiny_skia::FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
                let text = v.to_string();
                let vw = fonts.measure(&text, &VALUE);
                fonts.draw(
                    pm,
                    &text,
                    &VALUE,
                    tx - 14.0 - vw,
                    cy - VALUE.line_height() / 2.0,
                    None,
                );
                self.tracks.insert(index, (content_x + tx, SLIDER_W));
                self.hits.push((
                    (content_x + tx - 10.0, y, SLIDER_W + 20.0, ROW_H),
                    Hit::Control(index),
                ));
            }
            Control::Text => {
                let fw = FIELD_W.min(right - x0 - 120.0).max(120.0);
                let r = (right - fw, control_y, fw, CONTROL_H);
                let editing = self.editing.as_ref().filter(|e| e.0 == index);
                fill(pm, r, FIELD, 6.0);
                let edge = if editing.is_some() {
                    ACCENT
                } else {
                    FIELD_EDGE
                };
                outline(pm, r, edge, 6.0, if editing.is_some() { 2.0 } else { 1.0 });
                let (text, cursor) = match editing {
                    Some((_, f)) => (
                        f.text().to_string(),
                        Some(fonts.measure(f.before_cursor(), &TEXT)),
                    ),
                    None => (value.clone(), None),
                };
                fonts.draw(pm, &text, &TEXT, r.0 + 8.0, r.1 + 4.0, Some(fw - 16.0));
                if let Some(cx) = cursor {
                    let cx = (r.0 + 8.0 + cx).min(r.0 + fw - 6.0);
                    fill(pm, (cx, r.1 + 5.0, 1.5, CONTROL_H - 10.0), WHITE, 0.0);
                }
                self.hits
                    .push(((content_x + r.0, r.1, r.2, r.3), Hit::Control(index)));
            }
        }
        popup
    }

    fn draw_popup(&mut self, pm: &mut Canvas, fonts: &mut Fonts, row: usize, (x, y, bw): Rect3) {
        let rows = self.rows();
        let Some(Control::Choice(options)) = rows.get(row).map(|r| r.control.clone()) else {
            return;
        };
        let current = rows
            .get(row)
            .map(|r| self.value(r.key).to_string())
            .unwrap_or_default();
        let widest = options
            .iter()
            .map(|o| fonts.measure(&o.1, &TEXT))
            .fold(0.0, f32::max);
        let mw = (widest + 44.0).max(bw);
        let mh = options.len() as f32 * OPTION_H + 10.0;
        let h = pm.height() as f32;
        let w = pm.width() as f32;
        // Below the button, or above it when there is no room; never off
        // the window.
        let my = if y + mh <= h - 8.0 {
            y
        } else {
            (y - CONTROL_H - 8.0 - mh).max(8.0)
        };
        let mx = (x + bw - mw).clamp(8.0, (w - mw - 8.0).max(8.0));
        let r = (mx, my, mw, mh);
        fill(pm, r, MENU, 8.0);
        outline(pm, r, MENU_EDGE, 8.0, 1.0);
        for (i, (value, label)) in options.iter().enumerate() {
            let oy = my + 5.0 + i as f32 * OPTION_H;
            let o = (mx + 5.0, oy, mw - 10.0, OPTION_H);
            if self.hover == Some(Hit::Option(i)) {
                fill(pm, o, ACCENT, 5.0);
            }
            if *value == current {
                check(pm, o.0 + 12.0, oy + OPTION_H / 2.0);
            }
            fonts.draw(pm, label, &TEXT, o.0 + 26.0, oy + 4.0, Some(o.2 - 32.0));
            self.hits
                .push(((SIDEBAR_W + o.0, o.1, o.2, o.3), Hit::Option(i)));
        }
    }

    fn draw_wallpapers(
        &mut self,
        pm: &mut Canvas,
        fonts: &mut Fonts,
        list: &[(String, String)],
        mut y: f32,
        w: f32,
    ) -> f32 {
        let (tw, th) = (THUMB.0 as f32, THUMB.1 as f32);
        let per_row = (((w - 2.0 * PAD + THUMB_GAP) / (tw + THUMB_GAP)).floor() as usize).max(1);
        let current = self.value("look.wallpaper").to_string();
        for (i, (value, label)) in list.iter().enumerate() {
            let (col, line) = (i % per_row, i / per_row);
            let x = PAD + col as f32 * (tw + THUMB_GAP);
            let ty = y + line as f32 * (th + 44.0);
            let r = (x, ty, tw, th);
            if *value == current {
                outline(
                    pm,
                    (x - 4.0, ty - 4.0, tw + 8.0, th + 8.0),
                    ACCENT,
                    11.0,
                    3.0,
                );
            }
            match self.thumbs.get(value) {
                Some(thumb) => pm.draw_icon(thumb.as_ref(), x, ty, &PixmapPaint::default()),
                None => fill(pm, r, BOX, 8.0),
            }
            let lw = fonts.measure(label, &TEXT).min(tw);
            fonts.draw(
                pm,
                label,
                &TEXT,
                x + (tw - lw) / 2.0,
                ty + th + 10.0,
                Some(tw),
            );
            self.hits
                .push(((SIDEBAR_W + x, ty, tw, th + 30.0), Hit::Wallpaper(i)));
        }
        let lines = list.len().div_ceil(per_row) as f32;
        y += lines * (th + 44.0) + 8.0;
        let label = t(self.lang, "settings.wallpaper_pick");
        let bw = fonts.measure(label, &TEXT) + 32.0;
        self.button(
            pm,
            fonts,
            (PAD, y, bw, CONTROL_H + 2.0),
            label,
            Hit::Pick,
            false,
        );
        y + 40.0
    }

    fn button(
        &mut self,
        pm: &mut Canvas,
        fonts: &mut Fonts,
        r: Rect,
        label: &str,
        hit: Hit,
        accent: bool,
    ) {
        let hover = self.hover == Some(hit);
        let bg = match (accent, hover) {
            (true, _) => ACCENT,
            (false, true) => BUTTON_HOVER,
            (false, false) => BUTTON,
        };
        fill(pm, r, bg, 6.0);
        let lw = fonts.measure(label, &TEXT);
        fonts.draw(
            pm,
            label,
            &TEXT,
            r.0 + (r.2 - lw) / 2.0,
            r.1 + (r.3 - TEXT.line_height()) / 2.0,
            None,
        );
        self.hits.push(((SIDEBAR_W + r.0, r.1, r.2, r.3), hit));
    }
}

/// `(x, y, width)` of where a popup goes.
type Rect3 = (f32, f32, f32);

fn row_height(row: &Row) -> f32 {
    if row.hint.is_some() {
        ROW_H + HINT_H
    } else {
        ROW_H
    }
}

fn color(c: [u8; 4]) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], c[3])
}

fn fill(pm: &mut Canvas, (x, y, w, h): Rect, c: [u8; 4], r: f32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    if let Some(path) = rounded(x, y, w, h, r) {
        pm.fill_path(
            &path,
            &paint(c),
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn outline(pm: &mut Canvas, (x, y, w, h): Rect, c: [u8; 4], r: f32, width: f32) {
    let half = width / 2.0;
    if let Some(path) = rounded(x + half, y + half, w - width, h - width, r) {
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(c), &stroke, Transform::identity(), None);
    }
}

/// The up-and-down chevrons of a popup button, centred at `(x, y)`.
fn chevrons(pm: &mut Canvas, x: f32, y: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x - 3.5, y - 2.0);
    pb.line_to(x, y - 5.5);
    pb.line_to(x + 3.5, y - 2.0);
    pb.move_to(x - 3.5, y + 2.0);
    pb.line_to(x, y + 5.5);
    pb.line_to(x + 3.5, y + 2.0);
    line(pm, pb, [0xd0, 0xd0, 0xd6, 0xff], 1.5);
}

/// The check mark of the chosen option, centred at `(x, y)`.
fn check(pm: &mut Canvas, x: f32, y: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x - 4.5, y);
    pb.line_to(x - 1.5, y + 3.5);
    pb.line_to(x + 4.5, y - 3.5);
    line(pm, pb, WHITE, 1.8);
}

fn line(pm: &mut Canvas, pb: PathBuilder, c: [u8; 4], width: f32) {
    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(c), &stroke, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::values;
    use mimshel_core::config::Config;

    fn view() -> SettingsView {
        SettingsView::new(Language::Ru, values(&Config::default()), Env::default())
    }

    fn drawn(v: &mut SettingsView) {
        let mut fonts = Fonts::new("Inter");
        let mut icons = Icons::new("hicolor");
        let pm = v.render(SIZE, &mut fonts, &mut icons);
        assert_eq!((pm.width(), pm.height()), SIZE);
    }

    /// The centre of the first place that gives `hit`.
    fn at(v: &SettingsView, hit: Hit) -> (f32, f32) {
        let (r, _) = v.hits.iter().find(|(_, h)| *h == hit).expect("drawn");
        (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
    }

    #[test]
    fn pages_switch_from_the_sidebar_and_keys() {
        let mut v = view();
        drawn(&mut v);
        let (x, y) = at(&v, Hit::Page(3));
        assert_eq!(v.press(x, y), Action::Redraw);
        assert_eq!(v.pages[v.page].id, "dock");
        v.key(KeyPress::plain(Key::Up));
        assert_eq!(v.pages[v.page].id, "menubar");
    }

    #[test]
    fn a_choice_opens_a_popup_and_sets_the_value() {
        let mut v = view();
        v.show_page("dock");
        drawn(&mut v);
        // Rows of the dock page: icon size, magnify, amount, hide.
        let (x, y) = at(&v, Hit::Control(3));
        assert_eq!(v.press(x, y), Action::Redraw);
        drawn(&mut v);
        let (x, y) = at(&v, Hit::Option(0));
        assert_eq!(v.press(x, y), Action::Set("dock.hide", "never".into()));
        assert_eq!(v.value("dock.hide"), "never");
    }

    #[test]
    fn switch_and_slider() {
        let mut v = view();
        v.show_page("dock");
        drawn(&mut v);
        // Dragging the size slider to its right end writes the maximum.
        let (x0, w) = v.tracks[&0];
        let (_, y) = at(&v, Hit::Control(0));
        assert_eq!(v.press(x0 + 1.0, y), Action::Redraw);
        assert_eq!(v.motion(x0 + w + 50.0, y), Action::Redraw);
        assert_eq!(v.release(), Action::Set("dock.icon_size", "128".into()));
        // Magnification off: the amount row goes away.
        drawn(&mut v);
        let (x, y) = at(&v, Hit::Control(1));
        assert_eq!(v.press(x, y), Action::Set("dock.zoom", "false".into()));
        assert_eq!(v.rows().len(), 3);
        drawn(&mut v);
        let (x, y) = at(&v, Hit::Control(1));
        assert_eq!(v.press(x, y), Action::Set("dock.zoom", "140".into()));
    }

    #[test]
    fn text_is_written_on_return_or_when_left() {
        let mut v = view();
        v.show_page("lock");
        drawn(&mut v);
        let (x, y) = at(&v, Hit::Control(2));
        assert_eq!(v.press(x, y), Action::Redraw);
        // The whole old text is selected: typing replaces it.
        for c in "Пароль".chars() {
            v.key(KeyPress::plain(Key::Char(c)));
        }
        assert_eq!(
            v.key(KeyPress::plain(Key::Return)),
            Action::Set("lock.placeholder", "Пароль".into())
        );
        // Escape leaves without writing.
        drawn(&mut v);
        let (x, y) = at(&v, Hit::Control(2));
        v.press(x, y);
        v.key(KeyPress::plain(Key::Char('x')));
        assert_eq!(v.key(KeyPress::plain(Key::Escape)), Action::Redraw);
        assert_eq!(v.value("lock.placeholder"), "Пароль");
    }

    #[test]
    fn links_open_their_program() {
        let env = Env {
            links: vec!["displays"],
            ..Env::default()
        };
        let mut v = SettingsView::new(Language::En, values(&Config::default()), env);
        drawn(&mut v);
        let last = v.pages.len() - 1;
        let (x, y) = at(&v, Hit::Page(last));
        assert_eq!(
            v.press(x, y),
            Action::Open(vec!["xfce4-display-settings".into()])
        );
        assert_ne!(v.page, last, "the page stays");
        assert_eq!(
            v.key(KeyPress {
                ctrl: true,
                ..KeyPress::plain(Key::Char('w'))
            }),
            Action::Close
        );
    }

    #[test]
    fn wallpaper_pick() {
        let mut v = view();
        v.show_page("wallpaper");
        drawn(&mut v);
        let (x, y) = at(&v, Hit::Wallpaper(2));
        assert_eq!(
            v.press(x, y),
            Action::Set("look.wallpaper", "aurora".into())
        );
        drawn(&mut v);
        let (x, y) = at(&v, Hit::Pick);
        assert_eq!(v.press(x, y), Action::PickWallpaper);
    }
}
