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
                // Свой знак — планета (ставит `mimshel apply menubar`); нет его
                // в теме — zenity покажет обычный значок сведений.
                &format!("--icon={}", crate::parts::LOGO_NAME),
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

/// Вопрос «да/нет» окном zenity (`text` — разметка Pango). `true` — нажали `ok`.
pub fn confirm(title: &str, text: &str, ok: &str, icon: &str) -> bool {
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

/// Сведения «Об этом компьютере»: название системы, её сборка и строки
/// «что — какое» (у второй видеокарты подпись пустая).
#[derive(Debug, Clone, PartialEq)]
pub struct About {
    pub name: String,
    pub build: String,
    pub rows: Vec<(&'static str, String)>,
}

/// Собрать сведения (доли секунды: lspci, df, xrandr, glxinfo).
pub fn about() -> About {
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
    // Название видеокарты, которая рисует экран, — у OpenGL (у встроенной AMD
    // lspci знает только кодовое имя «Phoenix1»). nvidia-smi не спрашиваем:
    // он будит спящую видеокарту NVIDIA.
    let renderer = output_any(&["glxinfo", "-B"])
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("OpenGL renderer string:")
                .map(str::to_string)
        })
        .unwrap_or_default();
    let gpus: Vec<String> = output_any(&["lspci"])
        .lines()
        .filter(|l| {
            l.contains(" VGA ") || l.contains("3D controller") || l.contains("Display controller")
        })
        .map(|l| gpu_name(l, &renderer))
        .collect();
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
    let mut rows = vec![
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
    ];
    if gpus.is_empty() {
        rows.push(("Графика", "—".to_string()));
    }
    for (i, g) in gpus.into_iter().enumerate() {
        rows.push((if i == 0 { "Графика" } else { "" }, g));
    }
    rows.extend([
        ("Диск", disk),
        ("Оболочка", format!("Mimshel {}", env!("CARGO_PKG_VERSION"))),
        ("Экран", screen),
        (
            "Ядро",
            read(Path::new("/proc/sys/kernel/osrelease"))
                .trim()
                .to_string(),
        ),
    ]);
    About {
        name: os("NAME").unwrap_or_else(|| "Linux".into()),
        build: os("BUILD_ID")
            .filter(|v| !v.is_empty())
            .or_else(|| os("VERSION_ID"))
            .unwrap_or_default(),
        rows,
    }
}

/// Понятное название видеокарты из строки `lspci`. `renderer` — имя от
/// OpenGL (`AMD Radeon 780M Graphics (radeonsi, …)`): у AMD оно лучше
/// кодового имени из lspci.
fn gpu_name(line: &str, renderer: &str) -> String {
    let desc = line.split_once(": ").map_or(line, |(_, d)| d);
    let desc = desc.split(" (rev").next().unwrap_or(desc).trim();
    let bracket = desc
        .rsplit_once('[')
        .and_then(|(_, b)| b.split_once(']'))
        .map(|(b, _)| b.trim());
    if desc.starts_with("NVIDIA") {
        return match bracket {
            Some(b) => format!("NVIDIA {b}"),
            None => desc.to_string(),
        };
    }
    if desc.starts_with("Advanced Micro Devices") || desc.contains("[AMD/ATI]") {
        let gl = renderer.split(" (").next().unwrap_or("").trim();
        if gl.starts_with("AMD") {
            return gl.to_string();
        }
        let model = desc.rsplit("] ").next().unwrap_or(desc);
        return format!("AMD {model}");
    }
    if desc.starts_with("Intel") {
        return match bracket {
            Some(b) => format!("Intel {b}"),
            None => desc.to_string(),
        };
    }
    desc.to_string()
}

/// Текст «Об этом компьютере» (разметка Pango для zenity — когда службы нет).
pub fn about_text() -> String {
    let a = about();
    let mut text = format!(
        "<span size=\"x-large\" weight=\"bold\">{}</span>\n<span foreground=\"#888\">{}</span>\n\n",
        escape(&a.name),
        escape(&a.build)
    );
    let lines: Vec<String> = a
        .rows
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

#[cfg(test)]
mod about_tests {
    use super::*;

    #[test]
    fn gpu_names() {
        let nv = "01:00.0 VGA compatible controller: NVIDIA Corporation AD106M [GeForce RTX 4070 Max-Q / Mobile] (rev a1)";
        let amd = "06:00.0 VGA compatible controller: Advanced Micro Devices, Inc. [AMD/ATI] Phoenix1 (rev c1)";
        assert_eq!(gpu_name(nv, ""), "NVIDIA GeForce RTX 4070 Max-Q / Mobile");
        assert_eq!(
            gpu_name(
                amd,
                "AMD Radeon 780M Graphics (radeonsi, phoenix, ACO, DRM 3.64)"
            ),
            "AMD Radeon 780M Graphics"
        );
        assert_eq!(
            gpu_name(amd, "NVIDIA GeForce RTX 4070 Laptop GPU/PCIe/SSE2"),
            "AMD Phoenix1",
            "экран рисует NVIDIA — у AMD остаётся кодовое имя"
        );
        let intel = "00:02.0 VGA compatible controller: Intel Corporation Alder Lake-P GT2 [Iris Xe Graphics] (rev 0c)";
        assert_eq!(gpu_name(intel, ""), "Intel Iris Xe Graphics");
    }
}
