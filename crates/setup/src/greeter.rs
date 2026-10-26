//! Login screen look (`mimshel apply greeter`): style LightDM's slick-greeter
//! with the Mimshel look — our wallpaper, GTK theme, cursor and font. Only the
//! look changes; logging in stays LightDM's job.
//!
//! The greeter runs as the `lightdm` user and cannot read the user's home
//! (mode 700), so everything it shows is installed system-wide under
//! `/usr/local/share` and `/etc/lightdm/slick-greeter.conf` is rewritten to
//! point there. One `sudo` call does the system part; `--dry-run` only prints.
//! The previous config is kept as `slick-greeter.conf.mimshel-orig`, the list
//! of installed paths — as `/usr/local/share/mimshel/greeter-installed`
//! (both are removed again by `mimshel restore`).

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use mimshel_core::config::Config;

use crate::sys::{home, output_any, read};

/// slick-greeter config (root:root 644 on a stock EndeavourOS).
pub const CONF: &str = "/etc/lightdm/slick-greeter.conf";
/// Where the pre-Mimshel config is kept (only when it is not ours already).
pub const CONF_ORIG: &str = "/etc/lightdm/slick-greeter.conf.mimshel-orig";
/// What we installed: one absolute path per line (undo removes them).
pub const MANIFEST: &str = "/usr/local/share/mimshel/greeter-installed";
/// Wallpaper the greeter shows (readable by the `lightdm` user).
pub const WALLPAPER: &str = "/usr/local/share/backgrounds/mimshel/login.png";
/// Theme/cursor install targets.
pub const THEME_DIR: &str = "/usr/local/share/themes";
pub const ICON_DIR: &str = "/usr/local/share/icons";
/// First line of our config: marks it as ours (idempotent apply, undo, doctor).
pub const MARKER: &str = "# mimshel: login screen look (mimshel apply greeter); original saved as slick-greeter.conf.mimshel-orig";

/// The one root step, run as `sudo sh -c ROOT_SCRIPT sh ARG…`. A constant:
/// user data arrives only through the arguments. Args: staged wallpaper,
/// marker, staged conf, then (source, target) pairs for themes/cursors.
/// `cp -a` keeps symlinks inside cursor themes as they are. Every copy
/// destination is checked before `rm -rf`: it must be under
/// /usr/local/share/themes/ or /usr/local/share/icons/ and contain no `..`.
/// Missing directories are created 755; existing ones are left untouched.
pub const ROOT_SCRIPT: &str = concat!(
    "set -eu\n",
    "WALL=/usr/local/share/backgrounds/mimshel/login.png\n",
    "CONF=/etc/lightdm/slick-greeter.conf\n",
    "ORIG=/etc/lightdm/slick-greeter.conf.mimshel-orig\n",
    "MANIFEST=/usr/local/share/mimshel/greeter-installed\n",
    "for d in /usr/local/share/backgrounds /usr/local/share/backgrounds/mimshel /usr/local/share/themes /usr/local/share/icons /usr/local/share/mimshel; do\n",
    "    [ -d \"$d\" ] || install -d -m 755 \"$d\"\n",
    "done\n",
    "WALL_SRC=$1\n",
    "MARK=$2\n",
    "CONF_SRC=$3\n",
    "shift 3\n",
    "install -m 644 \"$WALL_SRC\" \"$WALL\"\n",
    "printf '%s\\n' \"$WALL\" >> \"$MANIFEST\"\n",
    "while [ \"$#\" -ge 2 ]; do\n",
    "    case \"$2\" in\n",
    "        /usr/local/share/themes/?*|/usr/local/share/icons/?*)\n",
    "            ;;\n",
    "        *)\n",
    "            echo \"mimshel greeter: refusing to touch $2\" >&2\n",
    "            exit 1\n",
    "            ;;\n",
    "    esac\n",
    "    case \"$2\" in\n",
    "        *..*)\n",
    "            echo \"mimshel greeter: refusing '..': $2\" >&2\n",
    "            exit 1\n",
    "            ;;\n",
    "    esac\n",
    "    rm -rf \"$2\"\n",
    "    cp -a \"$1\" \"$2\"\n",
    "    chmod -R a+rX \"$2\"\n",
    "    printf '%s\\n' \"$2\" >> \"$MANIFEST\"\n",
    "    shift 2\n",
    "done\n",
    "sort -u \"$MANIFEST\" > \"$MANIFEST.tmp\"\n",
    "mv \"$MANIFEST.tmp\" \"$MANIFEST\"\n",
    "if [ ! -f \"$ORIG\" ] && [ -f \"$CONF\" ] && ! grep -qxF \"$MARK\" \"$CONF\"; then\n",
    "    cp \"$CONF\" \"$ORIG\"\n",
    "fi\n",
    "install -m 644 \"$CONF_SRC\" \"$CONF\"\n",
);

/// Undo step for `mimshel restore`: a constant, no arguments. Restores the
/// original config and removes every manifest path under /usr/local/share/;
/// lines with a `..` component are skipped.
pub const UNDO_SCRIPT: &str = concat!(
    "set -eu\n",
    "CONF=/etc/lightdm/slick-greeter.conf\n",
    "ORIG=/etc/lightdm/slick-greeter.conf.mimshel-orig\n",
    "MANIFEST=/usr/local/share/mimshel/greeter-installed\n",
    "if [ -f \"$ORIG\" ]; then\n",
    "    cp \"$ORIG\" \"$CONF\"\n",
    "fi\n",
    "if [ -f \"$MANIFEST\" ]; then\n",
    "    while IFS= read -r p; do\n",
    "        case \"$p\" in\n",
    "            *..*)\n",
    "                echo \"mimshel greeter: refusing '..': $p\" >&2\n",
    "                ;;\n",
    "            /usr/local/share/?*/?*)\n",
    "                rm -rf \"$p\"\n",
    "                ;;\n",
    "        esac\n",
    "    done < \"$MANIFEST\"\n",
    "    rm -f \"$MANIFEST\"\n",
    "fi\n",
);

/// Values for the `[Greeter]` keys we set (from the Mimshel config and what
/// was found on disk). `None` means: not found anywhere — leave the original
/// conf value of that key alone.
pub struct GreeterLook {
    /// `background`: installed wallpaper, system-wide path.
    pub background: String,
    /// `theme-name`: our GTK theme.
    pub theme: Option<String>,
    /// `cursor-theme-name`: our cursor theme.
    pub cursor: Option<String>,
    /// `font-name` (`<font> 11`), only when the font is installed system-wide.
    pub font: Option<String>,
}

/// Effective `greeter-session` of LightDM: `greeter-session` from `[Seat:*]`
/// (or the old name `[SeatDefaults]`) over config texts in load order —
/// later files win; within one file the last value wins.
pub fn greeter_session(files: &[&str]) -> Option<String> {
    files
        .iter()
        .fold(None, |found, text| parse_greeter_session(text).or(found))
}

/// `greeter-session` from one LightDM config text (last value wins).
fn parse_greeter_session(text: &str) -> Option<String> {
    let mut section = "";
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim();
        } else if !line.starts_with(['#', ';'])
            && let Some((key, value)) = line.split_once('=')
            && (section == "Seat:*" || section == "SeatDefaults")
            && key.trim() == "greeter-session"
        {
            found = Some(value.trim().to_string());
        }
    }
    found
}

/// New slick-greeter.conf: the original text with our `[Greeter]` keys set —
/// every key we do not set (and every other section) is preserved in place.
/// The first line is the marker. A previous marker is dropped first, so
/// applying our own file again gives the same text (idempotent).
pub fn greeter_conf(original: &str, look: &GreeterLook) -> String {
    let cleaned: Vec<&str> = original.lines().filter(|l| l.trim() != MARKER).collect();
    let base = cleaned.join("\n");
    let mut set: Vec<(&str, String)> = vec![
        ("background", look.background.clone()),
        ("draw-grid", "false".into()),
        ("draw-user-backgrounds", "false".into()),
    ];
    if let Some(theme) = &look.theme {
        set.push(("theme-name", theme.clone()));
    }
    if let Some(cursor) = &look.cursor {
        set.push(("cursor-theme-name", cursor.clone()));
    }
    if let Some(font) = &look.font {
        set.push(("font-name", font.clone()));
    }
    let body = set_in_section(&base, "Greeter", &set);
    format!("{MARKER}\n{body}")
}

/// Set `key=value` pairs inside `[section]`, keeping all other lines as they
/// are. Existing keys are replaced in place, new ones appended at the end of
/// the section (in `set` order); a missing section is created at the end.
fn set_in_section(text: &str, section: &str, set: &[(&str, String)]) -> String {
    let header = format!("[{section}]");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.trim() == header);
    let mut out: Vec<String> = Vec::new();
    let mut replaced = vec![false; set.len()];
    let mut insert_at;
    match start {
        Some(start) => {
            let end = lines[start + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map_or(lines.len(), |p| start + 1 + p);
            for (i, line) in lines.iter().enumerate() {
                let mut replacement: Option<String> = None;
                if (start + 1..end).contains(&i) {
                    let trimmed = line.trim();
                    if !trimmed.starts_with(['#', ';'])
                        && let Some((key, _)) = trimmed.split_once('=')
                        && let Some(n) = set.iter().position(|(k, _)| *k == key.trim())
                        && !replaced[n]
                    {
                        replaced[n] = true;
                        replacement = Some(format!("{}={}", set[n].0, set[n].1));
                    }
                }
                out.push(replacement.unwrap_or_else(|| line.to_string()));
            }
            insert_at = end;
        }
        None => {
            out.extend(lines.iter().map(|l| (*l).to_string()));
            if !out.is_empty() && !out.last().is_some_and(|l| l.is_empty()) {
                out.push(String::new());
            }
            out.push(header);
            insert_at = out.len();
        }
    }
    for (n, (key, value)) in set.iter().enumerate() {
        if !replaced[n] {
            out.insert(insert_at, format!("{key}={value}"));
            insert_at += 1;
        }
    }
    let mut text = out.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Is this slick-greeter.conf text ours (does it carry our marker line)?
pub fn is_ours(text: &str) -> bool {
    text.lines().any(|l| l.trim() == MARKER)
}

/// Theme/cursor names end up in `rm -rf`/`cp` targets, so only plain names:
/// non-empty, no path separators or NUL, not `.`/`..`, not starting with `-`,
/// and of sane length (≤ 128 chars).
pub fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= 128
        && !s.contains(['/', '\0'])
        && s != "."
        && s != ".."
        && !s.starts_with('-')
}

/// What kind of theme set we are looking for (recognized by marker files).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    /// GTK theme: `<name>/index.theme` or `<name>/gtk-3.0`.
    Gtk,
    /// Cursor theme: `<name>/cursors`.
    Cursor,
}

impl ThemeKind {
    fn present(self, dir: &Path, name: &str) -> bool {
        let base = dir.join(name);
        match self {
            ThemeKind::Gtk => base.join("index.theme").is_file() || base.join("gtk-3.0").is_dir(),
            ThemeKind::Cursor => base.join("cursors").is_dir(),
        }
    }
}

/// First dir in `dirs` that holds a set called `name`.
pub fn find_theme(name: &str, dirs: &[PathBuf], kind: ThemeKind) -> Option<PathBuf> {
    dirs.iter().find(|d| kind.present(d, name)).cloned()
}

/// Is the font installed system-wide? `fc-list -f '%{file}\n' NAME` output:
/// at least one file and none under `/home` (the greeter user cannot read
/// home directories anyway).
pub fn font_system_wide(fc_list: &str) -> bool {
    let mut any = false;
    for file in fc_list.lines().map(str::trim).filter(|l| !l.is_empty()) {
        any = true;
        if file.starts_with("/home/") {
            return false;
        }
    }
    any
}

/// Staged files plus theme/cursor copies, ready for [`ROOT_SCRIPT`].
pub struct RootPlan {
    /// Staged slick-greeter.conf.
    pub conf: PathBuf,
    /// Staged wallpaper copy.
    pub wallpaper: PathBuf,
    /// (source dir, target dir) for themes/cursors to install.
    pub copies: Vec<(PathBuf, PathBuf)>,
}

/// Arguments for `sudo sh -c ROOT_SCRIPT sh …`: staged wallpaper, marker,
/// staged conf, then (source, target) pairs. Built here so tests can check
/// that every target stays under `/usr/local/share/`.
pub fn root_args(plan: &RootPlan) -> Vec<String> {
    let mut args = vec![
        plan.wallpaper.display().to_string(),
        MARKER.to_string(),
        plan.conf.display().to_string(),
    ];
    for (src, dst) in &plan.copies {
        args.push(src.display().to_string());
        args.push(dst.display().to_string());
    }
    args
}

/// Paths the undo step removes: manifest lines filtered to real paths under
/// `/usr/local/share/` (the undo script checks the same prefix).
pub fn installed_paths() -> Vec<String> {
    read(Path::new(MANIFEST))
        .lines()
        .map(str::trim)
        .filter(|l| manifest_path_ok(l))
        .map(String::from)
        .collect()
}

/// Only a path with real components under `/usr/local/share/` may be removed.
pub fn manifest_path_ok(p: &str) -> bool {
    let Some(rest) = p.strip_prefix("/usr/local/share/") else {
        return false;
    };
    rest.contains('/')
        && rest
            .split('/')
            .all(|c| !c.is_empty() && c != "." && c != "..")
}

/// Where a theme set lives and what to do with it. The home copy wins (it is
/// refreshed on every apply, so the manifest keeps the theme and undo removes
/// it); only when the set is NOT at home but IS system-wide is there nothing
/// to do; nowhere — the conf keeps its current value.
#[derive(Debug)]
enum Located {
    System,
    CopyFrom(PathBuf),
    Missing,
}

fn locate(name: &str, kind: ThemeKind, user_dirs: &[PathBuf], system_dirs: &[PathBuf]) -> Located {
    if let Some(dir) = find_theme(name, user_dirs, kind) {
        return Located::CopyFrom(dir.join(name));
    }
    if find_theme(name, system_dirs, kind).is_some() {
        return Located::System;
    }
    Located::Missing
}

/// LightDM config texts in load order (later values win): the drop-in
/// directories, then the main config. `MIMSHEL_LIGHTDM_DIR` (for tests and
/// dry runs on machines without LightDM) replaces both `/usr/share/lightdm`
/// and `/etc/lightdm` with that directory.
fn lightdm_conf_texts() -> Vec<String> {
    if let Some(dir) = lightdm_dir() {
        let mut out = conf_dropins(&dir.join("lightdm.conf.d"));
        out.push(read(&dir.join("lightdm.conf")));
        return out;
    }
    let mut out = conf_dropins(Path::new("/usr/share/lightdm/lightdm.conf.d"));
    out.extend(conf_dropins(Path::new("/etc/lightdm/lightdm.conf.d")));
    out.push(read(Path::new("/etc/lightdm/lightdm.conf")));
    out
}

/// Where the wallpaper for `value` at `w`x`h` is or will be drawn — without
/// drawing (`--dry-run` must not write anything).
fn wallpaper_plan(value: &str, w: u32, h: u32) -> Option<PathBuf> {
    match mimshel_wallpaper::Style::parse(value) {
        Some(style) => Some(crate::parts::wallpaper_path(style, w, h)),
        None => {
            let path = PathBuf::from(value);
            path.is_file().then_some(path)
        }
    }
}

/// `MIMSHEL_LIGHTDM_DIR` — test/dry-run hook replacing /etc/lightdm.
fn lightdm_dir() -> Option<PathBuf> {
    std::env::var_os("MIMSHEL_LIGHTDM_DIR").map(PathBuf::from)
}

/// The greeter config to read. Real runs: /etc/lightdm/slick-greeter.conf;
/// with [`lightdm_dir`] set: that directory mirrors /etc/lightdm. (The root
/// step still writes the real path — the script is a constant.)
fn conf_path() -> PathBuf {
    lightdm_dir().map_or_else(|| PathBuf::from(CONF), |d| d.join("slick-greeter.conf"))
}

/// The saved original config, next to [`conf_path`].
fn conf_orig_path() -> PathBuf {
    lightdm_dir().map_or_else(
        || PathBuf::from(CONF_ORIG),
        |d| d.join("slick-greeter.conf.mimshel-orig"),
    )
}

fn conf_dropins(dir: &Path) -> Vec<String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "conf"))
        .collect();
    files.sort();
    files.iter().map(|f| read(f)).collect()
}

/// `mimshel apply greeter [--dry-run]`. Styles the LightDM login screen with
/// the Mimshel look. `dry` — only print the steps, the target paths and the
/// full new config; change nothing, ask for no sudo.
pub fn apply(config: &Config, dry: bool) -> Result<(), String> {
    let texts = lightdm_conf_texts();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    match greeter_session(&refs).as_deref() {
        Some("lightdm-slick-greeter") => {
            println!("▶ Экран входа в систему (LightDM, slick-greeter)");
        }
        Some(other) => {
            println!("▶ Экран входа: настроен greeter «{other}»");
            println!("  Оформлять умею только slick-greeter — ничего не меняю");
            return Err(format!("greeter «{other}», а нужен lightdm-slick-greeter"));
        }
        None => {
            println!("▶ Экран входа: в настройках LightDM greeter-session не задан");
            println!("  Оформлять умею только slick-greeter — ничего не меняю");
            return Err("greeter LightDM не задан (поддерживается только slick-greeter)".into());
        }
    }
    if !safe_name(&config.gtk_theme) {
        return Err(format!(
            "небезопасное имя темы «{}» — поправьте look.gtk_theme",
            config.gtk_theme
        ));
    }
    if !safe_name(&config.cursor_theme) {
        return Err(format!(
            "небезопасное имя курсора «{}» — поправьте look.cursor_theme",
            config.cursor_theme
        ));
    }
    let gtk_user = [home().join(".themes"), home().join(".local/share/themes")];
    let gtk_system = [
        PathBuf::from("/usr/share/themes"),
        PathBuf::from("/usr/local/share/themes"),
    ];
    let icon_user = [home().join(".icons"), home().join(".local/share/icons")];
    let icon_system = [
        PathBuf::from("/usr/share/icons"),
        PathBuf::from("/usr/local/share/icons"),
    ];
    let mut copies: Vec<(PathBuf, PathBuf)> = Vec::new();
    let theme = match locate(&config.gtk_theme, ThemeKind::Gtk, &gtk_user, &gtk_system) {
        Located::System => {
            println!("  тема {} — уже в системной папке", config.gtk_theme);
            Some(config.gtk_theme.clone())
        }
        Located::CopyFrom(src) => {
            let dst = PathBuf::from(THEME_DIR).join(&config.gtk_theme);
            println!(
                "  тема {}: {} → {}",
                config.gtk_theme,
                src.display(),
                dst.display()
            );
            copies.push((src, dst));
            Some(config.gtk_theme.clone())
        }
        Located::Missing => {
            println!(
                "  ⚠ тема {} нигде нет — theme-name в конфиге оставляю прежним",
                config.gtk_theme
            );
            None
        }
    };
    let cursor = match locate(
        &config.cursor_theme,
        ThemeKind::Cursor,
        &icon_user,
        &icon_system,
    ) {
        Located::System => {
            println!("  курсор {} — уже в системной папке", config.cursor_theme);
            Some(config.cursor_theme.clone())
        }
        Located::CopyFrom(src) => {
            let dst = PathBuf::from(ICON_DIR).join(&config.cursor_theme);
            println!(
                "  курсор {}: {} → {}",
                config.cursor_theme,
                src.display(),
                dst.display()
            );
            copies.push((src, dst));
            Some(config.cursor_theme.clone())
        }
        Located::Missing => {
            println!(
                "  ⚠ курсор {} нигде нет — cursor-theme-name в конфиге оставляю прежним",
                config.cursor_theme
            );
            None
        }
    };
    let fc = output_any(&["fc-list", "-f", "%{file}\n", &config.font]);
    let font = if font_system_wide(&fc) {
        let font = format!("{} 11", config.font);
        println!("  шрифт: {font}");
        Some(font)
    } else {
        println!(
            "  ⚠ шрифт {} не найден среди системных — font-name в конфиге не меняю",
            config.font
        );
        None
    };
    let rects = crate::parts::monitor_rects();
    let biggest = rects.iter().max_by_key(|r| r.2.saturating_mul(r.3));
    let (w, h) = biggest.map_or((1920, 1080), |r| (r.2, r.3));
    // В --dry-run обои не рисуем: ничего не должно появиться на диске.
    let wall = if dry {
        wallpaper_plan(&config.wallpaper, w, h)
    } else {
        crate::parts::wallpaper_file(&config.wallpaper, i64::from(w), i64::from(h))
    };
    let Some(wall) = wall else {
        return Err(format!(
            "не нарисовать обои «{}» — сначала выполните: mimshel apply wallpaper",
            config.wallpaper
        ));
    };
    println!("  обои: {} ({}x{})", wall.display(), w, h);
    // Наша конфигурация строится от исходного файла: если текущий — наш,
    // берём сохранённый оригинал, чтобы повторный apply не делал наш файл
    // «оригиналом».
    let current = read(&conf_path());
    let base = if is_ours(&current) {
        let orig = read(&conf_orig_path());
        if orig.is_empty() { current } else { orig }
    } else {
        current
    };
    let look = GreeterLook {
        background: WALLPAPER.to_string(),
        theme,
        cursor,
        font,
    };
    let conf_text = greeter_conf(&base, &look);
    println!("  конфиг: {CONF} (оригинал сохранится как {CONF_ORIG})");
    println!("  список установленного: {MANIFEST}");
    let stage = mimshel_core::names::dir_in(&home().join(".cache")).join("greeter");
    let plan = RootPlan {
        conf: stage.join("slick-greeter.conf"),
        wallpaper: stage.join("login.png"),
        copies,
    };
    let args = root_args(&plan);
    if dry {
        println!();
        println!("Новый {CONF}:");
        print!("{conf_text}");
        if !conf_text.ends_with('\n') {
            println!();
        }
        println!("Ничего не меняю. Для установки (один sudo) выполните:");
        println!("  {}", sudo_line(&args));
        return Ok(());
    }
    std::fs::create_dir_all(&stage).map_err(|e| format!("не создать {}: {e}", stage.display()))?;
    if let Err(e) = std::fs::write(&plan.conf, &conf_text) {
        return Err(format!("не записать {}: {e}", plan.conf.display()));
    }
    if let Err(e) = std::fs::copy(&wall, &plan.wallpaper) {
        return Err(format!(
            "не скопировать обои в {}: {e}",
            plan.wallpaper.display()
        ));
    }
    if !std::io::stdin().is_terminal() {
        println!("Нет терминала — sudo не запускаю (ему нужен пароль). Выполните сами:");
        println!("  {}", sudo_line(&args));
        return Err("подготовлено, осталось выполнить команду с правами".into());
    }
    let status = std::process::Command::new("sudo")
        .arg("sh")
        .arg("-c")
        .arg(ROOT_SCRIPT)
        .arg("sh")
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("не запустить sudo: {e}"))?;
    if !status.success() {
        return Err(format!("sudo завершился с ошибкой ({status})"));
    }
    println!("✔ Экран входа в систему оформлен");
    println!("  Проверить без выхода из системы: slick-greeter --test-mode");
    println!("  (Esc или закрыть окно — выйти; настоящий экран входа изменится");
    println!("   при следующем входе в систему или перезагрузке)");
    Ok(())
}

/// Undo step for `mimshel restore` (`greeter` part): back to the original
/// slick-greeter.conf and remove what the manifest lists. Without a terminal
/// there is no way to type the sudo password — print the exact command, warn
/// and let the restore continue.
pub fn undo() -> Result<(), String> {
    if !is_ours(&read(&conf_path())) {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        println!("⚠ sudo нужен пароль, а терминала нет — выполните сами, чтобы довести откат:");
        println!("  sudo sh -c {}", sh_quote(UNDO_SCRIPT));
        return Ok(());
    }
    let status = std::process::Command::new("sudo")
        .args(["sh", "-c", UNDO_SCRIPT])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("не запустить sudo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("sudo завершился с ошибкой ({status})"))
    }
}

/// The exact sudo command for the root step (printed when there is no
/// terminal to type the password into).
fn sudo_line(args: &[String]) -> String {
    let args: Vec<String> = args.iter().map(|a| sh_quote(a)).collect();
    format!("sudo sh -c {} sh {}", sh_quote(ROOT_SCRIPT), args.join(" "))
}

/// Shell-single-quote a string for printing a command to run by hand.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stock EndeavourOS slick-greeter.conf from the test laptop.
    const LAPTOP: &str = "[Greeter]\n\
        background=/usr/share/endeavouros/backgrounds/endeavouros-wallpaper.png\n\
        draw-user-backgrounds=false\n\
        draw-grid=true\n\
        theme-name=Arc-Dark\n\
        icon-theme-name=Qogir-Dark\n\
        cursor-theme-name=Qogir-Dark\n\
        cursor-theme-size=24\n\
        show-a11y=false\n\
        show-power=false\n";

    fn look() -> GreeterLook {
        GreeterLook {
            background: WALLPAPER.to_string(),
            theme: Some("Colloid-Dark".to_string()),
            cursor: Some("Colloid-cursors".to_string()),
            font: Some("Inter 11".to_string()),
        }
    }

    #[test]
    fn greeter_session_main_file() {
        let main = "[Seat:*]\ngreeter-session=lightdm-slick-greeter\n";
        assert_eq!(
            greeter_session(&[main]).as_deref(),
            Some("lightdm-slick-greeter")
        );
    }

    #[test]
    fn greeter_session_dropin_and_main_later_wins() {
        let dropin = "[Seat:*]\ngreeter-session=lightdm-gtk-greeter\n";
        let main = "[Seat:*]\n# comment\ngreeter-session = lightdm-slick-greeter\n";
        assert_eq!(
            greeter_session(&[dropin, main]).as_deref(),
            Some("lightdm-slick-greeter")
        );
        assert_eq!(
            greeter_session(&[dropin]).as_deref(),
            Some("lightdm-gtk-greeter")
        );
        // Within one file the last value wins too.
        let last = "[Seat:*]\ngreeter-session=a\ngreeter-session=b\n";
        assert_eq!(greeter_session(&[last]).as_deref(), Some("b"));
    }

    #[test]
    fn greeter_session_seat_defaults_old_name() {
        let old = "[SeatDefaults]\ngreeter-session=lightdm-slick-greeter\n";
        assert_eq!(
            greeter_session(&[old]).as_deref(),
            Some("lightdm-slick-greeter")
        );
    }

    #[test]
    fn greeter_session_missing_or_other_section() {
        assert_eq!(greeter_session(&[]), None);
        assert_eq!(greeter_session(&["[Seat:*]\nuser-session=xfce\n"]), None);
        assert_eq!(greeter_session(&["[lightdm]\ngreeter-session=x\n"]), None);
        assert_eq!(
            greeter_session(&["[Seat:0]\ngreeter-session=x\n[SeatDefaults]\n"]),
            None
        );
    }

    #[test]
    fn greeter_conf_sets_keys_and_preserves_the_rest() {
        let out = greeter_conf(LAPTOP, &look());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], MARKER, "marker first line");
        assert!(out.contains("[Greeter]\n"), "{out}");
        assert!(out.contains("background=/usr/local/share/backgrounds/mimshel/login.png"));
        assert!(out.contains("draw-grid=false"));
        assert!(out.contains("draw-user-backgrounds=false"));
        assert!(out.contains("theme-name=Colloid-Dark"));
        assert!(out.contains("cursor-theme-name=Colloid-cursors"));
        assert!(out.contains("font-name=Inter 11"));
        // Everything we do not set stays, order kept.
        for keep in [
            "icon-theme-name=Qogir-Dark",
            "cursor-theme-size=24",
            "show-a11y=false",
            "show-power=false",
        ] {
            assert!(out.contains(keep), "{keep}\n{out}");
        }
        let pos = |s: &str| out.find(s).unwrap();
        assert!(pos("background=") < pos("draw-user-backgrounds=false"));
        assert!(pos("draw-user-backgrounds=false") < pos("draw-grid=false"));
        assert!(pos("theme-name=") < pos("icon-theme-name="));
        assert!(pos("icon-theme-name=") < pos("cursor-theme-name="));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn greeter_conf_idempotent_with_ours_as_input() {
        let once = greeter_conf(LAPTOP, &look());
        assert!(is_ours(&once));
        let twice = greeter_conf(&once, &look());
        assert_eq!(once, twice, "applying our own file changes nothing");
    }

    #[test]
    fn greeter_conf_missing_values_keep_original() {
        let look = GreeterLook {
            background: WALLPAPER.to_string(),
            theme: None,
            cursor: None,
            font: None,
        };
        let out = greeter_conf(LAPTOP, &look);
        assert!(out.contains("theme-name=Arc-Dark"), "{out}");
        assert!(out.contains("cursor-theme-name=Qogir-Dark"), "{out}");
        assert!(!out.contains("font-name"), "{out}");
        assert!(out.contains("draw-grid=false"), "{out}");
        assert!(out.contains("background=/usr/local/share/backgrounds/mimshel/login.png"));
    }

    #[test]
    fn greeter_conf_creates_missing_section() {
        let out = greeter_conf("", &look());
        assert!(is_ours(&out));
        assert!(out.contains("[Greeter]\n"), "{out}");
        assert!(out.contains("theme-name=Colloid-Dark"), "{out}");
    }

    #[test]
    fn is_ours_finds_marker_only() {
        assert!(!is_ours(LAPTOP));
        assert!(!is_ours(""));
        assert!(is_ours(&greeter_conf(LAPTOP, &look())));
        // A mere mention is not the marker.
        assert!(!is_ours("# based on mimshel\n[Greeter]\n"));
    }

    #[test]
    fn safe_name_accepts_and_rejects() {
        for bad in ["", ".", "..", "-x", "--", "a/b", "a\0b", &"x".repeat(129)] {
            assert!(!safe_name(bad), "{bad:?}");
        }
        for ok in ["Colloid-Dark", "Colloid-cursors", "a b.c", &"x".repeat(128)] {
            assert!(safe_name(ok), "{ok:?}");
        }
    }

    #[test]
    fn font_system_wide_from_fc_list_output() {
        assert!(font_system_wide(
            "/usr/share/fonts/inter/Inter-Regular.ttf\n"
        ));
        assert!(font_system_wide(
            "/usr/share/fonts/inter/Inter-Regular.ttf\n/var/lib/flatpak/font.ttf\n"
        ));
        assert!(!font_system_wide(""));
        assert!(!font_system_wide("\n  \n"));
        // At least one file, but one is in a home directory — not system-wide.
        assert!(!font_system_wide(
            "/home/ded/.local/share/fonts/Inter.ttf\n/usr/share/fonts/inter/Inter-Bold.ttf\n"
        ));
        assert!(!font_system_wide("/home/ded/.fonts/Inter.ttf\n"));
    }

    #[test]
    fn root_script_is_constant_and_targets_stay_in_usr_local_share() {
        let plan = RootPlan {
            conf: PathBuf::from("/home/ded/.cache/mimshel/greeter/slick-greeter.conf"),
            wallpaper: PathBuf::from("/home/ded/.cache/mimshel/greeter/login.png"),
            copies: vec![
                (
                    PathBuf::from("/home/ded/.themes/Colloid-Dark"),
                    PathBuf::from("/usr/local/share/themes/Colloid-Dark"),
                ),
                (
                    PathBuf::from("/home/ded/.local/share/icons/Colloid-cursors"),
                    PathBuf::from("/usr/local/share/icons/Colloid-cursors"),
                ),
            ],
        };
        let args = root_args(&plan);
        // wallpaper, marker, conf, then two (source, target) pairs.
        assert_eq!(args.len(), 3 + 4);
        assert!(args[0].ends_with("login.png"));
        assert_eq!(args[1], MARKER);
        assert!(args[2].ends_with("slick-greeter.conf"));
        assert!(ROOT_SCRIPT.starts_with("set -eu\n"));
        // The script is a constant: no user data inside (only "$1"… args).
        for a in &args {
            assert!(
                !ROOT_SCRIPT.contains(a.as_str()),
                "user data in script: {a}"
            );
        }
        // Every copy target (and the manifest paths derived from them) stays
        // under /usr/local/share/.
        for pair in args[3..].chunks(2) {
            assert!(pair[1].starts_with("/usr/local/share/"), "{}", pair[1]);
        }
        for p in [
            WALLPAPER,
            "/usr/local/share/themes/Colloid-Dark",
            "/usr/local/share/icons/Colloid-cursors",
        ] {
            assert!(manifest_path_ok(p), "{p}");
        }
    }

    #[test]
    fn manifest_paths_filtered() {
        for bad in [
            "/usr/local/share/",
            "/usr/local/share",
            "/usr/local/share/../etc",
            "/etc/lightdm/slick-greeter.conf",
            "/home/ded/.themes/X",
            "relative/path",
            "",
        ] {
            assert!(!manifest_path_ok(bad), "{bad:?}");
        }
        assert!(manifest_path_ok(WALLPAPER));
        assert!(manifest_path_ok("/usr/local/share/themes/Colloid-Dark"));
    }

    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn find_theme_with_temp_dirs() {
        let t = Temp(
            std::env::temp_dir().join(format!("mimshel-greeter-theme-{}", std::process::id())),
        );
        let a = t.0.join("a");
        let b = t.0.join("b");
        std::fs::create_dir_all(a.join("Colloid-Dark/gtk-3.0")).unwrap();
        std::fs::create_dir_all(b.join("Colloid-cursors/cursors")).unwrap();
        std::fs::create_dir_all(a.join("Other")).unwrap();
        std::fs::write(a.join("Other/index.theme"), "[Icon Theme]\nName=Other\n").unwrap();
        let dirs = [a.clone(), b.clone()];
        assert_eq!(
            find_theme("Colloid-Dark", &dirs, ThemeKind::Gtk),
            Some(a.clone())
        );
        assert_eq!(
            find_theme("Colloid-cursors", &dirs, ThemeKind::Cursor),
            Some(b.clone())
        );
        // index.theme alone is enough for a GTK theme.
        assert_eq!(find_theme("Other", &dirs, ThemeKind::Gtk), Some(a.clone()));
        // A GTK theme is not a cursor theme and vice versa.
        assert_eq!(find_theme("Colloid-Dark", &dirs, ThemeKind::Cursor), None);
        assert_eq!(find_theme("Colloid-cursors", &dirs, ThemeKind::Gtk), None);
        assert_eq!(find_theme("Nope", &dirs, ThemeKind::Gtk), None);
        assert_eq!(find_theme("Nope", &dirs, ThemeKind::Cursor), None);
    }

    #[test]
    fn locate_prefers_home_copy() {
        let t = Temp(
            std::env::temp_dir().join(format!("mimshel-greeter-locate-{}", std::process::id())),
        );
        let user = t.0.join("user");
        let system = t.0.join("system");
        std::fs::create_dir_all(user.join("Colloid-Dark/gtk-3.0")).unwrap();
        std::fs::create_dir_all(system.join("Colloid-Dark/gtk-3.0")).unwrap();
        std::fs::create_dir_all(system.join("Only-System/gtk-3.0")).unwrap();
        let users = [user.clone()];
        let systems = [system.clone()];
        // Home copy wins: refreshed on every apply, so the manifest keeps it.
        match locate("Colloid-Dark", ThemeKind::Gtk, &users, &systems) {
            Located::CopyFrom(p) => assert_eq!(p, user.join("Colloid-Dark")),
            other => panic!("expected CopyFrom, got {other:?}"),
        }
        // Only system-wide — nothing to copy.
        assert!(matches!(
            locate("Only-System", ThemeKind::Gtk, &users, &systems),
            Located::System
        ));
        // Nowhere — keep the original conf value.
        assert!(matches!(
            locate("Nope", ThemeKind::Gtk, &users, &systems),
            Located::Missing
        ));
    }
}

#[cfg(test)]
mod script_tests {
    //! The root scripts are the part that runs as root — so these tests
    //! actually execute them with `sh` against a temporary fake root (the
    //! script text has /usr/local/share and /etc/lightdm mapped into it).
    use super::*;
    use std::process::Output;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static N: AtomicUsize = AtomicUsize::new(0);

    /// A temp tree standing in for the root file system.
    struct Root(PathBuf);

    impl Root {
        fn new(tag: &str) -> Root {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("mimshel-greeter-{tag}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Root(dir)
        }

        fn p(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }

        /// The script with the system paths mapped into this fake root.
        fn script(&self, text: &str) -> String {
            text.replace(
                "/usr/local/share",
                &self.p("usr/local/share").to_string_lossy(),
            )
            .replace("/etc/lightdm", &self.p("etc/lightdm").to_string_lossy())
        }

        fn run(&self, script: &str, args: &[String]) -> Output {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(script)
                .arg("sh")
                .args(args)
                .output()
                .expect("run sh")
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Write the staged wallpaper + conf, return (stage dir, staged args prefix).
    fn stage_files(r: &Root, conf_text: &str) -> (PathBuf, Vec<String>) {
        let stage = r.p("stage");
        std::fs::create_dir_all(&stage).unwrap();
        std::fs::write(stage.join("login.png"), [1u8, 2, 3]).unwrap();
        std::fs::write(stage.join("slick-greeter.conf"), conf_text).unwrap();
        (
            stage.clone(),
            vec![
                stage.join("login.png").display().to_string(),
                MARKER.to_string(),
                stage.join("slick-greeter.conf").display().to_string(),
            ],
        )
    }

    /// A theme source dir with a symlink inside (cursor themes have those).
    fn theme_src(r: &Root, rel: &str) -> PathBuf {
        let src = r.p(rel);
        std::fs::create_dir_all(src.join("gtk-3.0")).unwrap();
        std::fs::write(src.join("index.theme"), "[Icon Theme]\n").unwrap();
        std::os::unix::fs::symlink("index.theme", src.join("linked.theme")).unwrap();
        src
    }

    fn assert_ok(out: &Output) {
        assert!(
            out.status.success(),
            "script failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn manifest(r: &Root) -> Vec<String> {
        let text =
            std::fs::read_to_string(r.p("usr/local/share/mimshel/greeter-installed")).unwrap();
        let mut lines: Vec<String> = text.lines().map(String::from).collect();
        lines.sort();
        lines
    }

    #[test]
    fn script_installs_conf_saves_original_copies_themes() {
        let r = Root::new("apply");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        let orig_text = "[Greeter]\ntheme-name=Arc-Dark\n";
        std::fs::write(&conf, orig_text).unwrap();
        let (_, mut args) = stage_files(
            &r,
            &format!("{MARKER}\n[Greeter]\ntheme-name=Colloid-Dark\n"),
        );
        let theme_src = theme_src(&r, "home/.themes/Colloid-Dark");
        let cur_src = r.p("home/.icons/Colloid-cursors");
        std::fs::create_dir_all(cur_src.join("cursors")).unwrap();
        let theme_dst = r.p("usr/local/share/themes/Colloid-Dark");
        let cur_dst = r.p("usr/local/share/icons/Colloid-cursors");
        args.push(theme_src.display().to_string());
        args.push(theme_dst.display().to_string());
        args.push(cur_src.display().to_string());
        args.push(cur_dst.display().to_string());

        let out = r.run(&r.script(ROOT_SCRIPT), &args);
        assert_ok(&out);

        // The staged conf is installed; the previous one is saved once.
        assert_eq!(
            std::fs::read_to_string(&conf).unwrap(),
            std::fs::read_to_string(r.p("stage/slick-greeter.conf")).unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(r.p("etc/lightdm/slick-greeter.conf.mimshel-orig")).unwrap(),
            orig_text
        );
        // Themes copied, the symlink inside kept as a symlink.
        assert!(theme_dst.join("gtk-3.0").is_dir());
        assert!(theme_dst.join("index.theme").is_file());
        assert!(
            std::fs::symlink_metadata(theme_dst.join("linked.theme"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(cur_dst.join("cursors").is_dir());
        // Wallpaper installed.
        assert_eq!(
            std::fs::read(r.p("usr/local/share/backgrounds/mimshel/login.png")).unwrap(),
            [1, 2, 3]
        );
        // Manifest: wallpaper + both targets.
        let mut want = vec![
            r.p("usr/local/share/backgrounds/mimshel/login.png")
                .display()
                .to_string(),
            theme_dst.display().to_string(),
            cur_dst.display().to_string(),
        ];
        want.sort();
        assert_eq!(manifest(&r), want);
    }

    #[test]
    fn script_zero_and_one_pair() {
        for (n, tag) in [(0usize, "zero"), (1, "one")] {
            let r = Root::new(tag);
            let conf = r.p("etc/lightdm/slick-greeter.conf");
            std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
            std::fs::write(&conf, "[Greeter]\n").unwrap();
            let (_, mut args) = stage_files(&r, "[Greeter]\n");
            let dst = r.p("usr/local/share/themes/One");
            if n == 1 {
                let src = theme_src(&r, "home/.themes/One");
                args.push(src.display().to_string());
                args.push(dst.display().to_string());
            }
            let out = r.run(&r.script(ROOT_SCRIPT), &args);
            assert_ok(&out);
            assert!(conf.exists());
            let mut want = vec![
                r.p("usr/local/share/backgrounds/mimshel/login.png")
                    .display()
                    .to_string(),
            ];
            if n == 1 {
                assert!(dst.join("index.theme").is_file());
                want.push(dst.display().to_string());
            }
            want.sort();
            assert_eq!(manifest(&r), want);
        }
    }

    #[test]
    fn script_saves_original_only_when_not_ours() {
        // Ours, no backup yet: nothing is saved as the "original".
        let r = Root::new("ours1");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, format!("{MARKER}\n[Greeter]\n")).unwrap();
        let (_, args) = stage_files(&r, "[Greeter]\n");
        assert_ok(&r.run(&r.script(ROOT_SCRIPT), &args));
        assert!(!r.p("etc/lightdm/slick-greeter.conf.mimshel-orig").exists());

        // Ours with a backup: the backup is not overwritten.
        let r = Root::new("ours2");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, format!("{MARKER}\n[Greeter]\n")).unwrap();
        let orig = r.p("etc/lightdm/slick-greeter.conf.mimshel-orig");
        std::fs::write(&orig, "PRISTINE\n").unwrap();
        let (_, args) = stage_files(&r, "[Greeter]\n");
        assert_ok(&r.run(&r.script(ROOT_SCRIPT), &args));
        assert_eq!(std::fs::read_to_string(&orig).unwrap(), "PRISTINE\n");

        // Not ours but a backup exists: kept (the original is saved once).
        let r = Root::new("ours3");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "[Greeter]\ntheme-name=Arc-Dark\n").unwrap();
        let orig = r.p("etc/lightdm/slick-greeter.conf.mimshel-orig");
        std::fs::write(&orig, "PRISTINE\n").unwrap();
        let (_, args) = stage_files(&r, "[Greeter]\n");
        assert_ok(&r.run(&r.script(ROOT_SCRIPT), &args));
        assert_eq!(std::fs::read_to_string(&orig).unwrap(), "PRISTINE\n");
    }

    #[test]
    fn script_refuses_unsafe_destinations() {
        // A destination outside themes/ or icons/: refused, nothing removed.
        let r = Root::new("refuse1");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "[Greeter]\ntheme-name=Arc-Dark\n").unwrap();
        let (_, mut args) = stage_files(&r, "[Greeter]\n");
        let evil = r.p("usr/local/share/other/evil");
        std::fs::create_dir_all(&evil).unwrap();
        std::fs::write(evil.join("sentinel"), "keep").unwrap();
        let src = theme_src(&r, "home/.themes/Ok");
        args.push(src.display().to_string());
        args.push(evil.display().to_string());
        let out = r.run(&r.script(ROOT_SCRIPT), &args);
        assert!(!out.status.success());
        assert_eq!(
            std::fs::read_to_string(evil.join("sentinel")).unwrap(),
            "keep"
        );
        // The run aborted before installing the conf.
        assert_eq!(
            std::fs::read_to_string(&conf).unwrap(),
            "[Greeter]\ntheme-name=Arc-Dark\n"
        );

        // A '..' component: refused as well.
        let r = Root::new("refuse2");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "[Greeter]\n").unwrap();
        let (_, mut args) = stage_files(&r, "[Greeter]\n");
        let sneaky = r.p("usr/local/share/themes/../icons/sneaky");
        args.push(src.display().to_string());
        args.push(sneaky.display().to_string());
        let out = r.run(&r.script(ROOT_SCRIPT), &args);
        assert!(!out.status.success());
        assert!(!r.p("usr/local/share/icons/sneaky").exists());
    }

    #[test]
    fn apply_twice_then_undo_removes_everything() {
        let r = Root::new("twice");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        let orig_text = "[Greeter]\ntheme-name=Arc-Dark\n";
        std::fs::write(&conf, orig_text).unwrap();
        let (_, mut args) = stage_files(
            &r,
            &format!("{MARKER}\n[Greeter]\ntheme-name=Colloid-Dark\n"),
        );
        let src = theme_src(&r, "home/.themes/Colloid-Dark");
        let dst = r.p("usr/local/share/themes/Colloid-Dark");
        args.push(src.display().to_string());
        args.push(dst.display().to_string());

        // First apply: theme copied, manifest lists it.
        assert_ok(&r.run(&r.script(ROOT_SCRIPT), &args));
        assert!(dst.join("index.theme").is_file());
        assert_eq!(
            manifest(&r),
            vec![
                r.p("usr/local/share/backgrounds/mimshel/login.png")
                    .display()
                    .to_string(),
                dst.display().to_string(),
            ]
        );

        // Second apply copies again (home copy preferred) — theme stays.
        assert_ok(&r.run(&r.script(ROOT_SCRIPT), &args));
        assert!(dst.join("index.theme").is_file());

        // Third apply copies nothing (theme gone from home, our copy counts
        // as system) — the manifest must still list the theme.
        let no_copies = args[..3].to_vec();
        assert_ok(&r.run(&r.script(ROOT_SCRIPT), &no_copies));
        assert_eq!(
            manifest(&r),
            vec![
                r.p("usr/local/share/backgrounds/mimshel/login.png")
                    .display()
                    .to_string(),
                dst.display().to_string(),
            ]
        );

        // Undo: conf back to the original, everything we installed is gone.
        assert_ok(&r.run(&r.script(UNDO_SCRIPT), &[]));
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), orig_text);
        assert!(!dst.exists());
        assert!(
            !r.p("usr/local/share/backgrounds/mimshel/login.png")
                .exists()
        );
        assert!(!r.p("usr/local/share/mimshel/greeter-installed").exists());
        // The saved original stays (a later apply keeps using it).
        assert_eq!(
            std::fs::read_to_string(r.p("etc/lightdm/slick-greeter.conf.mimshel-orig")).unwrap(),
            orig_text
        );
    }

    #[test]
    fn undo_skips_dotdot_paths() {
        let r = Root::new("undodot");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, format!("{MARKER}\n[Greeter]\n")).unwrap();
        std::fs::write(r.p("etc/lightdm/slick-greeter.conf.mimshel-orig"), "ORIG\n").unwrap();
        // A valid entry and a '..' traversal that must be skipped.
        let gone = r.p("usr/local/share/themes/Gone");
        std::fs::create_dir_all(&gone).unwrap();
        let pwn = r.p("etc/pwn");
        std::fs::create_dir_all(&pwn).unwrap();
        std::fs::write(pwn.join("sentinel"), "keep").unwrap();
        // Resolves to <root>/etc/pwn if something rm -rf's it — the guard
        // must skip the line, or the sentinel disappears.
        let sneaky = format!("{}/etc/pwn", r.p("usr/local/share/../..").display());
        std::fs::create_dir_all(r.p("usr/local/share/mimshel")).unwrap();
        std::fs::write(
            r.p("usr/local/share/mimshel/greeter-installed"),
            format!("{}\n{sneaky}\n", gone.display()),
        )
        .unwrap();

        assert_ok(&r.run(&r.script(UNDO_SCRIPT), &[]));
        assert!(!gone.exists());
        assert_eq!(
            std::fs::read_to_string(pwn.join("sentinel")).unwrap(),
            "keep"
        );
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), "ORIG\n");
        assert!(!r.p("usr/local/share/mimshel/greeter-installed").exists());
    }

    #[test]
    fn scripts_refuse_the_bare_directories() {
        // Destination = the themes directory itself (empty name): refused,
        // the directory and its other themes survive.
        let r = Root::new("bare1");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "[Greeter]\n").unwrap();
        let other = r.p("usr/local/share/themes/Other");
        std::fs::create_dir_all(&other).unwrap();
        let (_, mut args) = stage_files(&r, "[Greeter]\n");
        let src = theme_src(&r, "home/.themes/Ok");
        args.push(src.display().to_string());
        args.push(format!("{}/", r.p("usr/local/share/themes").display()));
        let out = r.run(&r.script(ROOT_SCRIPT), &args);
        assert!(!out.status.success());
        assert!(other.is_dir(), "other system themes untouched");

        // A manifest line naming /usr/local/share/ itself: skipped by undo.
        let r = Root::new("bare2");
        let conf = r.p("etc/lightdm/slick-greeter.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, format!("{MARKER}\n[Greeter]\n")).unwrap();
        let keep = r.p("usr/local/share/fonts/keep");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::create_dir_all(r.p("usr/local/share/themes")).unwrap();
        std::fs::create_dir_all(r.p("usr/local/share/mimshel")).unwrap();
        std::fs::write(
            r.p("usr/local/share/mimshel/greeter-installed"),
            format!(
                "{}/\n{}\n",
                r.p("usr/local/share").display(),
                r.p("usr/local/share/themes").display()
            ),
        )
        .unwrap();
        assert_ok(&r.run(&r.script(UNDO_SCRIPT), &[]));
        assert!(keep.is_dir(), "/usr/local/share itself not removed");
        assert!(
            r.p("usr/local/share/themes").is_dir(),
            "a whole share directory is never removed"
        );
    }
}
