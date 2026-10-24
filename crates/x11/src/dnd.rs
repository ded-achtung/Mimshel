//! Перетаскивание файлов из других программ (Проводник и др.) на наше окно —
//! протокол XDND (freedesktop, версия 5):
//! 1. окно объявляет, что принимает перетаскивание (`XdndAware`);
//! 2. программа-источник шлёт `XdndPosition` — отвечаем `XdndStatus`
//!    («беру», с тем же действием: переместить или скопировать);
//! 3. `XdndDrop` — просим у источника список (`XdndSelection`,
//!    `text/uri-list`); пришёл — сообщаем источнику `XdndFinished`, а службе —
//!    [`Event::Dropped`]: что и где бросили. Пути забирает [`Display::take_drop`].
//!
//! И в обратную сторону — значки со стола в окно другой программы (мы —
//! источник): [`Display::drag_begin`], [`Display::drag_motion`] на каждое
//! движение (шлём `XdndEnter`/`XdndPosition`/`XdndLeave` окну под указателем,
//! которое объявило `XdndAware`), [`Display::drag_end`] — `XdndDrop`; список
//! программа просит у нас через `XdndSelection`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, PropMode, SelectionRequestEvent,
    Window,
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
    /// Источник предлагает и перемещение (`XdndActionList`).
    can_move: bool,
    /// Тащим мы сами — со стола.
    out: DragOut,
}

/// Что под указателем, пока тащим значки со стола.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragTarget {
    /// Сам стол — значки переставляются по нему.
    #[default]
    Own,
    /// Окно программы, принимающей файлы; `accepts` — согласна ли взять.
    App { accepts: bool },
    /// Окно, которое файлы не принимает (док, строка меню…).
    Nothing,
}

/// Перетаскивание со стола в другую программу.
#[derive(Default)]
struct DragOut {
    /// Наше окно-источник; 0 — не тащим.
    source: Window,
    uris: String,
    /// Окно программы под указателем и версия протокола у неё.
    target: Window,
    version: u32,
    last: DragTarget,
    accepted: bool,
    /// Ждём XdndStatus на прошлое XdndPosition — следующее пока придержим.
    waiting: bool,
    queued: Option<[u32; 5]>,
    /// Что сейчас разрешено в `XdndActionList`: `Some(true)` — только копия.
    only_copy: Option<bool>,
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

    /// Начали тащить значки `paths` из окна `source`.
    pub fn drag_begin(&self, source: Window, paths: &[PathBuf]) {
        let a = &self.atoms;
        let uris: String = paths
            .iter()
            .map(|p| {
                format!(
                    "file://{}\r\n",
                    crate::clipboard::percent_encode(&p.to_string_lossy())
                )
            })
            .collect();
        let _ = self
            .conn
            .set_selection_owner(source, a.XdndSelection, x11rb::CURRENT_TIME);
        let _ = self.conn.flush();
        let mut st = self.drop.lock().unwrap_or_else(|p| p.into_inner());
        st.out = DragOut {
            source,
            uris,
            ..DragOut::default()
        };
    }

    /// Указатель с значками — в точке `x, y` экрана (в точках); `copy` —
    /// скопировать (зажат Ctrl), иначе переместить.
    pub fn drag_motion(&self, x: i16, y: i16, copy: bool) -> DragTarget {
        let a = &self.atoms;
        let at = |v: i16| (f32::from(v) * self.scale).round() as i16;
        let (px, py) = (at(x), at(y));
        let mut st = self.drop.lock().unwrap_or_else(|p| p.into_inner());
        let out = &mut st.out;
        if out.source == 0 {
            return DragTarget::Own;
        }
        let (hit, version) = self.drop_window_at(out.source, px, py);
        if hit != out.target {
            if out.target != 0 {
                self.send_client(out.target, a.XdndLeave, [out.source, 0, 0, 0, 0]);
            }
            out.target = hit;
            out.version = version.min(VERSION);
            out.accepted = false;
            out.waiting = false;
            out.queued = None;
            if hit != 0 {
                self.send_client(
                    hit,
                    a.XdndEnter,
                    [out.source, out.version << 24, a.TEXT_URI_LIST, 0, 0],
                );
            }
        }
        if out.target == 0 {
            out.last = if version == u32::MAX {
                DragTarget::Own
            } else {
                DragTarget::Nothing
            };
            return out.last;
        }
        // Проводник выбирает сам из разрешённых (на том же диске —
        // перемещение), поэтому с Ctrl разрешаем только копию.
        if out.only_copy != Some(copy) {
            out.only_copy = Some(copy);
            let list: &[u32] = if copy {
                &[a.XdndActionCopy]
            } else {
                &[a.XdndActionMove, a.XdndActionCopy]
            };
            let _ = self.conn.change_property32(
                PropMode::REPLACE,
                out.source,
                a.XdndActionList,
                AtomEnum::ATOM,
                list,
            );
        }
        let action = if copy {
            a.XdndActionCopy
        } else {
            a.XdndActionMove
        };
        let at = (u32::from(px as u16) << 16) | u32::from(py as u16);
        let msg = [out.source, 0, at, x11rb::CURRENT_TIME, action];
        if out.waiting {
            out.queued = Some(msg);
        } else {
            out.waiting = true;
            self.send_client(out.target, a.XdndPosition, msg);
        }
        out.last = DragTarget::App {
            accepts: out.accepted,
        };
        out.last
    }

    /// Отпустили кнопку. Над согласной программой — бросаем ей файлы
    /// (дальше она сама: переместит или скопирует); иначе — отменяем.
    pub fn drag_end(&self) -> DragTarget {
        let a = &self.atoms;
        let mut st = self.drop.lock().unwrap_or_else(|p| p.into_inner());
        let out = &mut st.out;
        let last = out.last;
        if out.target != 0 {
            if out.accepted {
                self.send_client(
                    out.target,
                    a.XdndDrop,
                    [out.source, 0, x11rb::CURRENT_TIME, 0, 0],
                );
                // Список отдадим по запросу; источник забудем по XdndFinished.
                out.waiting = false;
                out.queued = None;
                return DragTarget::App { accepts: true };
            }
            self.send_client(out.target, a.XdndLeave, [out.source, 0, 0, 0, 0]);
            out.source = 0;
            return DragTarget::App { accepts: false };
        }
        out.source = 0;
        last
    }

    /// Окно, принимающее перетаскивание, под точкой (в пикселях): спускаемся
    /// от корня по окнам под указателем (картинка, которую тащат, мышь
    /// пропускает). Наше окно — `(0, u32::MAX)`, не принимает — `(0, 0)`.
    fn drop_window_at(&self, own: Window, x: i16, y: i16) -> (Window, u32) {
        let mut parent = self.root;
        for _ in 0..8 {
            let Some(child) = self
                .conn
                .translate_coordinates(self.root, parent, x, y)
                .ok()
                .and_then(|c| c.reply().ok())
                .map(|r| r.child)
                .filter(|&c| c != x11rb::NONE)
            else {
                break;
            };
            if child == own {
                return (0, u32::MAX);
            }
            let version = self
                .conn
                .get_property(false, child, self.atoms.XdndAware, AtomEnum::ATOM, 0, 1)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|r| r.value32().and_then(|mut v| v.next()));
            if let Some(v) = version.filter(|&v| v >= 3) {
                return (child, v);
            }
            parent = child;
        }
        (0, 0)
    }

    /// Программа, куда бросили, просит список файлов.
    pub(crate) fn dnd_request(&self, req: &SelectionRequestEvent) {
        use crate::clipboard::{Data, answer};
        let a = &self.atoms;
        let st = self.drop.lock().unwrap_or_else(|p| p.into_inner());
        let data = if st.out.uris.is_empty() {
            None
        } else if req.target == a.TEXT_URI_LIST {
            Some((a.TEXT_URI_LIST, Data::Text(st.out.uris.clone())))
        } else if req.target == a.TARGETS {
            Some((
                AtomEnum::ATOM.into(),
                Data::Atoms(vec![a.TARGETS, a.TEXT_URI_LIST]),
            ))
        } else {
            None
        };
        let _ = answer(&self.conn, req, data);
    }

    /// Предлагает ли источник перемещение (свойство `XdndActionList`).
    fn offers_move(&self, source: Window) -> bool {
        self.conn
            .get_property(
                false,
                source,
                self.atoms.XdndActionList,
                AtomEnum::ATOM,
                0,
                16,
            )
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| {
                r.value32()
                    .map(|mut v| v.any(|x| x == self.atoms.XdndActionMove))
            })
            .unwrap_or(false)
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
        // Ответы программы, куда тащим со стола.
        if window == st.out.source && st.out.source != 0 {
            let out = &mut st.out;
            if kind == a.XdndStatus && data[0] == out.target {
                out.accepted = data[1] & 1 != 0;
                out.waiting = false;
                if let Some(msg) = out.queued.take() {
                    out.waiting = true;
                    self.send_client(out.target, a.XdndPosition, msg);
                }
                return Some(Event::Other);
            }
            if kind == a.XdndFinished && data[0] == out.target {
                out.source = 0;
                out.target = 0;
                return Some(Event::Other);
            }
        }
        if kind == a.XdndEnter {
            st.source = data[0];
            st.target = window;
            st.can_move = self.offers_move(data[0]);
            return Some(Event::Other);
        }
        if kind == a.XdndPosition {
            st.source = data[0];
            st.target = window;
            // Источник присылает точку экрана — переводим в точку окна (стол
            // во весь экран с угла, а окно папки — где угодно).
            let (rx, ry) = ((data[2] >> 16) as i16, (data[2] & 0xffff) as i16);
            st.at = self
                .conn
                .translate_coordinates(self.root, window, rx, ry)
                .ok()
                .and_then(|c| c.reply().ok())
                .map_or((rx, ry), |r| (r.dst_x, r.dst_y));
            // Как в Finder: без Ctrl — переместить, если источник это
            // разрешает (Проводник предлагает «копировать», но в списке
            // действий есть и «переместить»); с Ctrl — скопировать.
            let ctrl = self.modifiers().0;
            let can_move = data[4] == a.XdndActionMove || st.can_move;
            st.action = if can_move && !ctrl {
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
