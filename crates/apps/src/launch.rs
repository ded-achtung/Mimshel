//! Запуск программы отдельно от оболочки: своя группа процессов (Ctrl+C в
//! терминале оболочки её не тронет), ввод-вывод в никуда, рабочая папка из
//! ярлыка, при `Terminal=true` — внутри терминала.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use crate::App;

impl App {
    /// Запускает программу. `terminal` — команда терминала, после которой
    /// идёт запускаемая программа (например, `xfce4-terminal -x`).
    pub fn launch(&self, terminal: &[String]) -> io::Result<()> {
        let argv = self.argv(terminal);
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "пустая команда"))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            // По этой переменной окна программы потом узнаёт док (как в GLib).
            .env("GIO_LAUNCHED_DESKTOP_FILE", &self.path);
        if let Some(dir) = self.working_dir.as_ref().filter(|d| d.is_dir()) {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn()?;
        // Дожидаемся завершения в отдельном потоке, чтобы не копились «зомби».
        std::thread::spawn(move || child.wait());
        Ok(())
    }

    /// Полная команда запуска с учётом терминала.
    pub fn argv(&self, terminal: &[String]) -> Vec<String> {
        if self.terminal {
            terminal.iter().chain(&self.command).cloned().collect()
        } else {
            self.command.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn app(command: &[&str], terminal: bool) -> App {
        App {
            id: "t.desktop".into(),
            path: PathBuf::from("/tmp/t.desktop"),
            name: "T".into(),
            name_untranslated: "T".into(),
            generic_name: None,
            comment: None,
            keywords: vec![],
            categories: vec![],
            icon: None,
            command: command.iter().map(|s| s.to_string()).collect(),
            working_dir: None,
            terminal,
            startup_wm_class: None,
        }
    }

    #[test]
    fn terminal_prefix() {
        let term = vec!["xfce4-terminal".to_string(), "-x".to_string()];
        assert_eq!(
            app(&["vim"], true).argv(&term),
            ["xfce4-terminal", "-x", "vim"]
        );
        assert_eq!(app(&["mousepad"], false).argv(&term), ["mousepad"]);
    }

    #[test]
    fn launches_detached() {
        let dir = std::env::temp_dir().join(format!("shell-launch-{}", std::process::id()));
        let flag = dir.join("ran");
        std::fs::create_dir_all(&dir).unwrap();
        let script = format!("echo \"$GIO_LAUNCHED_DESKTOP_FILE\" > {}", flag.display());
        app(&["sh", "-c", &script], false).launch(&[]).unwrap();
        for _ in 0..100 {
            if std::fs::read_to_string(&flag).is_ok_and(|s| !s.is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&flag).unwrap().trim(),
            "/tmp/t.desktop"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
