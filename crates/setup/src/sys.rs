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
pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}
