//! Пункты меню яблока (`mimshel action ДЕЙСТВИЕ`): «Об этом компьютере»,
//! настройки, принудительное завершение, сон, перезагрузка, выключение,
//! блокировка, выход. Подтверждения — окнами zenity, как было в macos-look.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::sys::{command, output, output_any, read, run, spawn, succeeds};

/// Выполнить действие. `shell` — путь к своей программе (`mimshel`) (для блокировки).
/// Код выхода: 0 — сделано или отменено, 2 — нет такого действия.
pub fn action(name: &str, shell: &Path) -> i32 {
    let shell = shell.to_string_lossy();
    match name {
        "about" => {
            zenity(&[
                "--info",
                "--title=Об этом компьютере",
                &format!("--text={}", about_text()),
                "--icon=computer",
                "--width=420",
            ]);
        }
        "settings" => spawn(&["xfce4-settings-manager"]),
        "forcequit" => spawn(&["xfce4-taskmanager"]),
        "sleep" => {
            if in_vm() {
                // В VirtualBox из сна s2idle не будят ни клавиатура, ни мышь —
                // только кнопка ACPI снаружи. Поэтому в ВМ «сон» — это
                // блокировка и тёмный экран.
                spawn(&[&shell, "lock"]);
                std::thread::sleep(Duration::from_millis(1500));
                run(&["xset", "dpms", "force", "off"]);
            } else {
                run(&["xfce4-session-logout", "--suspend"]);
            }
        }
        "lock" => spawn(&[&shell, "lock"]),
        "restart" => {
            if confirm(
                "Перезагрузить",
                "Вы действительно хотите перезагрузить компьютер?",
                "Перезагрузить",
                "system-reboot",
            ) {
                run(&["xfce4-session-logout", "--reboot", "--fast"]);
            }
        }
        "shutdown" => {
            if confirm(
                "Выключить",
                "Вы действительно хотите выключить компьютер?",
                "Выключить",
                "system-shutdown",
            ) {
                run(&["xfce4-session-logout", "--halt", "--fast"]);
            }
        }
        "logout" => {
            if confirm(
                "Завершить сеанс",
                "Закрыть все программы и выйти из системы?",
                "Выйти",
                "system-log-out",
            ) {
                run(&["xfce4-session-logout", "--logout", "--fast"]);
            }
        }
        _ => {
            eprintln!("Неизвестное действие «{name}»");
            return 2;
        }
    }
    0
}

/// Запущены ли мы в виртуальной машине.
pub fn in_vm() -> bool {
    succeeds(&["systemd-detect-virt", "-q"])
}

/// Окно zenity; `true` — нажали «да». Другая версия zenity может не знать
/// `--icon` — тогда повторяем без иконки. Только по тексту ошибки: окно,
/// закрытое принудительно, заново не открываем.
fn zenity(args: &[&str]) -> bool {
    let Ok(out) = command("zenity")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    else {
        return false;
    };
    let code = out.status.code();
    if !matches!(code, Some(0 | 1)) && String::from_utf8_lossy(&out.stderr).contains("--icon") {
        let args: Vec<&str> = args
            .iter()
            .copied()
            .filter(|a| !a.starts_with("--icon"))
            .collect();
        return command("zenity")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    }
    out.status.success()
}

fn confirm(title: &str, text: &str, ok: &str, icon: &str) -> bool {
    zenity(&[
        "--question",
        &format!("--title={title}"),
        &format!("--text={text}"),
        &format!("--ok-label={ok}"),
        "--cancel-label=Отменить",
        &format!("--icon={icon}"),
        "--width=360",
    ])
}

/// Текст «Об этом компьютере» (разметка Pango для zenity).
pub fn about_text() -> String {
    let os_release = read(Path::new("/etc/os-release"));
    let os = |key: &str| {
        os_release.lines().find_map(|l| {
            let v = l.strip_prefix(key)?.strip_prefix('=')?;
            Some(v.trim_matches('"').to_string())
        })
    };
    let cpuinfo = read(Path::new("/proc/cpuinfo"));
    let cpu = cpuinfo
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map_or("—".to_string(), |(_, v)| v.trim().to_string());
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mem_kb: f64 = read(Path::new("/proc/meminfo"))
        .lines()
        .find(|l| l.starts_with("MemTotal"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(0.0);
    let gpu = output_any(&["lspci"])
        .lines()
        .find(|l| l.contains(" VGA ") || l.contains("3D controller"))
        .map_or("—".to_string(), |l| {
            l.split_once(": ").map_or(l, |(_, v)| v).to_string()
        });
    let disk = output(&["df", "-B1", "--output=used,size", "/"])
        .and_then(|o| {
            let mut n = o.lines().nth(1)?.split_whitespace();
            let used: u64 = n.next()?.parse().ok()?;
            let size: u64 = n.next()?.parse().ok()?;
            Some(format!("занято {} из {} ГБ", used >> 30, size >> 30))
        })
        .unwrap_or_else(|| "—".into());
    let screen = output_any(&["xrandr", "--current"])
        .lines()
        .find_map(|l| {
            let v = l.split_once("current")?.1.split(',').next()?;
            Some(v.trim().replace(" x ", "×"))
        })
        .unwrap_or_else(|| "—".into());
    let rows = [
        (
            "Компьютер",
            read(Path::new("/proc/sys/kernel/hostname"))
                .trim()
                .to_string(),
        ),
        ("Процессор", format!("{cpu} ({threads} потоков)")),
        (
            "Память",
            format!("{} ГБ", (mem_kb / 1024.0 / 1024.0).round_ties_even()),
        ),
        ("Графика", gpu),
        ("Диск", disk),
        ("Экран", screen),
        (
            "Ядро",
            read(Path::new("/proc/sys/kernel/osrelease"))
                .trim()
                .to_string(),
        ),
    ];
    let name = os("NAME").unwrap_or_else(|| "Linux".into());
    let build = os("BUILD_ID")
        .filter(|v| !v.is_empty())
        .or_else(|| os("VERSION_ID"))
        .unwrap_or_default();
    let mut text = format!(
        "<span size=\"x-large\" weight=\"bold\">{}</span>\n<span foreground=\"#888\">{}</span>\n\n",
        escape(&name),
        escape(&build)
    );
    let lines: Vec<String> = rows
        .iter()
        .map(|(k, v)| format!("<b>{}</b>   {}", escape(k), escape(v)))
        .collect();
    text.push_str(&lines.join("\n"));
    text
}

/// Знаки, особые для разметки, — как `html.escape` в Python.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
