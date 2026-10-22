//! Сведения о процессах из `/proc`: из какого ярлыка запущен и кто родитель.
//! Нужны доку, чтобы приписать окно программе.

use std::path::PathBuf;

/// Ярлык, из которого запущен процесс, — по переменным, как в GLib:
/// `GIO_LAUNCHED_DESKTOP_FILE` учитывается, только если
/// `GIO_LAUNCHED_DESKTOP_FILE_PID` равен номеру самого процесса. Иначе это
/// чужая переменная, унаследованная от родителя (программа из Терминала).
pub fn launched_from(pid: u32) -> Option<PathBuf> {
    let env = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    let mut file = None;
    let mut owner = None;
    for var in env.split(|&b| b == 0) {
        let var = String::from_utf8_lossy(var);
        if let Some(v) = var.strip_prefix("GIO_LAUNCHED_DESKTOP_FILE=") {
            file = Some(PathBuf::from(v));
        } else if let Some(v) = var.strip_prefix("GIO_LAUNCHED_DESKTOP_FILE_PID=") {
            owner = v.parse::<u32>().ok();
        }
    }
    file.filter(|_| owner == Some(pid))
}

/// Номер родительского процесса.
pub fn parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Имя процесса в скобках может содержать пробелы — считаем после «)».
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process() {
        let me = std::process::id();
        let p = parent(me).unwrap();
        assert!(p > 0 && p != me);
        assert_eq!(parent(u32::MAX), None);
    }

    #[test]
    fn launched_desktop_file_needs_matching_pid() {
        use std::process::{Command, Stdio};
        let run = |pid_value: &str| {
            let mut child = Command::new("sleep")
                .arg("5")
                .env("GIO_LAUNCHED_DESKTOP_FILE", "/x/app.desktop")
                .env("GIO_LAUNCHED_DESKTOP_FILE_PID", pid_value)
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let id = child.id();
            let r = launched_from(id);
            let _ = child.kill();
            let _ = child.wait();
            r
        };
        // Чужой номер — переменная унаследована, верить нельзя.
        assert_eq!(run("1"), None);
    }
}
