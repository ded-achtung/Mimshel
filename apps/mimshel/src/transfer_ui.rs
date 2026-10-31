//! Service-owned transfer window; progress and drawing live outside X11.

use crate::platform::{AppWindow, Display, Event, Key, Result};
use mimshel_core::i18n::{Language, t};
use mimshel_folder::transfer::{self, Phase, Row};
use mimshel_text::Fonts;

use crate::transfer_job::Handle;

pub struct TransferUi {
    win: Option<AppWindow>,
    jobs: Vec<Handle>,
    rows: Vec<(Handle, Row)>,
    language: Language,
    dirty: bool,
    hidden: bool,
    offset: usize,
    capacity: usize,
}

impl TransferUi {
    pub fn new(language: Language) -> Self {
        Self {
            win: None,
            jobs: Vec::new(),
            rows: Vec::new(),
            language,
            dirty: false,
            hidden: false,
            offset: 0,
            capacity: 1,
        }
    }

    pub fn started(&mut self, handle: Handle) {
        self.jobs.push(handle);
        self.hidden = false;
    }

    pub fn set_language(&mut self, language: Language) {
        self.language = language;
        self.dirty = true;
    }

    pub fn tick(&mut self, d: &Display) -> Result<()> {
        self.jobs.retain(|job| job.snapshot().phase != Phase::Done);
        self.rows = self
            .jobs
            .iter()
            .filter_map(|job| {
                let row = job.snapshot();
                row.visible().then(|| (job.clone(), row))
            })
            .collect();
        if self.rows.is_empty() || self.hidden {
            if let Some(win) = self.win.take() {
                win.destroy(d)?;
            }
            return Ok(());
        }
        self.capacity = (((f32::from(d.screen_size().1) / d.scale()).floor() as u32)
            .saturating_sub(140)
            / transfer::ROW_HEIGHT)
            .max(1) as usize;
        self.offset = self
            .offset
            .min(self.rows.len().saturating_sub(self.capacity));
        let size = (
            transfer::WIDTH,
            transfer::height(self.rows.len().min(self.capacity)),
        );
        if self.win.is_none() {
            let win = d.create_app_window(
                "mimshel-transfer",
                t(self.language, "transfer.title"),
                size,
                (transfer::WIDTH, transfer::height(1)),
            )?;
            win.show(d)?;
            self.win = Some(win);
        } else if let Some(win) = &self.win {
            let current = win.size(d).unwrap_or(size);
            if current.1 != size.1 {
                win.resize(d, (current.0, size.1))?;
            }
        }
        self.dirty = true;
        Ok(())
    }

    fn own(&self, window: u32) -> bool {
        self.win.as_ref().is_some_and(|w| w.id() == window)
    }

    pub fn wants(&self, d: &Display, ev: &Event) -> bool {
        match *ev {
            Event::Expose { window }
            | Event::Configure { window }
            | Event::Close { window }
            | Event::Button { window, .. }
            | Event::Release { window, .. } => self.own(window),
            Event::Key(_) => d.active_window().is_some_and(|w| self.own(w)),
            _ => false,
        }
    }

    pub fn event(&mut self, d: &Display, ev: Event) -> Result<()> {
        match ev {
            Event::Expose { .. } | Event::Configure { .. } => self.dirty = true,
            Event::Close { .. } => self.hidden = true,
            Event::Key(k) if k.key == Key::Escape => self.hidden = true,
            Event::Button {
                x, y, button: 1, ..
            } => {
                if let Some(win) = &self.win {
                    let width = win.size(d).map_or(transfer::WIDTH, |s| s.0);
                    for (i, (job, row)) in self
                        .rows
                        .iter()
                        .skip(self.offset)
                        .take(self.capacity)
                        .enumerate()
                    {
                        if transfer::stop_hit(i, width, f32::from(x), f32::from(y)) {
                            eprintln!(
                                "Окно копирования: «Остановить» для «{}» ({:?})",
                                row.name, row.phase
                            );
                            job.stop();
                        }
                    }
                }
            }
            Event::Button { button: 4, .. } => self.offset = self.offset.saturating_sub(1),
            Event::Button { button: 5, .. } => {
                self.offset = (self.offset + 1).min(self.rows.len().saturating_sub(self.capacity))
            }
            _ => {}
        }
        self.tick(d)
    }

    pub fn flush(
        &mut self,
        d: &Display,
        fonts: &mut Fonts,
        icons: &mut mimshel_icons::Icons,
    ) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        if let Some(win) = &self.win {
            win.set_title(d, t(self.language, "transfer.title"))?;
            let rows: Vec<Row> = self
                .rows
                .iter()
                .skip(self.offset)
                .take(self.capacity)
                .map(|(_, row)| row.clone())
                .collect();
            let size = win
                .size(d)
                .unwrap_or((transfer::WIDTH, transfer::height(rows.len())));
            if let Some(frame) = transfer::render(&rows, size, fonts, icons, self.language) {
                win.present(d, &frame)?;
            }
            d.sync()?;
        }
        Ok(())
    }
}

pub fn snapshot(file: &str) -> Result<()> {
    let config = crate::ui::load_config();
    crate::ui::set_scale(&config);
    let row = transfer::example();
    let mut fonts = Fonts::new(&config.font);
    let mut icons = mimshel_icons::Icons::new(&config.icon_theme);
    transfer::render(
        &[row],
        (transfer::WIDTH, transfer::height(1)),
        &mut fonts,
        &mut icons,
        config.language,
    )
    .ok_or("could not allocate transfer snapshot")?
    .save_png(file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer_job::Job;

    #[test]
    fn isolated_window_lifecycle_and_stop() -> Result<()> {
        // Always start a private server; never draw on the user's display.
        if std::env::var_os("MIMSHEL_TEST_PRIVATE_TRANSFER_X11").is_none() {
            let status = std::process::Command::new("xvfb-run")
                .args(["-a", "-s", "-screen 0 1024x768x24"])
                .arg(std::env::current_exe()?)
                .args([
                    "--exact",
                    "transfer_ui::tests::isolated_window_lifecycle_and_stop",
                    "--nocapture",
                ])
                .env("MIMSHEL_TEST_PRIVATE_TRANSFER_X11", "1")
                .status()?;
            assert!(status.success());
            return Ok(());
        }
        let display = Display::connect()?;
        let mut ui = TransferUi::new(Language::Ru);
        let mut row = transfer::example();
        row.folder = "Рабочий стол".into();
        let first = Job::preview(row.clone());
        ui.started(first.handle.clone());
        ui.tick(&display)?;
        let window = ui.win.as_ref().unwrap().id();
        let mut fonts = Fonts::new("DejaVu Sans");
        let mut icons = mimshel_icons::Icons::new("hicolor");
        ui.flush(&display, &mut fonts, &mut icons)?;
        let screenshot = display
            .capture_window(window)
            .ok_or("no transfer window image")?;
        screenshot.save_png(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/transfer-window.png"),
        )?;
        row.moving = true;
        row.phase = Phase::Removing;
        let second = Job::preview(row);
        ui.started(second.handle.clone());
        ui.tick(&display)?;
        assert_eq!(ui.win.as_ref().unwrap().id(), window);
        assert_eq!(ui.rows.len(), 2);
        ui.flush(&display, &mut fonts, &mut icons)?;
        assert_eq!(
            ui.win.as_ref().unwrap().size(&display).unwrap().1,
            transfer::height(2)
        );
        for i in 0..2 {
            let (x, y) = transfer::stop_center(i, transfer::WIDTH);
            ui.event(
                &display,
                Event::Button {
                    window,
                    x: x as i16,
                    y: y as i16,
                    button: 1,
                },
            )?;
        }
        assert!(first.stopped());
        assert!(!second.stopped());
        drop(first);
        ui.tick(&display)?;
        assert_eq!(ui.rows.len(), 1);
        assert_eq!(ui.win.as_ref().unwrap().id(), window);
        drop(second);
        ui.tick(&display)?;
        assert!(ui.win.is_none());
        Ok(())
    }

    /// A real click through the X server (xdotool) on the round stop button
    /// stops the copy. The other test feeds events straight into the window;
    /// this one checks they arrive. `MIMSHEL_TEST_WM` names a window manager
    /// to run in the private server (xfwm4 reparents and grabs clicks).
    #[test]
    fn isolated_real_click_stops() -> Result<()> {
        if std::env::var_os("MIMSHEL_TEST_PRIVATE_TRANSFER_CLICK").is_none() {
            if std::process::Command::new("xdotool")
                .arg("version")
                .output()
                .is_err()
            {
                eprintln!("xdotool is not installed: skipped");
                return Ok(());
            }
            let wm = std::env::var("MIMSHEL_TEST_WM").unwrap_or_default();
            let test = format!(
                "{wm} & sleep 2; MIMSHEL_TEST_PRIVATE_TRANSFER_CLICK=1 '{}' --exact \
                 transfer_ui::tests::isolated_real_click_stops --nocapture",
                std::env::current_exe()?.display()
            );
            let status = std::process::Command::new("xvfb-run")
                .args([
                    "-a",
                    "-s",
                    "-screen 0 1024x768x24",
                    "dbus-run-session",
                    "sh",
                    "-c",
                ])
                .arg(if wm.is_empty() {
                    test.replacen(" & sleep 2;", "", 1)
                } else {
                    test
                })
                .status()?;
            assert!(status.success());
            return Ok(());
        }
        let display = Display::connect()?;
        let mut ui = TransferUi::new(Language::Ru);
        let job = Job::preview(transfer::example());
        ui.started(job.handle.clone());
        ui.tick(&display)?;
        let window = ui.win.as_ref().unwrap().id();
        let mut fonts = Fonts::new("DejaVu Sans");
        let mut icons = mimshel_icons::Icons::new("hicolor");
        ui.flush(&display, &mut fonts, &mut icons)?;
        // Wait until the window is on screen and drawn.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while display.capture_window(window).is_none() {
            assert!(std::time::Instant::now() < deadline, "window never mapped");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (x, y) = transfer::stop_center(0, transfer::WIDTH);
        let status = std::process::Command::new("xdotool")
            .args(["mousemove", "--window", &window.to_string()])
            .args([(x as i32).to_string(), (y as i32).to_string()])
            .args(["click", "1"])
            .status()?;
        assert!(status.success());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !job.stopped() {
            assert!(
                std::time::Instant::now() < deadline,
                "the click did not stop the copy"
            );
            let ev = display.next_event()?;
            eprintln!("event: {ev:?}");
            if ui.wants(&display, &ev) {
                ui.event(&display, ev)?;
            }
        }
        Ok(())
    }
}
