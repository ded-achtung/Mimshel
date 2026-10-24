//! Файлы из буфера обмена: что скопировали или вырезали в Проводнике
//! (Thunar кладёт список в `x-special/gnome-copied-files`, другие программы —
//! в `text/uri-list`). Отдельное соединение с X: ответ владельца буфера
//! ждём здесь, не мешая основному потоку событий службы.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event as XEvent;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, WindowClass};
use x11rb::rust_connection::RustConnection;
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

/// `copy`/`cut` первой строкой (у Thunar) и адреса `file://` по строке.
fn parse(text: &str) -> Option<(bool, Vec<PathBuf>)> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let mut cut = false;
    let mut paths = Vec::new();
    for line in lines.by_ref() {
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
    }
}
