//! Экран блокировки (`shell lock`). Пароль проверяет i3lock-color (PAM) —
//! надёжная готовая программа; свою блокировку не пишем (урок 1: на X11
//! падение блокировщика снимает блокировку). Здесь — сторож: он запускает
//! i3lock и, если тот вышел без верного пароля (упал, не захватил
//! клавиатуру), сразу запускает снова. Сторож ничего не рисует и почти ничего
//! не делает — падать ему не с чего.
//!
//! Вид — как у macos-look: фон с аватаром и овальным полем, сверху дата и
//! крупное время, индикатор ввода — кольцо справа в поле. Положения считаем
//! сами: i3lock не умеет складывать в координатах (урок 7).

use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use shell_lock::{Layout, round};

use crate::look::Look;
use crate::sys::{command, home, running, succeeds, which};

/// Метка в i3lock, собранном с заплаткой macos-look (подсказка в поле
/// сменяется точками ● при вводе).
pub const LOCKER_MARK: &[u8] = b"macos-look-field-v3";

/// Команда i3lock: картинка фона, часы, кольцо ввода, подсказка в поле
/// (`placeholder`, только у собранного с заплаткой — иначе она на фоне).
pub fn lock_args(bg: &Path, l: &Layout, placeholder: &str, patched: bool) -> Vec<String> {
    let (white, clear) = ("ffffffff", "00000000");
    let mut a: Vec<String> = [
        "i3lock",
        "--image",
        &bg.to_string_lossy(),
        "--ignore-empty-password",
        "--show-failed-attempts",
        "--pass-media-keys",
        "--pass-volume-keys",
        "--pointer",
        "default",
        // Часы: дата мелко над крупным временем.
        "--clock",
        "--force-clock",
        "--time-str",
        "%H:%M",
        "--date-str",
        "%A, %-d %B",
        "--time-font",
        "Inter",
        "--date-font",
        "Inter",
        "--layout-font",
        "Inter",
        "--time-size",
        &l.px(104.0),
        "--date-size",
        &l.px(23.0),
        "--time-color",
        "ffffffe0",
        "--date-color",
        "ffffffd0",
        "--time-pos",
        &format!("{}:{}", l.w.div_euclid(2), l.time_y),
        "--date-pos",
        &format!("{}:{}", l.w.div_euclid(2), l.date_y),
        // Индикатор ввода — маленькое кольцо справа в поле, как стрелка на Mac.
        "--indicator",
        "--radius",
        &l.px(9.0),
        "--ring-width",
        &l.px(2.0),
        "--ind-pos",
        &format!("{}:{}", l.ind_x, l.ind_y),
        "--inside-color",
        clear,
        "--ring-color",
        "ffffff55",
        "--line-color",
        clear,
        "--separator-color",
        clear,
        "--keyhl-color",
        white,
        "--bshl-color",
        "ff9f0aff",
        "--insidever-color",
        clear,
        "--ringver-color",
        "0a84ffff",
        "--insidewrong-color",
        clear,
        "--ringwrong-color",
        "ff453aff",
        "--verif-text",
        "",
        "--noinput-text",
        "",
        "--lock-text",
        "",
        "--lockfailed-text",
        "",
        "--wrong-text",
        "Неверный пароль",
        "--wrong-font",
        "Inter",
        "--wrong-size",
        &l.px(13.0),
        "--wrong-color",
        "ffffffd0",
        "--wrong-pos",
        &format!(
            "{}:{}",
            l.w.div_euclid(2),
            l.pill_y + l.pill_h + round(30.0 * l.s)
        ),
        "--modif-pos",
        "-100:-100",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if patched {
        a.extend(
            [
                "--greeter-text",
                placeholder,
                "--greeter-font",
                "Inter",
                "--greeter-size",
                &l.px(13.5),
                "--greeter-color",
                "ffffff9e",
                "--greeter-align",
                "1",
                "--greeter-pos",
                &format!(
                    "{}:{}",
                    l.pill_x + round(14.0 * l.s),
                    l.pill_y + l.pill_h.div_euclid(2) + round(5.0 * l.s)
                ),
            ]
            .iter()
            .map(|s| s.to_string()),
        );
    }
    a
}

/// Собран ли i3lock с заплаткой.
pub fn locker_patched() -> bool {
    which("i3lock")
        .and_then(|p| std::fs::read(p).ok())
        .is_some_and(|b| b.windows(LOCKER_MARK.len()).any(|w| w == LOCKER_MARK))
}

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

/// Версия рисования фона: поменялась — старые картинки перерисовываются.
const BACKGROUND_VERSION: u32 = 1;

/// Картинка фона под этот экран. Рисуется заранее (`shell apply lock`) или
/// при блокировке, если обои, аватар, имя или экран поменялись. Не
/// получилось — фон macos-look, иначе просто обои: блокировка важнее вида.
pub fn background(look: &Look, w: i64, h: i64) -> PathBuf {
    let wallpaper = crate::look::Paths::new()
        .wallpapers
        .join(format!("{}.jpg", look.names().wallpaper));
    // Падение при рисовании не должно помешать заблокировать экран.
    match std::panic::catch_unwind(|| draw_background(look, &wallpaper, w, h)) {
        Ok(Ok(path)) => return path,
        Ok(Err(e)) => log(&format!("фон блокировки не нарисован: {e}")),
        Err(_) => log("фон блокировки: сбой при рисовании"),
    }
    let old = home().join(format!(".cache/macos-look/lock/bg-{w}x{h}.png"));
    if old.exists() { old } else { wallpaper }
}

/// Нарисовать фон (если готового с тем же ключом нет) и вернуть путь к нему.
pub fn draw_background(look: &Look, wallpaper: &Path, w: i64, h: i64) -> Result<PathBuf, String> {
    let dir = home().join(".cache/shell/lock");
    let out = dir.join(format!("bg-{w}x{h}.png"));
    let stamp = dir.join(format!("bg-{w}x{h}.key"));
    let face = home().join(".face");
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| home().file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let patched = locker_patched();
    let placeholder = look.get("lock", "placeholder");
    let mtime = |p: &Path| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos())
    };
    let key = format!(
        "{w}|{h}|{}|{}|{}|{user}|{placeholder}|{patched}|{BACKGROUND_VERSION}",
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
            // Собранный с заплаткой i3lock пишет подсказку сам (и прячет её при вводе).
            placeholder: (!patched).then_some(placeholder),
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
/// Возвращает код выхода i3lock (0 — разблокировали паролем).
pub fn supervise(look: &Look) -> i32 {
    if running("i3lock") {
        return 0;
    }
    let (w, h) = screen_size();
    let l = Layout::new(w, h);
    let bg = background(look, w, h);
    let args = lock_args(&bg, &l, look.get("lock", "placeholder"), locker_patched());
    supervise_with(
        || {
            command(&args[0])
                .args(&args[1..])
                .arg("--nofork")
                .env("LC_TIME", look.get("lock", "date_locale"))
                .env("MACOS_LOOK_MAX_DOTS", l.max_dots().to_string())
                .status()
        },
        || succeeds(&["xset", "q"]),
        |restarts, how| {
            log(&format!(
                "блокировка: i3lock завершился {how} без пароля — перезапуск #{restarts}"
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
fn log(msg: &str) {
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

    /// Эталон — команда, которую для тех же экранов составляет macos-look
    /// (`testdata/`, по аргументу в строке).
    #[test]
    fn same_as_macos_look() {
        let cases = [
            (
                1278,
                798,
                include_str!("../testdata/i3lock-1278x798.txt"),
                true,
            ),
            (
                2560,
                1440,
                include_str!("../testdata/i3lock-2560x1440.txt"),
                true,
            ),
            (
                1920,
                1080,
                include_str!("../testdata/i3lock-1920x1080.txt"),
                true,
            ),
            (
                1278,
                798,
                include_str!("../testdata/i3lock-1278x798-plain.txt"),
                false,
            ),
            (
                2560,
                1440,
                include_str!("../testdata/i3lock-2560x1440-plain.txt"),
                false,
            ),
            (
                1920,
                1080,
                include_str!("../testdata/i3lock-1920x1080-plain.txt"),
                false,
            ),
        ];
        for (w, h, want, patched) in cases {
            let l = Layout::new(w, h);
            let got = lock_args(Path::new("/bg.png"), &l, "Введите пароль", patched);
            assert_eq!(format!("{}\n", got.join("\n")), want, "{w}x{h}");
        }
        assert_eq!(Layout::new(1278, 798).max_dots(), 11);
        assert_eq!(Layout::new(1920, 1080).max_dots(), 11);
    }
}
