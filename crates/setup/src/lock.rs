//! Сторож экрана блокировки (`shell lock`). Сам экран рисует и пароль
//! проверяет отдельный процесс (`shell lock-screen`, PAM): на X11 падение
//! блокировщика снимает блокировку (урок 1), поэтому сторож перезапускает
//! экран при любом выходе без верного пароля. Сторож ничего не рисует и
//! почти ничего не делает — падать ему не с чего.
//!
//! Вид — как был при macos-look: фон с аватаром и овальным полем, сверху
//! дата и крупное время, кольцо состояния справа в поле (`crates/lock`).

use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use shell_lock::Layout;

use crate::sys::{command, home, running, succeeds};

/// Экран блокировки в списке процессов (команда `shell lock-screen`).
const LOCK_SCREEN: &str = "shell lock-screen";

/// Размер экрана из xrandr.
pub fn screen_size() -> (i64, i64) {
    crate::sys::output_any(&["xrandr", "--current"])
        .lines()
        .find_map(|l| {
            let (w, h) = l
                .split_once("current")?
                .1
                .split(',')
                .next()?
                .split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
        .unwrap_or((1920, 1080))
}

/// Наш файл, который перекрывает системный запуск заставки Xfce через D-Bus.
/// Автозапуск заставки отключён, но D-Bus поднимает её по первому обращению
/// к `org.xfce.ScreenSaver` — и она блокирует экран сама, своим окном пароля
/// (по своему таймеру и перед сном), мимо нашей команды. Файл в домашней
/// папке главнее `/usr/share/dbus-1/services`; удалить — вернётся как было.
pub fn screensaver_service() -> PathBuf {
    home().join(".local/share/dbus-1/services/org.xfce.ScreenSaver.service")
}

/// Пометка в этом файле: его создал shell (и откат может его удалить).
pub const SCREENSAVER_MARK: &str = "# shell: заставка Xfce не запускается";

/// Содержимое файла: имя то же, запуск — пустая команда.
pub fn screensaver_service_text() -> String {
    format!(
        "{SCREENSAVER_MARK} (shell apply lock)\n[D-BUS Service]\nName=org.xfce.ScreenSaver\nExec=/bin/false\n"
    )
}

/// Версия рисования фона: поменялась — старые картинки перерисовываются.
const BACKGROUND_VERSION: u32 = 2;

/// Картинка фона под этот экран. Рисуется заранее (`shell apply lock`) или
/// при блокировке, если обои, аватар, имя или экран поменялись. `None` —
/// нарисовать не удалось и готового нет: экран блокировки обойдётся тёмным
/// фоном (блокировка важнее вида).
/// `wallpaper` — обои из настроек (`look.wallpaper`): свои по имени или путь.
pub fn background(wallpaper: &str, w: i64, h: i64) -> Option<PathBuf> {
    // Падение при рисовании не должно помешать заблокировать экран.
    let drawn = std::panic::catch_unwind(|| {
        let wallpaper = crate::parts::wallpaper_file(wallpaper, w, h)
            .ok_or_else(|| "обои не нарисованы".to_string())?;
        draw_background(&wallpaper, w, h)
    });
    match drawn {
        Ok(Ok(path)) => return Some(path),
        Ok(Err(e)) => log(&format!("фон блокировки не нарисован: {e}")),
        Err(_) => log("фон блокировки: сбой при рисовании"),
    }
    // Годится кеш прошлой блокировки (прежние обои лучше, чем ничего).
    let cached = home().join(format!(".cache/shell/lock/bg-{w}x{h}.png"));
    cached.exists().then_some(cached)
}

/// Нарисовать фон (если готового с тем же ключом нет) и вернуть путь к нему.
pub fn draw_background(wallpaper: &Path, w: i64, h: i64) -> Result<PathBuf, String> {
    let dir = home().join(".cache/shell/lock");
    let out = dir.join(format!("bg-{w}x{h}.png"));
    let stamp = dir.join(format!("bg-{w}x{h}.key"));
    let face = home().join(".face");
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| home().file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let mtime = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos())
    };
    let key = format!(
        "{w}|{h}|{}|{}|{}|{user}|{BACKGROUND_VERSION}",
        wallpaper.display(),
        mtime(wallpaper),
        mtime(&face),
    );
    if out.exists() && std::fs::read_to_string(&stamp).ok().as_deref() == Some(key.as_str()) {
        return Ok(out);
    }
    let l = Layout::new(w, h);
    let (pw, ph) = (
        u32::try_from(w).map_err(|_| "ширина экрана")?,
        u32::try_from(h).map_err(|_| "высота экрана")?,
    );
    let fit = shell_icons::Fit::Cover;
    let wall = shell_icons::render_file(wallpaper, pw, ph, fit);
    if wall.is_none() {
        log(&format!(
            "обои {} не прочитаны — тёмный фон",
            wallpaper.display()
        ));
    }
    let a = u32::try_from(l.avatar).map_err(|_| "размер аватара")?;
    let face = shell_icons::render_file(&face, a, a, fit);
    let mut fonts = shell_text::Fonts::new("Inter");
    let pm = shell_lock::draw(
        &l,
        shell_lock::Scene {
            wallpaper: wall,
            face,
            user: &user,
        },
        &mut fonts,
    )
    .ok_or("пустой экран")?;
    let png = pm.encode_png().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".bg-{w}x{h}.png.tmp"));
    std::fs::write(&tmp, png).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    std::fs::write(&stamp, &key).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Заблокировать экран и держать блокировку, пока не введён верный пароль.
/// `exe` — наша программа (запускается как `shell lock-screen`).
/// Возвращает код выхода экрана блокировки (0 — разблокировали паролем).
pub fn supervise(exe: &Path) -> i32 {
    // Экран уже показан (например, блокировка по бездействию и команда из
    // меню одновременно) — второй не нужен.
    if running(LOCK_SCREEN) {
        return 0;
    }
    supervise_with(
        || command(&exe.to_string_lossy()).arg("lock-screen").status(),
        || succeeds(&["xset", "q"]),
        |restarts, how| {
            log(&format!(
                "блокировка: экран завершился {how} без пароля — перезапуск #{restarts}"
            ));
            // Первый перезапуск — сразу. При постоянной ошибке запуска
            // ограничиваем нагрузку, но не бросаем попытки заблокировать экран.
            if restarts > 1 {
                std::thread::sleep(Duration::from_millis(if restarts < 10 {
                    200
                } else {
                    1000
                }));
            }
        },
    )
}

fn supervise_with(
    mut launch: impl FnMut() -> std::io::Result<std::process::ExitStatus>,
    mut session_alive: impl FnMut() -> bool,
    mut retry: impl FnMut(u32, &str),
) -> i32 {
    let mut restarts = 0u32;
    loop {
        let (code, how) = match launch() {
            Ok(s) if s.success() => return 0,
            Ok(s) => match (s.code(), s.signal()) {
                (Some(c), _) => (c, format!("с кодом {c}")),
                (None, Some(sig)) => (128 + sig, format!("от сигнала {sig}")),
                (None, None) => (1, "непонятно как".into()),
            },
            Err(e) => (1, format!("не запустившись ({e})")),
        };
        if !session_alive() {
            return code;
        }
        restarts = restarts.saturating_add(1);
        retry(restarts, &how);
    }
}

/// Журнал блокировки: `~/.local/state/shell/lock.log`.
pub fn log(msg: &str) {
    let path = home().join(".local/state/shell/lock.log");
    let _ = std::fs::create_dir_all(home().join(".local/state/shell"));
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let now = crate::sys::output(&["date", "+%F %T"]).unwrap_or_default();
        let _ = writeln!(f, "[{}] {msg}", now.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_retrying_until_success_even_after_100_failures() {
        let mut attempts = 0;
        let mut retries = 0;
        let result = supervise_with(
            || {
                attempts += 1;
                if attempts == 1 {
                    Err(std::io::Error::other("не удалось запустить"))
                } else {
                    Ok(std::process::ExitStatus::from_raw(if attempts <= 150 {
                        6
                    } else {
                        0
                    }))
                }
            },
            || true,
            |n, _| retries = n,
        );
        assert_eq!((result, attempts, retries), (0, 151, 150));
    }

    #[test]
    fn stops_when_session_is_gone() {
        let result = supervise_with(
            || Ok(std::process::ExitStatus::from_raw(256)),
            || false,
            |_, _| panic!("сеанс завершён — перезапуск не нужен"),
        );
        assert_eq!(result, 1);
    }
}
