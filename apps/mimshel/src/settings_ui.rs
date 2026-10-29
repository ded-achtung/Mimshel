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
use std::time::{Duration, Instant};

use mimshel_core::i18n::{Language, t};
use mimshel_icons::Icons;
use mimshel_settings::{Action, Env, LINKS, SettingsView, view};
use mimshel_setup::themes;
use mimshel_text::Fonts;
use mimshel_x11::{Display, Event};
use tiny_skia::Pixmap;

use crate::settings_system as system;
use crate::ui::load_config;
use mimshel_setup::displays::{self, Output};

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
    Applied(String, Result<(), String>),
    /// New display settings are on the screen; these were before them.
    Tried(Vec<Output>),
    /// A second went by (the countdown of «Keep these display settings?»).
    Tick,
    /// The picture chosen for the wallpaper.
    Picked(Option<String>),
}

/// Work for the worker thread, done one after another.
enum Job {
    Set(String, String),
    /// Keep the display settings on the screen (Xfce's profile).
    SaveDisplays,
    /// Go back to these display settings.
    Restore(Vec<Output>),
}

/// How long «Keep these display settings?» waits before going back.
const KEEP_SECONDS: u64 = 15;

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
        .find(|&w| d.window_info(w).is_some_and(|i| i.instance == WINDOW_CLASS));
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
    let outputs = displays::outputs();
    let mut view = SettingsView::new(lang, all_values(&config), env(&config, &outputs));
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
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while tx.send(Msg::Tick).is_ok() {
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
    // Display settings being tried: what to go back to, and by when.
    let mut trying: Option<(Vec<Output>, Instant)> = None;

    let mut dirty = true;
    // The last frame sent (pixel size and RGBA), and whether the window
    // lost what it showed (Expose: send it all).
    let mut shown: Option<(u32, u32, Vec<u8>)> = None;
    let mut exposed = true;
    loop {
        if dirty {
            dirty = false;
            let start = std::time::Instant::now();
            let size = win.size(d).unwrap_or(view::SIZE);
            let frame = view.render(size, &mut fonts, &mut icons);
            let drawn = start.elapsed();
            // Only what changed since the last frame (a hover moves a few
            // rows); the whole window when its size changed or after Expose.
            let pixmap = frame.pixmap();
            let same = shown
                .as_ref()
                .is_some_and(|(w, h, _)| (*w, *h) == (pixmap.width(), pixmap.height()));
            let rect = match (&shown, same && !exposed) {
                (Some((_, _, before)), true) => changed(before, pixmap),
                _ => Some((0, 0, pixmap.width(), pixmap.height())),
            };
            if let Some(rect) = rect {
                win.present_rect(d, frame, rect)?;
                d.sync()?;
            }
            match &mut shown {
                Some((w, h, before)) if same => {
                    before.copy_from_slice(pixmap.data());
                    (*w, *h) = (pixmap.width(), pixmap.height());
                }
                _ => shown = Some((pixmap.width(), pixmap.height(), pixmap.data().to_vec())),
            }
            exposed = false;
            if mimshel_core::names::flag("TIMING") {
                eprintln!(
                    "кадр {size:?}: рисование {drawn:?}, всего {:?}",
                    start.elapsed()
                );
            }
        }
        let Ok(first) = rx.recv() else {
            return Ok(());
        };
        // Everything that came meanwhile, then one frame: a fast pointer
        // sends many motions, and drawing each would fall behind it.
        let batch: Vec<Msg> = std::iter::once(first).chain(rx.try_iter()).collect();
        for msg in batch {
            let action = match msg {
                Msg::X(ev) => match ev {
                    Event::Expose { .. } | Event::Configure { .. } => {
                        exposed = true;
                        Action::Redraw
                    }
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
                    view.set_values(config.language, all_values(&config));
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
                Msg::Tried(before) => {
                    trying = Some((before, Instant::now() + Duration::from_secs(KEEP_SECONDS)));
                    view.set_status(None);
                    view.set_confirm(Some(KEEP_SECONDS as u32));
                    Action::Redraw
                }
                Msg::Tick => match &trying {
                    Some((_, by)) => {
                        let left = by
                            .saturating_duration_since(Instant::now())
                            .as_secs_f32()
                            .ceil() as u32;
                        if left == 0 {
                            Action::Revert
                        } else {
                            view.set_confirm(Some(left));
                            Action::Redraw
                        }
                    }
                    None => Action::Nothing,
                },
                Msg::Picked(Some(path)) => {
                    let _ = jobs.send(Job::Set("look.wallpaper".to_string(), path));
                    applying(&mut view, "look.wallpaper");
                    Action::Redraw
                }
                Msg::Picked(None) => Action::Nothing,
            };
            match action {
                Action::Nothing => {}
                Action::Redraw => dirty = true,
                Action::Set(key, value) => {
                    applying(&mut view, &key);
                    let _ = jobs.send(Job::Set(key, value));
                    dirty = true;
                }
                Action::Keep => {
                    if trying.take().is_some() {
                        let _ = jobs.send(Job::SaveDisplays);
                    }
                    view.set_confirm(None);
                    dirty = true;
                }
                Action::Revert => {
                    if let Some((before, _)) = trying.take() {
                        let _ = jobs.send(Job::Restore(before));
                    }
                    view.set_confirm(None);
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
                    // Closed while trying display settings: not kept.
                    if let Some((before, _)) = trying.take()
                        && let Err(e) = displays::apply(&before)
                    {
                        eprintln!("Не вернуть прежние настройки экранов: {e}");
                    }
                    win.destroy(d)?;
                    return Ok(());
                }
            }
        }
    }
}

/// Where `now` differs from `before` (a frame of the same size), in
/// pixels `(x0, y0, x1, y1)`; `None` — nothing changed.
fn changed(before: &[u8], now: &Pixmap) -> Option<(u32, u32, u32, u32)> {
    let w = now.width() as usize;
    let row = w * 4;
    let data = now.data();
    let (mut x0, mut x1, mut y0, mut y1) = (w, 0, None, 0);
    for (y, (a, b)) in before.chunks(row).zip(data.chunks(row)).enumerate() {
        if a == b {
            continue;
        }
        y0.get_or_insert(y);
        y1 = y + 1;
        let px = |r: &[u8], x: usize| [r[x * 4], r[x * 4 + 1], r[x * 4 + 2], r[x * 4 + 3]];
        if let Some(first) = (0..x0).find(|&x| px(a, x) != px(b, x)) {
            x0 = first;
        }
        if let Some(last) = (x1..w).rev().find(|&x| px(a, x) != px(b, x)) {
            x1 = last + 1;
        }
    }
    let y0 = y0?;
    Some((x0 as u32, y0 as u32, x1.max(x0 + 1) as u32, y1 as u32))
}

/// «Applying…» — and whether Mimshel restarts for it.
fn applying(view: &mut SettingsView, key: &str) {
    let lang = view.language();
    let text = if !system::is_system(key) && crate::setup::restarts_service(key) {
        t(lang, "settings.restarting")
    } else {
        t(lang, "settings.applying")
    };
    view.set_status(Some(text.to_string()));
}

/// The thread that writes and applies settings, one at a time, in order.
fn worker(tx: Sender<Msg>) -> Sender<Job> {
    let (jobs, rx) = mpsc::channel::<Job>();
    std::thread::spawn(move || {
        while let Ok(job) = rx.recv() {
            let msg = match job {
                Job::Set(key, value) if key.starts_with("display.") => {
                    let before = displays::outputs();
                    match system::changed_outputs(&before, &key, &value) {
                        Some(new) => match displays::apply(&new) {
                            Ok(()) => Msg::Tried(before),
                            Err(e) => Msg::Applied(key, Err(e)),
                        },
                        None => Msg::Applied(key, Ok(())),
                    }
                }
                Job::Set(key, value) if key.starts_with("keyboard.") => {
                    let result = system::change_keyboard(&key, &value);
                    Msg::Applied(key, result)
                }
                Job::Set(key, value) => {
                    let result = match crate::setup::write_setting(&key, &value) {
                        Ok(key) => crate::setup::apply_setting_checked(&key),
                        Err(
                            crate::setup::SetError::Value(_, e) | crate::setup::SetError::Write(e),
                        ) => Err(e),
                    };
                    Msg::Applied(key, result)
                }
                Job::SaveDisplays => {
                    Msg::Applied("display".into(), displays::save(&displays::outputs()))
                }
                Job::Restore(before) => Msg::Applied("display".into(), displays::apply(&before)),
            };
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    jobs
}

/// The config's settings and the system pages' state, as the pages show them.
fn all_values(config: &mimshel_core::config::Config) -> std::collections::HashMap<String, String> {
    let mut v = mimshel_settings::values(config);
    v.extend(system::values(
        &displays::outputs(),
        &mimshel_setup::keyboards::current(),
    ));
    v
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
fn env(config: &mimshel_core::config::Config, outputs: &[Output]) -> Env {
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
        // Our own pages need xrandr and setxkbmap; without them the Xfce
        // dialogs stay.
        system: themes::installed("xrandr") && themes::installed("setxkbmap"),
        monitors: system::monitors(outputs),
        layouts: mimshel_setup::keyboards::available_layouts(),
        switches: system::switches(config.language),
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
        all_values(&config),
        env(&config, &displays::outputs()),
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
