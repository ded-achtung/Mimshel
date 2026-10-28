//! Private, explicitly scoped diagnostic archives.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, BufRead, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mimshel_core::config::Config;

/// Only these sources may enter a report; crashes contains regular text reports only.
pub const FILES: &[&str] = &[
    "README.txt",
    "system.txt",
    "doctor.txt",
    "config.toml",
    "daemon.log",
    "lock.log",
    "crashes/",
];

pub struct Identity {
    pub home: String,
    pub user: String,
    pub host: String,
    /// The real name from the account (GECOS), e.g. "Ivan Petrov".
    pub real_name: String,
}

fn word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn replace_word(text: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut start = 0;
    for (at, _) in text.match_indices(needle) {
        let end = at + needle.len();
        if !text[..at].chars().next_back().is_some_and(word)
            && !text[end..].chars().next().is_some_and(word)
        {
            out.push_str(&text[start..at]);
            out.push_str(replacement);
            start = end;
        }
    }
    out.push_str(&text[start..]);
    out
}

/// Redact every payload, preserving unrelated text and line endings.
pub fn redact(text: &str, id: &Identity) -> String {
    let mut out = text.to_owned();
    for home in [&id.home, &format!("/home/{}", id.user)] {
        if !home.is_empty() && home != "/" {
            out = replace_word(&out, home, "~");
        }
    }
    out = replace_word(&out, &id.host, "<host>");
    out = replace_word(&out, &id.user, "<user>");
    // Short or empty names would hit ordinary words.
    if id.real_name.chars().count() >= 3 {
        out = replace_word(&out, &id.real_name, "<name>");
    }
    out.split_inclusive('\n')
        .map(|line| {
            if let Some(at) = line
                .as_bytes()
                .windows(4)
                .enumerate()
                .find(|(at, w)| {
                    w.eq_ignore_ascii_case(b"ssid")
                        && !line[..*at].chars().next_back().is_some_and(word)
                        && !line[*at + 4..].chars().next().is_some_and(word)
                })
                .map(|(at, _)| at)
            {
                // Keep the diagnostic prefix; a network name may contain spaces or quotes.
                format!(
                    "{}SSID: <wifi>{}",
                    &line[..at],
                    if line.ends_with('\n') { "\n" } else { "" }
                )
            } else {
                line.to_owned()
            }
        })
        .collect()
}

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp(dir: &Path) -> io::Result<Temp> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let path = dir.join(format!(
            ".mimshel-report-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => return Ok(Temp(path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn write(path: &Path, text: &str, id: &Identity) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !FILES.contains(&name)
        && path
            .parent()
            .and_then(Path::file_name)
            .is_none_or(|s| s != "crashes")
    {
        return Err(io::Error::other("файл не входит в список отчёта"));
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(redact(text, id).as_bytes())
}

fn optional(args: &[&str]) -> String {
    let mut command = crate::sys::command(args[0]);
    command
        .args(&args[1..])
        .env("LC_ALL", "C")
        .stderr(Stdio::null());
    command
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_else(|| format!("{}: missing\n", args[0]))
}

fn system(cfg: &Config) -> String {
    let mut text = format!("Mimshel: {}\n", env!("CARGO_PKG_VERSION"));
    let os = fs::read_to_string("/etc/os-release").unwrap_or_default();
    for line in os.lines().filter(|l| {
        l.split_once('=')
            .is_some_and(|(k, _)| ["NAME", "VERSION_ID", "ID"].contains(&k))
    }) {
        text.push_str(&format!("{line}\n"));
    }
    text.push_str(&format!("Kernel: {}\n", optional(&["uname", "-r"]).trim()));
    let session = crate::sys::session_env();
    for key in ["XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP"] {
        let value = session
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
            .unwrap_or_else(|| "missing".into());
        text.push_str(&format!("{key}: {value}\n"));
    }
    text.push_str(
        optional(&["xfce4-session", "--version"])
            .lines()
            .next()
            .unwrap_or("xfce4-session: missing"),
    );
    text.push('\n');
    text.push_str(&optional(&["xrandr", "--listmonitors"]));
    let xrdb = optional(&["xrdb", "-query"]);
    let dpi = xrdb.lines().find(|l| l.starts_with("Xft.dpi:"));
    text.push_str(dpi.unwrap_or("xrdb: missing (Xft.dpi)"));
    text.push('\n');
    let gpu = optional(&["glxinfo", "-B"]);
    let lines: Vec<_> = gpu
        .lines()
        .filter(|l| {
            l.contains("OpenGL renderer")
                || l.contains("OpenGL version")
                || *l == "glxinfo: missing"
        })
        .collect();
    text.push_str(&lines.join("\n"));
    text.push('\n');
    let scale = mimshel_core::names::env("SCALE")
        .and_then(|v| v.into_string().ok())
        .and_then(|v| v.replace(',', ".").parse::<f32>().ok())
        .or(cfg.scale)
        .unwrap_or_else(|| {
            dpi.and_then(|v| v.split_once(':')?.1.trim().parse::<f32>().ok())
                .map_or(1.0, |d| ((d / 96.0) * 4.0).round().clamp(4.0, 12.0) / 4.0)
        });
    let scale = if scale.is_finite() {
        scale.clamp(1.0, 4.0)
    } else {
        1.0
    };
    text.push_str(&format!("[look] scale (effective): {scale}\n"));
    text
}

fn tail(path: &Path) -> io::Result<String> {
    let mut lines = std::collections::VecDeque::new();
    for line in io::BufReader::new(fs::File::open(path)?).split(b'\n') {
        let line = line?;
        if lines.len() == 500 {
            lines.pop_front();
        }
        lines.push_back(String::from_utf8_lossy(&line).into_owned());
    }
    Ok(lines.into_iter().collect::<Vec<_>>().join("\n"))
}

fn safe_crash_name(name: &OsStr) -> Option<&str> {
    name.to_str().filter(|name| {
        !name.starts_with('.')
            && name.ends_with(".txt")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    })
}

fn read_crash(path: &Path, name: &str, skipped: &mut Vec<String>) -> Option<String> {
    match fs::read(path) {
        Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Err(_) => {
            skipped.push(format!("{name}: unreadable"));
            None
        }
    }
}

fn collect_crashes(
    source: &Path,
    stage: &Path,
    add: &mut impl FnMut(&str, &str) -> io::Result<()>,
    skipped: &mut Vec<String>,
) -> io::Result<()> {
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            skipped.push("crashes/: missing".into());
            return Ok(());
        }
        Err(_) => {
            skipped.push("crashes/: unreadable".into());
            return Ok(());
        }
    };
    DirBuilder::new()
        .mode(0o700)
        .create(stage.join("crashes"))?;
    let mut entries = entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(_) => {
                skipped.push("crashes/: unreadable entry".into());
                None
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    // Reserve original names before assigning fallbacks, so neither can overwrite the other.
    let mut used: HashSet<String> = entries
        .iter()
        .filter_map(|entry| safe_crash_name(&entry.file_name()).map(str::to_owned))
        .collect();
    let mut next = 1;
    for entry in entries {
        if !entry.path().extension().is_some_and(|ext| ext == "txt") {
            continue;
        }
        let name = safe_crash_name(&entry.file_name())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                loop {
                    let name = format!("crash-{next}.txt");
                    next += 1;
                    if used.insert(name.clone()) {
                        break name;
                    }
                }
            });
        let name = format!("crashes/{name}");
        match entry.file_type() {
            Ok(kind) if kind.is_file() => {
                if let Some(text) = read_crash(&entry.path(), &name, skipped) {
                    add(&name, &text)?;
                }
            }
            Ok(_) => {}
            Err(_) => skipped.push(format!("{name}: unreadable")),
        }
    }
    Ok(())
}

pub fn archive_name(seconds: u64) -> String {
    format!(
        "mimshel-report-{}.tar.gz",
        mimshel_core::crash::stamp(seconds)
    )
}

pub struct Report {
    pub path: PathBuf,
    pub size: u64,
    pub files: Vec<String>,
}

/// The account's real name: the first GECOS field of `getent passwd`.
fn real_name(user: &str) -> String {
    crate::sys::output(&["getent", "passwd", user])
        .and_then(|line| line.trim().split(':').nth(4).map(str::to_owned))
        .and_then(|gecos| gecos.split(',').next().map(|n| n.trim().to_owned()))
        .unwrap_or_default()
}

/// Collect only the allowlisted sources, without applying repairs.
pub fn make(dir: &Path, cfg: &Config, config: Option<&Path>) -> io::Result<Report> {
    let home = crate::sys::home();
    let id = Identity {
        home: home.to_string_lossy().into_owned(),
        // From the system (`id -un`), not $USER: that may be empty or
        // someone else's (su, env), and then the name would stay in.
        user: crate::lock::user_name(),
        real_name: real_name(&crate::lock::user_name()),
        host: fs::read_to_string("/proc/sys/kernel/hostname")
            .unwrap_or_default()
            .trim()
            .into(),
    };
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"))
        .join("mimshel");
    let stage = temp(dir)?;
    let mut files = vec!["README.txt".to_owned()];
    let mut skipped = Vec::new();
    let mut add = |name: &str, text: &str| -> io::Result<()> {
        write(&stage.0.join(name), text, &id)?;
        files.push(name.to_owned());
        Ok(())
    };
    add("system.txt", &system(cfg))?;
    let checks = crate::doctor::checks(cfg, config);
    let doctor: String = checks
        .iter()
        .map(|c| {
            format!(
                "{} [{}] {}{}\n",
                if c.ok { "✔" } else { "✘" },
                c.part,
                c.what,
                if !c.ok && !c.hint.is_empty() {
                    format!("  → {}", c.hint)
                } else {
                    String::new()
                }
            )
        })
        .collect();
    add("doctor.txt", &doctor)?;
    match config.map(fs::read_to_string) {
        Some(Ok(text)) => add("config.toml", &text)?,
        Some(Err(e)) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => skipped.push("config.toml: missing".to_owned()),
    }
    for name in ["daemon.log", "lock.log"] {
        match tail(&state.join(name)) {
            Ok(text) => add(name, &text)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                skipped.push(format!("{name}: missing"))
            }
            Err(e) => return Err(e),
        }
    }
    collect_crashes(&state.join("crashes"), &stage.0, &mut add, &mut skipped)?;
    let readme = format!(
        "Mimshel diagnostic report\n\nIncluded:\n{}\n\nREADME.txt — report description.\nsystem.txt — version, OS, desktop session, monitors, GPU and effective scale.\ndoctor.txt — full checklist without repairs; uses the UI language (currently Russian).\nconfig.toml — Mimshel configuration.\ndaemon.log and lock.log — daemon and lock-screen logs (last 500 lines).\ncrashes/ — crash reports; safe original names are preserved, others use crash-N.txt.\n\nSkipped:\n{}\n\nRedacted: home paths, user name, real name, host name and Wi-Fi network names.\nThe full environment, clipboard, notification texts, .ssh, keyrings and browser data are not collected.\nOther personal data may remain in arbitrary configuration and log text.\nInspect the contents before sending this archive to anyone!\n\nList: tar tzf ARCHIVE.tar.gz\nExtract into a separate directory: mkdir review; tar xzf ARCHIVE.tar.gz -C review\n",
        files.join("\n"),
        if skipped.is_empty() {
            "none".into()
        } else {
            skipped.join("\n")
        }
    );
    write(&stage.0.join("README.txt"), &readme, &id)?;
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_secs();
    let path = dir.join(archive_name(seconds));
    let archive = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    let result = crate::sys::command("tar")
        .args(["czf", "-", "--owner=0", "--group=0", "-C"])
        .arg(&stage.0)
        .arg(".")
        .stdout(archive)
        .stderr(Stdio::null())
        .status();
    if !result.is_ok_and(|s| s.success()) {
        let _ = fs::remove_file(&path);
        return Err(io::Error::other(
            "не удалось создать архив: tar недоступен или завершился с ошибкой",
        ));
    }
    Ok(Report {
        size: fs::metadata(&path)?.len(),
        path,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            home: "/srv/private/ded".into(),
            user: "ded".into(),
            host: "workstation".into(),
            real_name: "Ded Achtung".into(),
        }
    }

    #[test]
    fn preserves_safe_crash_names() {
        let name = "2026-10-26_14-44-58-service-16965.txt";
        assert_eq!(safe_crash_name(OsStr::new(name)), Some(name));
        assert_eq!(
            safe_crash_name(OsStr::new("Crash_A.1-2.txt")),
            Some("Crash_A.1-2.txt")
        );
    }

    #[test]
    fn rejects_unsafe_crash_names() {
        for name in [
            ".hidden.txt",
            "../escape.txt",
            "dir/file.txt",
            "dir\\file.txt",
            "personal name.txt",
            "сбой.txt",
            "file.log",
            "",
        ] {
            assert_eq!(safe_crash_name(OsStr::new(name)), None);
        }
    }

    #[test]
    fn unreadable_crash_is_skipped() {
        let root = temp(&std::env::current_dir().unwrap()).unwrap();
        let mut skipped = Vec::new();
        assert!(
            read_crash(
                &root.0.join("missing.txt"),
                "crashes/missing.txt",
                &mut skipped
            )
            .is_none()
        );
        assert_eq!(skipped, ["crashes/missing.txt: unreadable"]);
    }

    #[test]
    fn redacts_home_and_uri() {
        assert_eq!(
            redact(
                "/srv/private/ded/a /home/ded/b file:///home/ded/c",
                &identity()
            ),
            "~/a ~/b file://~/c"
        );
    }

    #[test]
    fn user_words_and_unicode() {
        assert_eq!(
            redact(
                "ded /ded/x deduce добавлено embedded_ded ded-дом",
                &identity()
            ),
            "<user> /<user>/x deduce добавлено embedded_ded <user>-дом"
        );
    }

    #[test]
    fn real_name_words() {
        assert_eq!(
            redact("Author: Ded Achtung <x>", &identity()),
            "Author: <name> <x>"
        );
        let short = Identity {
            real_name: "Ed".into(),
            ..identity()
        };
        assert_eq!(redact("Ed edits", &short), "Ed edits");
    }

    #[test]
    fn host_words() {
        assert_eq!(
            redact("workstation workstation.local workstation2", &identity()),
            "<host> <host>.local workstation2"
        );
    }

    #[test]
    fn wifi_lines() {
        assert_eq!(
            redact(
                "Ошибка: No network with SSID 'My private wifi' found.\nSSID=Other\n",
                &identity()
            ),
            "Ошибка: No network with SSID: <wifi>\nSSID: <wifi>\n"
        );
    }

    #[test]
    fn unchanged_bytes() {
        let text = "добавлено deduce dissident\r\nобычная строка\n\t123";
        assert_eq!(redact(text, &identity()).as_bytes(), text.as_bytes());
    }

    #[test]
    fn explicit_file_set() {
        assert_eq!(
            FILES,
            [
                "README.txt",
                "system.txt",
                "doctor.txt",
                "config.toml",
                "daemon.log",
                "lock.log",
                "crashes/"
            ]
        );
    }

    #[test]
    fn archive_date_format() {
        assert_eq!(
            archive_name(1_700_000_000),
            "mimshel-report-2023-11-14_22-13-20.tar.gz"
        );
    }

    #[test]
    fn missing_tool_and_log() {
        assert_eq!(
            optional(&["mimshel-nonexistent-report-tool"]),
            "mimshel-nonexistent-report-tool: missing\n"
        );
        let root = temp(&std::env::current_dir().unwrap()).unwrap();
        assert_eq!(
            tail(&root.0.join("absent")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn last_500_lines() {
        let root = temp(&std::env::current_dir().unwrap()).unwrap();
        let path = root.0.join("log");
        fs::write(
            &path,
            (0..600).map(|i| format!("{i}\n")).collect::<String>(),
        )
        .unwrap();
        let text = tail(&path).unwrap();
        assert_eq!(text.lines().count(), 500);
        assert!(text.starts_with("100\n"));
        assert!(text.ends_with("599"));
    }
}
