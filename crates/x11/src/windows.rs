//! Окна программ по стандартам EWMH/ICCCM: список, сведения, иконка,
//! положение; переключиться на окно, свернуть его.

use mimshel_core::windows::WindowInfo;
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::ImageFormat;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, EventMask, Window,
};

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::{Display, Result};

/// A rectangle: x, y, width, height.
type Rect = (i32, i32, u32, u32);
/// How long to wait for a new window to draw (`Display::undrawn_for`).
const NEW_WINDOW_DRAW_WAIT: Duration = Duration::from_secs(3);

/// A new window that has not drawn yet (`Display::watch_new_windows`).
pub(crate) struct Undrawn {
    damage: u32,
    since: Instant,
    /// Points read at map time and their pixels then (window coordinates).
    samples: Samples,
    /// Where it is on the screen, once seen mapped and empty.
    rect: Option<ScreenRect>,
}

/// `(x, y, w, h)` on the screen, in pixels.
type ScreenRect = (i32, i32, u32, u32);

/// Points of a window and their pixels.
type Samples = Vec<((i16, i16), u32)>;

/// Five points inside a `w`×`h` window, away from the edges (client-side
/// shadows, title bars): the centre and halfway towards the corners.
fn sample_points(w: u16, h: u16) -> Vec<(i16, i16)> {
    let (w, h) = (i32::from(w), i32::from(h));
    [(1, 1), (1, 1), (3, 1), (1, 3), (3, 3)]
        .iter()
        .enumerate()
        .map(|(i, (fx, fy))| {
            if i == 0 {
                (w / 2, h / 2)
            } else {
                (w * fx / 4, h * fy / 4)
            }
        })
        .map(|(x, y)| {
            (
                x.clamp(0, i16::MAX.into()) as i16,
                y.clamp(0, i16::MAX.into()) as i16,
            )
        })
        .collect()
}
/// Monitors with the «primary» mark, as `Display::monitor_list` reads them.
pub(crate) type MonitorList = Vec<(bool, Rect)>;

/// `now` differs from what was `seen` (nothing seen yet counts as a change);
/// `seen` becomes `now`.
fn layout_changed(seen: &mut Option<MonitorList>, now: MonitorList) -> bool {
    let changed = seen.as_ref() != Some(&now);
    *seen = Some(now);
    changed
}

/// The primary monitor of `list`, else the first; none — the whole screen.
fn pick_primary(list: &[(bool, Rect)], screen: (u32, u32)) -> Rect {
    list.iter()
        .find(|(primary, _)| *primary)
        .or_else(|| list.first())
        .map_or((0, 0, screen.0, screen.1), |(_, r)| *r)
}

/// IconicState из ICCCM — «свернуть».
const ICONIC_STATE: u32 = 3;

impl Display {
    pub fn root(&self) -> Window {
        self.root
    }

    /// Геометрия активных мониторов непосредственно из RandR, без
    /// запуска внешнего xrandr при событиях изменения размера.
    pub fn monitors(&self) -> Vec<(i32, i32, u32, u32)> {
        self.monitor_list().into_iter().map(|(_, r)| r).collect()
    }

    /// Did the monitors change since the last call (or since `watch_root`)?
    /// Remembers the new list.
    pub(crate) fn monitors_changed(&self) -> bool {
        let now = self.monitor_list();
        let mut seen = self.monitors_seen.lock().unwrap_or_else(|p| p.into_inner());
        layout_changed(&mut seen, now)
    }

    /// Monitors with the «primary» mark, in pixels.
    fn monitor_list(&self) -> Vec<(bool, Rect)> {
        self.conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|request| request.reply().ok())
            .map(|reply| {
                reply
                    .monitors
                    .into_iter()
                    .filter(|m| m.width > 0 && m.height > 0)
                    .map(|m| {
                        (
                            m.primary,
                            (
                                i32::from(m.x),
                                i32::from(m.y),
                                u32::from(m.width),
                                u32::from(m.height),
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The monitor the menu bar and the dock live on, in points: the one
    /// marked primary (Xfce «Display» settings), else the first; without
    /// RandR — the whole screen. With a second monitor the bar and the
    /// dock used to stretch over both screens.
    pub fn primary_area(&self) -> Rect {
        let (sw, sh) = self.screen_pixels();
        let (x, y, w, h) = pick_primary(&self.monitor_list(), (u32::from(sw), u32::from(sh)));
        let pt = |v: f32| (v / self.scale).round();
        (
            pt(x as f32) as i32,
            pt(y as f32) as i32,
            pt(w as f32) as u32,
            pt(h as f32) as u32,
        )
    }

    /// Доступная рабочая область по EWMH в физических пикселях. У xfwm4 она
    /// учитывает резервы панелей и Dock. Нет свойства — вызывающий применит
    /// прежние безопасные отступы.
    pub fn workarea(&self) -> Option<(i32, i32, u32, u32)> {
        let a = &self.atoms;
        let desk = self
            .cardinals(self.root, a._NET_CURRENT_DESKTOP, AtomEnum::CARDINAL)
            .first()
            .copied()
            .unwrap_or(0) as usize;
        let all = self.cardinals(self.root, a._NET_WORKAREA, AtomEnum::CARDINAL);
        let index = desk.checked_mul(4)?;
        let r = all.get(index..index + 4)?;
        let (x, y, w, h) = (
            i32::try_from(r[0]).ok()?,
            i32::try_from(r[1]).ok()?,
            r[2],
            r[3],
        );
        (w > 0 && h > 0).then_some((x, y, w, h))
    }

    /// Keep the pointer on `window` while something is dragged with the
    /// button held: motion and the release come to it wherever the pointer
    /// is (coordinates relative to `window`). `false` — someone else holds
    /// the pointer.
    pub fn grab_pointer_for_drag(&self, window: Window) -> Result<bool> {
        use x11rb::protocol::xproto::{GrabMode, GrabStatus};
        let mask = EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        let status = self
            .conn
            .grab_pointer(
                false,
                window,
                mask,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                x11rb::NONE,
                x11rb::NONE,
                x11rb::CURRENT_TIME,
            )?
            .reply()?
            .status;
        Ok(status == GrabStatus::SUCCESS)
    }

    /// Let the pointer go after [`Display::grab_pointer_for_drag`].
    pub fn ungrab_pointer(&self) -> Result<()> {
        self.conn.ungrab_pointer(x11rb::CURRENT_TIME)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Получать события о списке окон, активном окне и размере экрана.
    /// Monitor changes come from RandR too: monitors rearranged or the
    /// primary one changed leave the root size as it was, so without them
    /// nothing would move. They arrive as `Event::Configure` of the root.
    pub fn watch_root(&self) -> Result<()> {
        let mask = EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY;
        self.conn.change_window_attributes(
            self.root,
            &ChangeWindowAttributesAux::new().event_mask(mask),
        )?;
        use x11rb::protocol::randr::NotifyMask;
        // No RandR (a bare X server): the root's own events still work.
        let _ = self.conn.randr_select_input(
            self.root,
            NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE | NotifyMask::OUTPUT_CHANGE,
        );
        // The starting point for `monitors_changed`.
        let now = self.monitor_list();
        *self.monitors_seen.lock().unwrap_or_else(|p| p.into_inner()) = Some(now);
        self.conn.flush()?;
        Ok(())
    }

    /// Получать события о появлении и перестановке любых окон на экране и о
    /// смене его размера ([`Event::Mapped`], [`Event::Configure`]) — для экрана
    /// блокировки, который должен оставаться поверх всего. Добавляет к уже
    /// выбранным событиям корневого окна (урок 11).
    pub fn watch_stacking(&self) -> Result<()> {
        let mine = self
            .conn
            .get_window_attributes(self.root)?
            .reply()?
            .your_event_mask;
        let mask = mine | EventMask::SUBSTRUCTURE_NOTIFY | EventMask::STRUCTURE_NOTIFY;
        self.stacking.store(true, Ordering::Relaxed);
        self.conn.change_window_attributes(
            self.root,
            &ChangeWindowAttributesAux::new().event_mask(mask),
        )?;
        self.conn.flush()?;
        Ok(())
    }

    /// Know when a new program window draws for the first time
    /// ([`Event::Drawn`], [`Display::undrawn_for`]). Until then the server
    /// shows stale screen contents in it — Chrome takes about a second — and
    /// anything that moves over it (the dock sliding away) leaves frozen
    /// copies of itself there. Windows are caught at creation, before they
    /// can draw, so no first drawing is missed. Without DAMAGE — nothing.
    pub fn watch_new_windows(&self) -> Result<()> {
        use x11rb::protocol::damage::ConnectionExt as _;
        let damage = self
            .conn
            .damage_query_version(1, 1)
            .ok()
            .and_then(|c| c.reply().ok());
        if damage.is_none() {
            return Ok(());
        }
        let mine = self
            .conn
            .get_window_attributes(self.root)?
            .reply()?
            .your_event_mask;
        self.new_windows.store(true, Ordering::Relaxed);
        self.conn.change_window_attributes(
            self.root,
            &ChangeWindowAttributesAux::new().event_mask(mine | EventMask::SUBSTRUCTURE_NOTIFY),
        )?;
        // Wait until the server has it: a window created right after this
        // call must not be missed.
        self.conn.get_input_focus()?.reply()?;
        Ok(())
    }

    /// The window was created after `watch_new_windows` and has not drawn
    /// yet: how much longer to wait for it. A program that draws nothing for
    /// this long is not waited for any more.
    pub fn undrawn_for(&self, window: Window) -> Option<Duration> {
        let mut undrawn = self.undrawn.lock().unwrap_or_else(|p| p.into_inner());
        let since = undrawn.get(&window)?.since;
        match NEW_WINDOW_DRAW_WAIT.checked_sub(since.elapsed()) {
            Some(left) if !left.is_zero() => Some(left),
            _ => {
                if let Some(u) = undrawn.remove(&window) {
                    self.destroy_damage(u.damage);
                }
                None
            }
        }
    }

    /// A new window that has not drawn yet covers part of `(x, y, w, h)`
    /// (screen pixels): how much longer to wait for it. Until it draws, it
    /// shows a frozen copy of what was under it when it appeared — anything
    /// that changes under it there (a dock icon bouncing, the dock sliding
    /// away) leaves its old picture visible through the window.
    pub fn undrawn_over(&self, (x, y, w, h): (i32, i32, u32, u32)) -> Option<Duration> {
        let covering: Vec<Window> = {
            let undrawn = self.undrawn.lock().unwrap_or_else(|p| p.into_inner());
            undrawn
                .iter()
                .filter(|(_, u)| {
                    u.rect.is_some_and(|(rx, ry, rw, rh)| {
                        rx < x + w as i32
                            && x < rx + rw as i32
                            && ry < y + h as i32
                            && y < ry + rh as i32
                    })
                })
                .map(|(w, _)| *w)
                .collect()
        };
        covering
            .into_iter()
            .filter_map(|w| self.undrawn_for(w))
            .max()
    }

    pub(crate) fn is_undrawn(&self, window: Window) -> bool {
        self.undrawn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains_key(&window)
    }

    /// Caught at creation, before it can draw: watch its drawing (DAMAGE)
    /// and its own map (to look at what it shows then).
    pub(crate) fn track_new_window(&self, window: Window) {
        use x11rb::protocol::damage::{ConnectionExt as _, ReportLevel};
        let Ok(damage) = self.conn.generate_id() else {
            return;
        };
        if self
            .conn
            .damage_create(damage, window, ReportLevel::NON_EMPTY)
            .is_err()
        {
            return;
        }
        // Nothing of ours is selected on a window that was just created.
        let _ = self.conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        );
        let _ = self.conn.flush();
        self.undrawn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                window,
                Undrawn {
                    damage,
                    since: Instant::now(),
                    samples: Vec::new(),
                    rect: None,
                },
            );
    }

    /// The watched window was mapped. Read a few points of it: if they are
    /// still the window's empty start (fully transparent, or a copy of the
    /// screen under it), remember them and wait; otherwise it has drawn
    /// already (an opaque background counts) — forget it, `true`.
    pub(crate) fn mapped(&self, window: Window) -> bool {
        let empty = self.empty_start(window);
        let mut undrawn = self.undrawn.lock().unwrap_or_else(|p| p.into_inner());
        match empty {
            Some((samples, rect)) => {
                if let Some(u) = undrawn.get_mut(&window) {
                    u.samples = samples;
                    u.rect = Some(rect);
                }
                false
            }
            None => {
                if let Some(u) = undrawn.remove(&window) {
                    self.destroy_damage(u.damage);
                }
                true
            }
        }
    }

    /// The window watched by `damage` changed: if any sample point differs
    /// from its empty start, it drew — forget it, return it.
    pub(crate) fn damaged(&self, damage: u32) -> Option<Window> {
        use x11rb::protocol::damage::ConnectionExt as _;
        let (window, samples) = {
            let undrawn = self.undrawn.lock().unwrap_or_else(|p| p.into_inner());
            undrawn
                .iter()
                .find_map(|(w, u)| (u.damage == damage).then(|| (*w, u.samples.clone())))?
        };
        // Report the next change too.
        let _ = self.conn.damage_subtract(damage, 0u32, 0u32);
        if samples.is_empty() {
            // Its map went by unseen (it was mapped before we watched it):
            // look at it now — something of its own means it drew.
            return self.mapped(window).then_some(window);
        }
        let now = self.read_points(window, &samples.iter().map(|s| s.0).collect::<Vec<_>>());
        let drawn = now.len() != samples.len() || now.iter().zip(&samples).any(|(n, s)| *n != s.1);
        if !drawn {
            return None;
        }
        if let Some(u) = self
            .undrawn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&window)
        {
            self.destroy_damage(u.damage);
        }
        Some(window)
    }

    pub(crate) fn forget_new_window(&self, window: Window) {
        // A destroyed window takes its DAMAGE object with it.
        self.undrawn
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&window);
    }

    /// The window's sample points and pixels, and where it is on the screen,
    /// when it still shows nothing of its own; `None` when it already shows
    /// something (or cannot be read).
    fn empty_start(&self, window: Window) -> Option<(Samples, ScreenRect)> {
        let geo = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let at = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        let points = sample_points(geo.width, geo.height);
        let mine = self.read_points(window, &points);
        if mine.len() != points.len() {
            return None;
        }
        let empty = if geo.depth == 32 {
            // A window with transparency starts fully transparent.
            mine.iter().all(|p| p >> 24 == 0)
        } else {
            // Without a background it starts as a copy of the screen under it.
            let screen: Vec<(i16, i16)> = points
                .iter()
                .map(|(x, y)| (x + at.dst_x, y + at.dst_y))
                .collect();
            let under = self.read_root_points(&screen);
            under.len() == mine.len()
                && under
                    .iter()
                    .zip(&mine)
                    .all(|(a, b)| a & 0xff_ffff == b & 0xff_ffff)
        };
        let rect = (
            i32::from(at.dst_x),
            i32::from(at.dst_y),
            u32::from(geo.width),
            u32::from(geo.height),
        );
        empty.then(|| (points.into_iter().zip(mine).collect(), rect))
    }

    fn read_points(&self, drawable: Window, points: &[(i16, i16)]) -> Vec<u32> {
        use x11rb::protocol::xproto::ImageFormat;
        let cookies: Vec<_> = points
            .iter()
            .filter_map(|&(x, y)| {
                self.conn
                    .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, 1, 1, !0)
                    .ok()
            })
            .collect();
        cookies
            .into_iter()
            .filter_map(|c| c.reply().ok())
            .filter_map(|img| {
                let d = img.data.get(..4)?;
                Some(u32::from_le_bytes([d[0], d[1], d[2], d[3]]))
            })
            .collect()
    }

    fn read_root_points(&self, points: &[(i16, i16)]) -> Vec<u32> {
        self.read_points(self.root, points)
    }

    /// A root substructure event we only get because of `watch_new_windows`.
    pub(crate) fn substructure_only(&self, event: Window, window: Window) -> bool {
        event == self.root
            && window != self.root
            && self.new_windows.load(Ordering::Relaxed)
            && !self.stacking.load(Ordering::Relaxed)
    }

    fn destroy_damage(&self, damage: u32) {
        use x11rb::protocol::damage::ConnectionExt as _;
        let _ = self.conn.damage_destroy(damage);
        let _ = self.conn.flush();
    }

    /// Получать события об изменениях окна программы (свёрнуто, заголовок,
    /// иконка, перемещение). Набор событий окна у нашего соединения один на
    /// всех (урок 11 в CLAUDE.md): подписка заменила бы события, которые уже
    /// выбраны, — например, щелчки у нашего же окна строки меню. Поэтому
    /// добавляем к уже выбранным.
    pub fn watch_window(&self, w: Window) -> Result<()> {
        let mine = self
            .conn
            .get_window_attributes(w)?
            .reply()
            .map_or(EventMask::NO_EVENT, |a| a.your_event_mask);
        let mask = mine | EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY;
        self.conn
            .change_window_attributes(w, &ChangeWindowAttributesAux::new().event_mask(mask))?;
        Ok(())
    }

    fn cardinals(&self, w: Window, prop: u32, kind: impl Into<u32>) -> Vec<u32> {
        self.conn
            .get_property(false, w, prop, kind.into(), 0, u32::MAX / 4)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }

    fn text(&self, w: Window, prop: impl Into<u32>, kind: impl Into<u32>) -> Option<Vec<u8>> {
        let r = self
            .conn
            .get_property(false, w, prop.into(), kind.into(), 0, 4096)
            .ok()?
            .reply()
            .ok()?;
        (r.format == 8 && !r.value.is_empty()).then_some(r.value)
    }

    /// Окна программ, от нижнего к верхнему (последнее — сверху).
    pub fn client_list(&self) -> Vec<Window> {
        let a = &self.atoms;
        let list = self.cardinals(self.root, a._NET_CLIENT_LIST_STACKING, AtomEnum::WINDOW);
        if list.is_empty() {
            self.cardinals(self.root, a._NET_CLIENT_LIST, AtomEnum::WINDOW)
        } else {
            list
        }
    }

    pub fn active_window(&self) -> Option<Window> {
        self.cardinals(self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW)
            .first()
            .copied()
            .filter(|&w| w != 0)
    }

    /// Сведения об окне — или `None`, если в доке его показывать не надо
    /// (панели, меню, окна с пометкой «не в панели задач»).
    pub fn window_info(&self, w: Window) -> Option<WindowInfo> {
        let a = &self.atoms;
        let types = self.cardinals(w, a._NET_WM_WINDOW_TYPE, AtomEnum::ATOM);
        let ordinary = types.is_empty()
            || types
                .iter()
                .any(|&t| t == a._NET_WM_WINDOW_TYPE_NORMAL || t == a._NET_WM_WINDOW_TYPE_DIALOG);
        let state = self.cardinals(w, a._NET_WM_STATE, AtomEnum::ATOM);
        if !ordinary || state.contains(&a._NET_WM_STATE_SKIP_TASKBAR) {
            return None;
        }
        let class = self.text(w, AtomEnum::WM_CLASS, AtomEnum::STRING)?;
        let mut parts = class
            .split(|&b| b == 0)
            .map(|p| String::from_utf8_lossy(p).into_owned());
        let instance = parts.next().unwrap_or_default();
        let class = parts.next().unwrap_or_default();
        let title = self
            .text(w, a._NET_WM_NAME, a.UTF8_STRING)
            .or_else(|| self.text(w, AtomEnum::WM_NAME, AtomEnum::STRING))
            .map(|t| String::from_utf8_lossy(&t).into_owned())
            .unwrap_or_default();
        // Свёрнуто: так помечает EWMH; WM_STATE (ICCCM) — запасной признак.
        let minimized = state.contains(&a._NET_WM_STATE_HIDDEN)
            || self.cardinals(w, a.WM_STATE, a.WM_STATE).first() == Some(&ICONIC_STATE);
        Some(WindowInfo {
            id: w,
            instance,
            class,
            title,
            pid: self
                .cardinals(w, a._NET_WM_PID, AtomEnum::CARDINAL)
                .first()
                .copied(),
            minimized,
            maximized: state.contains(&a._NET_WM_STATE_MAXIMIZED_VERT)
                && state.contains(&a._NET_WM_STATE_MAXIMIZED_HORZ),
        })
    }

    /// Иконка окна (`_NET_WM_ICON`) размером `size`×`size` точек (картинка —
    /// в пикселях, как у `mimshel_icons::Icons::get`) — для программ без
    /// ярлыка. Берётся наименьшая не меньше нужной, иначе самая большая.
    pub fn window_icon(&self, w: Window, size: u32) -> Option<Pixmap> {
        let size = self.px(size);
        let data = self.cardinals(w, self.atoms._NET_WM_ICON, AtomEnum::CARDINAL);
        let mut icons = Vec::new();
        let mut rest = data.as_slice();
        while let [iw, ih, tail @ ..] = rest {
            let n = (*iw as usize).checked_mul(*ih as usize)?;
            if n == 0 || tail.len() < n {
                break;
            }
            icons.push((*iw, *ih, &tail[..n]));
            rest = &tail[n..];
        }
        let &(iw, ih, px) = icons
            .iter()
            .filter(|(iw, ih, _)| (*iw).min(*ih) >= size)
            .min_by_key(|(iw, ..)| *iw)
            .or_else(|| icons.iter().max_by_key(|(iw, ..)| *iw))?;
        // ARGB без предварительного умножения → RGBA с умножением (как в tiny-skia).
        let mut rgba = Vec::with_capacity(px.len() * 4);
        for &p in px {
            let [b, g, r, a] = p.to_le_bytes();
            let m = |c: u8| (u32::from(c) * u32::from(a) / 255) as u8;
            rgba.extend_from_slice(&[m(r), m(g), m(b), a]);
        }
        let src = Pixmap::from_vec(rgba, IntSize::from_wh(iw, ih)?)?;
        let mut out = Pixmap::new(size, size)?;
        let k = size as f32 / iw.max(ih) as f32;
        let paint = PixmapPaint {
            quality: FilterQuality::Bicubic,
            ..PixmapPaint::default()
        };
        out.draw_pixmap(
            0,
            0,
            src.as_ref(),
            &paint,
            Transform::from_scale(k, k),
            None,
        );
        Some(out)
    }

    /// Положение и размер окна вместе с рамкой, в координатах экрана — то,
    /// что видно на экране. Рамку рисует xfwm4 (`_NET_FRAME_EXTENTS`) или сама
    /// программа: тогда вокруг окна ещё и невидимая тень (`_GTK_FRAME_EXTENTS`,
    /// у Firefox около 25 пикселей) — её не считаем, иначе «умное» скрытие
    /// думало бы, что окно заходит на док.
    pub fn frame_geometry(&self, w: Window) -> Option<(i32, i32, u32, u32)> {
        let g = self.conn.get_geometry(w).ok()?.reply().ok()?;
        let t = self
            .conn
            .translate_coordinates(w, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        let (l, r, top, b) = self.extents(w, self.atoms._NET_FRAME_EXTENTS);
        let (sl, sr, st, sb) = self.extents(w, self.atoms._GTK_FRAME_EXTENTS);
        let x = i32::from(t.dst_x) - l as i32 + sl as i32;
        let y = i32::from(t.dst_y) - top as i32 + st as i32;
        let width = (u32::from(g.width) + l + r).saturating_sub(sl + sr);
        let height = (u32::from(g.height) + top + b).saturating_sub(st + sb);
        // В точки.
        let k = self.scale();
        let pt = |v: f32| (v / k).round();
        Some((
            pt(x as f32) as i32,
            pt(y as f32) as i32,
            (pt(width as f32) as u32).max(1),
            (pt(height as f32) as u32).max(1),
        ))
    }

    /// Поля окна (слева, справа, сверху, снизу) из свойства `atom`.
    fn extents(&self, w: Window, atom: u32) -> (u32, u32, u32, u32) {
        match self.cardinals(w, atom, AtomEnum::CARDINAL).as_slice() {
            [l, r, t, b, ..] => (*l, *r, *t, *b),
            _ => (0, 0, 0, 0),
        }
    }

    /// Снимок содержимого окна программы (без рамки). С композитором
    /// (xfwm4) окно рисуется в своё внеэкранное место, поэтому снимок верен,
    /// даже если окно закрыто другими. Свёрнутое окно не нарисовано — `None`.
    pub fn capture_window(&self, w: Window) -> Option<Pixmap> {
        let g = self.conn.get_geometry(w).ok()?.reply().ok()?;
        let mut data = match self.shm_get(w, g.width, g.height) {
            Some(Some(data)) => data,
            Some(None) => return None,
            None => {
                self.conn
                    .get_image(ImageFormat::Z_PIXMAP, w, 0, 0, g.width, g.height, !0)
                    .ok()?
                    .reply()
                    .ok()?
                    .data
            }
        };
        if data.len() != usize::from(g.width) * usize::from(g.height) * 4 {
            return None;
        }
        // BGRX → RGBA. Прозрачность не берём: у многих окон в ней мусор.
        for px in data.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            px[3] = 255;
        }
        let full = Pixmap::from_vec(
            data,
            IntSize::from_wh(u32::from(g.width), u32::from(g.height))?,
        )?;
        // Невидимую тень вокруг окна (рамку рисует программа) обрезаем — иначе
        // без прозрачности она стала бы чёрной каймой.
        let (l, r, t, b) = self.extents(w, self.atoms._GTK_FRAME_EXTENTS);
        if l + r + t + b == 0 {
            return Some(full);
        }
        let (cw, ch) = (
            full.width().checked_sub(l + r)?,
            full.height().checked_sub(t + b)?,
        );
        let rect = tiny_skia::IntRect::from_xywh(l as i32, t as i32, cw, ch)?;
        full.clone_rect(rect)
    }

    /// Текстовое свойство окна по имени (UTF-8), например `_GTK_UNIQUE_BUS_NAME`.
    pub fn window_text(&self, w: Window, name: &str) -> Option<String> {
        let atom = self
            .conn
            .intern_atom(true, name.as_bytes())
            .ok()?
            .reply()
            .ok()?
            .atom;
        if atom == 0 {
            return None;
        }
        self.text(w, atom, self.atoms.UTF8_STRING)
            .map(|t| String::from_utf8_lossy(&t).into_owned())
            .filter(|t| !t.is_empty())
    }

    /// Зажаты ли сейчас Ctrl и Shift (для Ctrl/Shift+щелчка).
    pub fn modifiers(&self) -> (bool, bool) {
        self.conn
            .query_pointer(self.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map_or((false, false), |r| {
                let m = u16::from(r.mask);
                (m & 0x4 != 0, m & 0x1 != 0)
            })
    }

    /// Где указатель мыши на экране (в точках).
    pub fn pointer_position(&self) -> Option<(i16, i16)> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((self.pt(r.root_x), self.pt(r.root_y)))
    }

    fn client_message(&self, w: Window, kind: u32, data: [u32; 5]) -> Result<()> {
        let ev = ClientMessageEvent::new(32, w, kind, data);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        self.conn.send_event(false, self.root, mask, ev)?;
        self.conn.flush()?;
        Ok(())
    }

    /// Переключиться на окно (свёрнутое — развернётся). `2` — «просит панель
    /// задач»: такую просьбу оконный менеджер выполняет без возражений.
    pub fn activate(&self, w: Window) -> Result<()> {
        self.client_message(w, self.atoms._NET_ACTIVE_WINDOW, [2, 0, 0, 0, 0])
    }

    /// Свернуть окно (ICCCM WM_CHANGE_STATE).
    pub fn minimize(&self, w: Window) -> Result<()> {
        self.client_message(w, self.atoms.WM_CHANGE_STATE, [ICONIC_STATE, 0, 0, 0, 0])
    }

    /// Попросить окно закрыться (как крестик в заголовке).
    pub fn close(&self, w: Window) -> Result<()> {
        self.client_message(w, self.atoms._NET_CLOSE_WINDOW, [0, 2, 0, 0, 0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_monitor_is_picked() {
        let laptop = (false, (0, 0, 1920, 1080));
        let external = (true, (1920, 0, 2560, 1440));
        assert_eq!(
            pick_primary(&[laptop, external], (4480, 1440)),
            (1920, 0, 2560, 1440)
        );
        let unmarked = (false, (1920, 0, 2560, 1440));
        assert_eq!(
            pick_primary(&[laptop, unmarked], (4480, 1440)),
            (0, 0, 1920, 1080)
        );
        assert_eq!(pick_primary(&[], (1280, 800)), (0, 0, 1280, 800));
    }

    #[test]
    fn only_real_monitor_changes_count() {
        let one = vec![(true, (0, 0, 1920, 1200))];
        let mut seen = None;
        // Nothing seen yet: the first list is a change.
        assert!(layout_changed(&mut seen, one.clone()));
        // The same list again — what a program querying the outputs causes.
        assert!(!layout_changed(&mut seen, one.clone()));
        assert!(!layout_changed(&mut seen, one.clone()));
        // A second monitor, then the primary mark moving: real changes.
        let two = vec![(true, (0, 0, 1920, 1200)), (false, (1920, 0, 2560, 1440))];
        assert!(layout_changed(&mut seen, two.clone()));
        assert!(!layout_changed(&mut seen, two));
        let moved = vec![(false, (0, 0, 1920, 1200)), (true, (1920, 0, 2560, 1440))];
        assert!(layout_changed(&mut seen, moved));
        // Back to one monitor.
        assert!(layout_changed(&mut seen, one));
    }
}
