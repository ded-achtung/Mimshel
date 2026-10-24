//! Перетаскивание файлов из других программ (Проводник и др.) на наше окно —
//! протокол XDND (freedesktop, версия 5):
//! 1. окно объявляет, что принимает перетаскивание (`XdndAware`);
//! 2. программа-источник шлёт `XdndPosition` — отвечаем `XdndStatus`
//!    («беру», с тем же действием: переместить или скопировать);
//! 3. `XdndDrop` — просим у источника список (`XdndSelection`,
//!    `text/uri-list`); пришёл — сообщаем источнику `XdndFinished`, а службе —
//!    [`Event::Dropped`]: что и где бросили. Пути забирает [`Display::take_drop`].

use std::path::PathBuf;
use std::sync::Mutex;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, PropMode, Window,
};
use x11rb::wrapper::ConnectionExt as _;

use crate::{Display, Event, Result};

/// Версия протокола, которую понимаем.
const VERSION: u32 = 5;

/// Перетаскивание, которое сейчас над нами.
#[derive(Default)]
pub(crate) struct DropState {
    /// Окно-источник и окно, над которым несут.
    source: Window,
    target: Window,
    /// Где указатель (в пикселях экрана) и какое действие предложено.
    at: (i16, i16),
    action: u32,
    /// Брошенные пути — ждут, пока служба их заберёт.
    paths: Vec<PathBuf>,
}

pub(crate) type DropCell = Mutex<DropState>;

impl Display {
    /// Окно принимает файлы, перетащенные из других программ.
    pub fn enable_drop(&self, window: Window) -> Result<()> {
        self.conn.change_property32(
            PropMode::REPLACE,
            window,
            self.atoms.XdndAware,
            AtomEnum::ATOM,
            &[VERSION],
        )?;
        self.conn.flush()?;
        Ok(())
    }

    /// Пути последнего броска (после [`Event::Dropped`]).
    pub fn take_drop(&self) -> Vec<PathBuf> {
        std::mem::take(&mut self.drop.lock().unwrap_or_else(|p| p.into_inner()).paths)
    }

    fn send_client(&self, to: Window, kind: u32, data: [u32; 5]) {
        let ev = ClientMessageEvent::new(32, to, kind, data);
        let _ = self.conn.send_event(false, to, EventMask::NO_EVENT, ev);
        let _ = self.conn.flush();
    }

    /// Сообщение XDND от источника → событие службе (или `None` — не XDND).
    pub(crate) fn dnd_message(&self, window: Window, kind: u32, data: [u32; 5]) -> Option<Event> {
        let a = &self.atoms;
        let mut st = self.drop.lock().unwrap_or_else(|p| p.into_inner());
        if kind == a.XdndEnter {
            st.source = data[0];
            st.target = window;
            return Some(Event::Other);
        }
        if kind == a.XdndPosition {
            st.source = data[0];
            st.target = window;
            st.at = ((data[2] >> 16) as i16, (data[2] & 0xffff) as i16);
            // Переместить или скопировать — как предложил источник (у
            // Проводника: перенос на том же диске — переместить, Ctrl —
            // скопировать); другое — копируем.
            st.action = if data[4] == a.XdndActionMove {
                a.XdndActionMove
            } else {
                a.XdndActionCopy
            };
            // 1 — берём, 2 — присылай положения и дальше.
            self.send_client(st.source, a.XdndStatus, [window, 3, 0, 0, st.action]);
            let (x, y) = (self.pt(st.at.0), self.pt(st.at.1));
            return Some(Event::DropHover { window, x, y });
        }
        if kind == a.XdndLeave {
            return Some(Event::DropLeave { window });
        }
        if kind == a.XdndDrop {
            let time = data[2];
            let _ = self.conn.convert_selection(
                window,
                a.XdndSelection,
                a.TEXT_URI_LIST,
                a.SHELL_XDND,
                time,
            );
            let _ = self.conn.flush();
            return Some(Event::Other);
        }
        None
    }

    /// Пришёл список брошенных файлов (ответ на просьбу из `XdndDrop`).
    pub(crate) fn dnd_selection(&self, requestor: Window, property: u32) -> Event {
        let a = &self.atoms;
        let mut st = self.drop.lock().unwrap_or_else(|p| p.into_inner());
        let paths = (property != x11rb::NONE)
            .then(|| {
                self.conn
                    .get_property(true, requestor, property, AtomEnum::ANY, 0, 1 << 20)
                    .ok()?
                    .reply()
                    .ok()
            })
            .flatten()
            .and_then(|r| crate::clipboard::parse(&String::from_utf8_lossy(&r.value)))
            .map(|(_, p)| p)
            .unwrap_or_default();
        let ok = u32::from(!paths.is_empty());
        let action = if ok == 1 { st.action } else { x11rb::NONE };
        self.send_client(st.source, a.XdndFinished, [st.target, ok, action, 0, 0]);
        let moving = st.action == a.XdndActionMove;
        st.paths = paths;
        let (x, y) = (self.pt(st.at.0), self.pt(st.at.1));
        Event::Dropped {
            window: st.target,
            x,
            y,
            moving,
        }
    }
}
