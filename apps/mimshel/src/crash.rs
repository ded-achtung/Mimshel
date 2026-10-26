//! Crash reports: the panic hook ([`install`]) writes a report and one
//! short line to the log; the service guard leaves a `last-crash` so the
//! restart can be explained to the user; `mimshel crashes` lists the
//! reports. The pure pieces (text, reasons, file names) live in
//! `mimshel_core::crash`; the pid -> report lookup both guards share
//! lives in `mimshel_setup::crash`.

use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{SystemTime, UNIX_EPOCH};

use mimshel_core::crash as common;
use mimshel_dbus::notify::{Notification, NotifyEvent};
use mimshel_icons::Icons;

use crate::notify_ui::NotifyUi;

/// The id of the restart notification: our own, so it can never collide
/// with the ids `NotifyServer` hands out over D-Bus (it counts from 1).
const INTERNAL_ID: u32 = u32::MAX;
/// The summary of the restart notification, wherever it is shown.
const RESTART_SUMMARY: &str = "Mimshel перезапустился после сбоя";

/// Install the panic hook: writes a report into
/// `~/.local/state/mimshel/crashes` and one line to stderr (which lands
/// in daemon.log / lock.log). The hook must never panic itself and must
/// not wait on locks other code may hold: only std fs and formatting,
/// every error ignored.
pub fn install(component: &str) {
    let component = component.to_string();
    std::panic::set_hook(Box::new(move |info| {
        write_report(&component, info);
    }));
}

fn write_report(id: &str, info: &PanicHookInfo) {
    let Some(dir) = report_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let at = common::stamp(epoch_now());
    let pid = std::process::id();
    let path = dir.join(common::report_name(&at, id, pid));
    // The message goes into the header and the log line: newlines would
    // break both, so they become spaces.
    let message = message(info);
    let location = location(info);
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();
    let text = common::Report {
        version: env!("CARGO_PKG_VERSION"),
        id,
        at: &at,
        pid,
        thread: std::thread::current().name().unwrap_or("без имени"),
        message: &message,
        location: &location,
        backtrace: &backtrace,
    }
    .text();
    if std::fs::write(&path, text.as_bytes()).is_err() {
        return;
    }
    mimshel_setup::crash::prune(&dir);
    let detail = common::report_reason(&text).unwrap_or_else(|| "«паника»".into());
    eprintln!(
        "[сбой] {}: паника {detail} — отчёт: {}",
        common::component(id),
        path.display()
    );
}

/// The panic message on one line.
fn message(info: &PanicHookInfo) -> String {
    let payload = info.payload();
    let raw = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "неизвестная паника".into()
    };
    raw.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

fn location(info: &PanicHookInfo) -> String {
    match info.location() {
        Some(l) => format!("{}:{}:{}", l.file(), l.line(), l.column()),
        None => "неизвестно".into(),
    }
}

/// `~/.local/state/mimshel/crashes`.
fn report_dir() -> Option<PathBuf> {
    mimshel_setup::crash::report_dir()
}

/// `~/.local/state/mimshel/last-crash`.
pub fn last_crash_path() -> Option<PathBuf> {
    common::state_dir().map(|d| d.join("last-crash"))
}

fn epoch_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A `last-crash` record: the guard restarted the service after a crash,
/// and the service shows one notification about it (the file is removed
/// once read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastCrash {
    /// Epoch seconds (UTC) — the moment of the crash.
    pub time: u64,
    /// Component id (`service`).
    pub component: String,
    /// The plain-Russian reason, as in the guard's log.
    pub reason: String,
    /// Path to the report, if the process fell with a panic.
    pub report: Option<PathBuf>,
}

impl LastCrash {
    /// Younger than [`common::FRESH_SECS`] relative to `now` — worth a
    /// notification.
    pub fn fresh(&self, now: u64) -> bool {
        now.saturating_sub(self.time) < common::FRESH_SECS
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        let mut f = std::fs::File::create(path)?;
        use std::io::Write;
        writeln!(f, "time={}", self.time)?;
        writeln!(f, "component={}", self.component)?;
        writeln!(f, "reason={}", self.reason)?;
        writeln!(
            f,
            "report={}",
            self.report.as_ref().and_then(|p| p.to_str()).unwrap_or("")
        )?;
        Ok(())
    }

    pub fn parse(text: &str) -> Option<LastCrash> {
        let mut time = None;
        let mut component = None;
        let mut reason = None;
        let mut report = None;
        for line in text.lines() {
            let (key, value) = line.split_once('=')?;
            match key {
                "time" => time = value.parse().ok(),
                "component" => component = Some(value.to_string()),
                "reason" => reason = Some(value.to_string()),
                "report" => report = Some(value),
                _ => {}
            }
        }
        Some(LastCrash {
            time: time?,
            component: component?,
            reason: reason?,
            report: report.filter(|r| !r.is_empty()).map(PathBuf::from),
        })
    }
}

/// The guard restarts the service after a crash — write `last-crash`, so
/// the service can explain on startup what happened (once).
pub fn note_last_crash(id: &str, reason: &str, report: Option<&Path>) {
    let Some(path) = last_crash_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let crash = LastCrash {
        time: epoch_now(),
        component: id.to_string(),
        reason: reason.to_string(),
        report: report.map(PathBuf::from),
    };
    let _ = crash.write(&path);
}

/// The service is up: if `last-crash` exists and is fresh, say so. With
/// our own notification UI the banner goes straight to it; without one
/// (`parts.notify` = xfce) the same text goes through `notify-send`. The
/// file is removed either way, so the message shows once.
pub fn announce_restart(notify: Option<&mut NotifyUi>, icons: &mut Icons) {
    let Some(path) = last_crash_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Some(crash) = LastCrash::parse(&text) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    if !crash.fresh(epoch_now()) {
        return;
    }
    let body = format!("{}\nПодробности: mimshel crashes", crash.reason);
    match notify {
        Some(notify) => notify.notify_event(
            icons,
            NotifyEvent::Show(Notification {
                id: INTERNAL_ID,
                app_name: "Mimshel".into(),
                app_icon: "dialog-error".into(),
                summary: RESTART_SUMMARY.into(),
                body,
                actions: Vec::new(),
                image: None,
                desktop_entry: None,
                // Critical: shown even while the UI still assumes Do Not
                // Disturb at startup, and kept until clicked.
                urgency: 2,
                expire_timeout: 0, // never expires on its own: a crash must be seen
                transient: false,
            }),
        ),
        None => notify_send(&body),
    }
}

/// No own notification UI (xfce4-notifyd shows banners instead): the same
/// text through `notify-send`, critical urgency so it stays.
fn notify_send(body: &str) {
    let mut cmd = std::process::Command::new("notify-send");
    cmd.args(["-a", "Mimshel", "-u", "critical", RESTART_SUMMARY, body]);
    if let Err(e) = crate::ui::spawn_detached(cmd) {
        eprintln!("Не удалось показать уведомление о сбое: {e}");
    }
}

/// The reason the service child fell, for the guard's log: by code or
/// signal, and for a panic (code 101) also the message and place from
/// this process's report (the file name carries the pid).
pub fn service_failure(status: &ExitStatus, pid: u32) -> (String, Option<PathBuf>) {
    use std::os::unix::process::ExitStatusExt;
    service_failure_at(report_dir(), status.code(), status.signal(), pid)
}

fn service_failure_at(
    dir: Option<PathBuf>,
    code: Option<i32>,
    signal: Option<i32>,
    pid: u32,
) -> (String, Option<PathBuf>) {
    let mut reason = common::exit_reason(code, signal);
    let mut report = None;
    if code == Some(101)
        && let Some(dir) = dir
        && let Some((detail, path)) = mimshel_setup::crash::panic_detail(&dir, pid)
    {
        reason = format!("{reason}: {detail}");
        report = Some(path);
    }
    (reason, report)
}

/// `mimshel crashes [last]`: the list of crashes (newest first, up to 10
/// lines) or the newest report in full.
pub fn crashes_cmd(args: &[String]) -> i32 {
    let Some(dir) = report_dir() else {
        println!("Сбоев не было");
        return 0;
    };
    let reports = list_reports(&dir);
    if args.first().is_some_and(|a| a == "last") {
        let Some(newest) = reports.first() else {
            println!("Сбоев не было");
            return 0;
        };
        return match std::fs::read_to_string(newest) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(e) => {
                println!("✘ Не прочитать {}: {e}", newest.display());
                1
            }
        };
    }
    if reports.is_empty() {
        println!("Сбоев не было");
        return 0;
    }
    for path in reports.iter().take(10) {
        println!("{}", listing_line(path));
    }
    0
}

/// All reports, newest first.
fn list_reports(dir: &Path) -> Vec<PathBuf> {
    let mut reports: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(".txt") && common::stamp_seconds(n).is_some())
                })
                .collect()
        })
        .unwrap_or_default();
    reports.sort();
    reports.reverse();
    reports
}

/// One list line: time, component, the reason from the report, path.
fn listing_line(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let Some((at, id, _pid)) = parse_report_name(&name) else {
        return name;
    };
    let reason = std::fs::read_to_string(path)
        .ok()
        .as_deref()
        .and_then(common::report_summary)
        .unwrap_or_else(|| common::exit_reason(Some(101), None));
    common::listing_line(at, id, &reason, &path.display().to_string())
}

/// A report file name -> (time, component, pid). The time is the first 19
/// characters, the pid is after the last `-` (a component may contain
/// one: `lock-screen`).
fn parse_report_name(name: &str) -> Option<(&str, &str, u32)> {
    let bare = name.strip_suffix(".txt")?;
    common::stamp_seconds(name)?;
    let rest = bare.get(19..)?.strip_prefix('-')?;
    let (id, pid) = rest.rsplit_once('-')?;
    Some((&bare[..19], id, pid.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty per-test folder: `mimshel-crash-<tag>-<pid>-<n>`.
    fn temp_dir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mimshel-crash-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn report(pid: u32, message: &str) -> String {
        common::Report {
            version: "0.0.2-alpha",
            id: "service",
            at: "2026-10-26_12-34-56",
            pid,
            thread: "main",
            message,
            location: "crates/dock/src/lib.rs:412:17",
            backtrace: "stack backtrace:\n  0: main",
        }
        .text()
    }

    #[test]
    fn retention_keeps_twenty_newest() {
        let dir = temp_dir("retention");
        for i in 0..=20u32 {
            let at = common::stamp(1_761_000_000 + u64::from(i) * 60);
            std::fs::write(
                dir.join(common::report_name(&at, "service", 1000 + i)),
                report(1000 + i, "boom"),
            )
            .unwrap();
        }
        // A file that is not a report is left alone.
        std::fs::write(dir.join("notes.txt"), "keep").unwrap();
        mimshel_setup::crash::prune(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), 21, "20 reports + notes.txt: {left:?}");
        assert!(left.contains(&"notes.txt".to_string()));
        // The oldest report is gone, the rest is intact.
        assert!(!left.contains(&common::report_name(
            &common::stamp(1_761_000_000),
            "service",
            1000
        )));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn last_crash_round_trip_and_freshness() {
        let dir = temp_dir("last-crash");
        let path = dir.join("last-crash");
        let crash = LastCrash {
            time: 1_761_000_000,
            component: "service".into(),
            reason: "упала (паника): «boom» в crates/dock/src/lib.rs:412".into(),
            report: Some(PathBuf::from("/tmp/crashes/x.txt")),
        };
        crash.write(&path).unwrap();
        let back = LastCrash::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(crash, back);
        assert_eq!(
            back.report.as_deref(),
            Some(Path::new("/tmp/crashes/x.txt"))
        );
        // 10 minutes: still fresh — already not.
        assert!(back.fresh(1_761_000_000 + common::FRESH_SECS - 1));
        assert!(!back.fresh(1_761_000_000 + common::FRESH_SECS + 1));
        // Without a report path the field stays empty and reads back.
        let bare = LastCrash {
            report: None,
            ..crash.clone()
        };
        bare.write(&path).unwrap();
        let back = LastCrash::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.report, None);
        assert_eq!(back.reason, crash.reason);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn service_failure_reason_for_every_case() {
        let dir = temp_dir("failure");
        let pid = 4242;
        std::fs::write(
            dir.join(common::report_name("2026-10-26_12-34-56", "service", pid)),
            report(pid, "index out of bounds"),
        )
        .unwrap();
        // A panic: the reason gets the message and place from this pid's
        // report.
        let (reason, report_path) = service_failure_at(Some(dir.clone()), Some(101), None, pid);
        assert_eq!(
            reason,
            "упала (паника): «index out of bounds» в crates/dock/src/lib.rs:412"
        );
        assert_eq!(
            report_path.as_deref(),
            Some(dir.join("2026-10-26_12-34-56-service-4242.txt").as_path())
        );
        // A panic with no report of this pid — just "упала (паника)".
        let (reason, report_path) = service_failure_at(Some(dir.clone()), Some(101), None, 7777);
        assert_eq!(reason, "упала (паника)");
        assert_eq!(report_path, None);
        // Codes and signals — as in the reason table.
        assert_eq!(
            service_failure_at(None, Some(1), None, pid).0,
            "завершилась с кодом 1"
        );
        assert_eq!(
            service_failure_at(None, None, Some(9), pid).0,
            "убита извне (SIGKILL) — чаще всего системе не хватило памяти"
        );
        assert_eq!(
            service_failure_at(None, None, Some(6), pid).0,
            "завершилась аварийно (SIGABRT)"
        );
        assert_eq!(
            service_failure_at(None, None, Some(42), pid).0,
            "убита сигналом 42"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn crashes_listing_is_newest_first_with_reason() {
        let dir = temp_dir("listing");
        for (i, id) in ["service", "terminal"].iter().enumerate() {
            let at = common::stamp(1_761_000_000 + i as u64 * 3600);
            std::fs::write(
                dir.join(common::report_name(&at, id, 500 + i as u32)),
                report(500, &format!("boom-{id}")),
            )
            .unwrap();
        }
        let reports = list_reports(&dir);
        assert_eq!(reports.len(), 2);
        let lines: Vec<String> = reports.iter().map(|p| listing_line(p)).collect();
        assert!(
            lines[0].contains("терминал: упала (паника): «boom-terminal»"),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains('—'));
        assert!(lines[1].contains("служба: упала (паника): «boom-service»"));
        // The file name parses: time, a hyphenated component, pid.
        assert_eq!(
            parse_report_name("2026-10-26_12-34-56-lock-screen-42.txt"),
            Some(("2026-10-26_12-34-56", "lock-screen", 42))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn real_panic_writes_a_report() {
        let dir = temp_dir("real-panic");
        if std::env::var_os("MIMSHEL_TEST_CRASH").is_none() {
            // Parent: re-run this very test with a flag — the child falls.
            // `--exact` wants the full test name (with the module path).
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "crash::tests::real_panic_writes_a_report",
                    "--nocapture",
                ])
                .env("MIMSHEL_TEST_CRASH", "1")
                .env("XDG_STATE_HOME", &dir)
                // Captured: the child's own "FAILED" is expected and would
                // only confuse a CI log. Shown if the check below fails.
                .output()
                .unwrap();
            assert_eq!(
                status.status.code(),
                Some(101),
                "the child must fall: {}\n{}",
                status.status,
                String::from_utf8_lossy(&status.stdout)
            );
            let crashes = dir.join("mimshel/crashes");
            let mut reports: Vec<_> = std::fs::read_dir(&crashes)
                .unwrap_or_else(|e| panic!("no reports folder: {crashes:?}: {e}"))
                .flatten()
                .map(|e| e.path())
                .collect();
            assert_eq!(reports.len(), 1, "one report: {reports:?}");
            let path = reports.pop().unwrap();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.contains("-service-"), "name: {name}");
            let text = std::fs::read_to_string(&path).unwrap();
            for want in [
                "MIMSHEL_CRASH_MARKER_7f3d",
                "src/crash.rs",
                "стек вызовов:",
                "компонент: служба (service)",
            ] {
                assert!(text.contains(want), "no {want} in the report:\n{text}");
            }
            // There is a backtrace: in a debug build the frames follow
            // right away (numbered), in release after "stack backtrace:".
            assert!(
                text.contains("   0: ") || text.contains("stack backtrace:"),
                "no backtrace in the report:\n{text}"
            );
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        }
        // Child: a real panic — the hook writes the report.
        install("service");
        panic!("MIMSHEL_CRASH_MARKER_7f3d");
    }
}
