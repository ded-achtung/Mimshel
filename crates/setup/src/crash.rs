//! Crash-report lookup shared by the guards: why did the child process
//! fall, in plain Russian, read back from the report it wrote before
//! dying. The pure pieces (parsing, reasons) live in `mimshel_core::crash`.

use std::path::{Path, PathBuf};

use mimshel_core::crash as common;

/// `~/.local/state/mimshel/crashes` — every crash report lands here.
pub fn report_dir() -> Option<PathBuf> {
    common::state_dir().map(|d| d.join("crashes"))
}

/// The «message» в file:line from the report of process `pid` (the report
/// file name carries the pid) plus the report path. `None` — no report:
/// the process died before writing one, or it was not ours.
pub fn panic_detail(dir: &Path, pid: u32) -> Option<(String, PathBuf)> {
    let want = format!("-{pid}.txt");
    let mut newest: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(&want) && common::stamp_seconds(&name).is_some() {
            let path = entry.path();
            if newest.as_ref().is_none_or(|n| path > *n) {
                newest = Some(path);
            }
        }
    }
    let path = newest?;
    let text = std::fs::read_to_string(&path).ok()?;
    common::report_reason(&text).map(|detail| (detail, path))
}

/// Like [`panic_detail`], but in the real reports folder.
pub fn panic_reason(pid: u32) -> Option<String> {
    let dir = report_dir()?;
    panic_detail(&dir, pid).map(|(detail, _)| detail)
}

/// Keep at most [`common::KEEP_REPORTS`] newest reports in `dir`: the name
/// starts with the time, so age is the name order.
pub fn prune(dir: &Path) {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".txt") && common::stamp_seconds(n).is_some())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    while names.len() > common::KEEP_REPORTS {
        let oldest = names.remove(0);
        let _ = std::fs::remove_file(dir.join(oldest));
    }
}

/// How many log lines go into an exit report.
const LOG_TAIL_LINES: usize = 30;

/// A crash that left no panic report (a signal, an exit code): the guard
/// writes an [`common::ExitReport`] for process `pid` with the last lines of
/// `log`, so `mimshel crashes` and `doctor` see it too. Returns its path.
pub fn write_exit_report(id: &str, pid: u32, reason: &str, log: Option<&Path>) -> Option<PathBuf> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let log_text = log
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    write_exit_report_in(
        &report_dir()?,
        &common::stamp(now),
        id,
        pid,
        reason,
        &log_text,
    )
}

fn write_exit_report_in(
    dir: &Path,
    at: &str,
    id: &str,
    pid: u32,
    reason: &str,
    log_text: &str,
) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(common::report_name(at, id, pid));
    let text = common::ExitReport {
        version: env!("CARGO_PKG_VERSION"),
        id,
        at,
        pid,
        reason,
        log_tail: &tail(log_text, LOG_TAIL_LINES),
    }
    .text();
    std::fs::write(&path, text).ok()?;
    prune(dir);
    Some(path)
}

/// The last `n` lines of `text`, each ending with a newline.
fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].iter().map(|l| format!("{l}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty per-test folder: `mimshel-crash-lookup-<pid>-<n>`.
    fn temp_dir() -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mimshel-crash-lookup-{}-{}",
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
            id: "lock-screen",
            at: "2026-10-26_12-34-56",
            pid,
            thread: "main",
            message,
            location: "apps/mimshel/src/lock_ui.rs:55:9",
            backtrace: "stack backtrace:\n  0: main",
        }
        .text()
    }

    #[test]
    fn finds_the_report_of_this_pid() {
        let dir = temp_dir();
        let pid = 4242;
        // Two reports: an older one of this pid (replaced by a newer) and
        // a report of another pid (left alone).
        std::fs::write(
            dir.join(common::report_name(
                "2026-10-26_10-00-00",
                "lock-screen",
                pid,
            )),
            report(pid, "old"),
        )
        .unwrap();
        let newest = dir.join(common::report_name(
            "2026-10-26_12-34-56",
            "lock-screen",
            pid,
        ));
        std::fs::write(&newest, report(pid, "bad credentials buffer")).unwrap();
        std::fs::write(
            dir.join(common::report_name("2026-10-26_12-34-56", "service", 7777)),
            report(7777, "other"),
        )
        .unwrap();
        let (detail, path) = panic_detail(&dir, pid).unwrap();
        assert_eq!(
            detail,
            "«bad credentials buffer» в apps/mimshel/src/lock_ui.rs:55"
        );
        assert_eq!(path, newest);
        // No report of this pid — nothing to append.
        assert_eq!(panic_detail(&dir, 1), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn exit_report_written_with_log_tail_and_found_by_summary() {
        let dir = temp_dir();
        let log: String = (1..=40).map(|i| format!("строка {i}\n")).collect();
        let path = write_exit_report_in(
            &dir,
            "2026-10-26_17-28-43",
            "service",
            13778,
            "завершилась аварийно (SIGABRT)",
            &log,
        )
        .expect("written");
        assert!(path.ends_with("2026-10-26_17-28-43-service-13778.txt"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("строка 40\n") && text.contains("строка 11\n"));
        assert!(!text.contains("строка 10\n"), "only the last 30 lines");
        assert_eq!(
            common::report_summary(&text).as_deref(),
            Some("завершилась аварийно (SIGABRT)")
        );
        // Not a panic report: the panic lookup by pid stays empty.
        assert!(panic_detail(&dir, 13778).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_the_newest() {
        let dir = temp_dir();
        for i in 0..23 {
            let name = format!("2026-10-26_10-00-{i:02}-service-1.txt");
            std::fs::write(dir.join(name), "x").unwrap();
        }
        prune(&dir);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left.len(), common::KEEP_REPORTS);
        assert!(
            left[0].contains("10-00-03"),
            "oldest three removed: {left:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
