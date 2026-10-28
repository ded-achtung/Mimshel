//! Crash reports: when a part of Mimshel falls, the report must explain
//! why — to the user, in plain Russian, and to a developer. This module
//! holds only the pure parts (no files, no clocks): the report text, the
//! file name, the plain-Russian reason of an exit, the list line. The
//! writer lives in `apps/mimshel/src/crash.rs`; the readers are the
//! service guard and `mimshel doctor`.

use std::path::PathBuf;

use crate::names;

/// Reports kept in the `crashes` folder (the writer prunes by name).
pub const KEEP_REPORTS: usize = 20;
/// A `last-crash` younger than this gets a restart notification.
pub const FRESH_SECS: u64 = 10 * 60;
/// `mimshel doctor` looks this far back for crashes.
pub const WEEK_SECS: u64 = 7 * 24 * 60 * 60;

/// Russian name of a component by its short id (`service` -> "служба").
/// The table lives in one place: the report header, `mimshel crashes`
/// and the restart notification all take names from here.
pub fn component(id: &str) -> &'static str {
    match id {
        "service" => "служба",
        "lock-screen" => "экран блокировки",
        "terminal" => "терминал",
        "folder" => "Проводник",
        _ => "команда mimshel",
    }
}

/// The state folder (`~/.local/state/mimshel`, or `$XDG_STATE_HOME/mimshel`
/// when set): crash reports and `last-crash`. Overridable through the
/// environment so tests write into an empty folder (see `guard::log_path`).
pub fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(names::dir_in(&base))
}

/// Seconds since the epoch -> `YYYY-MM-DD_HH-MM-SS` (UTC). A panic hook
/// must not spawn `date`, so the civil date is computed directly
/// (Howard Hinnant's days->civil algorithm).
pub fn stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let left = secs % 86_400;
    let (h, m, s) = (left / 3600, left % 3600 / 60, left % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}_{h:02}-{m:02}-{s:02}")
}

/// Epoch seconds from the start of a report file name (the first 19
/// characters are the time), if they hold a time.
pub fn stamp_seconds(file_name: &str) -> Option<u64> {
    let at = file_name.get(..19)?;
    let date = at.get(..10)?;
    let time = at.get(11..19)?;
    if at.as_bytes().get(10) != Some(&b'_') {
        return None;
    }
    let (y, m, d) = triplet(date, '-')?;
    let (h, min, s) = triplet(time, '-')?;
    if !(1..=12).contains(&m)
        || !(1..=31).contains(&d)
        || !(0..=23).contains(&h)
        || min > 59
        || s > 60
    {
        return None;
    }
    Some(
        days_from_civil(y, m, d) as u64 * 86_400
            + h as u64 * 3600
            + u64::from(min) * 60
            + u64::from(s),
    )
}

/// Report file name: `<time>-<component>-<pid>.txt` (starting with the
/// time, sorting by name matches sorting by time).
pub fn report_name(at: &str, id: &str, pid: u32) -> String {
    format!("{at}-{id}-{pid}.txt")
}

/// Report data: a header for a human (what fell, when, which process),
/// then a backtrace for a developer. The time is already formatted
/// ([`stamp`]) — the writer must not read the clock here.
pub struct Report<'a> {
    pub version: &'a str,
    /// Component id (`service`).
    pub id: &'a str,
    pub at: &'a str,
    pub pid: u32,
    pub thread: &'a str,
    pub message: &'a str,
    pub location: &'a str,
    pub backtrace: &'a str,
}

impl Report<'_> {
    /// The full text of the report.
    pub fn text(&self) -> String {
        format!(
            "Mimshel {version} — отчёт о сбое\n\
             компонент: {ru} ({id})\n\
             время (UTC): {at}\n\
             процесс: {pid}\n\
             поток: {thread}\n\
             паника: «{message}»\n\
             место: {location}\n\
             \n\
             стек вызовов:\n{backtrace}",
            version = self.version,
            id = self.id,
            at = self.at,
            pid = self.pid,
            thread = self.thread,
            message = self.message,
            location = self.location,
            backtrace = self.backtrace,
            ru = component(self.id),
        )
    }
}

/// A crash without a panic (a signal, an exit code): the guard writes this
/// report itself — there is no backtrace, but the log lines just before the
/// exit often show what was going on.
pub struct ExitReport<'a> {
    pub version: &'a str,
    /// Component id (`service`).
    pub id: &'a str,
    pub at: &'a str,
    pub pid: u32,
    /// Plain-Russian reason, as in the guard's log.
    pub reason: &'a str,
    /// The last lines of the component's log before the exit.
    pub log_tail: &'a str,
}

impl ExitReport<'_> {
    /// The full text of the report.
    pub fn text(&self) -> String {
        format!(
            "Mimshel {version} — отчёт о сбое\n\
             компонент: {ru} ({id})\n\
             время (UTC): {at}\n\
             процесс: {pid}\n\
             причина: {reason}\n\
             (паники не было — стека вызовов нет)\n\
             \n\
             журнал перед сбоем:\n{tail}",
            version = self.version,
            id = self.id,
            at = self.at,
            pid = self.pid,
            reason = self.reason,
            tail = self.log_tail,
            ru = component(self.id),
        )
    }
}

/// The short reason of any report, as the guard logs it: the `причина:`
/// line of an [`ExitReport`], or `упала (паника): «message» в file:line`
/// for a panic [`Report`].
pub fn report_summary(text: &str) -> Option<String> {
    if let Some(reason) = text.lines().find_map(|l| l.strip_prefix("причина: ")) {
        return Some(reason.to_string());
    }
    report_reason(text).map(|detail| format!("{}: {detail}", exit_reason(Some(101), None)))
}

/// The «message» в file:line from a report text (the column is dropped
/// for brevity). Pure — read by the guard and `doctor`.
pub fn report_reason(text: &str) -> Option<String> {
    let mut message: Option<&str> = None;
    let mut location: Option<&str> = None;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("паника: ") {
            message = Some(v);
        } else if let Some(v) = line.strip_prefix("место: ") {
            location = Some(v);
        }
    }
    let message = message?.strip_prefix('«')?.strip_suffix('»')?;
    let location = location?;
    // The location is file:line:column; the short reason drops the column.
    let short = location.rsplit_once(':').map_or(location, |(file, _)| file);
    Some(format!("«{message}» в {short}"))
}

/// The reason a process exited, in plain Russian: an exit code or a
/// signal with an explanation (by signal name, so it is clear what
/// happened). Kept as the Russian variant for existing callers/tests;
/// new UI code should use `exit_reason_lang`.
pub fn exit_reason(code: Option<i32>, signal: Option<i32>) -> String {
    exit_reason_lang(crate::i18n::Language::Ru, code, signal)
}

/// Language-aware variant of `exit_reason`.
pub fn exit_reason_lang(
    lang: crate::i18n::Language,
    code: Option<i32>,
    signal: Option<i32>,
) -> String {
    match lang.resolve() {
        crate::i18n::Language::En => match (code, signal) {
            (Some(101), _) => "crashed (panic)".into(),
            (Some(c), _) => format!("exited with code {c}"),
            (None, Some(sig)) => signal_reason_lang(lang, sig),
            (None, None) => "exited".into(),
        },
        crate::i18n::Language::Zh => match (code, signal) {
            (Some(101), _) => "崩溃（panic）".into(),
            (Some(c), _) => format!("退出码 {c}"),
            (None, Some(sig)) => signal_reason_lang(lang, sig),
            (None, None) => "已退出".into(),
        },
        crate::i18n::Language::ZhTw => match (code, signal) {
            (Some(101), _) => "當機（panic）".into(),
            (Some(c), _) => format!("結束碼 {c}"),
            (None, Some(sig)) => signal_reason_lang(lang, sig),
            (None, None) => "已結束".into(),
        },
        _ => match (code, signal) {
            (Some(101), _) => "упала (паника)".into(),
            (Some(c), _) => format!("завершилась с кодом {c}"),
            (None, Some(sig)) => signal_reason(sig),
            (None, None) => "завершилась".into(),
        },
    }
}

fn signal_reason_lang(lang: crate::i18n::Language, sig: i32) -> String {
    match lang.resolve() {
        crate::i18n::Language::En => match sig {
            1 => "disconnected: session closed (SIGHUP)".into(),
            2 => "interrupted (SIGINT)".into(),
            4 => "crashed: illegal instruction (SIGILL)".into(),
            6 => "aborted (SIGABRT)".into(),
            7 => "crashed: bus error (SIGBUS)".into(),
            8 => "crashed: arithmetic error (SIGFPE)".into(),
            9 => "killed externally (SIGKILL) — most often out of memory".into(),
            11 => "crashed: memory access error (SIGSEGV)".into(),
            15 => "stopped by command (SIGTERM)".into(),
            s => format!("killed by signal {s}"),
        },
        crate::i18n::Language::Zh => match sig {
            1 => "已断开：会话已关闭（SIGHUP）".into(),
            2 => "已中断（SIGINT）".into(),
            4 => "崩溃：非法指令（SIGILL）".into(),
            6 => "异常终止（SIGABRT）".into(),
            7 => "崩溃：总线错误（SIGBUS）".into(),
            8 => "崩溃：算术错误（SIGFPE）".into(),
            9 => "被外部杀死（SIGKILL）——多为内存不足".into(),
            11 => "崩溃：内存访问错误（SIGSEGV）".into(),
            15 => "被命令停止（SIGTERM）".into(),
            s => format!("被信号 {s} 杀死"),
        },
        crate::i18n::Language::ZhTw => match sig {
            1 => "已中斷連線：工作階段已關閉（SIGHUP）".into(),
            2 => "已中斷（SIGINT）".into(),
            4 => "當機：非法指令（SIGILL）".into(),
            6 => "異常終止（SIGABRT）".into(),
            7 => "當機：匯流排錯誤（SIGBUS）".into(),
            8 => "當機：算術錯誤（SIGFPE）".into(),
            9 => "被外部終止（SIGKILL）——多為記憶體不足".into(),
            11 => "當機：記憶體存取錯誤（SIGSEGV）".into(),
            15 => "被命令停止（SIGTERM）".into(),
            s => format!("被訊號 {s} 終止"),
        },
        _ => signal_reason(sig),
    }
}

/// Is this exit a crash? SIGTERM, SIGINT, SIGHUP and exit code 0 are the
/// usual ways the service is stopped on purpose (deploying a new build
/// kills `daemon --child` with TERM, the session ends with HUP): no
/// `last-crash`, no notification, and they do not count towards the
/// guard's give-up. Everything else is a crash worth investigating.
pub fn is_crash(code: Option<i32>, signal: Option<i32>) -> bool {
    !matches!(code, Some(0)) && !matches!(signal, Some(1 | 2 | 15))
}

/// One line of `mimshel crashes`: time, component, reason, report path.
pub fn listing_line(at: &str, id: &str, reason: &str, path: &str) -> String {
    format!("{} {}: {} — {path}", pretty(at), component(id), reason)
}

/// `2026-10-26_14-03-22` -> `2026-10-26 14:03:22`.
pub fn pretty(at: &str) -> String {
    match at.split_once('_') {
        Some((date, time)) => format!("{date} {}", time.replace('-', ":")),
        None => at.to_string(),
    }
}

fn signal_reason(sig: i32) -> String {
    match sig {
        1 => "отключена: сеанс закрыт (SIGHUP)".into(),
        2 => "прервана (SIGINT)".into(),
        4 => "упала: недопустимая инструкция (SIGILL)".into(),
        6 => "завершилась аварийно (SIGABRT)".into(),
        7 => "упала: ошибка шины (SIGBUS)".into(),
        8 => "упала: ошибка арифметики (SIGFPE)".into(),
        9 => "убита извне (SIGKILL) — чаще всего системе не хватило памяти".into(),
        11 => "упала: ошибка доступа к памяти (SIGSEGV)".into(),
        15 => "остановлена командой (SIGTERM)".into(),
        s => format!("убита сигналом {s}"),
    }
}

/// Three numbers separated by `sep` (a date or a time), all non-negative.
fn triplet(s: &str, sep: char) -> Option<(i64, u32, u32)> {
    let (a, rest) = s.split_once(sep)?;
    let (b, c) = rest.split_once(sep)?;
    if c.contains(sep) {
        return None;
    }
    Some((a.parse().ok()?, b.parse().ok()?, c.parse().ok()?))
}

/// Days since the epoch -> (year, month, day), Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (year, month, day) -> days since the epoch, Howard Hinnant's algorithm.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64; // [0, 11]
    let doy = (153 * mp + 2) / 5 + u64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_round_trips_known_dates() {
        assert_eq!(stamp(0), "1970-01-01_00-00-00");
        assert_eq!(stamp_seconds("1970-01-01_00-00-00"), Some(0));
        // 2026-10-26 12:34:56 UTC.
        let secs = days_from_civil(2026, 10, 26) as u64 * 86_400 + 12 * 3600 + 34 * 60 + 56;
        assert_eq!(stamp(secs), "2026-10-26_12-34-56");
        assert_eq!(stamp_seconds(&stamp(secs)), Some(secs));
        assert_eq!(pretty(&stamp(secs)), "2026-10-26 12:34:56");
    }

    #[test]
    fn stamp_seconds_rejects_garbage() {
        assert_eq!(stamp_seconds("no-time-here-service-1.txt"), None);
        assert_eq!(stamp_seconds("2026-13-26_12-00-00-x-1.txt"), None);
        assert_eq!(stamp_seconds("2026-10-26_25-00-00-x-1.txt"), None);
        assert_eq!(
            stamp_seconds("2026-10-26_12-00-00-x-1.txt"),
            Some(days_from_civil(2026, 10, 26) as u64 * 86_400 + 12 * 3600)
        );
    }

    #[test]
    fn report_name_has_time_component_and_pid() {
        assert_eq!(
            report_name("2026-10-26_12-34-56", "service", 4242),
            "2026-10-26_12-34-56-service-4242.txt"
        );
    }

    #[test]
    fn report_text_has_every_field_and_no_env() {
        let text = Report {
            version: "0.0.2-alpha",
            id: "service",
            at: "2026-10-26_12-34-56",
            pid: 4242,
            thread: "main",
            message: "boom",
            location: "src/x.rs:9:3",
            backtrace: "stack backtrace:\n  0: main",
        }
        .text();
        for want in [
            "Mimshel 0.0.2-alpha",
            "компонент: служба (service)",
            "время (UTC): 2026-10-26_12-34-56",
            "процесс: 4242",
            "поток: main",
            "паника: «boom»",
            "место: src/x.rs:9:3",
            "stack backtrace:",
        ] {
            assert!(text.contains(want), "no {want} in the report:\n{text}");
        }
    }

    #[test]
    fn report_reason_parses_message_and_location() {
        let text = Report {
            version: "0.0.2-alpha",
            id: "service",
            at: "2026-10-26_12-34-56",
            pid: 1,
            thread: "main",
            message: "index out of bounds",
            location: "crates/dock/src/lib.rs:412:17",
            backtrace: "bt",
        }
        .text();
        assert_eq!(
            report_reason(&text).as_deref(),
            Some("«index out of bounds» в crates/dock/src/lib.rs:412")
        );
        assert_eq!(report_reason("пусто"), None);
    }

    #[test]
    fn exit_reason_covers_codes_and_every_listed_signal() {
        assert_eq!(exit_reason(Some(101), None), "упала (паника)");
        assert_eq!(exit_reason(Some(1), None), "завершилась с кодом 1");
        assert_eq!(exit_reason(None, None), "завершилась");
        let cases = [
            (1, "отключена: сеанс закрыт (SIGHUP)"),
            (2, "прервана (SIGINT)"),
            (4, "упала: недопустимая инструкция (SIGILL)"),
            (6, "завершилась аварийно (SIGABRT)"),
            (7, "упала: ошибка шины (SIGBUS)"),
            (8, "упала: ошибка арифметики (SIGFPE)"),
            (
                9,
                "убита извне (SIGKILL) — чаще всего системе не хватило памяти",
            ),
            (11, "упала: ошибка доступа к памяти (SIGSEGV)"),
            (15, "остановлена командой (SIGTERM)"),
        ];
        for (sig, want) in cases {
            assert_eq!(exit_reason(None, Some(sig)), want, "signal {sig}");
        }
        assert_eq!(exit_reason(None, Some(3)), "убита сигналом 3");
        assert_eq!(exit_reason(None, Some(99)), "убита сигналом 99");
    }

    #[test]
    fn stops_are_not_crashes() {
        // On purpose: deploy kills `daemon --child` with TERM, Ctrl+C is
        // INT, the session end is HUP, a clean exit is code 0.
        for (code, signal) in [
            (Some(0), None),
            (None, Some(1)),
            (None, Some(2)),
            (None, Some(15)),
        ] {
            assert!(!is_crash(code, signal), "({code:?}, {signal:?})");
        }
        // Everything else is a crash worth a report and a notification.
        for (code, signal) in [
            (Some(101), None),
            (Some(1), None),
            (None, Some(6)),
            (None, Some(9)),
            (None, Some(11)),
            (None, None),
        ] {
            assert!(is_crash(code, signal), "({code:?}, {signal:?})");
        }
    }

    #[test]
    fn listing_line_formats_time_component_reason_path() {
        let line = listing_line(
            "2026-10-26_12-34-56",
            "service",
            "упала (паника): «boom» в src/x.rs:9",
            "/tmp/crashes/2026-10-26_12-34-56-service-4242.txt",
        );
        assert_eq!(
            line,
            "2026-10-26 12:34:56 служба: упала (паника): «boom» в src/x.rs:9 — \
             /tmp/crashes/2026-10-26_12-34-56-service-4242.txt"
        );
    }

    #[test]
    fn components_have_russian_names() {
        assert_eq!(component("service"), "служба");
        assert_eq!(component("lock-screen"), "экран блокировки");
        assert_eq!(component("terminal"), "терминал");
        assert_eq!(component("folder"), "Проводник");
        assert_eq!(component("anything"), "команда mimshel");
    }

    #[test]
    fn exit_report_and_summary_of_both_kinds() {
        let text = ExitReport {
            version: "0.0.2-alpha",
            id: "service",
            at: "2026-10-26_17-28-43",
            pid: 13778,
            reason: "завершилась аварийно (SIGABRT)",
            log_tail: "Служба готова за 0.9 с\n",
        }
        .text();
        for want in [
            "компонент: служба (service)",
            "процесс: 13778",
            "причина: завершилась аварийно (SIGABRT)",
            "журнал перед сбоем:\nСлужба готова",
        ] {
            assert!(text.contains(want), "no {want} in:\n{text}");
        }
        assert_eq!(
            report_summary(&text).as_deref(),
            Some("завершилась аварийно (SIGABRT)")
        );
        let panic = Report {
            version: "0.0.2-alpha",
            id: "service",
            at: "2026-10-26_17-28-43",
            pid: 1,
            thread: "main",
            message: "boom",
            location: "src/ui.rs:10:5",
            backtrace: "",
        }
        .text();
        assert_eq!(
            report_summary(&panic).as_deref(),
            Some("упала (паника): «boom» в src/ui.rs:10")
        );
        assert_eq!(report_summary("мусор"), None);
    }
}
