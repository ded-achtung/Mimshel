//! Мелочи для работы с системой: запуск команд, поиск процессов, настройки
//! Xfce (xfconf), окружение графического сеанса.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Переменные графического сеанса, без которых команды (xfconf-query,
/// setxkbmap, busctl) не найдут экран и шину, если нас запустили по SSH.
const SESSION_VARS: &[&str] = &[
    "DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "DESKTOP_SESSION",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
];

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

pub fn uid() -> u32 {
    std::fs::metadata("/proc/self").map_or(0, |m| m.uid())
}

/// Команда с окружением графического сеанса. Запущены не из сеанса (по SSH) —
/// переменные и язык берутся у процесса xfce4-session (урок 14 в CLAUDE.md).
pub fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());
    for (k, v) in session_extra() {
        cmd.env(k, v);
    }
    cmd
}

/// Чего не хватает нашему окружению по сравнению с сеансом.
fn session_extra() -> &'static [(String, String)] {
    static EXTRA: OnceLock<Vec<(String, String)>> = OnceLock::new();
    EXTRA.get_or_init(|| {
        if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("x11") {
            return Vec::new();
        }
        let wanted = |k: &str| {
            SESSION_VARS.contains(&k) || k == "LANG" || k == "LANGUAGE" || k.starts_with("LC_")
        };
        let mut extra: Vec<(String, String)> = session_env()
            .into_iter()
            .filter(|(k, _)| wanted(k))
            .collect();
        let uid = uid();
        for (k, v) in [
            ("DISPLAY", ":0".to_string()),
            ("XDG_RUNTIME_DIR", format!("/run/user/{uid}")),
            (
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path=/run/user/{uid}/bus"),
            ),
        ] {
            if std::env::var_os(k).is_none() && !extra.iter().any(|(e, _)| e == k) {
                extra.push((k.to_string(), v));
            }
        }
        if std::env::var_os("XAUTHORITY").is_none()
            && !extra.iter().any(|(e, _)| e == "XAUTHORITY")
            && let Some(xa) = [
                home().join(".Xauthority"),
                PathBuf::from(format!("/run/user/{uid}/xauth")),
            ]
            .into_iter()
            .find(|p| p.exists())
        {
            extra.push(("XAUTHORITY".into(), xa.to_string_lossy().into_owned()));
        }
        extra
    })
}

/// Что команда напечатала, если она завершилась без ошибки.
pub fn output(args: &[&str]) -> Option<String> {
    let (program, rest) = args.split_first()?;
    let out = command(program)
        .args(rest)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Что команда напечатала, даже если она сообщила об ошибке.
pub fn output_any(args: &[&str]) -> String {
    let Some((program, rest)) = args.split_first() else {
        return String::new();
    };
    command(program)
        .args(rest)
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Завершилась ли команда без ошибки.
pub fn succeeds(args: &[&str]) -> bool {
    output(args).is_some()
}

/// Номера своих процессов, у которых командная строка подходит под шаблон.
pub fn pgrep(pattern: &str) -> Vec<u32> {
    output(&["pgrep", "-u", &uid().to_string(), "-f", pattern])
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// Запущена ли программа. Ищем по командной строке: имя процесса в ядре
/// обрезается до 15 знаков (урок 2).
pub fn running(name: &str) -> bool {
    !pgrep(&format!(r"^(\S*/)?{name}(\s|$)")).is_empty()
}

/// Процесс xfce4-session — по нему узнаём графический сеанс.
/// Запускается без [`command`]: окружение сеанса для неё ещё ищется по этому
/// процессу.
fn session_pid() -> Option<u32> {
    let out = Command::new("pgrep")
        .args(["-u", &uid().to_string(), "-x", "xfce4-session"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Окружение графического сеанса (процесса xfce4-session).
pub fn session_env() -> HashMap<String, String> {
    let Some(pid) = session_pid() else {
        return HashMap::new();
    };
    std::fs::read(format!("/proc/{pid}/environ"))
        .unwrap_or_default()
        .split(|&b| b == 0)
        .filter_map(|kv| {
            let kv = String::from_utf8_lossy(kv);
            let (k, v) = kv.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// Сеанс logind процесса.
pub fn logind_session(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/sessionid"))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Запущен ли процесс в графическом сеансе (а не, например, по SSH — урок 3).
pub fn in_desktop_session(pid: u32) -> bool {
    let desktop = session_pid().and_then(logind_session);
    desktop.is_some() && logind_session(pid) == desktop
}

/// Родитель процесса.
pub fn parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // После имени в скобках: состояние, затем номер родителя.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Значение настройки Xfce.
pub fn xget(channel: &str, prop: &str) -> Option<String> {
    output(&["xfconf-query", "-c", channel, "-p", prop]).map(|s| s.trim().to_string())
}

/// Массив из настроек Xfce (первые две строки ответа — заголовок).
pub fn xget_list(channel: &str, prop: &str) -> Vec<String> {
    xget(channel, prop)
        .unwrap_or_default()
        .lines()
        .skip(2)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

/// Какая программа держит имя на сессионной шине D-Bus (`""` — никто).
pub fn bus_owner(name: &str) -> String {
    output(&["busctl", "--user", "status", name])
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("Comm="))
        .unwrap_or_default()
        .to_string()
}

/// Где программа в `PATH`.
pub fn which(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|p| p.is_file())
}

/// Текст файла, если он есть.
/// Пометка в ярлыках и автозапусках, которые создал Mimshel: такие он может
/// переписать и убрать при откате.
pub const MANAGED_MARK: &str = "X-Mimshel=true";
/// Прежние пометки (до переименования — `shell`, ещё раньше — macos-look):
/// такие файлы тоже наши.
const OLD_MANAGED_MARKS: &[&str] = &["X-Shell=true", "X-Macos-Look=true"];

/// Создан ли файл с таким текстом нами (новой или прежней пометкой).
pub fn is_managed(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .any(|l| l == MANAGED_MARK || OLD_MANAGED_MARKS.contains(&l))
}

/// Строка списка запуска сеанса вместо панели Xfce: пустая команда с
/// пометкой, по которой панель возвращается.
pub const PANEL_OFF: &[&str] = &["true", "mimshel-no-panel"];
/// Прежние пометки (от `shell` и macos-look) — узнаём и меняем на новую.
pub const OLD_PANEL_OFF: &[&[&str]] = &[
    &["true", "shell-no-panel"],
    &["true", "macos-look-no-panel"],
];
/// То же для рабочего стола Xfce (xfdesktop), когда стол рисует Mimshel.
pub const DESKTOP_OFF: &[&str] = &["true", "mimshel-no-desktop"];
pub const OLD_DESKTOP_OFF: &[&[&str]] = &[&["true", "shell-no-desktop"]];
/// The Thunar daemon's session line, when Mimshel has its own file manager.
pub const THUNAR_DAEMON: &[&str] = &["Thunar", "--daemon"];
/// And what it becomes: Thunar still starts on demand through D-Bus
/// (`org.xfce.FileManager`, e.g. the dock's "empty trash").
pub const THUNAR_OFF: &[&str] = &["true", "mimshel-no-thunar"];

pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Запустить команду и дождаться; ошибку только печатаем — так же поступал
/// macos-look: одна несработавшая настройка не должна обрывать остальные.
pub fn run(args: &[&str]) -> bool {
    let Some((program, rest)) = args.split_first() else {
        return false;
    };
    match command(program).args(rest).stdout(Stdio::null()).status() {
        Ok(s) if s.success() => true,
        Ok(s) => {
            eprintln!("  {} — ошибка ({s})", args.join(" "));
            false
        }
        Err(e) => {
            eprintln!("  {program}: {e}");
            false
        }
    }
}

/// Запустить фоновую программу, отвязанную от терминала.
pub fn spawn(args: &[&str]) {
    let _ = command("setsid")
        .arg("-f")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Остановить свои процессы с этим именем и дождаться, пока они выйдут (до 3 с).
/// Остановить службу — и под новым именем (`mimshel daemon`), и под
/// прежним (`shell daemon`: так её запускали до переименования).
pub fn kill_daemon() {
    for name in [mimshel_core::names::APP, mimshel_core::names::OLD] {
        kill(&format!("{name} daemon"));
    }
}

pub fn kill(name: &str) {
    let pattern = format!(r"^(\S*/)?{name}(\s|$)");
    let uid = uid().to_string();
    // Не вышла за 3 с — остановить силой: иначе новая служба увидит старую,
    // решит «уже запущена» и уйдёт, а старая выйдет следом — не останется ни
    // одной (так пропадали строка меню и док).
    for signal in ["-TERM", "-KILL"] {
        let _ = command("pkill")
            .args([signal, "-u", &uid, "-f", &pattern])
            .status();
        for _ in 0..30 {
            if !running(name) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

/// Записать настройку Xfce (`-n` — создать, если её нет).
pub fn xset(channel: &str, prop: &str, kind: &str, value: &str) -> bool {
    run(&[
        "xfconf-query",
        "-c",
        channel,
        "-p",
        prop,
        "-n",
        "-t",
        kind,
        "-s",
        value,
    ])
}

/// Записать массив в настройки Xfce.
pub fn xset_array(channel: &str, prop: &str, kind: &str, values: &[&str]) -> bool {
    let mut args = vec!["xfconf-query", "-c", channel, "-p", prop, "-n", "-a"];
    for v in values {
        args.extend(["-t", kind, "-s", v]);
    }
    run(&args)
}

/// Сбросить настройку Xfce (со всем, что под ней).
pub fn xreset(channel: &str, prop: &str) {
    let _ = command("xfconf-query")
        .args(["-c", channel, "-p", prop, "-r", "-R"])
        .stderr(Stdio::null())
        .status();
}

/// Записать файл, только если текст поменялся, и сразу целиком (через
/// временный файл): панель Xfce следит за файлами и успевала прочитать пустой.
pub fn write_if_changed(path: &Path, text: &str) -> std::io::Result<bool> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(text) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    Ok(true)
}

/// Удалить файл, если он есть.
pub fn remove(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("  не удалось удалить {}: {e}", path.display());
    }
}

/// Все раскладки из настроек загружены в X (после выхода панели Xfce её модуль
/// раскладки иногда оставляет одну — урок 15).
pub fn keyboard_ok() -> bool {
    // Общая раскладка X повторяет клавиатуру последнего нажатия, а у
    // виртуальной клавиатуры XTEST (нажатия от программ, xdotool) после
    // входа раскладка по умолчанию. Поэтому смотрим настоящие клавиатуры.
    keyboards_ok(false)
}

/// Все раскладки есть и у виртуальной клавиатуры XTEST. После входа у неё
/// только `us` (урок 18), и одно программное нажатие делает общую раскладку
/// `us` — значок раскладки в строке меню пропадает.
pub fn virtual_keyboard_ok() -> bool {
    keyboards_ok(true)
}

/// Все раскладки из настроек у настоящих (`virtual_kbd` = false) или у
/// виртуальных клавиатур. Не удалось узнать устройства — общая раскладка.
fn keyboards_ok(virtual_kbd: bool) -> bool {
    let want = xkb_query("layout");
    let display = std::env::var("DISPLAY")
        .ok()
        .or_else(|| session_env().remove("DISPLAY"))
        .unwrap_or_else(|| ":0".into());
    // `None` — у устройства нет раскладки вовсе (датчик крышки ноутбука
    // числится клавиатурой, но клавиш с буквами у него нет).
    let has_all = |args: &[&str]| {
        let symbols = output_any(args);
        let line = symbols.lines().find(|l| l.contains("xkb_symbols"))?;
        Some(
            want.split(',')
                .filter(|c| !c.is_empty())
                .all(|c| line.contains(c)),
        )
    };
    let ids = keyboards(&output_any(&["xinput", "list", "--short"]), virtual_kbd);
    if ids.is_empty() {
        return has_all(&["xkbcomp", "-xkb", &display, "-"]) == Some(true);
    }
    ids.iter()
        .all(|id| has_all(&["xkbcomp", "-i", id, "-xkb", &display, "-"]) != Some(false))
}

/// Номера клавиатур из `xinput list --short`: подчинённые настоящие или
/// (`virtual_kbd`) виртуальные XTEST.
fn keyboards(list: &str, virtual_kbd: bool) -> Vec<String> {
    list.lines()
        .filter(|l| l.contains("slave  keyboard") && l.contains("XTEST") == virtual_kbd)
        .filter_map(|l| l.split("id=").nth(1)?.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// Строка из `setxkbmap -query` (rules, model, layout, options).
pub fn xkb_query(key: &str) -> String {
    output_any(&["setxkbmap", "-query"])
        .lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Мониторы: из xrandr и из настроек обоев Xfce.
pub fn monitors() -> Vec<String> {
    let mut found: Vec<String> = output_any(&["xrandr", "--listmonitors"])
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().last())
        .map(|m| format!("monitor{m}"))
        .collect();
    for line in output_any(&["xfconf-query", "-c", "xfce4-desktop", "-l"]).lines() {
        let parts: Vec<&str> = line.split('/').collect();
        if parts.len() > 3 && parts[1] == "backdrop" && parts[3].starts_with("monitor") {
            found.push(parts[3].to_string());
        }
    }
    found.sort();
    found.dedup();
    if found.is_empty() {
        found.push("monitor0".into());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_and_virtual_keyboards() {
        let list = "⎣ Virtual core keyboard                   \tid=3\t[master keyboard (2)]\n\
            \x20   ↳ Virtual core XTEST keyboard             \tid=5\t[slave  keyboard (3)]\n\
            \x20   ↳ Power Button                            \tid=9\t[slave  keyboard (3)]\n\
            \x20   ↳ AT Translated Set 2 keyboard            \tid=11\t[slave  keyboard (3)]\n\
            ⎡ Virtual core pointer                    \tid=2\t[master pointer  (3)]\n";
        assert_eq!(keyboards(list, false), ["9", "11"]);
        assert_eq!(keyboards(list, true), ["5"]);
    }
}
