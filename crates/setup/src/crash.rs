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
}
