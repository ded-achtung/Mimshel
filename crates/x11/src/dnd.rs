//! Перетаскивание файлов из других программ (Проводник и др.) на наше окно —
//! протокол XDND (freedesktop, версия 5):
//! 1. окно объявляет, что принимает перетаскивание (`XdndAware`);
//! 2. программа-источник шлёт `XdndPosition` — отвечаем `XdndStatus`
//!    («беру», с тем же действием: переместить или скопировать);
//! 3. `XdndDrop` — просим у источника список (`XdndSelection`,
//!    `text/uri-list`); пришёл — сообщаем источнику `XdndFinished`, а службе —
//!    [`Event::Dropped`]: что и где бросили. Пути забирает [`Display::take_drop`].

use std::collections::HashMap;
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

/// Параметры, подтверждённые последним XdndStatus.
#[derive(Clone, Copy)]
struct DropOffer {
    source: Window,
    target: Window,
    at: (i16, i16),
    action: u32,
}

/// Файловая операция может продолжаться после получения списка URI.
struct PendingDrop {
    ticket: u64,
    offer: DropOffer,
}

const MAX_PENDING_DROPS: usize = 16;

/// Текущее перетаскивание, ожидаемый SelectionNotify и незавершённые операции.
#[derive(Default)]
pub(crate) struct DropState {
    source: Window,
    target: Window,
    at: (i16, i16),
    action: u32,
    selecting: Option<DropOffer>,
    pending: Vec<PendingDrop>,
    paths: HashMap<u64, Vec<PathBuf>>,
    next_ticket: u64,
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

    /// Пути конкретного броска; другой параллельный перенос их не затронет.
    pub fn take_drop(&self, ticket: u64) -> Vec<PathBuf> {
        self.drop
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .paths
            .remove(&ticket)
            .unwrap_or_default()
    }

    /// Сообщить источнику XDND результат только после файловой операции.
    pub fn finish_drop(&self, ticket: u64, success: bool) {
        let mut st = self.drop.lock().unwrap_or_else(|p| p.into_inner());
        let Some(index) = st.pending.iter().position(|p| p.ticket == ticket) else {
            return;
        };
        let offer = st.pending.remove(index).offer;
        st.paths.remove(&ticket);
        let action = if success { offer.action } else { x11rb::NONE };
        self.send_client(
            offer.source,
            self.atoms.XdndFinished,
            [offer.target, u32::from(success), action, 0, 0],
        );
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
            // Пока сервер возвращает SelectionNotify, запомним исходный запрос.
            // Новый XdndEnter не должен подменить источник этого броска.
            if st.selecting.is_some() {
                self.send_client(data[0], a.XdndFinished, [window, 0, 0, 0, 0]);
                return Some(Event::Other);
            }
            st.selecting = Some(DropOffer {
                source: data[0],
                target: window,
                at: st.at,
                action: st.action,
            });
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

    /// Данные URI получены, но XdndFinished отправится только после
    /// успешного завершения копирования/перемещения в рабочем потоке.
    pub(crate) fn dnd_selection(&self, requestor: Window, property: u32) -> Event {
        let a = &self.atoms;
        let mut st = self.drop.lock().unwrap_or_else(|p| p.into_inner());
        let Some(offer) = st.selecting.take() else {
            return Event::Other;
        };
        if requestor != offer.target {
            self.send_client(offer.source, a.XdndFinished, [offer.target, 0, 0, 0, 0]);
            return Event::Other;
        }
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
        if paths.is_empty() || st.pending.len() >= MAX_PENDING_DROPS {
            self.send_client(offer.source, a.XdndFinished, [offer.target, 0, 0, 0, 0]);
            return Event::Other;
        }
        st.next_ticket = st.next_ticket.wrapping_add(1).max(1);
        let ticket = st.next_ticket;
        st.pending.push(PendingDrop { ticket, offer });
        st.paths.insert(ticket, paths);
        let (x, y) = (self.pt(offer.at.0), self.pt(offer.at.1));
        Event::Dropped {
            window: offer.target,
            x,
            y,
            moving: offer.action == a.XdndActionMove,
            ticket,
        }
    }
}
