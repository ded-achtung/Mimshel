//! Обмен картинками с X-сервером через общую память (расширение MIT-SHM):
//! вместо того чтобы гнать мегабайты через сокет, кладём их в файл в памяти
//! (tmpfs), а сервер читает его сам (или пишет в него снимок). Файл передаём
//! серверу открытым (`attach_fd`), отображать его в свою память нам не нужно —
//! обычные чтение и запись по смещению, без `unsafe`.
//!
//! Нет расширения или что-то не вышло — работаем по-старому, через сокет.

use std::fs::{File, OpenOptions};
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, Drawable, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use crate::{Display, Result};

/// Файл в памяти, который видит и сервер.
pub(crate) struct Segment {
    id: u32,
    file: File,
    size: u64,
}

/// Общая память: есть ли она и текущий сегмент.
pub(crate) struct Shm {
    enabled: AtomicBool,
    segment: std::sync::Mutex<Option<Segment>>,
}

impl Shm {
    /// Есть ли у сервера MIT-SHM с передачей файла (версия 1.2+).
    pub(crate) fn new(conn: &RustConnection) -> Shm {
        let ok = conn
            .shm_query_version()
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|r| (r.major_version, r.minor_version) >= (1, 2));
        Shm {
            enabled: AtomicBool::new(ok && std::env::var_os("SHELL_NO_SHM").is_none()),
            segment: std::sync::Mutex::new(None),
        }
    }
}

impl Display {
    /// Сегмент не меньше `size` байт — с ним выполнить `f`. Ошибка — общая
    /// память выключается до конца работы (дальше по-старому).
    fn with_segment<T>(&self, size: u64, f: impl FnOnce(&Segment) -> Result<T>) -> Option<T> {
        if !self.shm.enabled.load(Ordering::Relaxed) {
            return None;
        }
        let mut slot = self.shm.segment.lock().unwrap_or_else(|p| p.into_inner());
        let result = self
            .grow(&mut slot, size)
            .and_then(|()| match slot.as_ref() {
                Some(seg) => f(seg),
                None => Err("нет сегмента".into()),
            });
        match result {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("Общая память с X не работает ({e}) — картинки пойдут через сокет");
                self.shm.enabled.store(false, Ordering::Relaxed);
                None
            }
        }
    }

    fn grow(&self, slot: &mut Option<Segment>, size: u64) -> Result<()> {
        if slot.as_ref().is_some_and(|s| s.size >= size) {
            return Ok(());
        }
        if let Some(old) = slot.take() {
            self.conn.shm_detach(old.id)?;
        }
        // С запасом до размера экрана, чтобы не пересоздавать на каждом окне.
        let (w, h) = self.screen_size();
        let size = size.max(u64::from(w) * u64::from(h) * 4);
        let file = temp_file()?;
        file.set_len(size)?;
        let id = self.conn.generate_id()?;
        self.conn
            .shm_attach_fd(id, OwnedFd::from(file.try_clone()?), false)?
            .check()?;
        *slot = Some(Segment { id, file, size });
        Ok(())
    }

    /// Вывести картинку `(w, h)` (уже в порядке байт сервера, BGRA) в окно
    /// со столбца `x` через общую память. `false` — не вышло, выводить по-старому.
    pub(crate) fn shm_put(
        &self,
        window: Window,
        gc: u32,
        (w, h): (u16, u16),
        x: i16,
        data: &[u8],
    ) -> bool {
        self.with_segment(data.len() as u64, |seg| {
            seg.file.write_all_at(data, 0)?;
            self.conn.shm_put_image(
                window,
                gc,
                w,
                h,
                0,
                0,
                w,
                h,
                x,
                0,
                32,
                u8::from(ImageFormat::Z_PIXMAP),
                false,
                seg.id,
                0,
            )?;
            // Дождаться, пока сервер прочтёт: потом сегмент можно переписывать.
            self.conn.get_input_focus()?.reply()?;
            Ok(())
        })
        .is_some()
    }

    /// Содержимое окна (BGRX, 4 байта на пиксель) через общую память.
    /// `None` — общей памяти нет (снимать по-старому); `Some(None)` — окно
    /// снять нельзя (например, свёрнуто: сервер отвечает ошибкой).
    pub(crate) fn shm_get(&self, drawable: Drawable, w: u16, h: u16) -> Option<Option<Vec<u8>>> {
        let len = usize::from(w) * usize::from(h) * 4;
        self.with_segment(len as u64, |seg| {
            let reply = self
                .conn
                .shm_get_image(
                    drawable,
                    0,
                    0,
                    w,
                    h,
                    !0,
                    u8::from(ImageFormat::Z_PIXMAP),
                    seg.id,
                    0,
                )?
                .reply();
            let reply = match reply {
                Ok(r) => r,
                Err(ReplyError::X11Error(_)) => return Ok(None),
                Err(e) => return Err(e.into()),
            };
            if reply.size as usize != len {
                return Ok(None);
            }
            let mut data = vec![0; len];
            seg.file.read_exact_at(&mut data, 0)?;
            Ok(Some(data))
        })
    }
}

/// Файл в памяти (tmpfs): создаём и сразу удаляем имя — файл живёт, пока открыт.
fn temp_file() -> std::io::Result<File> {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/dev/shm"));
    let path = dir.join(format!(
        "shell-shm-{}-{}",
        std::process::id(),
        COUNT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    Ok(file)
}
