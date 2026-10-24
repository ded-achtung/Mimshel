//! Файлы в буфере обмена — в обе стороны:
//! - [`clipboard_files`] — что скопировали или вырезали в Проводнике (Thunar
//!   кладёт список в `x-special/gnome-copied-files`, другие программы — в
//!   `text/uri-list`);
//! - [`copy_files`] — «Копировать»/«Вырезать» на столе: поток держит буфер и
//!   отдаёт список тем же видом, пока буфер не займёт кто-то другой.
//!
//! Отдельные соединения с X: ожидание ответов не мешает основному потоку
//! событий службы.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

/// Файлы в буфере обмена: вырезаны ли они (`true` — переместить) и пути.
/// `None` — в буфере не файлы или его владелец не ответил.
pub fn clipboard_files() -> Option<(bool, Vec<PathBuf>)> {
    let (conn, screen) = RustConnection::connect(None).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let window = conn.generate_id().ok()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )
    .ok()?;
    let atom = |name: &str| -> Option<u32> {
        Some(
            conn.intern_atom(false, name.as_bytes())
                .ok()?
                .reply()
                .ok()?
                .atom,
        )
    };
    let clipboard = atom("CLIPBOARD")?;
    let into = atom("SHELL_CLIPBOARD")?;
    for target in ["x-special/gnome-copied-files", "text/uri-list"] {
        let target = atom(target)?;
        conn.convert_selection(window, clipboard, target, into, CURRENT_TIME)
            .ok()?;
        conn.flush().ok()?;
        if !wait_notify(&conn) {
            continue;
        }
        let reply = conn
            .get_property(true, window, into, AtomEnum::ANY, 0, 1 << 20)
            .ok()?
            .reply()
            .ok()?;
        if let Some(found) = parse(&String::from_utf8_lossy(&reply.value)) {
            return Some(found);
        }
    }
    None
}

/// Ждём ответа владельца буфера (до секунды). `false` — не ответил или
/// такого вида данных у него нет.
fn wait_notify(conn: &RustConnection) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        match conn.poll_for_event() {
            Ok(Some(XEvent::SelectionNotify(e))) => return e.property != NONE,
            Ok(Some(_)) => {}
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return false,
        }
    }
    false
}

/// Положить файлы в буфер обмена (`cut` — вырезаны: вставка переместит их).
/// Поток отвечает на просьбы вставить, пока буфер наш.
pub fn copy_files(paths: Vec<PathBuf>, cut: bool) {
    std::thread::spawn(move || {
        if let Err(e) = serve(&paths, cut) {
            eprintln!("Буфер обмена: {e}");
        }
    });
}

fn serve(paths: &[PathBuf], cut: bool) -> crate::Result<()> {
    let (conn, screen) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id()?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )?;
    let atom = |name: &str| -> crate::Result<Atom> {
        Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
    };
    let clipboard = atom("CLIPBOARD")?;
    let targets = atom("TARGETS")?;
    let gnome = atom("x-special/gnome-copied-files")?;
    let uri_list = atom("text/uri-list")?;
    let utf8 = atom("UTF8_STRING")?;
    let plain = atom("text/plain;charset=utf-8")?;
    conn.set_selection_owner(window, clipboard, CURRENT_TIME)?;
    if conn.get_selection_owner(clipboard)?.reply()?.owner != window {
        return Err("буфер обмена не удалось занять".into());
    }
    let uris: Vec<String> = paths
        .iter()
        .map(|p| format!("file://{}", percent_encode(&p.to_string_lossy())))
        .collect();
    let gnome_text = format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n"));
    let uri_text = uris.join("\r\n") + "\r\n";
    let plain_text = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    loop {
        match conn.wait_for_event()? {
            // Буфер занял кто-то другой — наш список больше не нужен.
            XEvent::SelectionClear(_) => return Ok(()),
            XEvent::SelectionRequest(req) => {
                let data: Option<(Atom, Data)> = if req.target == targets {
                    Some((
                        AtomEnum::ATOM.into(),
                        Data::Atoms(vec![targets, gnome, uri_list, utf8, plain]),
                    ))
                } else if req.target == gnome {
                    Some((gnome, Data::Text(gnome_text.clone())))
                } else if req.target == uri_list {
                    Some((uri_list, Data::Text(uri_text.clone())))
                } else if req.target == utf8 || req.target == plain {
                    Some((req.target, Data::Text(plain_text.clone())))
                } else {
                    None
                };
                answer(&conn, &req, data)?;
            }
            _ => {}
        }
    }
}

/// Что отдать просящей программе.
enum Data {
    Atoms(Vec<Atom>),
    Text(String),
}

/// Ответить просящей программе: данные — в её свойство, и сообщить.
fn answer(
    conn: &RustConnection,
    req: &SelectionRequestEvent,
    data: Option<(Atom, Data)>,
) -> crate::Result<()> {
    // Старые программы не указывают свойство — тогда пишут в «target».
    let property = if req.property == NONE {
        req.target
    } else {
        req.property
    };
    let property = match data {
        Some((kind, Data::Atoms(list))) => {
            conn.change_property32(PropMode::REPLACE, req.requestor, property, kind, &list)?;
            property
        }
        Some((kind, Data::Text(text))) => {
            conn.change_property8(
                PropMode::REPLACE,
                req.requestor,
                property,
                kind,
                text.as_bytes(),
            )?;
            property
        }
        None => NONE,
    };
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property,
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
    conn.flush()?;
    Ok(())
}

/// `copy`/`cut` первой строкой (у Thunar) и адреса `file://` по строке.
pub(crate) fn parse(text: &str) -> Option<(bool, Vec<PathBuf>)> {
    let mut cut = false;
    let mut paths = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match line {
            "cut" => cut = true,
            "copy" => {}
            l if l.starts_with('#') => {}
            l => {
                if let Some(p) = l.strip_prefix("file://") {
                    paths.push(PathBuf::from(percent_decode(p)));
                }
            }
        }
    }
    (!paths.is_empty()).then_some((cut, paths))
}

/// Путь → адрес `file://` (всё, кроме букв, цифр и `/-._~`, — `%XX`).
fn percent_encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `%D0%9F` → байты UTF-8.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thunar_and_uri_list() {
        let (cut, paths) = parse(
            "cut\nfile:///home/ded/%D0%A0%D0%B0%D0%B1%D0%BE%D1%87%D0%B8%D0%B9%20%D1%81%D1%82%D0%BE%D0%BB/a.txt\nfile:///tmp/b",
        )
        .unwrap();
        assert!(cut);
        assert_eq!(
            paths,
            [
                PathBuf::from("/home/ded/Рабочий стол/a.txt"),
                PathBuf::from("/tmp/b")
            ]
        );
        let (cut, paths) = parse("# комментарий\r\nfile:///tmp/c%20d\r\n").unwrap();
        assert!(!cut);
        assert_eq!(paths, [PathBuf::from("/tmp/c d")]);
        assert!(parse("просто текст").is_none());
        let p = "/home/ded/Рабочий стол/a b.txt";
        assert_eq!(percent_decode(&percent_encode(p)), p);
        assert!(!percent_encode(p).contains(' '));
    }
}
