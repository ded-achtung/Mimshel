//! Окна программ по стандартам EWMH/ICCCM: список, сведения, иконка,
//! положение; переключиться на окно, свернуть его.

use shell_core::windows::WindowInfo;
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ImageFormat;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt as _, EventMask, Window,
};

use crate::{Display, Result};

/// IconicState из ICCCM — «свернуть».
const ICONIC_STATE: u32 = 3;

impl Display {
    pub fn root(&self) -> Window {
        self.root
    }

    /// Получать события о списке окон, активном окне и размере экрана.
    pub fn watch_root(&self) -> Result<()> {
        let mask = EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY;
        self.conn.change_window_attributes(
            self.root,
            &ChangeWindowAttributesAux::new().event_mask(mask),
        )?;
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
        self.conn.change_window_attributes(
            self.root,
            &ChangeWindowAttributesAux::new().event_mask(mask),
        )?;
        self.conn.flush()?;
        Ok(())
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

    /// Иконка окна (`_NET_WM_ICON`) размером `size`×`size` — для программ без
    /// ярлыка. Берётся наименьшая не меньше нужной, иначе самая большая.
    pub fn window_icon(&self, w: Window, size: u32) -> Option<Pixmap> {
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
        Some((x, y, width.max(1), height.max(1)))
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

    /// Где указатель мыши на экране.
    pub fn pointer_position(&self) -> Option<(i16, i16)> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((r.root_x, r.root_y))
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
