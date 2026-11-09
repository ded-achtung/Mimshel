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

use crate::sys::{command, home, output, succeeds};

/// Сочетание блокировки, которое ставит `mimshel apply lock` (у Xfce есть
/// только Ctrl+Alt+L), и его команда: xflock4 вызывает `LockCommand`.
pub const LOCK_SHORTCUT: &str = "<Super>l";
pub const LOCK_SHORTCUT_COMMAND: &str = "xflock4";

/// PAM services the lock screen may check the password with, best first:
/// the local login of Arch (the one `i3lock` includes there), then `login`,
/// which every system has. Both are whole services: they have the account
/// step too.
const PAM_SERVICES: [&str; 2] = ["system-local-login", "login"];
/// Where PAM looks for a service file.
const PAM_DIRS: [&str; 3] = ["/etc/pam.d", "/usr/lib/pam.d", "/usr/share/pam.d"];

/// The PAM service for the lock screen: the first of [`PAM_SERVICES`] this
/// system has. `None` — it has none.
///
/// The name used to be fixed to Arch's. Where that file does not exist
/// (Debian, Ubuntu, Fedora) PAM falls back to `other`, which refuses
/// everybody: the screen locked and no password could ever unlock it.
pub fn pam_service() -> Option<&'static str> {
    pam_service_in(&PAM_DIRS.map(Path::new))
}

fn pam_service_in(dirs: &[&Path]) -> Option<&'static str> {
    PAM_SERVICES
        .into_iter()
        .find(|name| dirs.iter().any(|dir| dir.join(name).is_file()))
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
    let drawn = mimshel_core::crash::catch(|| {
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
    // both through (two lock screens, the password asked twice). The guard
    // holds a lock on a file in the user's private folder ([`take`]); the
    // kernel frees it when the process ends, even by a crash.
    //
    // The guard that loses must not return at once, though: xss-lock holds
    // the system's sleep until its locker exits, and when its guard is the
    // loser the laptop would fall asleep before the winner's screen is up —
    // on resume the desktop shows (and takes keys) until the window maps.
    // So it waits for the lock screen to say it covers the screen.
    let _guard = match take(&guard_name()) {
        Take::Held(held) => Some(held),
        Take::Busy => {
            wait_until_locked(LOSER_WAIT);
            return 0;
        }
        // No private folder to keep the lock in: lock the screen all the
        // same. Two lock screens are a nuisance, none is a hole.
        Take::Unavailable => {
            log("блокировка: нет своей папки для замка сторожа — блокирую без него");
            None
        }
    };
    // A lock screen started by hand (without a guard) already covers this
    // display — no second one. Asked of this display only: a lock screen on
    // another display, or a stray process with that name, must not stop this
    // one from locking.
    if lock_dir().is_some_and(|dir| held_in(&dir, &locked_name())) {
        return 0;
    }
    supervise_with(
        || {
            // The pid lets the guard attach the crash report (its file
            // name carries the pid) to the reason in the log.
            // The key debug log prints every decoded key: in the lock
            // screen that would be the password, written to lock.log.
            let mut child = command(&exe.to_string_lossy())
                .arg("lock-screen")
                .env_remove(format!(
                    "{}_DEBUG_KEYS",
                    mimshel_core::names::APP.to_uppercase()
                ))
                .env_remove("SHELL_DEBUG_KEYS")
                .spawn()?;
            let pid = child.id();
            let status = child.wait()?;
            Ok((status, pid))
        },
        // One failed `xset` (no memory to fork, the X client limit) must not
        // end the guard and leave the screen unlocked: ask a few times.
        || {
            (0..3).any(|i| {
                if i > 0 {
                    std::thread::sleep(Duration::from_millis(300));
                }
                succeeds(&["xset", "q"])
            })
        },
        |restarts, how| {
            log(&format!(
                "блокировка: {how} без пароля — перезапуск #{restarts}"
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
        crate::crash::panic_reason,
        |pid, how| {
            let _ = crate::crash::write_exit_report("lock-screen", pid, how, Some(&log_path()));
        },
    )
}

/// Name of the guard's lock file: one per display (the folder is the
/// user's own).
fn guard_name() -> String {
    format!("mimshel-lock-guard-{}.lock", display_number())
}

/// The X display number of `DISPLAY`, the same for every way to write it:
/// xss-lock runs with `:0.0`, the Xfce session (and xflock4 from the power
/// manager) with `:0`. Named by the raw string, the two guards did not see
/// each other and both put up a lock screen: the password asked twice.
fn display_number() -> String {
    display_key(&std::env::var("DISPLAY").unwrap_or_default())
}

fn display_key(display: &str) -> String {
    let after_host = display.rsplit_once(':').map_or(display, |(_, d)| d);
    // Digits only: the number becomes part of a file name.
    after_host
        .split('.')
        .next()
        .unwrap_or("")
        .chars()
        .filter(char::is_ascii_digit)
        .collect()
}

/// How long a guard that lost waits for the winner's lock screen: under
/// logind's usual 5 s limit for holding the sleep.
const LOSER_WAIT: Duration = Duration::from_secs(4);

/// Name of the file the lock screen keeps locked while it covers the screen
/// with the keyboard grabbed: one per display.
fn locked_name() -> String {
    format!("mimshel-lock-ready-{}.lock", display_number())
}

/// A lock file held by this process; released when dropped and when the
/// process ends, however it ends.
pub struct Held(#[allow(dead_code)] std::fs::File);

enum Take {
    Held(Held),
    /// Another process holds it.
    Busy,
    /// There is no private folder, or the file cannot be opened.
    Unavailable,
}

/// The folder for the lock files: the user's runtime folder, else the state
/// folder in the home. It must be a real folder of this user that nobody
/// else can enter.
///
/// These locks were abstract socket names before. Such a name has no owner:
/// any process on the machine — another user, a sandboxed program — could
/// take `mimshel-lock-guard-UID-DISPLAY` first, and the real guard then
/// believed it had lost the race and left without locking the screen.
fn lock_dir() -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let private = |dir: &Path| {
        std::fs::symlink_metadata(dir)
            .is_ok_and(|m| m.is_dir() && m.uid() == crate::sys::uid() && m.mode() & 0o077 == 0)
    };
    let uid = crate::sys::uid();
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .into_iter()
        .chain([PathBuf::from(format!("/run/user/{uid}"))])
        .find(|dir| private(dir))
        .or_else(|| {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            let dir = log_path().parent()?.join("locks");
            let _ = std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir);
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            private(&dir).then_some(dir)
        })
}

fn take(name: &str) -> Take {
    match lock_dir() {
        Some(dir) => take_in(&dir, name),
        None => Take::Unavailable,
    }
}

fn take_in(dir: &Path, name: &str) -> Take {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join(name));
    let Ok(file) = file else {
        return Take::Unavailable;
    };
    match file.try_lock() {
        Ok(()) => Take::Held(Held(file)),
        Err(std::fs::TryLockError::WouldBlock) => Take::Busy,
        Err(std::fs::TryLockError::Error(_)) => Take::Unavailable,
    }
}

/// Whether some process holds the lock file `name`. Asked with a shared
/// lock, so two askers do not see each other.
fn held_in(dir: &Path, name: &str) -> bool {
    std::fs::File::open(dir.join(name)).is_ok_and(|file| {
        matches!(
            file.try_lock_shared(),
            Err(std::fs::TryLockError::WouldBlock)
        )
    })
}

/// Called by the lock screen once its window is shown and the keyboard
/// and pointer are ours: other guards waiting in [`supervise`] may now
/// let the system sleep. Keep the value for as long as the screen is
/// locked.
pub fn announce_locked() -> Option<Held> {
    let dir = lock_dir()?;
    // A waiting guard's question holds the file for an instant: ask again.
    for _ in 0..20 {
        match take_in(&dir, &locked_name()) {
            Take::Held(held) => return Some(held),
            Take::Busy => std::thread::sleep(Duration::from_millis(5)),
            Take::Unavailable => return None,
        }
    }
    None
}

/// Wait until a lock screen has announced itself, at most `limit`.
fn wait_until_locked(limit: Duration) -> bool {
    let shown = lock_dir().is_some_and(|dir| wait_held_in(&dir, &locked_name(), limit));
    if !shown {
        log("блокировка: экран блокировки не показался вовремя — не жду дальше");
    }
    shown
}

fn wait_held_in(dir: &Path, name: &str, limit: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if held_in(dir, name) {
            return true;
        }
        if start.elapsed() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn supervise_with(
    mut launch: impl FnMut() -> std::io::Result<(std::process::ExitStatus, u32)>,
    mut session_alive: impl FnMut() -> bool,
    mut retry: impl FnMut(u32, &str),
    mut report: impl FnMut(u32) -> Option<String>,
    mut record: impl FnMut(u32, &str),
) -> i32 {
    let mut restarts = 0u32;
    loop {
        let (code, how) = match launch() {
            Ok((s, _)) if s.success() => return 0,
            Ok((s, pid)) => {
                // The plain-Russian reason — the same function the service
                // guard uses (`mimshel_core::crash::exit_reason`): codes and
                // signals explained, not just a bare number.
                let (code, how) = match (s.code(), s.signal()) {
                    (Some(c), _) => (c, mimshel_core::crash::exit_reason(Some(c), None)),
                    (None, Some(sig)) => {
                        (128 + sig, mimshel_core::crash::exit_reason(None, Some(sig)))
                    }
                    (None, None) => (1, mimshel_core::crash::exit_reason(None, None)),
                };
                // A panic (code 101) leaves a crash report: append its
                // message and place, so the log says why the screen fell.
                let how = match report(pid) {
                    Some(detail) if code == 101 => format!("{how}: {detail}"),
                    _ => {
                        // No panic report: leave an exit report, so the
                        // crash shows in `mimshel crashes` and `doctor`.
                        if mimshel_core::crash::is_crash(s.code(), s.signal()) {
                            record(pid, &how);
                        }
                        how
                    }
                };
                (code, how)
            }
            Err(e) => (1, format!("не запустившись ({e})")),
        };
        if !session_alive() {
            return code;
        }
        restarts = restarts.saturating_add(1);
        retry(restarts, &how);
    }
}

/// `~/.local/state/mimshel/lock.log`.
fn log_path() -> std::path::PathBuf {
    mimshel_core::names::dir_in(&home().join(".local/state")).join("lock.log")
}

/// Журнал блокировки: `~/.local/state/mimshel/lock.log`.
pub fn log(msg: &str) {
    let path = log_path();
    let dir = path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
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

    #[test]
    fn one_guard_name_for_every_way_to_write_the_display() {
        for d in [":0", ":0.0", "unix:0", "unix:0.0", "localhost:0.1"] {
            assert_eq!(display_key(d), "0", "{d}");
        }
        assert_eq!(display_key(":1"), "1");
        assert_ne!(display_key(":10.0"), display_key(":1.0"));
        assert_eq!(display_key(""), "");
    }

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
                    Ok((
                        std::process::ExitStatus::from_raw(if attempts <= 150 { 6 } else { 0 }),
                        5000 + attempts,
                    ))
                }
            },
            || true,
            |n, _| retries = n,
            |_| None,
            |_, _| {},
        );
        assert_eq!((result, attempts, retries), (0, 151, 150));
    }

    #[test]
    fn panic_appends_the_report_message_and_place() {
        // A panic exit (code 101), then a normal unlock: the reason passed
        // to `retry` carries the report detail for that pid.
        let mut reasons: Vec<String> = Vec::new();
        let mut runs = 0;
        let result = supervise_with(
            || {
                runs += 1;
                let code = if runs == 1 { 101 } else { 0 };
                Ok((std::process::ExitStatus::from_raw(code << 8), 4242))
            },
            || true,
            |_, how| reasons.push(how.to_string()),
            |pid| (pid == 4242).then(|| "«bad credentials» в lock_ui.rs:55".to_string()),
            |_, _| {},
        );
        assert_eq!(result, 0);
        assert_eq!(
            reasons,
            ["упала (паника): «bad credentials» в lock_ui.rs:55"]
        );
    }

    #[test]
    fn panic_without_a_report_keeps_the_plain_reason() {
        let mut reasons: Vec<String> = Vec::new();
        let mut runs = 0;
        let result = supervise_with(
            || {
                runs += 1;
                let code = if runs == 1 { 101 } else { 0 };
                Ok((std::process::ExitStatus::from_raw(code << 8), 7777))
            },
            || true,
            |_, how| reasons.push(how.to_string()),
            |_| None,
            |_, _| {},
        );
        assert_eq!(result, 0);
        assert_eq!(reasons, ["упала (паника)"]);
    }

    fn lock_folder(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mimshel-test-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn losing_guard_waits_for_the_lock_screen() {
        let dir = lock_folder("ready");
        let name = "ready.lock";
        assert!(!wait_held_in(&dir, name, Duration::from_millis(120)));
        let folder = dir.clone();
        let screen = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            let Take::Held(held) = take_in(&folder, name) else {
                panic!("the screen takes the lock");
            };
            std::thread::sleep(Duration::from_millis(500));
            drop(held);
        });
        let start = std::time::Instant::now();
        assert!(wait_held_in(&dir, name, Duration::from_secs(3)));
        assert!(start.elapsed() >= Duration::from_millis(100));
        screen.join().unwrap();
        assert!(!held_in(&dir, name), "free again once the screen is gone");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pam_service_is_one_the_system_has() {
        let dir = lock_folder("pam");
        assert_eq!(pam_service_in(&[&dir]), None);
        std::fs::write(dir.join("login"), "").unwrap();
        assert_eq!(
            pam_service_in(&[Path::new("/nonexistent"), &dir]),
            Some("login")
        );
        std::fs::write(dir.join("system-local-login"), "").unwrap();
        assert_eq!(pam_service_in(&[&dir]), Some("system-local-login"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_one_guard_at_a_time() {
        let dir = lock_folder("guard");
        let Take::Held(first) = take_in(&dir, "guard.lock") else {
            panic!("the first guard takes the lock");
        };
        assert!(
            matches!(take_in(&dir, "guard.lock"), Take::Busy),
            "the second one finds it taken"
        );
        // Asking whether it is held does not disturb the holder.
        assert!(held_in(&dir, "guard.lock"));
        assert!(matches!(take_in(&dir, "guard.lock"), Take::Busy));
        drop(first);
        assert!(
            matches!(take_in(&dir, "guard.lock"), Take::Held(_)),
            "released when the first ends"
        );
        // A folder that is not there: no lock, and the caller locks anyway.
        assert!(matches!(
            take_in(&dir.join("missing"), "guard.lock"),
            Take::Unavailable
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stops_when_session_is_gone() {
        let result = supervise_with(
            || Ok((std::process::ExitStatus::from_raw(256), 1)),
            || false,
            |_, _| panic!("сеанс завершён — перезапуск не нужен"),
            |_| None,
            |_, _| {},
        );
        assert_eq!(result, 1);
    }

    #[test]
    fn crashes_without_a_panic_report_are_recorded() {
        // SIGABRT (6), then a panic that has its report, then SIGTERM (15,
        // a stop on purpose), then a normal unlock: only the abort gets an
        // exit report.
        let mut recorded: Vec<(u32, String)> = Vec::new();
        let mut runs = 0;
        let result = supervise_with(
            || {
                runs += 1;
                let raw = match runs {
                    1 => 6,
                    2 => 101 << 8,
                    3 => 15,
                    _ => 0,
                };
                Ok((std::process::ExitStatus::from_raw(raw), 100 + runs))
            },
            || true,
            |_, _| {},
            |pid| (pid == 102).then(|| "«boom» в lock_ui.rs:1".to_string()),
            |pid, how| recorded.push((pid, how.to_string())),
        );
        assert_eq!(result, 0);
        assert_eq!(
            recorded,
            [(101, "завершилась аварийно (SIGABRT)".to_string())]
        );
    }
}
