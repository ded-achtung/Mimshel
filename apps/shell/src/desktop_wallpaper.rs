//! Обои рабочего стола: рисовальщик (один поток, только последний заказ;
//! фон блокировки — когда выбор успокоился), обои по мониторам, миниатюры
//! для окна «Обои», выбор своей картинки и запись выбора в настройки.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use shell_core::config::{set_value, setting_value};
use shell_icons::Fit;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use crate::desktop_ui::DesktopMsg;
use crate::ui::Msg;

/// Свои обои: (имя в настройках, подпись).
pub(crate) const WALLPAPERS: &[(&str, &str)] = &[
    ("space", "Космос"),
    ("waves", "Волны"),
    ("aurora", "Сияние"),
];

/// Пока выбор обоев не успокоился, фон экрана блокировки не рисуем.
const LOCK_DELAY: Duration = Duration::from_millis(1500);

/// Рисовальщик обоев: один поток, берёт только последний заказ (быстрые
/// щелчки по плиткам не рисуют все обои подряд одновременно). Когда новых
/// заказов нет [`LOCK_DELAY`], рисует и фон экрана блокировки — один раз
/// для последних обоев.
pub(crate) type WallpaperOrder = (String, (u32, u32), Vec<(i32, i32, u32, u32)>);

pub(crate) fn painter(tx: Sender<Msg>) -> Sender<WallpaperOrder> {
    let (orders, rx) = std::sync::mpsc::channel::<WallpaperOrder>();
    std::thread::spawn(move || {
        let mut lock_for: Option<String> = None;
        loop {
            let order = match &lock_for {
                Some(_) => match rx.recv_timeout(LOCK_DELAY) {
                    Ok(o) => o,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(value) = lock_for.take() {
                            let (w, h) = shell_setup::lock::screen_size();
                            shell_setup::lock::background(&value, w, h);
                        }
                        continue;
                    }
                    Err(_) => return,
                },
                None => match rx.recv() {
                    Ok(o) => o,
                    Err(_) => return,
                },
            };
            // Заказы, пришедшие, пока ждали, — остаётся только последний.
            let (value, (sw, sh), monitors) = rx.try_iter().last().unwrap_or(order);
            let start = Instant::now();
            if let Some(image) = compose(&value, sw, sh, &monitors) {
                let msg = DesktopMsg::Wallpaper(value.clone(), image, start.elapsed());
                if tx.send(Msg::Desktop(Box::new(msg))).is_err() {
                    return;
                }
            }
            lock_for = Some(value);
        }
    });
    orders
}

/// Записать `look.wallpaper` в файл настроек (пояснения в файле остаются).
pub(crate) fn save_wallpaper(value: &str) -> std::io::Result<()> {
    let Some(path) = crate::ui::config_path() else {
        return Ok(());
    };
    let v = setting_value("look.wallpaper", value).map_err(std::io::Error::other)?;
    crate::ui::config_store(path).edit(|text| {
        set_value(text, "look", "wallpaper", v)
            .ok_or_else(|| std::io::Error::other("не удалось изменить настройки"))
    })
}

/// Миниатюра обоев `tw`×`th` пикселей: из уже нарисованных под экран обоев
/// (так она такая же, как на столе), иначе свои рисуются сразу маленькими.
pub(crate) fn thumb(value: &str, tw: u32, th: u32) -> Option<Pixmap> {
    let (sw, sh) = shell_setup::lock::screen_size();
    let drawn = match shell_wallpaper::Style::parse(value) {
        Some(style) => {
            let (sw, sh) = (u32::try_from(sw).ok()?, u32::try_from(sh).ok()?);
            let file = shell_setup::parts::wallpaper_path(style, sw, sh);
            if file.exists() {
                Some(file)
            } else {
                return shell_wallpaper::draw(style, tw, th);
            }
        }
        None => Some(PathBuf::from(value)),
    };
    shell_icons::render_file(&drawn?, tw, th, Fit::Cover)
}

/// Окно выбора картинки (zenity) в отдельном потоке.
pub(crate) fn pick_file(tx: Sender<Msg>) {
    std::thread::spawn(move || {
        // Открыть сразу в папке «Изображения» (по xdg-user-dir).
        let pictures = Command::new("xdg-user-dir")
            .arg("PICTURES")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && std::path::Path::new(s).is_dir());
        let start = pictures.map(|p| format!("--filename={p}/"));
        let path = Command::new("zenity")
            .args(start)
            .args([
                "--file-selection",
                "--title=Картинка для обоев",
                "--file-filter=Картинки | *.png *.jpg *.jpeg *.svg *.webp *.PNG *.JPG *.JPEG",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let _ = tx.send(Msg::Desktop(Box::new(DesktopMsg::Picked(path))));
    });
}

/// Обои во весь экран `sw`×`sh` пикселей: на каждом мониторе — свои под его
/// размер (свои обои рисуются под размер один раз и хранятся). `None` —
/// нарисовать не вышло (останется ровный цвет).
fn compose(value: &str, sw: u32, sh: u32, monitors: &[(i32, i32, u32, u32)]) -> Option<Pixmap> {
    // Геометрия поступает из RandR; отдельный процесс xrandr не нужен.
    let fallback = [(0, 0, sw, sh)];
    let monitors = if monitors.is_empty() {
        &fallback[..]
    } else {
        monitors
    };
    let mut out = Pixmap::new(sw.max(1), sh.max(1))?;
    out.fill(tiny_skia::Color::from_rgba8(0x1e, 0x1b, 0x3a, 0xff));
    for &(x, y, w, h) in monitors {
        let Some(file) = shell_setup::parts::wallpaper_file(value, i64::from(w), i64::from(h))
        else {
            eprintln!("Обои «{value}» не нарисованы и не найдены");
            continue;
        };
        let Some(image) = shell_icons::render_file(&file, w, h, Fit::Cover) else {
            eprintln!("Обои {} не прочитаны", file.display());
            continue;
        };
        out.draw_pixmap(
            x,
            y,
            image.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    Some(out)
}
