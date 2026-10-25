//! Резервные копии настроек и откат (`mimshel backup`, `mimshel backups`,
//! `mimshel restore`). Копии — в `~/.local/share/mimshel/backups`; прежняя папка
//! macos-look переезжает туда целиком, с самой первой копией «как было до
//! всего».
//!
//! В копии: настройки Xfce (xfconf), настройки и значки Plank, свой файл
//! настроек Mimshel и отметки, были ли автозапуск Plank и тема GTK4. Откат
//! возвращает их, убирает только свои ссылки GTK4, ярлыки и автозапуски со своей пометкой и
//! перезапускает части Xfce.

use std::path::{Path, PathBuf};

use crate::paths::{Paths, SHORTCUTS};
use crate::sys::{
    home, is_managed, kill, kill_daemon, output_any, read, run, running, spawn, xget, xreset,
};

/// Папка копий. Прежняя (от macos-look) при первом обращении переезжает сюда.
pub fn backup_dir() -> PathBuf {
    let dir = Paths::new().data.join("backups");
    let old = home().join(".local/share/macos-look/backups");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::create_dir_all(Paths::new().data);
        if let Err(e) = std::fs::rename(&old, &dir) {
            eprintln!("Не перенести копии из {}: {e}", old.display());
            return old;
        }
    }
    dir
}

fn xfconf_dir() -> PathBuf {
    home().join(".config/xfce4/xfconf/xfce-perchannel-xml")
}

/// Копии по порядку (самая первая — «как было»).
pub fn list() -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = std::fs::read_dir(backup_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    all.sort();
    all
}

/// Сохранить копию; `label` — пометка в имени папки.
pub fn make(label: &str) -> std::io::Result<PathBuf> {
    let stamp = output_any(&["date", "+%Y%m%d-%H%M%S"]);
    let dest = backup_dir().join(format!("{}-{label}", stamp.trim()));
    std::fs::create_dir_all(&dest)?;
    copy_tree(&xfconf_dir(), &dest.join("xfconf"))?;
    let plank = output_any(&["dconf", "dump", "/net/launchpad/plank/"]);
    std::fs::write(dest.join("plank.dconf"), plank)?;
    copy_tree(&home().join(".config/plank"), &dest.join("plank-config"))?;
    let shell_config = mimshel_core::names::dir_in(&home().join(".config")).join("config.toml");
    if shell_config.exists() {
        std::fs::copy(&shell_config, dest.join("shell-config.toml"))?;
    }
    std::fs::create_dir_all(dest.join("had"))?;
    for f in [
        Paths::new().autostart.join("plank.desktop"),
        home().join(".config/gtk-4.0"),
    ] {
        let had = f.exists() || f.is_symlink();
        let name = f.file_name().unwrap_or_default();
        std::fs::write(dest.join("had").join(name), if had { "yes" } else { "no" })?;
    }
    Ok(dest)
}

/// Копия по началу имени (`None` — самая первая).
pub fn find(prefix: Option<&str>) -> Option<PathBuf> {
    let all = list();
    match prefix {
        None => all.into_iter().next(),
        Some(p) => all.into_iter().find(|b| {
            b.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(p))
        }),
    }
}

/// Вернуть настройки из копии `src`. `dry` — только рассказать, что будет
/// сделано. Перед откатом сохраняется копия «before-restore».
pub fn restore(src: &Path, dry: bool) -> std::io::Result<()> {
    let step = |what: &str| println!("{}{what}", if dry { "  будет: " } else { "▶ " });
    let name = src.file_name().unwrap_or_default().to_string_lossy();
    step(&format!(
        "копия текущих настроек «before-restore», затем откат к {name}"
    ));
    if !dry {
        make("before-restore")?;
    }
    step("остановить панель Xfce, Plank и xfconfd");
    if !dry {
        run(&["xfce4-panel", "--quit"]);
        kill("plank");
        kill("xfconfd");
    }
    if src.join("xfconf").is_dir() {
        step("вернуть настройки Xfce (xfconf)");
        if !dry {
            replace_tree(&src.join("xfconf"), &xfconf_dir())?;
        }
    }
    step("вернуть настройки Plank (dconf и значки)");
    if !dry {
        run(&["dconf", "reset", "-f", "/net/launchpad/plank/"]);
        let dump = read(&src.join("plank.dconf"));
        if !dump.trim().is_empty() {
            load_dconf(&dump);
        }
        let plank = home().join(".config/plank");
        if src.join("plank-config").is_dir() {
            replace_tree(&src.join("plank-config"), &plank)?;
        } else {
            remove_tree(&plank)?;
        }
    }
    let had = |name: &str| read(&src.join("had").join(name)).trim() != "no";
    let paths = Paths::new();
    if !had("plank.desktop") {
        step("убрать автозапуск Plank (его не было)");
        if !dry {
            remove_file(&paths.autostart.join("plank.desktop"))?;
        }
    }
    // Не удаляем ~/.config/gtk-4.0: пользователь мог создать в ней файлы
    // после резервной копии. Убираем только подтверждённые ссылки Shell.
    for link in crate::gtk_links::owned_links() {
        step(&format!("убрать свою ссылку GTK4 {}", link.display()));
    }
    if !dry {
        crate::gtk_links::cleanup()?;
    }
    if crate::gtk_fixes::present() {
        step("убрать поправки Mimshel из ~/.config/gtk-3.0/gtk.css");
        if !dry {
            crate::gtk_fixes::cleanup()?;
        }
    }
    for f in managed(&[&paths.applications, &paths.autostart]) {
        step(&format!("удалить созданный ярлык {}", f.display()));
        if !dry {
            remove_file(&f)?;
        }
    }
    if let Ok(text) = std::fs::read_to_string(src.join("shell-config.toml")) {
        step("вернуть файл настроек Mimshel");
        if !dry {
            let path = mimshel_core::names::dir_in(&home().join(".config")).join("config.toml");
            crate::sys::write_if_changed(&path, &text)?;
        }
    }
    step("остановить xss-lock и службу Mimshel, вернуть xfce4-notifyd и заставку Xfce");
    if !dry {
        kill("xss-lock");
        kill_daemon();
        run(&["systemctl", "--user", "unmask", "xfce4-notifyd.service"]);
        let service = crate::lock::screensaver_service();
        if crate::lock::is_screensaver_override(&read(&service)) {
            remove_file(&service)?;
            run(&[
                "dbus-send",
                "--session",
                "--type=method_call",
                "--dest=org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus.ReloadConfig",
            ]);
        }
        spawn(&["xfce4-screensaver"]);
    }
    // Свои сочетания: Mimshel, прежний macos-look или rofi, поставленный нами.
    let ours = [
        paths.shell_bin.display().to_string(),
        home().join(".local/bin/macos-look").display().to_string(),
        "rofi -show drun".to_string(),
    ];
    for key in SHORTCUTS.iter().map(|s| s.keys) {
        let prop = format!("/commands/custom/{key}");
        if xget("xfce4-keyboard-shortcuts", &prop)
            .is_some_and(|c| ours.iter().any(|o| c.starts_with(o.as_str())))
        {
            step(&format!("сбросить сочетание {key}"));
            if !dry {
                xreset("xfce4-keyboard-shortcuts", &prop);
            }
        }
    }
    let lock_prop = format!("/commands/custom/{}", crate::lock::LOCK_SHORTCUT);
    if xget("xfce4-keyboard-shortcuts", &lock_prop).as_deref()
        == Some(crate::lock::LOCK_SHORTCUT_COMMAND)
    {
        step(&format!(
            "сбросить сочетание {}",
            crate::lock::LOCK_SHORTCUT
        ));
        if !dry {
            xreset("xfce4-keyboard-shortcuts", &lock_prop);
        }
    }
    for what in crate::terminal::owned() {
        step(&what);
    }
    if !dry {
        crate::terminal::cleanup()?;
    }
    step("перезапустить настройки, окна, рабочий стол и панель Xfce");
    if !dry {
        spawn(&["xfsettingsd", "--replace"]);
        spawn(&["xfwm4", "--replace"]);
        spawn(&["xfdesktop", "--reload"]);
        spawn(&["xfce4-panel"]);
        if paths.autostart.join("plank.desktop").exists() && !running("plank") {
            spawn(&["plank"]);
        }
    }
    Ok(())
}

/// Свои ярлыки (с пометкой Mimshel или прежней macos-look) в этих папках.
fn managed(dirs: &[&Path]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "desktop"))
        .filter(|p| is_managed(&read(p)))
        .collect();
    out.sort();
    out
}

fn load_dconf(dump: &str) {
    use std::io::Write;
    if let Ok(mut child) = crate::sys::command("dconf")
        .args(["load", "/net/launchpad/plank/"])
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(dump.as_bytes());
        }
        let _ = child.wait();
    }
}

/// Скопировать папку целиком (ссылки — ссылками). Нет папки — нечего копировать.
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
        } else if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Replace folder `dst` with a copy of `src` (no `src` — just remove `dst`).
///
/// All or nothing: the copy is built next to `dst` first, then swapped in by
/// renames on the same file system. If copying fails, `dst` is untouched; if
/// the swap fails, the old `dst` is put back.
fn replace_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    if !src.is_dir() {
        return remove_tree(dst);
    }
    let sibling = |tag: &str| {
        let name = dst.file_name().unwrap_or_default().to_string_lossy();
        dst.with_file_name(format!(".{name}.mimshel-{tag}"))
    };
    let (new, old) = (sibling("new"), sibling("old"));
    // Leftovers of an interrupted earlier run.
    remove_tree(&new)?;
    remove_tree(&old)?;
    if let Err(e) = copy_tree(src, &new) {
        let _ = remove_tree(&new);
        return Err(e);
    }
    let had_dst = dst.symlink_metadata().is_ok();
    if had_dst {
        if let Err(e) = std::fs::rename(dst, &old) {
            let _ = remove_tree(&new);
            return Err(e);
        }
    }
    if let Err(e) = std::fs::rename(&new, dst) {
        if had_dst {
            let _ = std::fs::rename(&old, dst);
        }
        let _ = remove_tree(&new);
        return Err(e);
    }
    // The new settings are in place; a leftover old copy is harmless.
    let _ = remove_tree(&old);
    Ok(())
}

fn remove_tree(path: &Path) -> std::io::Result<()> {
    if path.is_symlink() || path.is_file() {
        return remove_file(path);
    }
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

fn remove_file(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn copy_replace_and_managed() {
        let t = Temp(std::env::temp_dir().join(format!("shell-backup-{}", std::process::id())));
        let src = t.0.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.xml"), "A").unwrap();
        std::fs::write(src.join("sub/b.xml"), "B").unwrap();
        std::os::unix::fs::symlink("a.xml", src.join("link")).unwrap();
        let dst = t.0.join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("old.xml"), "лишний").unwrap();
        replace_tree(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("sub/b.xml")).unwrap(), "B");
        assert!(!dst.join("old.xml").exists(), "лишнее убрано");
        assert_eq!(
            std::fs::read_link(dst.join("link")).unwrap(),
            Path::new("a.xml")
        );
        // Нет исходной папки — копировать нечего, ошибки нет.
        copy_tree(&t.0.join("нет"), &t.0.join("x")).unwrap();
        assert!(!t.0.join("x").exists());

        let apps = t.0.join("apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("ours.desktop"),
            "[Desktop Entry]\nX-Macos-Look=true\n",
        )
        .unwrap();
        std::fs::write(apps.join("user.desktop"), "[Desktop Entry]\n").unwrap();
        std::fs::write(apps.join("note.txt"), "X-Macos-Look=true").unwrap();
        assert_eq!(managed(&[&apps]), [apps.join("ours.desktop")]);
        remove_tree(&t.0.join("нет")).unwrap();
        remove_file(&t.0.join("нет")).unwrap();
    }

    #[test]
    fn replace_failure_keeps_old_folder() {
        let t = Temp(std::env::temp_dir().join(format!("shell-replace-{}", std::process::id())));
        let src = t.0.join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.xml"), "A").unwrap();
        // A socket cannot be copied: the copy fails halfway through.
        let _sock = std::os::unix::net::UnixListener::bind(src.join("sub/sock")).unwrap();
        let dst = t.0.join("dst");
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(dst.join("old.xml"), "OLD").unwrap();

        assert!(replace_tree(&src, &dst).is_err());
        assert_eq!(std::fs::read_to_string(dst.join("old.xml")).unwrap(), "OLD");
        assert!(!dst.join("a.xml").exists(), "nothing half-copied into dst");
        let mut left: Vec<_> = std::fs::read_dir(&t.0)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["dst", "src"], "no staging folders left behind");
    }

    #[test]
    fn replace_into_missing_and_from_missing() {
        let t = Temp(std::env::temp_dir().join(format!("shell-replace2-{}", std::process::id())));
        let src = t.0.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.xml"), "A").unwrap();
        // No dst yet — it appears.
        let dst = t.0.join("dst");
        replace_tree(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("a.xml")).unwrap(), "A");
        // Stale staging folders from an interrupted run do not get in the way.
        std::fs::create_dir_all(t.0.join(".dst.mimshel-new/junk")).unwrap();
        std::fs::create_dir_all(t.0.join(".dst.mimshel-old")).unwrap();
        replace_tree(&src, &dst).unwrap();
        assert!(!t.0.join(".dst.mimshel-new").exists());
        assert!(!t.0.join(".dst.mimshel-old").exists());
        // No src — dst is removed, as before.
        replace_tree(&t.0.join("нет"), &dst).unwrap();
        assert!(!dst.exists());
    }
}
