//! Сторож экрана блокировки (`mimshel lock`). Сам экран рисует и пароль
//! проверяет отдельный процесс (`mimshel lock-screen`, PAM): на X11 падение
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

use mimshel_lock::Layout;

use crate::sys::{command, home, output, running, succeeds};

/// Экран блокировки в списке процессов (команда `mimshel lock-screen`;
/// до переименования — `shell lock-screen`).
const LOCK_SCREEN: [&str; 2] = ["mimshel lock-screen", "shell lock-screen"];

/// Сочетание блокировки, которое ставит `mimshel apply lock` (у Xfce есть
/// только Ctrl+Alt+L), и его команда: xflock4 вызывает `LockCommand`.
pub const LOCK_SHORTCUT: &str = "<Super>l";
pub const LOCK_SHORTCUT_COMMAND: &str = "xflock4";

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

/// Пометка в этом файле: его создал Mimshel (и откат может его удалить).
pub const SCREENSAVER_MARK: &str = "# mimshel: заставка Xfce не запускается";
/// Та же пометка до переименования проекта.
const OLD_SCREENSAVER_MARK: &str = "# shell: заставка Xfce не запускается";

/// Создан ли файл-заглушка заставки нами (новой или прежней пометкой).
pub fn is_screensaver_override(text: &str) -> bool {
    text.contains(SCREENSAVER_MARK) || text.contains(OLD_SCREENSAVER_MARK)
}

/// Содержимое файла: имя то же, запуск — пустая команда.
pub fn screensaver_service_text() -> String {
    format!(
        "{SCREENSAVER_MARK} (mimshel apply lock)\n[D-BUS Service]\nName=org.xfce.ScreenSaver\nExec=/bin/false\n"
    )
}

/// Версия рисования фона: поменялась — старые картинки перерисовываются.
const BACKGROUND_VERSION: u32 = 2;

/// Картинка фона под этот экран. Рисуется заранее (`mimshel apply lock`) или
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
    let cached =
        mimshel_core::names::dir_in(&home().join(".cache")).join(format!("lock/bg-{w}x{h}.png"));
    cached.exists().then_some(cached)
}

/// Нарисовать фон (если готового с тем же ключом нет) и вернуть путь к нему.
pub fn draw_background(wallpaper: &Path, w: i64, h: i64) -> Result<PathBuf, String> {
    let dir = mimshel_core::names::dir_in(&home().join(".cache")).join("lock");
    let out = dir.join(format!("bg-{w}x{h}.png"));
    let stamp = dir.join(format!("bg-{w}x{h}.key"));
    let face = home().join(".face");
    let user = user_name();
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
    let fit = mimshel_icons::Fit::Cover;
    let wall = mimshel_icons::render_file(wallpaper, pw, ph, fit);
    if wall.is_none() {
        log(&format!(
            "обои {} не прочитаны — тёмный фон",
            wallpaper.display()
        ));
    }
    let a = u32::try_from(l.avatar).map_err(|_| "размер аватара")?;
    let face = mimshel_icons::render_file(&face, a, a, fit);
    // Только основное семейство: полный набор читает все шрифты системы (секунды).
    let mut fonts =
        mimshel_text::Fonts::quick("Inter").unwrap_or_else(|| mimshel_text::Fonts::new("Inter"));
    let pm = mimshel_lock::draw(
        &l,
        mimshel_lock::Scene {
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

/// Имя пользователя для PAM и подписи. Берётся у системы по номеру процесса
/// (`id -un`): `$USER` может быть пустым или чужим (запуск через `su`, `env`),
/// и тогда верный пароль не подходил бы — экран было бы не разблокировать.
pub fn user_name() -> String {
    output(&["id", "-un"])
        .map(|s| s.trim().to_string())
        .filter(|u| !u.is_empty())
        .or_else(|| std::env::var("USER").ok().filter(|u| !u.is_empty()))
        .or_else(|| home().file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// Заблокировать экран и держать блокировку, пока не введён верный пароль.
/// `exe` — наша программа (запускается как `mimshel lock-screen`).
/// Возвращает код выхода экрана блокировки (0 — разблокировали паролем).
pub fn supervise(exe: &Path) -> i32 {
    // Only one guard at a time. Before sleep both xss-lock and the Xfce power
    // manager ask to lock at the same moment; checking the process list let
    // both through (two lock screens, the password asked twice). The kernel
    // gives an abstract socket name to one process only and frees it when the
    // process ends, even by a crash.
    let Some(_guard) = single_guard(&guard_name()) else {
        return 0;
    };
    // A lock screen already shown without a guard (started by hand) — no second one.
    if LOCK_SCREEN.iter().any(|name| running(name)) {
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

/// Name of the guard's «lock»: one per user and display.
fn guard_name() -> String {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    format!("mimshel-lock-guard-{}-{display}", crate::sys::uid())
}

/// Take the guard «lock» (an abstract Unix socket name); `None` — another
/// guard holds it. Released when the returned listener is dropped.
fn single_guard(name: &str) -> Option<std::os::unix::net::UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let addr = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes()).ok()?;
    std::os::unix::net::UnixListener::bind_addr(&addr).ok()
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

/// Журнал блокировки: `~/.local/state/mimshel/lock.log`.
pub fn log(msg: &str) {
    let dir = mimshel_core::names::dir_in(&home().join(".local/state"));
    let path = dir.join("lock.log");
    let _ = std::fs::create_dir_all(&dir);
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
    fn only_one_guard_at_a_time() {
        let name = format!("mimshel-test-guard-{}", std::process::id());
        let first = single_guard(&name).expect("the first guard takes the lock");
        assert!(
            single_guard(&name).is_none(),
            "the second one finds it taken"
        );
        drop(first);
        assert!(
            single_guard(&name).is_some(),
            "released when the first ends"
        );
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
