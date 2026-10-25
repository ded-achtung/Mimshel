//! Сторож службы. `mimshel daemon` сам ничего не рисует: он запускает настоящую
//! службу (`mimshel daemon --child`) отдельным процессом и, если та упала,
//! запускает заново. Служба рисует док — без сторожа её падение оставило бы
//! рабочий стол без дока до перезахода (урок 12 в CLAUDE.md).
//!
//! Сам сторож почти ничего не делает, поэтому падать ему не с чего. С X у него
//! только тихое соединение: закрылся X (выход из сеанса) — сторож выходит, а
//! не перезапускает службу (иначе, запущенный не из сеанса, например по SSH,
//! он пережил бы выход и цеплялся к X следующего сеанса). Остановить обоих: `pkill -f 'mimshel daemon'` (так делает `mimshel apply service`).
//! Служба следит за сторожем и выходит, если его не стало.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Служба уже запущена — перезапускать незачем.
pub const ALREADY_RUNNING: i32 = 3;
/// Проработала дольше — значит, упала «случайно», а не сразу при запуске.
const STABLE: Duration = Duration::from_secs(60);
/// Столько падений подряд вскоре после запуска — сдаёмся (например, X закрылся
/// при выходе из сеанса, и служба не может подключиться).
const GIVE_UP: u32 = 5;
/// Журнал больше этого при запуске сторожа начинается заново.
const LOG_LIMIT: u64 = 1024 * 1024;

pub fn run() -> ! {
    if crate::ui::daemon_running() {
        eprintln!("Служба уже запущена.");
        // И в журнал: запуск при входе или из mimshel apply иначе выходит молча.
        log("служба уже запущена — второй сторож не нужен, выхожу");
        std::process::exit(0);
    }
    match mimshel_x11::ServerWatch::connect() {
        Ok(watch) => {
            std::thread::spawn(move || {
                watch.wait_closed();
                log("X-сервер закрылся (выход из сеанса) — выхожу");
                // Служба выйдет сама, заметив, что сторожа нет.
                std::process::exit(0);
            });
        }
        Err(e) => log(&format!("не подключился к X ({e}) — слежу без него")),
    }
    let exe = own_exe();
    let mut quick_failures = 0;
    loop {
        let started = Instant::now();
        let status = spawn(&exe);
        let lived = started.elapsed();
        let reason = match &status {
            Ok(s) if s.code() == Some(ALREADY_RUNNING) => {
                log("служба уже запущена другим сторожем — выхожу");
                std::process::exit(0);
            }
            Ok(s) => describe(s),
            Err(e) => format!("не запустилась: {e}"),
        };
        quick_failures = if lived >= STABLE {
            1
        } else {
            quick_failures + 1
        };
        if quick_failures >= GIVE_UP {
            log(&format!(
                "служба {reason}, {GIVE_UP} раз подряд вскоре после запуска — больше не перезапускаю"
            ));
            std::process::exit(1);
        }
        // Чем чаще падает, тем дольше пауза: 1, 2, 3, 4 с.
        let pause = Duration::from_secs(u64::from(quick_failures));
        log(&format!(
            "служба {reason} через {:.0?}, перезапуск через {pause:?}",
            lived
        ));
        std::thread::sleep(pause);
    }
}

/// Служба проверяет, жив ли её сторож: осиротевшая служба выходит (иначе
/// после `kill` сторожа она жила бы без присмотра).
pub fn watch_parent() {
    let parent = std::os::unix::process::parent_id();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if std::os::unix::process::parent_id() != parent {
                std::process::exit(0);
            }
        }
    });
}

fn spawn(exe: &PathBuf) -> std::io::Result<ExitStatus> {
    let mut cmd = Command::new(exe);
    cmd.args(["daemon", "--child"]).stdin(Stdio::null());
    // Распределитель памяти glibc после первого освобождения большого
    // буфера (снимок экрана, фон «Программ») поднимает порог и дальше держит
    // такие куски у себя. Твёрдый порог — большие буферы берутся у системы
    // и сразу возвращаются; две «арены» вместо одной на каждый поток.
    for (key, value) in [
        ("MALLOC_MMAP_THRESHOLD_", "131072"),
        ("MALLOC_ARENA_MAX", "2"),
    ] {
        if std::env::var_os(key).is_none() {
            cmd.env(key, value);
        }
    }
    // Всё, что служба пишет (и сообщение о панике), — в журнал.
    if let Some(file) = log_file() {
        cmd.stderr(file.try_clone()?).stdout(file);
    }
    cmd.status()
}

fn describe(status: &ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(101), _) => "упала (паника)".into(),
        (Some(code), _) => format!("завершилась с кодом {code}"),
        (None, Some(sig)) => format!("убита сигналом {sig}"),
        (None, None) => "завершилась".into(),
    }
}

/// Путь к своей программе. Если её файл заменили новой сборкой (`install`),
/// ядро добавляет к пути « (deleted)» — тогда берём новый файл по тому же пути.
pub fn own_exe() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(mimshel_core::names::APP));
    match exe.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(path) => PathBuf::from(path),
        None => exe,
    }
}

/// `~/.local/state/mimshel/daemon.log`.
pub fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(mimshel_core::names::dir_in(&base).join("daemon.log"))
}

fn log_file() -> Option<File> {
    let path = log_path()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT);
    OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(true)
        .truncate(too_big)
        .open(path)
        .ok()
}

fn log(message: &str) {
    let line = format!("[сторож {}] {message}\n", now());
    eprint!("{line}");
    if let Some(mut f) = log_file() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Местное время «ЧЧ:ММ:СС» — без лишних библиотек, через `date`.
fn now() -> String {
    Command::new("date")
        .arg("+%F %T")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}
