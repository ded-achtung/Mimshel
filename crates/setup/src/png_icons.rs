//! Готовые PNG для программ GTK. Библиотека картинок системы (gdk-pixbuf
//! 2.44) грузит каждую SVG через отдельный процесс-загрузчик glycin: ~70 мс
//! на значок, а PNG — в 10 раз быстрее (урок 24). Окно Thunar с ~40 значками
//! открывалось секундами. Поэтому `shell apply icons` один раз рисует значки
//! темы своей программой (resvg, ~1 мс на значок) в тему `ТЕМА-png`, которая
//! наследует саму тему: чего в ней нет, GTK возьмёт из SVG, как раньше.
//!
//! Одноцветные (`*-symbolic`) GTK перекрашивает под цвет текста — для них
//! есть свой вид PNG (`имя.symbolic.png`, как делает `gtk-encode-symbolic-svg`):
//! форма — в прозрачности, а красный, зелёный и синий — доли цветов
//! «успех», «внимание», «ошибка»; у обычных одноцветных значков их нет —
//! все три нули. Значки с такими цветами (`class="success"` и т. п.)
//! оставляем в SVG.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use shell_icons::{Fit, Icons, render_file};

/// Размеры одноцветных значков (меню, кнопки, боковые панели).
const SYMBOLIC: &[u32] = &[16, 24, 32];
/// Версия алгоритма PNG: при изменении кэш должен пересоздаваться.
const CACHE_VERSION: &str = "shell-png-v2";
/// Папки, файлы, устройства — в Проводнике бывают и крупными.
const LARGE: &[u32] = &[16, 24, 32, 48, 64, 96, 128];
/// Значки программ — в меню, окнах «Открыть с помощью», заголовках.
const APPS: &[u32] = &[16, 24, 32, 48, 64];
/// Цветные значки действий и состояний — в меню и на кнопках.
const SMALL: &[u32] = &[16, 24];

/// Разделы темы: папка, `Context=` и размеры цветных значков.
const CONTEXTS: &[(&str, &str, &[u32])] = &[
    ("places", "Places", LARGE),
    ("mimetypes", "MimeTypes", LARGE),
    ("devices", "Devices", LARGE),
    ("apps", "Applications", APPS),
    ("actions", "Actions", SMALL),
    ("status", "Status", SMALL),
    ("categories", "Categories", SMALL),
    ("emblems", "Emblems", SMALL),
];

/// Имя темы с готовыми PNG для темы `theme`.
pub fn theme_name(theme: &str) -> String {
    format!("{theme}-png")
}

/// Что сделано: сколько нарисовано, сколько уже было, сколько оставлено в SVG.
#[derive(Debug, Default)]
pub struct Stats {
    pub drawn: usize,
    pub kept: usize,
    pub skipped: usize,
}

/// Значок для отрисовки: откуда, куда и одноцветный ли.
struct Job {
    source: PathBuf,
    out: PathBuf,
    size: u32,
    symbolic: bool,
}

/// Нарисовать тему `ТЕМА-png` в папке `icons_dir` для темы `theme`, у
/// которой файлы лежат в папках `sources` (сама тема и её родители там же).
/// `apps` — значки установленных программ (остальные из тысяч значков
/// программ в теме не нужны).
pub fn build(icons_dir: &Path, theme: &str, sources: &[&str], apps: &[String]) -> Stats {
    build_with(icons_dir, &Icons::new(theme), theme, sources, apps)
}

fn build_with(
    icons_dir: &Path,
    icons: &Icons,
    theme: &str,
    sources: &[&str],
    apps: &[String],
) -> Stats {
    let out_dir = icons_dir.join(theme_name(theme));
    let mut names: BTreeSet<(&'static str, String, bool)> = BTreeSet::new();
    for source in sources {
        for &(ctx, _, _) in CONTEXTS {
            let mut visited = HashSet::new();
            collect(
                &icons_dir.join(source).join(ctx),
                ctx,
                &mut names,
                &mut visited,
            );
        }
    }
    let marker = out_dir.join(".shell-cache-version");
    let cache_current =
        std::fs::read_to_string(&marker).is_ok_and(|version| version == CACHE_VERSION);
    let wanted_apps: BTreeSet<&str> = apps.iter().map(String::as_str).collect();
    let mut stats = Stats::default();
    let mut jobs = Vec::new();
    let mut dirs: BTreeSet<(&'static str, u32)> = BTreeSet::new();
    for (ctx, name, symbolic) in &names {
        let sizes = if *symbolic {
            SYMBOLIC
        } else if *ctx == "apps" && !wanted_apps.contains(name.as_str()) {
            continue;
        } else {
            CONTEXTS.iter().find(|c| c.0 == *ctx).map_or(SMALL, |c| c.2)
        };
        for &size in sizes {
            let Some(source) = icons.find(name, size) else {
                continue;
            };
            if source.extension().is_none_or(|e| e != "svg") {
                continue;
            }
            if *symbolic && has_state_colors(&source) {
                stats.skipped += 1;
                continue;
            }
            let file = if *symbolic {
                format!("{name}.symbolic.png")
            } else {
                format!("{name}.png")
            };
            let out = out_dir.join(format!("{ctx}/{size}/{file}"));
            dirs.insert((ctx, size));
            if cache_current && newer(&out, &source) {
                stats.kept += 1;
                continue;
            }
            jobs.push(Job {
                source,
                out,
                size,
                symbolic: *symbolic,
            });
        }
    }
    let expected = jobs.len();
    stats.drawn = render_all(jobs);
    write_index(&out_dir, theme, &dirs);
    if stats.drawn == expected {
        let _ = std::fs::write(marker, CACHE_VERSION);
    }
    stats
}

/// Все `.svg` в папке раздела (с вложенными, по ссылкам тоже).
fn collect(
    dir: &Path,
    ctx: &'static str,
    out: &mut BTreeSet<(&'static str, String, bool)>,
    visited: &mut HashSet<PathBuf>,
) {
    // Циклическая ссылка в теме может возвращать обход в тот же каталог.
    let Ok(real) = std::fs::canonicalize(dir) else {
        return;
    };
    if !visited.insert(real) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, ctx, out, visited);
        } else if let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".svg"))
        {
            out.insert((ctx, name.to_string(), name.ends_with("-symbolic")));
        }
    }
}

/// В одноцветном значке есть цвета состояний — перекрасить его по-своему
/// умеет только загрузка SVG.
fn has_state_colors(svg: &Path) -> bool {
    std::fs::read_to_string(svg).is_ok_and(|t| {
        ["success", "warning", "error"]
            .iter()
            .any(|c| t.contains(&format!("class=\"{c}")))
    })
}

/// Готовая картинка новее исходника — рисовать заново не нужно.
fn newer(out: &Path, source: &Path) -> bool {
    let time = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (time(out), time(source)) {
        (Some(o), Some(s)) => o >= s,
        _ => false,
    }
}

/// Нарисовать все значки (на всех ядрах). Возвращает, сколько нарисовано.
fn render_all(jobs: Vec<Job>) -> usize {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get());
    let chunk = jobs.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| s.spawn(move || part.iter().filter(|j| render(j)).count()))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or(0)).sum()
    })
}

fn render(job: &Job) -> bool {
    let Some(mut pixmap) = render_file(&job.source, job.size, job.size, Fit::Contain) else {
        return false;
    };
    if job.symbolic {
        // Форма — в прозрачности, цвета состояний — нули (весь значок —
        // цвета текста).
        for px in pixmap.pixels_mut() {
            if let Some(c) = tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 0, px.alpha()) {
                *px = c;
            }
        }
    }
    if let Some(dir) = job.out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Через временный файл: прерванная запись не оставит битую картинку.
    let tmp = job.out.with_extension("tmp");
    pixmap.save_png(&tmp).is_ok() && std::fs::rename(&tmp, &job.out).is_ok()
}

fn write_index(out_dir: &Path, theme: &str, dirs: &BTreeSet<(&'static str, u32)>) {
    let list: Vec<String> = dirs.iter().map(|(c, s)| format!("{c}/{s}")).collect();
    let mut text = format!(
        "[Icon Theme]\nName={}\nComment=Готовые PNG для программ GTK (рисует shell apply icons)\n\
         Inherits={theme},hicolor\nDirectories={}\n",
        theme_name(theme),
        list.join(",")
    );
    for (ctx, size) in dirs {
        let context = CONTEXTS
            .iter()
            .find(|c| c.0 == *ctx)
            .map_or("Actions", |c| c.1);
        text.push_str(&format!(
            "\n[{ctx}/{size}]\nContext={context}\nSize={size}\nType=Fixed\n"
        ));
    }
    let _ = std::fs::create_dir_all(out_dir);
    let _ = std::fs::write(out_dir.join("index.theme"), text);
    // Время изменения папки темы — по нему GTK понимает, что кеш устарел.
    let _ = std::fs::File::open(out_dir).and_then(|f| f.set_modified(SystemTime::now()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_png_and_symbolic() {
        let id = std::process::id();
        let root = std::env::temp_dir().join(format!("shell-png-icons-{id}"));
        let _ = std::fs::remove_dir_all(&root);
        let theme = root.join("Проба");
        std::fs::create_dir_all(theme.join("places/scalable")).unwrap();
        std::fs::create_dir_all(theme.join("actions/symbolic")).unwrap();
        std::fs::write(
            theme.join("index.theme"),
            "[Icon Theme]\nName=Проба\nDirectories=places/scalable,actions/symbolic\n\n\
             [places/scalable]\nSize=48\nMinSize=8\nMaxSize=512\nType=Scalable\n\n\
             [actions/symbolic]\nSize=16\nMinSize=8\nMaxSize=512\nType=Scalable\n",
        )
        .unwrap();
        let svg = |fill: &str, class: &str| {
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\">\
                 <rect {class} width=\"16\" height=\"16\" fill=\"{fill}\"/></svg>"
            )
        };
        std::fs::write(theme.join("places/scalable/folder.svg"), svg("#3af", "")).unwrap();
        std::os::unix::fs::symlink(theme.join("places/scalable"), theme.join("places/scalable/cycle")).unwrap();
        std::fs::write(
            theme.join("actions/symbolic/go-next-symbolic.svg"),
            svg("#bebebe", ""),
        )
        .unwrap();
        std::fs::write(
            theme.join("actions/symbolic/dialog-warning-symbolic.svg"),
            svg("#f00", "class=\"warning\""),
        )
        .unwrap();
        // Темы ищутся в XDG_DATA_DIRS и HOME — даём папку явно.
        let icons = Icons::with_dirs("Проба", vec![root.clone()], vec![]);
        let stats = build_with(&root, &icons, "Проба", &["Проба"], &[]);
        let out = root.join("Проба-png");
        assert!(out.join("places/48/folder.png").exists());
        assert!(out.join("places/128/folder.png").exists());
        let sym = out.join("actions/16/go-next-symbolic.symbolic.png");
        let png = tiny_skia::Pixmap::load_png(&sym).unwrap();
        let p = png.pixel(8, 8).unwrap();
        assert_eq!((p.red(), p.green(), p.blue(), p.alpha()), (0, 0, 0, 255));
        assert!(
            !out.join("actions/16/dialog-warning-symbolic.symbolic.png")
                .exists()
        );
        assert_eq!(stats.skipped, 3, "значок с цветом состояния — в SVG");
        let index = std::fs::read_to_string(out.join("index.theme")).unwrap();
        assert!(index.contains("Inherits=Проба,hicolor"));
        assert!(index.contains("[places/48]\nContext=Places\nSize=48\nType=Fixed"));
        // Второй раз — ничего не рисуется заново.
        let again = build_with(&root, &icons, "Проба", &["Проба"], &[]);
        assert_eq!(again.drawn, 0);
        assert!(again.kept > 0);
        // Обновление версии принудительно пересобирает PNG.
        std::fs::write(out.join(".shell-cache-version"), "old").unwrap();
        let refreshed = build_with(&root, &icons, "Проба", &["Проба"], &[]);
        assert!(refreshed.drawn > 0);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
