//! `mimshel settings [PAGE]`: the System Settings window (`crates/settings`).
//!
//! A process of its own, like the terminal: applying most settings restarts
//! the service, and the window must outlive that (a crash here does not
//! touch the dock either). Changes are written and applied the way
//! `mimshel set` does it, one after another on a worker thread, so the
//! window never waits for an apply.

use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};

use mimshel_core::i18n::{Language, t};
use mimshel_icons::Icons;
use mimshel_settings::{Action, Env, LINKS, SettingsView, view};
use mimshel_setup::themes;
use mimshel_text::Fonts;
use mimshel_x11::{Display, Event};
use tiny_skia::Pixmap;

use crate::ui::load_config;

/// Class of the window: the dock and the menu bar know it by this.
pub const WINDOW_CLASS: &str = "mimshel-settings";

/// Clock formats the menu bar page offers (plus the one set now).
const CLOCKS: [&str; 5] = [
    "%a %-d %b  %H:%M",
    "%a %H:%M",
    "%-d %b  %H:%M",
    "%a %-d %b  %-I:%M %p",
    "%H:%M",
];

enum Msg {
    X(Event),
    Thumb(String, Pixmap),
    /// A setting was applied: an error to show, if any.
    Applied(&'static str, Result<(), String>),
    /// The picture chosen for the wallpaper.
    Picked(Option<String>),
}

/// Exit code: 0 — closed, 1 — no screen.
pub fn run(page: Option<&str>) -> i32 {
    let config = load_config();
    crate::ui::set_scale(&config);
    let d = match Display::connect() {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!("Нет экрана: {e}");
            return 1;
        }
    };
    // Already open: bring that window forward instead of a second one.
    let open = d
        .client_list()
        .into_iter()
        .find(|&w| d.window_info(w).is_some_and(|i| i.class == WINDOW_CLASS));
    if let Some(w) = open {
        let _ = d.activate(w);
        return 0;
    }
    match window(&d, config, page) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Окно настроек: {e}");
            1
        }
    }
}

fn window(
    d: &Arc<Display>,
    config: mimshel_core::config::Config,
    page: Option<&str>,
) -> mimshel_x11::Result<()> {
    let lang = config.language;
    let mut view = SettingsView::new(lang, mimshel_settings::values(&config), env(&config));
    if let Some(page) = page {
        view.show_page(page);
    }
    let win = d.create_app_window(
        WINDOW_CLASS,
        t(lang, "settings.title"),
        view::SIZE,
        view::MIN_SIZE,
    )?;
    win.show(d)?;
    let mut fonts = Fonts::new(&config.font);
    let mut icons = Icons::new(&config.icon_theme);

    let (tx, rx) = mpsc::channel();
    {
        let d = Arc::clone(d);
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = d.next_event() {
                if tx.send(Msg::X(ev)).is_err() {
                    break;
                }
            }
        });
    }
    let jobs = worker(tx.clone());
    request_thumbs(&view, d.scale(), &tx);

    let mut dirty = true;
    loop {
        if dirty {
            dirty = false;
            let size = win.size(d).unwrap_or(view::SIZE);
            let frame = view.render(size, &mut fonts, &mut icons);
            win.present(d, &frame)?;
            d.sync()?;
        }
        let Ok(msg) = rx.recv() else {
            return Ok(());
        };
        let action = match msg {
            Msg::X(ev) => match ev {
                Event::Expose { .. } | Event::Configure { .. } => Action::Redraw,
                Event::Close { window } if window == win.id() => Action::Close,
                Event::Button { x, y, button, .. } => match button {
                    1 => view.press(f32::from(x), f32::from(y)),
                    4 => view.wheel(f32::from(x), false),
                    5 => view.wheel(f32::from(x), true),
                    _ => Action::Nothing,
                },
                Event::Release { button: 1, .. } => view.release(),
                Event::Motion { x, y, .. } => view.motion(f32::from(x), f32::from(y)),
                Event::Leave { .. } => view.leave(),
                Event::Key(k) => view.key(k),
                _ => Action::Nothing,
            },
            Msg::Thumb(value, pixmap) => {
                view.set_thumb(value, pixmap);
                Action::Redraw
            }
            Msg::Applied(key, result) => {
                let config = load_config();
                if config.language != view.language() {
                    win.set_title(d, t(config.language, "settings.title"))?;
                }
                view.set_values(config.language, mimshel_settings::values(&config));
                let lang = view.language();
                view.set_status(Some(match result {
                    Ok(()) => t(lang, "settings.applied").to_string(),
                    Err(e) => t(lang, "settings.failed").replace("{e}", &e),
                }));
                if key == "look.wallpaper" {
                    request_thumbs(&view, d.scale(), &tx);
                }
                Action::Redraw
            }
            Msg::Picked(Some(path)) => {
                let _ = jobs.send(("look.wallpaper", path));
                applying(&mut view, "look.wallpaper");
                Action::Redraw
            }
            Msg::Picked(None) => Action::Nothing,
        };
        match action {
            Action::Nothing => {}
            Action::Redraw => dirty = true,
            Action::Set(key, value) => {
                let _ = jobs.send((key, value));
                applying(&mut view, key);
                dirty = true;
            }
            Action::PickWallpaper => pick_wallpaper(view.language(), tx.clone()),
            Action::Open(command) => {
                if let Some((program, args)) = command.split_first() {
                    let mut cmd = Command::new(program);
                    cmd.args(args);
                    if let Err(e) = crate::ui::spawn_detached(cmd) {
                        eprintln!("Не удалось открыть «{program}»: {e}");
                    }
                }
            }
            Action::Close => {
                win.destroy(d)?;
                return Ok(());
            }
        }
    }
}

/// «Applying…» — and whether Mimshel restarts for it.
fn applying(view: &mut SettingsView, key: &str) {
    let lang = view.language();
    let text = if crate::setup::restarts_service(key) {
        t(lang, "settings.restarting")
    } else {
        t(lang, "settings.applying")
    };
    view.set_status(Some(text.to_string()));
}

/// The thread that writes and applies settings, one at a time, in order.
fn worker(tx: Sender<Msg>) -> Sender<(&'static str, String)> {
    let (jobs, rx) = mpsc::channel::<(&'static str, String)>();
    std::thread::spawn(move || {
        while let Ok((key, value)) = rx.recv() {
            let result = match crate::setup::write_setting(key, &value) {
                Ok(key) => crate::setup::apply_setting_checked(&key),
                Err(crate::setup::SetError::Value(_, e) | crate::setup::SetError::Write(e)) => {
                    Err(e)
                }
            };
            if tx.send(Msg::Applied(key, result)).is_err() {
                break;
            }
        }
    });
    jobs
}

/// Thumbnails of the wallpapers that have none yet, drawn off the main
/// thread.
fn request_thumbs(view: &SettingsView, scale: f32, tx: &Sender<Msg>) {
    let (w, h) = SettingsView::thumb_size();
    let px = |v: u32| mimshel_render::to_px(v, scale);
    let missing: Vec<String> = view
        .wallpapers()
        .into_iter()
        .filter(|w| !view.has_thumb(w))
        .collect();
    let (pw, ph) = (px(w), px(h));
    let tx = tx.clone();
    std::thread::spawn(move || {
        for value in missing {
            if let Some(thumb) = crate::desktop_wallpaper::thumb(&value, pw, ph) {
                let _ = tx.send(Msg::Thumb(value, rounded(thumb, scale)));
            }
        }
    });
}

/// The thumbnail with rounded corners, like the boxes around it.
fn rounded(mut pm: Pixmap, scale: f32) -> Pixmap {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    if let Some(path) = mimshel_widgets::shape::rounded(0.0, 0.0, w, h, 8.0 * scale)
        && let Some(mut mask) = tiny_skia::Mask::new(pm.width(), pm.height())
    {
        mask.fill_path(
            &path,
            tiny_skia::FillRule::Winding,
            true,
            tiny_skia::Transform::identity(),
        );
        pm.apply_mask(&mask);
    }
    pm
}

/// Ask for a picture (zenity) on a thread; the answer comes as `Msg::Picked`.
fn pick_wallpaper(lang: Language, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let pictures = Command::new("xdg-user-dir")
            .arg("PICTURES")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && std::path::Path::new(s).is_dir());
        let filter = format!(
            "--file-filter={} | *.png *.jpg *.jpeg *.webp *.svg",
            t(lang, "wallpaper.pick_filter")
        );
        let path = Command::new("zenity")
            .args(pictures.map(|p| format!("--filename={p}/")))
            .args([
                "--file-selection".to_string(),
                format!("--title={}", t(lang, "wallpaper.pick_title")),
                filter,
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let _ = tx.send(Msg::Picked(path));
    });
}

/// What the pages offer on this machine.
fn env(config: &mimshel_core::config::Config) -> Env {
    let mut clocks: Vec<String> = CLOCKS.iter().map(|s| s.to_string()).collect();
    if !clocks.contains(&config.clock_format) {
        clocks.push(config.clock_format.clone());
    }
    let samples = clock_samples(&clocks, config.language);
    Env {
        gtk_themes: themes::gtk_themes(),
        icon_themes: themes::icon_themes(),
        cursor_themes: themes::cursor_themes(),
        clocks: clocks.into_iter().zip(samples).collect(),
        links: LINKS
            .iter()
            .filter(|l| l.3.first().is_some_and(|p| themes::installed(p)))
            .map(|l| l.0)
            .collect(),
    }
}

/// How each clock format looks now, in one run of `date`.
fn clock_samples(formats: &[String], lang: Language) -> Vec<String> {
    use mimshel_core::dates;
    let out = Command::new("date")
        .arg(dates::prepare(&format!("+{}", formats.join("\n"))))
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let out = dates::fill(&out, lang);
    let mut lines: Vec<String> = out.lines().map(|l| l.trim().to_string()).collect();
    lines.resize(formats.len(), String::new());
    lines
        .into_iter()
        .zip(formats)
        .map(|(sample, format)| {
            if sample.is_empty() {
                format.clone()
            } else {
                sample
            }
        })
        .collect()
}

/// `mimshel snapshot settings PAGE FILE.png`: the window as a picture.
pub fn snapshot(page: &str, file: &str) -> mimshel_x11::Result<()> {
    let config = load_config();
    let mut view = SettingsView::new(
        config.language,
        mimshel_settings::values(&config),
        env(&config),
    );
    view.show_page(page);
    let mut fonts = Fonts::new(&config.font);
    let mut icons = Icons::new(&config.icon_theme);
    let (w, h) = SettingsView::thumb_size();
    let scale = mimshel_render::scale();
    for value in view.wallpapers() {
        let px = |v: u32| mimshel_render::to_px(v, scale);
        if let Some(thumb) = crate::desktop_wallpaper::thumb(&value, px(w), px(h)) {
            view.set_thumb(value, rounded(thumb, scale));
        }
    }
    let frame = view.render(view::SIZE, &mut fonts, &mut icons);
    frame.save_png(file)?;
    Ok(())
}
