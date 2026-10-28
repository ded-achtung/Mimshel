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
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Служба уже запущена — перезапускать незачем.
pub const ALREADY_RUNNING: i32 = 3;
/// Проработала дольше — значит, упала «случайно», а не сразу при запуске.
const STABLE: Duration = Duration::from_secs(60);
/// Столько падений подряд вскоре после запуска — сдаёмся (например, X закрылся
/// при выходе из сеанса, и служба не может подключиться).
const GIVE_UP: u32 = 5;
/// Журнал больше этого начинается заново (the previous one is kept as
/// `daemon.log.1`); checked at every service start and every minute.
const LOG_LIMIT: u64 = 1024 * 1024;
const LOG_CHECK: Duration = Duration::from_secs(60);

/// How the service ended, as far as restarting goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// Another guard's service is already running.
    AlreadyRunning,
    /// Stopped on purpose: a new build deployed (TERM), the session ends (HUP).
    Stopped,
    /// A crash, or it did not start at all.
    Crashed,
}

/// What the guard does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Exit,
    /// Start it again after this pause.
    Restart(Duration),
    /// Too many quick crashes in a row: tell the user and stop.
    GiveUp,
}

/// The restart rules. `quick` counts crashes in a row soon after a start; a
/// run that lasted [`STABLE`] starts the count anew, a stop on purpose
/// clears it. The pause grows with the count: 1, 2, 3, 4 s.
fn next_step(quick: &mut u32, ended: Ended, lived: Duration) -> Next {
    match ended {
        Ended::AlreadyRunning => Next::Exit,
        Ended::Stopped => {
            *quick = 0;
            Next::Restart(Duration::ZERO)
        }
        Ended::Crashed => {
            *quick = if lived >= STABLE { 1 } else { *quick + 1 };
            if *quick >= GIVE_UP {
                Next::GiveUp
            } else {
                Next::Restart(Duration::from_secs(u64::from(*quick)))
            }
        }
    }
}

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
    // The service writes into the log for the whole session; without a
    // check while it runs, one long session grew the file without bound.
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(LOG_CHECK);
            if let Some(path) = log_path() {
                rotate_if_big(&path);
            }
        }
    });
    let exe = own_exe();
    let mut quick_failures = 0;
    loop {
        let started = Instant::now();
        let child = spawn(&exe);
        let lived = started.elapsed();
        let (reason, report, ended) = match &child {
            Ok((s, _)) if s.code() == Some(ALREADY_RUNNING) => {
                (String::new(), None, Ended::AlreadyRunning)
            }
            Ok((s, pid)) => {
                use std::os::unix::process::ExitStatusExt;
                let lang = mimshel_core::i18n::language_from_config();
                let (reason, report) = crate::crash::service_failure_lang(s, *pid, lang);
                let ended = if mimshel_core::crash::is_crash(s.code(), s.signal()) {
                    Ended::Crashed
                } else {
                    Ended::Stopped
                };
                (reason, report, ended)
            }
            Err(e) => (format!("не запустилась: {e}"), None, Ended::Crashed),
        };
        if ended == Ended::Crashed {
            // No panic report (a signal, an exit code): write one here, with
            // the last lines of the log, so `mimshel crashes` and `doctor` see it.
            let report = report.or_else(|| match &child {
                Ok((_, pid)) => mimshel_setup::crash::write_exit_report(
                    "service",
                    *pid,
                    &reason,
                    log_path().as_deref(),
                ),
                Err(_) => None,
            });
            // The restarted service will explain the crash to the user.
            crate::crash::note_last_crash("service", &reason, report.as_deref());
        }
        match next_step(&mut quick_failures, ended, lived) {
            Next::Exit => {
                log("служба уже запущена другим сторожем — выхожу");
                std::process::exit(0);
            }
            // Stopped on purpose: a plain restart — no last-crash, no
            // notification, and it does not count towards the give-up.
            Next::Restart(pause) if ended == Ended::Stopped => {
                log(&format!("служба {reason} — запускаю снова"));
                std::thread::sleep(pause);
            }
            Next::Restart(pause) => {
                log(&format!(
                    "служба {reason} через {:.0?}, перезапуск через {pause:?}",
                    lived
                ));
                std::thread::sleep(pause);
            }
            Next::GiveUp => {
                log(&format!(
                    "служба {reason}, {GIVE_UP} раз подряд вскоре после запуска — больше не перезапускаю"
                ));
                give_up(&reason, mimshel_core::i18n::language_from_config());
            }
        }
    }
}

/// Five quick crashes in a row — there will be no dock or menu bar, and
/// the user must be told why: a dialog with the reason, detached from the
/// guard (zenity, xmessage as the fallback, else just the log).
fn give_up(reason: &str, lang: mimshel_core::i18n::Language) -> ! {
    let text = format!(
        "{}\n{} {reason}\n{}\n{}",
        mimshel_core::i18n::t(lang, "crash.give_up_head"),
        mimshel_core::i18n::t(lang, "crash.give_up_reason"),
        mimshel_core::i18n::t(lang, "notify.crash_details"),
        mimshel_core::i18n::t(lang, "crash.give_up_restore")
    );
    let mut shown = false;
    for (program, args) in [
        (
            "zenity",
            vec!["--error", "--title", "Mimshel", "--text", &text],
        ),
        ("xmessage", vec![text.as_str()]),
    ] {
        let mut command = std::process::Command::new(program);
        command.args(args);
        match crate::ui::spawn_detached(command) {
            Ok(()) => {
                shown = true;
                break;
            }
            Err(e) => log(&format!("окно о сдаче не показано ({program}: {e})")),
        }
    }
    if !shown {
        log("ни zenity, ни xmessage не запустились — причина только в журнале");
    }
    std::process::exit(1);
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

fn spawn(exe: &PathBuf) -> std::io::Result<(ExitStatus, u32)> {
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
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let status = child.wait()?;
    Ok((status, pid))
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
    rotate_if_big(&path);
    // Always append: the running service writes through its own copy of
    // this handle, and with O_APPEND its writes land at the end even after
    // the file was emptied under it.
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Keep the log under [`LOG_LIMIT`]: the old text goes to `daemon.log.1`
/// and the file is emptied in place (the service keeps writing to it).
fn rotate_if_big(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::copy(path, path.with_extension("log.1"));
        if let Ok(f) = OpenOptions::new().write(true).open(path) {
            let _ = f.set_len(0);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const SOON: Duration = Duration::from_secs(2);

    #[test]
    fn gives_up_after_five_quick_crashes() {
        let mut quick = 0;
        for pause in 1..GIVE_UP {
            assert_eq!(
                next_step(&mut quick, Ended::Crashed, SOON),
                Next::Restart(Duration::from_secs(u64::from(pause))),
                "the pause grows with every quick crash"
            );
        }
        assert_eq!(next_step(&mut quick, Ended::Crashed, SOON), Next::GiveUp);
    }

    #[test]
    fn a_stable_run_starts_the_count_anew() {
        let mut quick = 0;
        for _ in 1..GIVE_UP {
            next_step(&mut quick, Ended::Crashed, SOON);
        }
        // Worked for a while and then fell: an accident, not a broken start.
        assert_eq!(
            next_step(&mut quick, Ended::Crashed, STABLE),
            Next::Restart(Duration::from_secs(1))
        );
        assert_eq!(quick, 1);
    }

    #[test]
    fn a_stop_on_purpose_is_not_a_crash() {
        let mut quick = 0;
        for _ in 1..GIVE_UP {
            next_step(&mut quick, Ended::Crashed, SOON);
        }
        // Deploying a build kills the service with TERM: restart at once,
        // and the crashes before it no longer count.
        assert_eq!(
            next_step(&mut quick, Ended::Stopped, SOON),
            Next::Restart(Duration::ZERO)
        );
        assert_eq!(quick, 0);
        assert_eq!(
            next_step(&mut quick, Ended::Crashed, SOON),
            Next::Restart(Duration::from_secs(1))
        );
    }

    #[test]
    fn a_second_guard_leaves() {
        let mut quick = 3;
        assert_eq!(
            next_step(&mut quick, Ended::AlreadyRunning, SOON),
            Next::Exit
        );
        assert_eq!(quick, 3);
    }
}
